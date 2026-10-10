//! Wevren, a note-taking app.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod completion;
mod editor;
mod markdown;
mod notes;
mod search;
mod settings;
mod theme;

use std::{
    collections::HashSet,
    fmt::Display,
    fs, io,
    ops::Range,
    path::{Path, PathBuf},
    time::Duration,
};

use editor::{Cancelled, Edited, Editor, Followed, Submitted};
use gpui::{
    AnyElement, App, Application, Bounds, Div, Entity, Focusable, KeyDownEvent, PathPromptOptions,
    Rgba, SharedString, Stateful, Task, TitlebarOptions, Window, WindowBounds, WindowControlArea,
    WindowOptions, div, prelude::*, px, relative, size,
};
use mimalloc::MiMalloc;
use settings::Settings;
use theme::Theme;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

/// Initial window size, roomy enough for the sidebar and a readable note.
const WINDOW_SIZE: (f32, f32) = (1000., 700.);
/// Titlebar height, a little taller than the Windows default of 32.
const TITLEBAR_HEIGHT: f32 = 40.;
/// Width of a titlebar button, matching native Windows caption buttons.
const CAPTION_WIDTH: f32 = 46.;
/// Sidebar width, fitting a typical note title on one line.
const SIDEBAR_WIDTH: f32 = 240.;
/// Widest a column of text gets, keeping lines comfortable to read.
const COLUMN_WIDTH: f32 = 640.;
/// Icon typeface that ships with Windows 11.
const ICON_FONT: &str = "Segoe Fluent Icons";
/// Minimize, maximize and close glyphs in [`ICON_FONT`].
const CAPTION_GLYPHS: [&str; 3] = ["\u{e921}", "\u{e922}", "\u{e8bb}"];
/// Gear glyph in [`ICON_FONT`].
const SETTINGS_GLYPH: &str = "\u{e713}";
/// Plus glyph in [`ICON_FONT`].
const ADD_GLYPH: &str = "\u{e710}";
/// Pause after the last edit before a note is written, sparing the disk while typing.
const AUTOSAVE_DELAY: Duration = Duration::from_secs(1);

#[derive(Debug)]
struct Find {
    input: Entity<Editor>,
    matches: Vec<Range<usize>>,
    current: Option<usize>,
}

/// The root view.
#[derive(Debug)]
struct Wevren {
    settings: Settings,
    /// Whether the settings page covers the open note.
    settings_open: bool,
    /// Why the last operation failed.
    error: Option<SharedString>,
    /// Every note in the notes folder, sorted by title.
    notes: Vec<PathBuf>,
    entries: Vec<notes::Entry>,
    expanded: HashSet<PathBuf>,
    selected_folder: Option<PathBuf>,
    folder_input: Option<Entity<Editor>>,
    /// The note shown in the editor.
    open: Option<PathBuf>,
    editor: Entity<Editor>,
    autosave: Option<Task<anyhow::Result<()>>>,
    dirty: bool,
    rename_editor: Option<Entity<Editor>>,
    find: Option<Find>,
}

