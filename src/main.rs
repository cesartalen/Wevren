//! Wevren, a note-taking app.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod editor;
mod notes;
mod settings;
mod theme;

use std::{
    fmt::Display,
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use editor::{Edited, Editor};
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
    /// The note shown in the editor.
    open: Option<PathBuf>,
    editor: Entity<Editor>,
    /// Pending write of the open note, present while it has unsaved edits.
    autosave: Option<Task<anyhow::Result<()>>>,
}

impl Wevren {
    /// Creates the view from the saved settings.
    fn new(cx: &mut Context<Self>) -> Self {
        let editor = cx.new(Editor::new);
        cx.subscribe(&editor, |this, _, _: &Edited, cx| this.schedule_save(cx))
            .detach();
        cx.on_release(|this, cx| this.save(cx)).detach();
        let mut this = Self {
            settings: Settings::load(),
            settings_open: false,
            error: None,
            notes: Vec::new(),
            open: None,
            editor,
            autosave: None,
        };
        this.reload();
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
    fn reload(&mut self) {
        let notes = self.settings.folder.as_deref().map(notes::list).transpose();
        self.notes = self.report(notes).flatten().unwrap_or_default();
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
    fn set_folder(&mut self, folder: PathBuf, cx: &App) {
        self.error = None;
        self.save(cx);
        self.open = None;
        self.settings.folder = Some(folder);
        self.report(self.settings.save());
        self.settings_open |= self.error.is_some();
        self.reload();
    }

    /// Shows the note at `path` in the editor.
    fn open_note(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.error = None;
        self.save(cx);
        if let Some(text) = self.report(notes::read(&path)) {
            self.editor
                .update(cx, |editor, cx| editor.set_text(text, cx));
            self.editor.focus_handle(cx).focus(window);
            self.open = Some(path);
            self.settings_open = false;
        }
        cx.notify();
    }

    /// Writes the open note once typing has paused.
    fn schedule_save(&mut self, cx: &mut Context<Self>) {
        self.autosave = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(AUTOSAVE_DELAY).await;
            this.update(cx, |this, cx| {
                this.save(cx);
                cx.notify();
            })
        }));
    }

    /// Writes the open note now if it has unsaved edits.
    fn save(&mut self, cx: &App) {
        if self.autosave.take().is_some()
            && let Some(path) = &self.open
        {
            let written = fs::write(path, self.editor.read(cx).text());
            self.report(written);
        }
    }

    /// Creates an empty note and opens it.
    fn new_note(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(folder) = self.settings.folder.clone() else {
            return;
        };
        if let Some(path) = self.report(notes::create(&folder)) {
            self.reload();
            self.open_note(path, window, cx);
        }
        cx.notify();
    }

    /// Handles the platform's shortcuts for saving and for a new note.
    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let modifiers = event.keystroke.modifiers;
        if !modifiers.secondary() || modifiers.alt {
            return;
        }
        match event.keystroke.key.as_str() {
            "s" => self.save(cx),
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
        let header = div()
            .flex()
            .flex_none()
            .items_center()
            .justify_between()
            .pl_3()
            .pr_1()
            .pb_1()
            .child(label("NOTES", theme))
            .child(add);
        let rows = self.notes.iter().enumerate().map(|(index, path)| {
            let active = self.open.as_ref() == Some(path);
            let path = path.clone();
            div()
                .id(index)
                .flex()
                .flex_none()
                .items_center()
                .h(px(32.))
                .pr_3()
                .rounded_md()
                .cursor_pointer()
                .font_family(theme.text_font)
                .text_color(if active { theme.text } else { theme.muted })
                .hover(|s| s.bg(theme.hover).text_color(theme.text))
                .child(knot(active, theme))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child(notes::title(&path)),
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_note(path.clone(), window, cx);
                }))
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
            .when(self.notes.is_empty(), |list| list.child(empty))
            .children(rows)
    }

    /// Returns the page a note is read and edited on.
    fn note_page(&self, path: &Path, theme: Theme, cx: &App) -> AnyElement {
        let column = column()
            .min_h_full()
            .pb_8()
            .line_height(relative(1.7))
            .child(heading(notes::title(path), theme))
            .child(rule(theme))
            .child(self.editor.clone());
        sheet(theme)
            .id("note")
            .items_center()
            .overflow_y_scroll()
            .track_scroll(self.editor.read(cx).scroll_handle())
            .child(column)
            .into_any_element()
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
            cx.new(Wevren::new)
        })
        .expect("the main window must open at startup");
        cx.activate(true);
    });
}