impl Wevren {
    /// Creates the view from the saved settings.
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let editor = cx.new(Editor::new);
        cx.subscribe(&editor, |this, _, _: &Edited, cx| {
            this.schedule_save(cx);
            this.refresh_find(false, cx);
        })
        .detach();
        cx.subscribe_in(&editor, window, |this, _, link: &Followed, window, cx| {
            this.follow(&link.0, window, cx);
        })
        .detach();
        cx.on_release(|this, cx| {
            this.save(cx);
        })
        .detach();
        let mut this = Self {
            settings: Settings::load(),
            settings_open: false,
            error: None,
            notes: Vec::new(),
            entries: Vec::new(),
            expanded: HashSet::new(),
            selected_folder: None,
            folder_input: None,
            open: None,
            editor,
            autosave: None,
            dirty: false,
            rename_editor: None,
            find: None,
        };
        this.reload(cx);
        this
    }

    /// Returns the value of `result`, keeping its error for display when it failed.
    fn report<T, E: Display>(&mut self, result: Result<T, E>) -> Option<T> {
        match result {
            Ok(value) => Some(value),
            Err(error) => {
                self.error = Some(format!("{error:#}").into());
                None
            }
        }
    }

    /// Reads the list of notes from the notes folder.
    fn reload(&mut self, cx: &mut App) {
        let entries = self.settings.folder.as_deref().map(notes::scan).transpose();
        if let Some(entries) = self.report(entries) {
            self.entries = entries.unwrap_or_default();
        }
        self.notes = self
            .entries
            .iter()
            .filter(|entry| !entry.directory)
            .map(|entry| entry.path.clone())
            .collect();
        self.expanded.retain(|path| {
            self.entries
                .iter()
                .any(|entry| entry.directory && entry.path == *path)
        });
        let titles = self.settings.folder.as_ref().map_or_else(Vec::new, |root| {
            self.notes
                .iter()
                .map(|path| notes::link_title(path, root))
                .collect()
        });
        self.editor.update(cx, |editor, cx| {
            editor.set_note_titles(titles, cx);
        });
    }

    /// Asks for a folder and keeps the notes in it from then on.
    fn choose_folder(cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: None,
        });
        cx.spawn(async move |this, cx| {
            let Some(folder) = picked.await??.and_then(|paths| paths.into_iter().next()) else {
                return Ok(());
            };
            this.update(cx, |this, cx| {
                this.set_folder(folder, cx);
                cx.notify();
            })
        })
        .detach_and_log_err(cx);
    }

    /// Stores `folder` as the notes folder, keeping the settings open on failure.
    fn set_folder(&mut self, folder: PathBuf, cx: &mut App) {
        self.error = None;
        if !self.save(cx) {
            return;
        }
        self.open = None;
        self.rename_editor = None;
        self.find = None;
        self.expanded.clear();
        self.entries.clear();
        self.selected_folder = Some(folder.clone());
        self.folder_input = None;
        self.settings.folder = Some(folder);
        self.report(self.settings.save());
        self.settings_open |= self.error.is_some();
        self.reload(cx);
    }

    /// Shows the note at `path` in the editor.
    fn open_note(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.error = None;
        if !self.save(cx) {
            cx.notify();
            return;
        }
        if let Some(text) = self.report(notes::read(&path)) {
            self.editor
                .update(cx, |editor, cx| editor.set_text(text, cx));
            self.editor.focus_handle(cx).focus(window);
            self.open = Some(path);
            self.selected_folder = self
                .open
                .as_ref()
                .and_then(|path| path.parent().map(Path::to_path_buf));
            self.reveal_open();
            self.rename_editor = None;
            self.find = None;
            self.settings_open = false;
        }
        cx.notify();
    }

    fn reveal_open(&mut self) {
        if let (Some(path), Some(root)) = (&self.open, &self.settings.folder) {
            for parent in path
                .ancestors()
                .skip(1)
                .take_while(|parent| *parent != root && parent.starts_with(root))
            {
                self.expanded.insert(parent.to_path_buf());
            }
        }
    }

    fn destination(&self) -> Option<PathBuf> {
        self.selected_folder
            .clone()
            .or_else(|| self.settings.folder.clone())
    }

    fn begin_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.destination().is_none() {
            return;
        }
        let input = cx.new(|cx| Editor::single_line(String::new(), cx));
        cx.subscribe_in(&input, window, |this, _, _: &Submitted, _, cx| {
            this.finish_folder(cx);
        })
        .detach();
        cx.subscribe_in(&input, window, |this, _, _: &Cancelled, window, cx| {
            this.folder_input = None;
            this.editor.focus_handle(cx).focus(window);
            cx.notify();
        })
        .detach();
        input.focus_handle(cx).focus(window);
        self.folder_input = Some(input);
        cx.notify();
    }

    fn finish_folder(&mut self, cx: &mut Context<Self>) {
        let (Some(parent), Some(input)) = (self.destination(), &self.folder_input) else {
            return;
        };
        let name = input.read(cx).text().trim().to_owned();
        self.error = None;
        if let Some(path) = self.report(notes::create_folder(&parent, &name)) {
            self.expanded.insert(parent);
            self.expanded.insert(path.clone());
            self.selected_folder = Some(path);
            self.folder_input = None;
            self.reload(cx);
        }
        cx.notify();
    }

    /// Writes the open note once typing has paused.
    fn schedule_save(&mut self, cx: &mut Context<Self>) {
        self.dirty = true;
        self.autosave = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(AUTOSAVE_DELAY).await;
            this.update(cx, |this, cx| {
                this.save(cx);
                cx.notify();
            })
        }));
    }

    /// Writes the open note now if it has unsaved edits.
    fn save(&mut self, cx: &App) -> bool {
        self.autosave.take();
        if !self.dirty {
            return true;
        }
        let Some(path) = &self.open else {
            return false;
        };
        let written = fs::write(path, self.editor.read(cx).text());
        if self.report(written).is_none() {
            return false;
        }
        self.dirty = false;
        true
    }

    fn begin_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.find.is_some() {
            self.close_find(window, cx);
        }
        let Some(path) = &self.open else {
            return;
        };
        if self.rename_editor.is_none() {
            let title = notes::title(path);
            let input = cx.new(|cx| Editor::single_line(title, cx));
            cx.subscribe_in(&input, window, |this, _, _: &Submitted, window, cx| {
                this.finish_rename(window, cx);
            })
            .detach();
            cx.subscribe_in(&input, window, |this, _, _: &Cancelled, window, cx| {
                this.cancel_rename(window, cx);
            })
            .detach();
            self.rename_editor = Some(input);
        }
        if let Some(input) = &self.rename_editor {
            input.focus_handle(cx).focus(window);
        }
        self.settings_open = false;
        cx.notify();
    }

    fn begin_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.open.is_none() || self.rename_editor.is_some() {
            return;
        }
        self.settings_open = false;
        if self.find.is_none() {
            let selected = self.editor.read(cx).selected_text();
            let query = if selected.contains('\n') {
                String::new()
            } else {
                selected.to_owned()
            };
            let input = cx.new(|cx| Editor::single_line(query, cx));
            cx.subscribe(&input, |this, _, _: &Edited, cx| {
                this.refresh_find(true, cx);
            })
            .detach();
            cx.subscribe_in(&input, window, |this, _, event: &Submitted, _, cx| {
                this.step_find(event.0, cx);
            })
            .detach();
            cx.subscribe_in(&input, window, |this, _, _: &Cancelled, window, cx| {
                this.close_find(window, cx);
            })
            .detach();
            self.find = Some(Find {
                input,
                matches: Vec::new(),
                current: None,
            });
            self.refresh_find(true, cx);
        }
        if let Some(find) = &self.find {
            find.input.focus_handle(cx).focus(window);
        }
        cx.notify();
    }

    fn refresh_find(&mut self, select: bool, cx: &mut Context<Self>) {
        let Some(find) = &mut self.find else {
            return;
        };
        let editor = self.editor.read(cx);
        find.matches = search::matches(editor.text(), find.input.read(cx).text());
        find.current = (!find.matches.is_empty()).then(|| {
            find.matches
                .iter()
                .position(|range| range.start >= editor.selection_start())
                .unwrap_or(0)
        });
        let matches = find.matches.clone();
        let current = find.current;
        self.editor.update(cx, |editor, cx| {
            editor.set_matches(matches, current, select, cx);
        });
        cx.notify();
    }

    fn step_find(&mut self, previous: bool, cx: &mut Context<Self>) {
        let Some(find) = &mut self.find else {
            return;
        };
        let count = find.matches.len();
        if count == 0 {
            return;
        }
        let current = find.current.unwrap_or(0);
        find.current = Some(if previous {
            (current + count - 1) % count
        } else {
            (current + 1) % count
        });
        let matches = find.matches.clone();
        let current = find.current;
        self.editor.update(cx, |editor, cx| {
            editor.set_matches(matches, current, true, cx);
        });
        cx.notify();
    }

    fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.find = None;
        self.editor.update(cx, |editor, cx| {
            editor.set_matches(Vec::new(), None, false, cx);
        });
        self.editor.focus_handle(cx).focus(window);
        cx.notify();
    }

    fn cancel_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.rename_editor = None;
        self.editor.focus_handle(cx).focus(window);
        cx.notify();
    }

    fn finish_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(path), Some(input)) = (&self.open, &self.rename_editor) else {
            return;
        };
        let path = path.clone();
        let title = input.read(cx).text().trim().to_owned();
        self.error = None;
        if self.save(cx)
            && let Some(renamed) = self.report(notes::rename(&path, &title))
        {
            self.open = Some(renamed);
            self.reload(cx);
            self.cancel_rename(window, cx);
        }
        cx.notify();
    }

    /// Creates an empty note and opens it.
    fn new_note(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(folder) = self.destination() else {
            return;
        };
        self.open_created(notes::create_untitled(&folder), window, cx);
    }

    /// Opens the note titled `title`, creating it when there is none.
    fn follow(&mut self, title: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(folder) = self.settings.folder.clone() else {
            return;
        };
        let current = self
            .open
            .as_ref()
            .and_then(|path| path.parent())
            .unwrap_or(&folder);
        let path = notes::resolve(&folder, current, title, &self.notes);
        self.open_created(path, window, cx);
    }

    /// Opens the note at `path`, which may have just been created.
    fn open_created(
        &mut self,
        path: io::Result<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(path) = self.report(path) {
            self.reload(cx);
            self.open_note(path, window, cx);
        }
        cx.notify();
    }

    /// Handles the platform's shortcuts for saving and for a new note.
    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.find.is_some() {
            match event.keystroke.key.as_str() {
                "escape" => self.close_find(window, cx),
                "f3" => self.step_find(event.keystroke.modifiers.shift, cx),
                _ => {}
            }
            if matches!(event.keystroke.key.as_str(), "escape" | "f3") {
                cx.stop_propagation();
                return;
            }
        }
        if event.keystroke.key == "f2" {
            self.begin_rename(window, cx);
            cx.stop_propagation();
            return;
        }
        let modifiers = event.keystroke.modifiers;
        if !modifiers.secondary() || modifiers.alt {
            return;
        }
        match event.keystroke.key.as_str() {
            "f" => self.begin_find(window, cx),
            "s" => {
                self.save(cx);
            }
            "n" => self.new_note(window, cx),
            _ => return,
        }
        cx.notify();
        cx.stop_propagation();
    }

    /// Returns the note list.
    fn sidebar(&self, theme: Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let empty = div()
            .px_3()
            .font_family(theme.text_font)
            .text_color(theme.faint)
            .child("No notes yet");
        let header = Self::sidebar_header(theme, cx);
        let root = self.settings.folder.as_deref();
        let rows = self
            .entries
            .iter()
            .filter(|entry| {
                entry
                    .path
                    .ancestors()
                    .skip(1)
                    .take_while(|parent| Some(*parent) != root)
                    .all(|parent| self.expanded.contains(parent))
            })
            .map(|entry| self.sidebar_row(entry, theme, cx));
        let folder_input = self.folder_input.as_ref().map(|input| {
            div()
                .px_3()
                .py_2()
                .flex()
                .flex_col()
                .gap_2()
                .child(label("NEW FOLDER (ENTER TO CREATE)", theme))
                .child(
                    div()
                        .border_1()
                        .border_color(theme.accent)
                        .rounded_md()
                        .px_2()
                        .py_1()
                        .child(input.clone()),
                )
                .child(
                    div().text_size(px(11.)).text_color(theme.muted).child(
                        self.destination()
                            .map_or_else(String::new, |path| path.display().to_string()),
                    ),
                )
        });
        div()
            .id("notes")
            .flex()
            .flex_col()
            .flex_none()
            .w(px(SIDEBAR_WIDTH))
            .pt_3()
            .overflow_y_scroll()
            .child(header)
            .children(folder_input)
            .when(self.entries.is_empty(), |list| list.child(empty))
            .children(rows)
    }

    fn sidebar_header(theme: Theme, cx: &mut Context<Self>) -> Div {
        let add = div()
            .id("new-note")
            .flex()
            .items_center()
            .justify_center()
            .size_6()
            .rounded_md()
            .font_family(ICON_FONT)
            .text_size(px(11.))
            .text_color(theme.muted)
            .cursor_pointer()
            .hover(|s| s.bg(theme.hover).text_color(theme.text))
            .on_click(cx.listener(|this, _, window, cx| this.new_note(window, cx)))
            .child(ADD_GLYPH);
        div()
            .flex()
            .flex_none()
            .items_center()
            .justify_between()
            .pl_3()
            .pr_1()
            .pb_1()
            .child(
                label("NOTES", theme)
                    .id("workspace-root")
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.selected_folder = this.settings.folder.clone();
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .id("new-folder")
                            .cursor_pointer()
                            .text_size(px(11.))
                            .text_color(theme.muted)
                            .hover(|style| style.text_color(theme.text))
                            .on_click(
                                cx.listener(|this, _, window, cx| this.begin_folder(window, cx)),
                            )
                            .child("+ Folder"),
                    )
                    .child(add),
            )
    }

    fn sidebar_row(
        &self,
        entry: &notes::Entry,
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let root = self.settings.folder.as_deref();
        let directory = entry.directory;
        let active = if directory {
            self.selected_folder.as_ref() == Some(&entry.path)
        } else {
            self.open.as_ref() == Some(&entry.path)
        };
        let expanded = self.expanded.contains(&entry.path);
        let indent = entry
            .path
            .strip_prefix(root.unwrap_or(Path::new("")))
            .map_or(0., |path| {
                path.components().skip(1).fold(0., |indent, _| indent + 14.)
            });
        let name = if directory {
            entry
                .path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        } else {
            notes::title(&entry.path)
        };
        let path = entry.path.clone();
        let marker = if directory {
            div()
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .w(px(28.))
                .h_full()
                .font_family(theme.ui_font)
                .child(if expanded { "\u{25be}" } else { "\u{25b8}" })
        } else {
            knot(active, theme)
        };
        div()
            .id(SharedString::from(path.to_string_lossy().into_owned()))
            .flex()
            .flex_none()
            .items_center()
            .h(px(32.))
            .pl(px(indent))
            .pr_3()
            .rounded_md()
            .cursor_pointer()
            .font_family(if directory {
                theme.ui_font
            } else {
                theme.text_font
            })
            .text_color(if active { theme.text } else { theme.muted })
            .when(directory && active, |row| row.bg(theme.hover))
            .hover(|style| style.bg(theme.hover).text_color(theme.text))
            .child(marker)
            .child(div().flex_1().min_w_0().truncate().child(name))
            .on_click(cx.listener(move |this, _, window, cx| {
                if directory {
                    this.selected_folder = Some(path.clone());
                    if !this.expanded.remove(&path) {
                        this.expanded.insert(path.clone());
                    }
                    cx.notify();
                } else {
                    this.open_note(path.clone(), window, cx);
                }
            }))
    }

    /// Returns the page a note is read and edited on.
    fn note_page(&self, path: &Path, theme: Theme, cx: &mut Context<Self>) -> AnyElement {
        let title = if let Some(input) = &self.rename_editor {
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(label("RENAME NOTE", theme))
                .child(
                    div()
                        .px_3()
                        .py_2()
                        .border_1()
                        .border_color(theme.accent)
                        .rounded_md()
                        .child(input.clone()),
                )
                .child(
                    div()
                        .flex()
                        .gap_4()
                        .font_family(theme.ui_font)
                        .child(
                            div()
                                .id("confirm-rename")
                                .cursor_pointer()
                                .text_color(theme.accent)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.finish_rename(window, cx);
                                }))
                                .child("Rename"),
                        )
                        .child(
                            div()
                                .id("cancel-rename")
                                .cursor_pointer()
                                .text_color(theme.muted)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.cancel_rename(window, cx);
                                }))
                                .child("Cancel"),
                        ),
                )
                .into_any_element()
        } else {
            heading(notes::title(path), theme)
                .id("note-title")
                .cursor_pointer()
                .on_click(cx.listener(|this, _, window, cx| this.begin_rename(window, cx)))
                .into_any_element()
        };
        let column = column()
            .min_h_full()
            .pb_8()
            .line_height(relative(1.7))
            .child(title)
            .child(rule(theme))
            .child(self.editor.clone());
        let scroll = div()
            .id("note")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .w_full()
            .items_center()
            .overflow_y_scroll()
            .track_scroll(self.editor.read(cx).scroll_handle())
            .child(column);
        sheet(theme)
            .when(self.find.is_some(), |sheet| {
                sheet.child(self.find_bar(theme, cx))
            })
            .child(scroll)
            .into_any_element()
    }

    fn find_bar(&self, theme: Theme, cx: &mut Context<Self>) -> Div {
        let Some(find) = &self.find else {
            return div();
        };
        let count = find.matches.len();
        let status = if find.input.read(cx).text().is_empty() {
            "Find in note".to_owned()
        } else if count == 0 {
            "No matches".to_owned()
        } else {
            format!("{} / {count}", find.current.map_or(0, |index| index + 1))
        };
        let buttons = [
            ("find-previous", "Previous", Some(true)),
            ("find-next", "Next", Some(false)),
            ("find-close", "Close", None),
        ]
        .into_iter()
        .map(|(id, label, previous)| {
            div()
                .id(id)
                .px_2()
                .py_1()
                .rounded_md()
                .cursor_pointer()
                .hover(|style| style.bg(theme.hover).text_color(theme.text))
                .on_click(cx.listener(move |this, _, window, cx| {
                    if let Some(previous) = previous {
                        this.step_find(previous, cx);
                    } else {
                        this.close_find(window, cx);
                    }
                }))
                .child(label)
        });
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .px_4()
            .py_2()
            .border_b_1()
            .border_color(theme.line)
            .font_family(theme.ui_font)
            .text_size(px(12.))
            .text_color(theme.muted)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .px_2()
                    .py_1()
                    .bg(theme.ground)
                    .border_1()
                    .border_color(theme.line)
                    .rounded_md()
                    .text_color(theme.text)
                    .child(find.input.clone()),
            )
            .child(div().flex_none().child(status))
            .children(buttons)
    }

    /// Returns the settings page, which doubles as the welcome page.
    fn settings_page(&self, theme: Theme, cx: &mut Context<Self>) -> AnyElement {
        let folder = self.settings.folder.as_deref();
        let (title, action) = if folder.is_some() {
            ("Settings", "Change folder")
        } else {
            ("Welcome", "Choose folder")
        };
        let column = column()
            .items_start()
            .child(heading(title, theme))
            .child(rule(theme))
            .child(label("NOTES FOLDER", theme))
            .child(folder.map_or_else(
                || "Choose the folder Wevren keeps your notes in.".to_owned(),
                |folder| folder.display().to_string(),
            ))
            .child(
                div()
                    .id("choose-folder")
                    .px_3()
                    .py_1p5()
                    .rounded_md()
                    .border_1()
                    .border_color(theme.accent)
                    .font_family(theme.ui_font)
                    .text_sm()
                    .text_color(theme.accent)
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.hover))
                    .on_click(cx.listener(|_, _, _, cx| Self::choose_folder(cx)))
                    .child(action),
            );
        sheet(theme).items_center().child(column).into_any_element()
    }
}

/// Returns the base of a titlebar button.
fn caption(id: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .w(px(CAPTION_WIDTH))
        .h_full()
}

/// Returns a titlebar button showing `glyph`, lit with `hover` under the pointer.
fn icon_button(glyph: &'static str, size: f32, hover: Rgba, theme: Theme) -> Stateful<Div> {
    caption(glyph)
        .font_family(ICON_FONT)
        .text_size(px(size))
        .text_color(theme.muted)
        .hover(|s| s.bg(hover).text_color(theme.text))
        .child(glyph)
}

/// Returns the custom titlebar: a draggable wordmark and the window buttons.
fn titlebar(theme: Theme, cx: &mut Context<Wevren>) -> impl IntoElement {
    let [min, max, close] = CAPTION_GLYPHS;
    let caption_button =
        |glyph, area, hover| icon_button(glyph, 10., hover, theme).window_control_area(area);
    div()
        .flex()
        .flex_none()
        .h(px(TITLEBAR_HEIGHT))
        .child(
            div()
                .flex()
                .flex_1()
                .items_center()
                .px_4()
                .window_control_area(WindowControlArea::Drag)
                .font_family(theme.display_font)
                .italic()
                .text_size(px(15.))
                .text_color(theme.muted)
                .child("Wevren"),
        )
        .child(
            icon_button(SETTINGS_GLYPH, 13., theme.hover, theme)
                .cursor_pointer()
                .on_click(cx.listener(|this, _, _, cx| {
                    this.settings_open ^= true;
                    cx.notify();
                })),
        )
        .child(
            caption("theme")
                .cursor_pointer()
                .hover(|s| s.bg(theme.hover))
                .on_click(cx.listener(move |_, _, _, cx| {
                    cx.set_global(theme.toggled());
                    cx.notify();
                }))
                .child(
                    div()
                        .size(px(11.))
                        .rounded_full()
                        .border_1()
                        .border_color(theme.muted)
                        .when(theme.dark, |d| d.bg(theme.muted)),
                ),
        )
        .child(caption_button(min, WindowControlArea::Min, theme.hover))
        .child(caption_button(max, WindowControlArea::Max, theme.hover))
        .child(caption_button(
            close,
            WindowControlArea::Close,
            theme.danger,
        ))
}

/// Returns a small heading for a group of controls.
fn label(text: &'static str, theme: Theme) -> Div {
    div()
        .font_family(theme.ui_font)
        .text_size(px(11.))
        .text_color(theme.faint)
        .child(text)
}

/// Returns the title of a page.
fn heading(text: impl Into<SharedString>, theme: Theme) -> Div {
    div()
        .font_family(theme.display_font)
        .text_size(px(38.))
        .line_height(relative(1.3))
        .child(text.into())
}

/// Returns the short accent line that sets off a heading.
fn rule(theme: Theme) -> Div {
    div()
        .flex_none()
        .w(px(36.))
        .h(px(2.))
        .rounded_full()
        .bg(theme.accent)
}

/// Returns the marker of a note on the thread that runs down the note list.
fn knot(active: bool, theme: Theme) -> Div {
    let thread = div()
        .absolute()
        .top_0()
        .bottom_0()
        .left(px(13.5))
        .w(px(1.))
        .bg(theme.line);
    let bead = div()
        .size(px(7.))
        .rounded_full()
        .border_1()
        .border_color(if active { theme.accent } else { theme.faint })
        .bg(if active { theme.accent } else { theme.ground });
    div()
        .relative()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .w(px(28.))
        .h_full()
        .child(thread)
        .child(bead)
}

/// Returns the inset surface that pages are laid out on.
fn sheet(theme: Theme) -> Div {
    div()
        .flex()
        .flex_col()
        .flex_1()
        .min_w_0()
        .bg(theme.sheet)
        .border_1()
        .border_color(theme.line)
        .rounded(px(14.))
        .font_family(theme.text_font)
        .text_size(px(16.))
}

/// Returns the centered column a page's content is stacked in.
fn column() -> Div {
    div()
        .flex()
        .flex_col()
        .gap_5()
        .w_full()
        .max_w(px(COLUMN_WIDTH))
        .px_8()
        .pt(px(72.))
}

/// Returns the page shown while no note is open.
fn empty_page(theme: Theme) -> AnyElement {
    sheet(theme)
        .items_center()
        .justify_center()
        .gap_4()
        .text_color(theme.muted)
        .child(rule(theme))
        .child("No note is open")
        .into_any_element()
}

impl Render for Wevren {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let has_folder = self.settings.folder.is_some();
        let page = match &self.open {
            _ if self.settings_open || !has_folder => self.settings_page(theme, cx),
            Some(path) => self.note_page(path, theme, cx),
            None => empty_page(theme),
        };
        let error = self
            .error
            .clone()
            .map(|error| div().px_4().pb_2().text_color(theme.danger).child(error));
        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(theme.ground)
            .font_family(theme.ui_font)
            .text_sm()
            .text_color(theme.text)
            .on_key_down(cx.listener(Self::key_down))
            .child(titlebar(theme, cx))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .gap_2()
                    .px_2()
                    .pb_2()
                    .when(has_folder, |body| body.child(self.sidebar(theme, cx)))
                    .child(page),
            )
            .children(error)
    }
}

fn main() {
    Application::new().run(|cx: &mut App| {
        let (width, height) = WINDOW_SIZE;
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                None,
                size(px(width), px(height)),
                cx,
            ))),
            titlebar: Some(TitlebarOptions {
                title: Some("Wevren".into()),
                appears_transparent: true,
                ..Default::default()
            }),
            ..Default::default()
        };
        cx.open_window(options, |window, cx| {
            cx.set_global(Theme::new(window.appearance()));
            cx.new(|cx| Wevren::new(window, cx))
        })
        .expect("the main window must open at startup");
        cx.activate(true);
    });
}
