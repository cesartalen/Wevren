//! Wevren, a note-taking app.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod completion;
mod editor;
mod formatting;
mod links;
mod markdown;
mod notes;
mod search;
mod session;
mod settings;
mod shortcuts;
mod theme;
mod window_controls;
mod workspace_search;

use std::{
    cmp::Reverse,
    collections::HashSet,
    fmt::Display,
    io,
    ops::Range,
    path::{Path, PathBuf},
    time::Duration,
};

use editor::{Cancelled, Edited, Editor, Followed, Navigated, Submitted};
use gpui::{
    AnyElement, App, Application, Bounds, Div, Entity, FocusHandle, Focusable, KeyDownEvent,
    MouseButton, PathPromptOptions, Rgba, SharedString, Stateful, Task, TitlebarOptions, Window,
    WindowBounds, WindowControlArea, WindowOptions, div, prelude::*, px, relative, size,
};
use mimalloc::MiMalloc;
use settings::Settings;
use shortcuts::{Action, Bindings, Chord};
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

#[derive(Debug)]
struct Switcher {
    input: Entity<Editor>,
    results: Vec<PathBuf>,
    selected: usize,
    content: bool,
    hits: workspace_search::Results,
    busy: bool,
    scroll: gpui::ScrollHandle,
}

#[derive(Clone, Debug)]
struct Conflict {
    disk: Option<String>,
}

#[derive(Debug)]
struct Tab {
    path: PathBuf,
    editor: Entity<Editor>,
    baseline: Option<String>,
    dirty: bool,
    conflict: Option<Conflict>,
}

#[derive(Debug, Default)]
struct PaneLayout {
    split: bool,
    paths: [Option<PathBuf>; 2],
    active: usize,
}

/// The root view.
#[derive(Debug)]
struct Wevren {
    focus: FocusHandle,
    settings: Settings,
    /// Whether the settings page covers the open note.
    settings_open: bool,
    shortcut_capture: Option<Action>,
    shortcut_error: Option<String>,
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
    switcher: Option<Switcher>,
    baseline: Option<String>,
    conflict: Option<Conflict>,
    disk_revision: u64,
    watcher: Option<Task<anyhow::Result<()>>>,
    navigation: Vec<PathBuf>,
    navigation_index: Option<usize>,
    tabs: Vec<Tab>,
    content_search: Option<Task<anyhow::Result<()>>>,
    move_picker: bool,
    layout: PaneLayout,
    last_session: Option<session::Session>,
}

impl Wevren {
    /// Creates the view from the saved settings.
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let settings = Settings::load();
        cx.set_global(settings.shortcuts.clone());
        let editor = Self::note_editor(window, cx);
        cx.on_release(|this, cx| {
            this.save_all(cx);
            this.persist_session(cx);
        })
        .detach();
        let mut this = Self {
            focus: cx.focus_handle(),
            settings,
            settings_open: false,
            shortcut_capture: None,
            shortcut_error: None,
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
            switcher: None,
            baseline: None,
            conflict: None,
            disk_revision: 0,
            watcher: None,
            navigation: Vec::new(),
            navigation_index: None,
            tabs: Vec::new(),
            content_search: None,
            move_picker: false,
            layout: PaneLayout::default(),
            last_session: None,
        };
        this.reload(cx);
        this.restore_session(window, cx);
        if this.open.is_some() {
            this.editor.focus_handle(cx).focus(window);
        } else {
            this.focus.focus(window);
        }
        this.start_watcher(cx);
        let view = cx.entity().downgrade();
        window.on_window_should_close(cx, move |window, cx| {
            view.update(cx, |this, cx| this.prepare_close(window, cx))
                .unwrap_or(true)
        });
        this
    }

    fn note_editor(window: &mut Window, cx: &mut Context<Self>) -> Entity<Editor> {
        let editor = cx.new(Editor::new);
        cx.subscribe_in(&editor, window, |this, source, _: &Edited, window, cx| {
            this.select_editor(source, window, cx);
            this.schedule_save(cx);
            this.refresh_find(false, cx);
            cx.notify();
        })
        .detach();
        cx.subscribe_in(
            &editor,
            window,
            |this, source, link: &Followed, window, cx| {
                this.select_editor(source, window, cx);
                this.follow(&link.0, window, cx);
            },
        )
        .detach();
        let source = editor.downgrade();
        cx.on_focus(&editor.focus_handle(cx), window, move |this, window, cx| {
            if let Some(source) = source.upgrade() {
                this.select_editor(&source, window, cx);
            }
        })
        .detach();
        editor
    }

    fn restore_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.settings.folder.clone() else {
            return;
        };
        let Some(Some(mut saved)) = self.report(session::Session::load(&root)) else {
            return;
        };
        let folders: Vec<_> = std::iter::once(root.clone())
            .chain(
                self.entries
                    .iter()
                    .filter(|entry| entry.directory)
                    .map(|entry| entry.path.clone()),
            )
            .collect();
        saved.sanitize(&self.notes, &folders);
        for tab in &saved.tabs {
            let Some(text) = self.report(notes::read(&tab.path)) else {
                continue;
            };
            let editor = Self::note_editor(window, cx);
            editor.update(cx, |editor, cx| {
                editor.set_text(text.clone(), cx);
                editor.restore_view(&tab.view, cx);
            });
            self.tabs.push(Tab {
                path: tab.path.clone(),
                editor,
                baseline: Some(text),
                dirty: false,
                conflict: None,
            });
        }
        let active_had_note = saved.panes[saved.active].is_some();
        self.layout = PaneLayout {
            split: saved.split,
            active: saved.active,
            paths: saved
                .panes
                .clone()
                .map(|path| path.filter(|path| self.tabs.iter().any(|tab| tab.path == *path))),
        };
        if active_had_note
            && self.layout.paths[self.layout.active].is_none()
            && self.layout.paths[1 - self.layout.active].is_some()
        {
            self.layout.active = 1 - self.layout.active;
        }
        if let Some(path) = &self.layout.paths[self.layout.active]
            && let Some(index) = self.tabs.iter().position(|tab| tab.path == *path)
        {
            self.load_tab_state(index);
            if let Some(path) = self.open.clone() {
                self.record_visit(path);
            }
        }
        self.expanded = saved.expanded.iter().cloned().collect();
        self.selected_folder.clone_from(&saved.selected_folder);
        self.last_session = Some(saved);
        self.sync_entries(cx);
    }

    fn persist_session(&mut self, cx: &App) {
        let Some(root) = &self.settings.folder else {
            return;
        };
        if !root.is_dir() {
            return;
        }
        let mut expanded: Vec<_> = self.expanded.iter().cloned().collect();
        expanded.sort();
        let saved = session::Session {
            folder: root.clone(),
            tabs: self
                .tabs
                .iter()
                .map(|tab| session::SavedTab {
                    path: tab.path.clone(),
                    view: tab.editor.read(cx).session_view(),
                })
                .collect(),
            panes: self.layout.paths.clone(),
            split: self.layout.split,
            active: self.layout.active,
            expanded,
            selected_folder: self.selected_folder.clone(),
        };
        if self.last_session.as_ref() != Some(&saved) && self.report(saved.save()).is_some() {
            self.last_session = Some(saved);
        }
    }

    fn store_active_tab(&mut self) {
        if let Some(tab) = self
            .tabs
            .iter_mut()
            .find(|tab| Some(&tab.path) == self.open.as_ref())
        {
            tab.baseline.clone_from(&self.baseline);
            tab.conflict.clone_from(&self.conflict);
            tab.dirty = self.dirty;
        }
    }

    fn load_tab_state(&mut self, index: usize) {
        let tab = &self.tabs[index];
        self.open = Some(tab.path.clone());
        self.editor = tab.editor.clone();
        self.baseline.clone_from(&tab.baseline);
        self.conflict.clone_from(&tab.conflict);
        self.dirty = tab.dirty;
        if self.layout.split
            && self.layout.paths[1 - self.layout.active].as_ref() == Some(&tab.path)
        {
            self.layout.active = 1 - self.layout.active;
        }
        self.layout.paths[self.layout.active].clone_from(&self.open);
        self.selected_folder = tab.path.parent().map(Path::to_path_buf);
        self.find = None;
        self.rename_editor = None;
        self.settings_open = false;
        self.disk_revision = self.disk_revision.wrapping_add(1);
        self.reveal_open();
    }

    fn select_editor(
        &mut self,
        source: &Entity<Editor>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.editor == *source && self.open.is_some() {
            return;
        }
        let Some(index) = self.tabs.iter().position(|tab| tab.editor == *source) else {
            return;
        };
        self.save(cx);
        self.store_active_tab();
        self.editor.update(cx, |editor, cx| {
            editor.set_matches(Vec::new(), None, false, cx);
        });
        self.load_tab_state(index);
        if let Some(path) = self.open.clone() {
            self.record_visit(path);
        }
        cx.notify();
    }

    fn select_pane(&mut self, pane: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(path) = &self.layout.paths[pane]
            && let Some(tab) = self.tabs.iter().find(|tab| tab.path == *path)
        {
            let editor = tab.editor.clone();
            self.select_editor(&editor, window, cx);
            self.layout.active = pane;
            editor.focus_handle(cx).focus(window);
        } else {
            self.save(cx);
            self.store_active_tab();
            self.layout.active = pane;
            self.open = None;
            self.baseline = None;
            self.conflict = None;
            self.dirty = false;
            self.find = None;
            self.rename_editor = None;
            self.disk_revision = self.disk_revision.wrapping_add(1);
            self.focus.focus(window);
        }
        self.settings_open = false;
        cx.notify();
    }

    fn toggle_split(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.store_active_tab();
        if self.layout.split {
            let keep = self
                .open
                .clone()
                .or_else(|| self.layout.paths[1 - self.layout.active].clone());
            self.layout.split = false;
            self.layout.paths = [keep, None];
            self.layout.active = 0;
            if self.open.is_none() {
                self.select_pane(0, window, cx);
            }
        } else {
            self.layout.split = true;
            self.layout.paths[0].clone_from(&self.open);
            self.layout.paths[1] = self
                .tabs
                .iter()
                .find(|tab| Some(&tab.path) != self.open.as_ref())
                .map(|tab| tab.path.clone());
            self.select_pane(1, window, cx);
        }
        cx.notify();
    }

    /// Shows the note at `path` in the left or right pane, splitting the page to make room.
    fn place_note(
        &mut self,
        path: PathBuf,
        pane: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let other = 1 - pane;
        if self.layout.split {
            if self.layout.paths[other].as_ref() == Some(&path) {
                self.layout.paths.swap(0, 1);
            }
            self.select_pane(pane, window, cx);
        } else if self.open.as_ref().is_some_and(|open| *open != path) {
            self.store_active_tab();
            self.layout.split = true;
            self.layout.paths = [None, None];
            self.layout.paths[other].clone_from(&self.open);
            self.layout.active = other;
            self.select_pane(pane, window, cx);
        }
        self.open_note(path, window, cx);
    }

    /// Returns the half of the page that takes a dragged note into the pane on that side.
    fn drop_zone(pane: usize, theme: Theme, cx: &mut Context<Self>) -> Div {
        div()
            .absolute()
            .top_0()
            .bottom_0()
            .w(relative(0.5))
            .when(pane == 0, Styled::left_0)
            .when(pane == 1, Styled::right_0)
            .rounded(px(14.))
            .drag_over::<DraggedNote>(move |style, _, _, _| {
                style.border_2().border_color(theme.accent)
            })
            .on_drop(cx.listener(move |this, note: &DraggedNote, window, cx| {
                this.place_note(note.0.clone(), pane, window, cx);
            }))
    }

    fn save_all(&mut self, cx: &App) -> bool {
        let active_saved = self.save(cx);
        self.store_active_tab();
        let mut failed = if active_saved {
            None
        } else {
            self.tabs
                .iter()
                .position(|tab| Some(&tab.path) == self.open.as_ref())
        };
        for index in 0..self.tabs.len() {
            let tab = &self.tabs[index];
            if Some(&tab.path) == self.open.as_ref() {
                continue;
            }
            if tab.conflict.is_some() {
                failed.get_or_insert(index);
                continue;
            }
            if !tab.dirty {
                continue;
            }
            let path = tab.path.clone();
            let text = tab.editor.read(cx).text().to_owned();
            let baseline = tab.baseline.clone();
            match self.report(notes::write_if_unchanged(&path, baseline.as_deref(), &text)) {
                Some(true) => {
                    self.tabs[index].baseline = Some(text);
                    self.tabs[index].dirty = false;
                }
                Some(false) => {
                    if let Some(disk) = self.report(notes::disk_text(&path)) {
                        self.tabs[index].conflict = Some(Conflict { disk });
                    }
                    failed.get_or_insert(index);
                }
                None => {
                    failed.get_or_insert(index);
                }
            }
        }
        if let Some(index) = failed {
            self.load_tab_state(index);
            return false;
        }
        active_saved
    }

    fn prepare_close(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let saved = self.save_all(cx);
        self.persist_session(cx);
        if !saved {
            self.settings_open = false;
            self.editor.focus_handle(cx).focus(window);
            self.error
                .get_or_insert_with(|| "Resolve unsaved changes before closing Wevren.".into());
            cx.notify();
        }
        saved
    }

    fn start_watcher(&mut self, cx: &mut Context<Self>) {
        self.watcher = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let (root, paths, revision) = this.update(cx, |this, cx| {
                    this.persist_session(cx);
                    (
                        this.settings.folder.clone(),
                        this.tabs
                            .iter()
                            .map(|tab| tab.path.clone())
                            .collect::<Vec<_>>(),
                        this.disk_revision,
                    )
                })?;
                let Some(root) = root else {
                    continue;
                };
                let scan_root = root.clone();
                let (entries, disk) = cx
                    .background_executor()
                    .spawn(async move {
                        let entries = notes::scan(&scan_root);
                        let disk: Vec<_> = paths
                            .into_iter()
                            .map(|path| {
                                let text = notes::disk_text(&path);
                                (path, text)
                            })
                            .collect();
                        (entries, disk)
                    })
                    .await;
                this.update(cx, |this, cx| {
                    if this.settings.folder.as_ref() != Some(&root)
                        || this.disk_revision != revision
                    {
                        return;
                    }
                    match entries {
                        Ok(entries) if entries != this.entries => {
                            this.entries = entries;
                            this.sync_entries(cx);
                            this.refresh_switcher(cx);
                            cx.notify();
                        }
                        Err(error) => this.watch_error(&error, cx),
                        _ => {}
                    }
                    for (path, text) in disk {
                        match text {
                            Ok(disk) if this.open.as_ref() == Some(&path) => {
                                this.apply_disk(disk, cx);
                            }
                            Ok(disk) => this.apply_inactive_disk(&path, disk, cx),
                            Err(error) => this.watch_error(&error, cx),
                        }
                    }
                    this.store_active_tab();
                })?;
            }
        }));
    }

    fn apply_inactive_disk(&mut self, path: &Path, disk: Option<String>, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.iter_mut().find(|tab| tab.path == path) else {
            return;
        };
        if tab.baseline == disk {
            if tab.conflict.take().is_some() {
                cx.notify();
            }
        } else if tab.dirty || disk.is_none() || tab.conflict.is_some() {
            if tab
                .conflict
                .as_ref()
                .is_none_or(|conflict| conflict.disk != disk)
            {
                tab.conflict = Some(Conflict { disk });
                cx.notify();
            }
        } else if let Some(text) = disk {
            tab.baseline = Some(text.clone());
            tab.editor
                .update(cx, |editor, cx| editor.reload_text(text, cx));
            cx.notify();
        }
    }

    fn watch_error(&mut self, error: &io::Error, cx: &mut Context<Self>) {
        let message: SharedString = format!("{error}").into();
        if self.error.as_ref() != Some(&message) {
            self.error = Some(message);
            cx.notify();
        }
    }

    fn apply_disk(&mut self, disk: Option<String>, cx: &mut Context<Self>) {
        if disk == self.baseline {
            if self.conflict.take().is_some() {
                if self.dirty {
                    self.schedule_save(cx);
                }
                cx.notify();
            }
            return;
        }
        if self.dirty || disk.is_none() || self.conflict.is_some() {
            let changed = self
                .conflict
                .as_ref()
                .is_none_or(|conflict| conflict.disk != disk);
            self.autosave.take();
            self.conflict = Some(Conflict { disk });
            if changed {
                cx.notify();
            }
        } else if let Some(text) = disk {
            self.baseline = Some(text.clone());
            self.disk_revision = self.disk_revision.wrapping_add(1);
            self.editor
                .update(cx, |editor, cx| editor.reload_text(text, cx));
            self.refresh_find(false, cx);
            cx.notify();
        }
    }

    fn begin_switcher(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.begin_workspace_search(false, window, cx);
    }

    fn begin_workspace_search(
        &mut self,
        content: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.settings.folder.is_none() {
            return;
        }
        if self.switcher.is_none() {
            let input = cx.new(|cx| Editor::single_line(String::new(), cx));
            cx.subscribe(&input, |this, _, _: &Edited, cx| {
                this.refresh_switcher(cx);
            })
            .detach();
            cx.subscribe(&input, |this, _, event: &Navigated, cx| {
                if let Some(switcher) = &mut this.switcher {
                    let count = switcher.results.len();
                    if count > 0 {
                        switcher.selected = if event.0 {
                            (switcher.selected + count - 1) % count
                        } else {
                            (switcher.selected + 1) % count
                        };
                        switcher.scroll.scroll_to_item(switcher.selected);
                    }
                }
                cx.notify();
            })
            .detach();
            cx.subscribe_in(&input, window, |this, _, _: &Submitted, window, cx| {
                if let Some(switcher) = &this.switcher {
                    this.open_search_result(switcher.selected, window, cx);
                }
            })
            .detach();
            cx.subscribe_in(&input, window, |this, _, _: &Cancelled, window, cx| {
                this.close_switcher(window, cx);
            })
            .detach();
            self.switcher = Some(Switcher {
                input,
                results: Vec::new(),
                selected: 0,
                content,
                hits: workspace_search::Results::default(),
                busy: false,
                scroll: gpui::ScrollHandle::new(),
            });
            self.refresh_switcher(cx);
        }
        if let Some(switcher) = &self.switcher {
            switcher.input.focus_handle(cx).focus(window);
        }
        cx.notify();
    }

    fn refresh_switcher(&mut self, cx: &mut Context<Self>) {
        if self
            .switcher
            .as_ref()
            .is_some_and(|switcher| switcher.content)
        {
            self.refresh_content_search(cx);
            return;
        }
        let (Some(switcher), Some(root)) = (&mut self.switcher, &self.settings.folder) else {
            return;
        };
        let query = switcher.input.read(cx).text();
        let mut scored: Vec<_> = self
            .notes
            .iter()
            .filter_map(|path| {
                let title = notes::title(path);
                let score = search::fuzzy_score(&title, query).or_else(|| {
                    search::fuzzy_score(&notes::link_title(path, root), query)
                        .map(|score| score - 25)
                })?;
                Some((Reverse(score), title.to_lowercase(), path.clone()))
            })
            .collect();
        scored.sort();
        switcher.results = scored
            .into_iter()
            .take(10)
            .map(|(_, _, path)| path)
            .collect();
        switcher.selected = 0;
        cx.notify();
    }

    fn refresh_content_search(&mut self, cx: &mut Context<Self>) {
        self.store_active_tab();
        self.content_search.take();
        let Some(switcher) = &mut self.switcher else {
            return;
        };
        let input = switcher.input.clone();
        let query = input.read(cx).text().to_owned();
        switcher.results.clear();
        switcher.hits = workspace_search::Results::default();
        switcher.selected = 0;
        switcher.busy = !query.is_empty();
        switcher.scroll.set_offset(gpui::Point::default());
        if query.is_empty() {
            cx.notify();
            return;
        }
        let paths = self.notes.clone();
        let open: Vec<_> = self
            .tabs
            .iter()
            .filter(|tab| tab.dirty || tab.conflict.is_some())
            .map(|tab| (tab.path.clone(), tab.editor.read(cx).text().to_owned()))
            .collect();
        self.content_search = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(150))
                .await;
            let search_query = query.clone();
            let hits = cx
                .background_executor()
                .spawn(async move { workspace_search::find(&paths, &search_query, &open) })
                .await;
            this.update(cx, |this, cx| {
                if let Some(switcher) = &mut this.switcher
                    && switcher.input == input
                    && switcher.input.read(cx).text() == query
                {
                    switcher.results = hits.hits.iter().map(|hit| hit.path.clone()).collect();
                    switcher.hits = hits;
                    switcher.busy = false;
                    cx.notify();
                }
            })
        }));
        cx.notify();
    }

    fn open_search_result(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(switcher) = &self.switcher else {
            return;
        };
        let Some(path) = switcher.results.get(index).cloned() else {
            return;
        };
        let hit = switcher.hits.hits.get(index).cloned();
        let query = switcher.input.read(cx).text().to_owned();
        self.close_switcher(window, cx);
        if self.open_note_with_history(path, true, window, cx)
            && let Some(hit) = hit
            && let Some(range) = search::matches(self.editor.read(cx).text(), &query)
                .into_iter()
                .min_by_key(|range| range.start.abs_diff(hit.range.start))
        {
            self.editor
                .update(cx, |editor, cx| editor.select_range(range, cx));
        }
    }

    fn close_switcher(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.content_search.take();
        self.switcher = None;
        if self.open.is_some() && !self.settings_open {
            self.editor.focus_handle(cx).focus(window);
        } else {
            self.focus.focus(window);
        }
        cx.notify();
    }

    fn switcher_rows(
        &self,
        switcher: &Switcher,
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> Vec<Stateful<Div>> {
        switcher
            .results
            .iter()
            .enumerate()
            .map(|(index, path)| {
                let path = path.clone();
                let mut location = self
                    .settings
                    .folder
                    .as_ref()
                    .map_or_else(String::new, |root| notes::link_title(&path, root));
                let hit = switcher.hits.hits.get(index);
                if let Some(hit) = hit {
                    location = format!("{location}:{}", hit.line);
                }
                div()
                    .id(index)
                    .flex()
                    .flex_col()
                    .gap_1()
                    .px_4()
                    .py_2()
                    .cursor_pointer()
                    .when(index == switcher.selected, |row| row.bg(theme.hover))
                    .hover(|row| row.bg(theme.hover))
                    .child(div().text_color(theme.text).child(notes::title(&path)))
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(theme.muted)
                            .truncate()
                            .child(location),
                    )
                    .children(hit.map(|hit| {
                        div()
                            .truncate()
                            .text_color(theme.muted)
                            .child(hit.snippet.clone())
                    }))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_search_result(index, window, cx);
                        cx.stop_propagation();
                    }))
            })
            .collect()
    }

    fn switcher_page(&self, theme: Theme, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        let switcher = self.switcher.as_ref()?;
        let rows = self.switcher_rows(switcher, theme, cx);
        let panel =
            div()
                .id("switcher-panel")
                .flex()
                .flex_col()
                .w(px(520.))
                .max_w_full()
                .bg(theme.sheet)
                .border_1()
                .border_color(theme.line)
                .rounded_lg()
                .shadow_lg()
                .overflow_hidden()
                .font_family(theme.ui_font)
                .text_size(px(14.))
                .on_click(|_, _, cx| cx.stop_propagation())
                .child(
                    label(
                        if switcher.content {
                            "SEARCH WORKSPACE"
                        } else {
                            "OPEN NOTE"
                        },
                        theme,
                    )
                    .px_4()
                    .pt_3()
                    .pb_2(),
                )
                .child(
                    div()
                        .mx_3()
                        .mb_2()
                        .px_3()
                        .py_2()
                        .border_1()
                        .border_color(theme.accent)
                        .rounded_md()
                        .child(switcher.input.clone()),
                )
                .when(switcher.results.is_empty(), |panel| {
                    panel.child(div().px_4().py_3().text_color(theme.muted).child(
                        if switcher.busy {
                            "Searching..."
                        } else if switcher.content && switcher.input.read(cx).text().is_empty() {
                            "Type to search note contents"
                        } else {
                            "No matching notes"
                        },
                    ))
                })
                .child(
                    div()
                        .id("search-results")
                        .max_h(px(340.))
                        .overflow_y_scroll()
                        .track_scroll(&switcher.scroll)
                        .children(rows),
                )
                .when(switcher.content, |panel| {
                    panel.child(
                        div()
                            .px_4()
                            .py_1()
                            .text_color(theme.muted)
                            .text_size(px(11.))
                            .child(switcher.hits.summary()),
                    )
                })
                .child(
                    div()
                        .px_4()
                        .py_2()
                        .border_t_1()
                        .border_color(theme.line)
                        .text_color(theme.muted)
                        .text_size(px(11.))
                        .child("Up/Down to choose · Enter to open · Escape to close"),
                );
        Some(
            div()
                .id("switcher-overlay")
                .absolute()
                .size_full()
                .flex()
                .justify_center()
                .items_start()
                .pt(px(72.))
                .px_4()
                .bg(gpui::rgba(0x0000_0066))
                .on_click(cx.listener(|this, _, window, cx| this.close_switcher(window, cx)))
                .child(panel),
        )
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
        self.sync_entries(cx);
    }

    fn sync_entries(&mut self, cx: &mut App) {
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
        if self.selected_folder.as_ref().is_some_and(|path| {
            Some(path) != self.settings.folder.as_ref()
                && !self
                    .entries
                    .iter()
                    .any(|entry| entry.directory && entry.path == *path)
        }) {
            self.selected_folder.clone_from(&self.settings.folder);
        }
        let titles = self.settings.folder.as_ref().map_or_else(Vec::new, |root| {
            self.notes
                .iter()
                .map(|path| notes::link_title(path, root))
                .collect()
        });
        self.editor.update(cx, |editor, cx| {
            editor.set_note_titles(titles.clone(), cx);
        });
        for tab in &self.tabs {
            if tab.editor != self.editor {
                tab.editor.update(cx, |editor, cx| {
                    editor.set_note_titles(titles.clone(), cx);
                });
            }
        }
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
        if !self.save_all(cx) {
            self.settings_open = false;
            return;
        }
        self.open = None;
        self.rename_editor = None;
        self.find = None;
        self.baseline = None;
        self.conflict = None;
        self.disk_revision = self.disk_revision.wrapping_add(1);
        self.expanded.clear();
        self.navigation.clear();
        self.navigation_index = None;
        self.tabs.clear();
        self.layout.split = false;
        self.layout.paths = [None, None];
        self.layout.active = 0;
        self.move_picker = false;
        self.entries.clear();
        self.selected_folder = Some(folder.clone());
        self.folder_input = None;
        self.settings.folder = Some(folder);
        self.report(self.settings.save());
        self.persist_session(cx);
        self.settings_open |= self.error.is_some();
        self.reload(cx);
    }

    /// Shows the note at `path` in the editor.
    fn open_note(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.open_note_with_history(path, true, window, cx);
    }

    fn open_note_with_history(
        &mut self,
        path: PathBuf,
        record: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.error = None;
        self.save(cx);
        self.store_active_tab();
        if self.activate_tab(&path, window, cx) {
            self.disk_revision = self.disk_revision.wrapping_add(1);
            self.editor.focus_handle(cx).focus(window);
            if record {
                self.record_visit(path.clone());
            }
            self.open = Some(path);
            if self.layout.split && self.layout.paths[1 - self.layout.active] == self.open {
                self.layout.active = 1 - self.layout.active;
            }
            self.layout.paths[self.layout.active].clone_from(&self.open);
            self.selected_folder = self
                .open
                .as_ref()
                .and_then(|path| path.parent().map(Path::to_path_buf));
            self.reveal_open();
            self.rename_editor = None;
            self.find = None;
            self.settings_open = false;
            cx.notify();
            return true;
        }
        cx.notify();
        false
    }

    fn record_visit(&mut self, path: PathBuf) {
        if self
            .navigation_index
            .is_none_or(|index| self.navigation[index] != path)
        {
            self.navigation
                .truncate(self.navigation_index.map_or(0, |index| index + 1));
            self.navigation.push(path);
            self.navigation_index = Some(self.navigation.len() - 1);
        }
    }

    fn activate_tab(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let existing = self.tabs.iter().position(|tab| tab.path == path);
        let Some(disk) = self.report(notes::disk_text(path)) else {
            return false;
        };
        if disk.is_none() && existing.is_none() {
            self.error = Some("This note no longer exists.".into());
            return false;
        }
        self.editor.update(cx, |editor, cx| {
            editor.set_matches(Vec::new(), None, false, cx);
        });
        self.editor = if let Some(index) = existing {
            self.tabs[index].editor.clone()
        } else {
            let editor = Self::note_editor(window, cx);
            editor.update(cx, |editor, cx| {
                editor.set_text(disk.clone().unwrap_or_default(), cx);
            });
            self.tabs.push(Tab {
                path: path.to_path_buf(),
                editor: editor.clone(),
                baseline: disk.clone(),
                dirty: false,
                conflict: None,
            });
            editor
        };
        let index = existing.unwrap_or(self.tabs.len() - 1);
        self.baseline.clone_from(&self.tabs[index].baseline);
        self.conflict.clone_from(&self.tabs[index].conflict);
        self.dirty = self.tabs[index].dirty;
        if !self.dirty
            && self.conflict.is_none()
            && disk == self.baseline
            && let Some(text) = &disk
            && self.editor.read(cx).text() != text
        {
            self.editor
                .update(cx, |editor, cx| editor.reload_text(text.clone(), cx));
        }
        self.apply_disk(disk, cx);
        self.sync_entries(cx);
        true
    }

    fn close_tab(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.tabs.iter().position(|tab| tab.path == path) else {
            return;
        };
        if self.open.as_deref() != Some(path)
            && (self.tabs[index].dirty || self.tabs[index].conflict.is_some())
        {
            let editor = self.tabs[index].editor.clone();
            self.select_editor(&editor, window, cx);
        }
        let active = self.open.as_deref() == Some(path);
        if active && !self.save(cx) {
            self.settings_open = false;
            self.editor.focus_handle(cx).focus(window);
            cx.notify();
            return;
        }
        self.tabs.remove(index);
        for pane in &mut self.layout.paths {
            if pane.as_deref() == Some(path) {
                *pane = None;
            }
        }
        if active {
            self.open = None;
            self.baseline = None;
            self.conflict = None;
            self.find = None;
            self.rename_editor = None;
            self.disk_revision = self.disk_revision.wrapping_add(1);
            if let Some(tab) = self.tabs.get(index.min(self.tabs.len().saturating_sub(1))) {
                self.open_note(tab.path.clone(), window, cx);
            } else {
                self.focus.focus(window);
            }
        }
        if self.layout.split && self.layout.paths.contains(&None) {
            self.toggle_split(window, cx);
        }
        cx.notify();
    }

    fn step_tab(&mut self, backwards: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self
            .tabs
            .iter()
            .position(|tab| Some(&tab.path) == self.open.as_ref())
        else {
            return;
        };
        let count = self.tabs.len();
        let next = if backwards {
            (index + count - 1) % count
        } else {
            (index + 1) % count
        };
        self.open_note(self.tabs[next].path.clone(), window, cx);
    }

    fn tab_bar(&self, theme: Theme, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        if self.tabs.is_empty() && !self.layout.split {
            return None;
        }
        let tabs = self.tabs.iter().enumerate().map(|(index, tab)| {
            let path = tab.path.clone();
            let close_path = path.clone();
            let dragged = DraggedNote(path.clone(), gpui::Point::default());
            let active = self.open.as_ref() == Some(&path) && !self.settings_open;
            let title = notes::title(&path);
            let duplicate = self
                .tabs
                .iter()
                .any(|other| other.path != path && notes::title(&other.path) == title);
            let mut title = if duplicate {
                self.settings
                    .folder
                    .as_ref()
                    .map_or(title.clone(), |root| notes::link_title(&path, root))
            } else {
                title
            };
            if (active && (self.dirty || self.conflict.is_some()))
                || tab.dirty
                || tab.conflict.is_some()
            {
                title.push_str(" *");
            }
            div()
                .id(("tab", index))
                .flex()
                .flex_none()
                .items_center()
                .gap_2()
                .px_3()
                .py_2()
                .max_w(px(240.))
                .rounded_md()
                .cursor_pointer()
                .text_color(if active { theme.text } else { theme.muted })
                .when(active, |tab| tab.bg(theme.sheet))
                .hover(|tab| tab.bg(theme.hover))
                .child(div().min_w_0().truncate().child(title))
                .on_drag(dragged, |note, offset, _, cx| {
                    cx.new(|_| DraggedNote(note.0.clone(), offset))
                })
                .child(
                    div()
                        .id(("close-tab", index))
                        .flex_none()
                        .font_family(ICON_FONT)
                        .text_size(px(10.))
                        .rounded_sm()
                        .px_1()
                        .hover(|button| button.bg(theme.hover).text_color(theme.text))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.close_tab(&close_path, window, cx);
                        }))
                        .child("\u{e8bb}"),
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.open_note(path.clone(), window, cx);
                }))
        });
        Some(
            div()
                .id("tabs")
                .flex()
                .flex_none()
                .gap_1()
                .w_full()
                .overflow_x_scroll()
                .children(tabs),
        )
    }

    fn navigation_target(&self, back: bool) -> Option<usize> {
        let current = self.navigation_index?;
        if back {
            (0..current)
                .rev()
                .find(|&index| self.navigation[index].is_file())
        } else {
            (current + 1..self.navigation.len()).find(|&index| self.navigation[index].is_file())
        }
    }

    fn navigate(&mut self, back: bool, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(index) = self.navigation_target(back)
            && self.open_note_with_history(self.navigation[index].clone(), false, window, cx)
        {
            self.navigation_index = Some(index);
            cx.notify();
        }
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
        self.store_active_tab();
        self.disk_revision = self.disk_revision.wrapping_add(1);
        if self.conflict.is_some() {
            return;
        }
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
        if self.conflict.is_some() {
            return false;
        }
        if !self.dirty {
            return true;
        }
        let Some(path) = &self.open else {
            return false;
        };
        let path = path.clone();
        let text = self.editor.read(cx).text().to_owned();
        match self.report(notes::write_if_unchanged(
            &path,
            self.baseline.as_deref(),
            &text,
        )) {
            Some(true) => {
                self.baseline = Some(text);
                self.disk_revision = self.disk_revision.wrapping_add(1);
            }
            Some(false) => {
                if let Some(disk) = self.report(notes::disk_text(&path)) {
                    self.conflict = Some(Conflict { disk });
                }
                return false;
            }
            None => return false,
        }
        self.dirty = false;
        true
    }

    fn reload_conflict(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.open.clone() else {
            return;
        };
        let Some(disk) = self.report(notes::disk_text(&path)) else {
            cx.notify();
            return;
        };
        self.dirty = false;
        self.conflict = None;
        self.baseline.clone_from(&disk);
        self.disk_revision = self.disk_revision.wrapping_add(1);
        if let Some(text) = disk {
            self.editor
                .update(cx, |editor, cx| editor.reload_text(text, cx));
            self.refresh_find(false, cx);
            self.editor.focus_handle(cx).focus(window);
        } else {
            self.close_tab(&path, window, cx);
        }
        self.error = None;
        cx.notify();
    }

    fn keep_local(&mut self, cx: &mut Context<Self>) {
        let (Some(path), Some(conflict)) = (&self.open, &self.conflict) else {
            return;
        };
        let path = path.clone();
        let expected = conflict.disk.clone();
        let text = self.editor.read(cx).text().to_owned();
        match self.report(notes::write_if_unchanged(&path, expected.as_deref(), &text)) {
            Some(true) => {
                self.baseline = Some(text);
                self.conflict = None;
                self.dirty = false;
                self.error = None;
                self.disk_revision = self.disk_revision.wrapping_add(1);
                self.reload(cx);
            }
            Some(false) => {
                if let Some(disk) = self.report(notes::disk_text(&path)) {
                    self.conflict = Some(Conflict { disk });
                }
                self.error = Some(
                    "The file changed again. Review the conflict before keeping your version."
                        .into(),
                );
            }
            None => {}
        }
        cx.notify();
    }

    fn save_conflict_copy(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(path), Some(root)) = (&self.open, &self.settings.folder) else {
            return;
        };
        let folder = path
            .parent()
            .filter(|parent| parent.is_dir())
            .unwrap_or(root);
        let created = notes::create_unique(folder, &format!("{} (conflict)", notes::title(path)));
        if let Some(copy) = self.report(created) {
            let text = self.editor.read(cx).text();
            if self.report(notes::write_if_unchanged(&copy, Some(""), text)) == Some(true) {
                self.conflict = None;
                self.dirty = false;
                self.open_created(Ok(copy), window, cx);
            }
        }
        cx.notify();
    }

    fn conflict_bar(&self, theme: Theme, cx: &mut Context<Self>) -> Option<Div> {
        let conflict = self.conflict.as_ref()?;
        let removed = conflict.disk.is_none();
        let message = if removed {
            "This note was removed outside Wevren. Your text is still here."
        } else {
            "This note changed outside Wevren. Autosave is paused to protect both versions."
        };
        let buttons = [
            (
                "conflict-reload",
                if removed { "Close note" } else { "Reload disk" },
                0,
            ),
            ("conflict-keep", "Keep my version", 1),
            ("conflict-copy", "Save a copy", 2),
        ]
        .into_iter()
        .map(|(id, label, action)| {
            div()
                .id(id)
                .px_2()
                .py_1()
                .rounded_md()
                .cursor_pointer()
                .text_color(theme.accent)
                .hover(|style| style.bg(theme.hover))
                .on_click(cx.listener(move |this, _, window, cx| match action {
                    0 => this.reload_conflict(window, cx),
                    1 => this.keep_local(cx),
                    _ => this.save_conflict_copy(window, cx),
                }))
                .child(label)
        });
        Some(
            div()
                .flex()
                .flex_none()
                .flex_col()
                .gap_2()
                .px_4()
                .py_3()
                .border_b_1()
                .border_color(theme.line)
                .font_family(theme.ui_font)
                .text_size(px(12.))
                .text_color(theme.text)
                .child(message)
                .child(div().flex().flex_wrap().gap_2().children(buttons)),
        )
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
        let Some(root) = self.settings.folder.clone() else {
            return;
        };
        self.error = None;
        if self.save_all(cx) {
            let baseline = self.baseline.clone().unwrap_or_default();
            if let Some((renamed, text)) =
                self.report(links::rename(&root, &path, &title, &baseline))
            {
                self.apply_relocation(&path, renamed, text, window, cx);
            }
        }
        cx.notify();
    }

    fn apply_relocation(
        &mut self,
        old: &Path,
        path: PathBuf,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for visited in &mut self.navigation {
            if *visited == old {
                visited.clone_from(&path);
            }
        }
        for pane in &mut self.layout.paths {
            if pane.as_deref() == Some(old) {
                *pane = Some(path.clone());
            }
        }
        self.tabs.retain(|tab| tab.path != path || tab.path == old);
        for tab in &mut self.tabs {
            if tab.path == old {
                tab.path.clone_from(&path);
            }
        }
        self.selected_folder = path.parent().map(Path::to_path_buf);
        self.open = Some(path);
        self.baseline = Some(text.clone());
        self.disk_revision = self.disk_revision.wrapping_add(1);
        if self.editor.read(cx).text() != text {
            self.editor
                .update(cx, |editor, cx| editor.reload_text(text, cx));
        }
        self.refresh_find(false, cx);
        self.reload(cx);
        self.reveal_open();
        self.move_picker = false;
        self.cancel_rename(window, cx);
        self.store_active_tab();
    }

    fn begin_move(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.open.is_none() || self.rename_editor.is_some() {
            return;
        }
        self.settings_open = false;
        if self.save(cx) {
            self.move_picker = true;
            self.focus.focus(window);
        }
        cx.notify();
    }

    fn cancel_move(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.move_picker = false;
        self.editor.focus_handle(cx).focus(window);
        cx.notify();
    }

    fn finish_move(&mut self, folder: &Path, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(path), Some(root)) = (self.open.clone(), self.settings.folder.clone()) else {
            return;
        };
        self.error = None;
        if self.save_all(cx) {
            let baseline = self.baseline.clone().unwrap_or_default();
            if let Some((moved, text)) =
                self.report(links::move_note(&root, &path, folder, &baseline))
            {
                self.apply_relocation(&path, moved, text, window, cx);
            }
        }
        self.cancel_move(window, cx);
    }

    /// Moves the note dragged from the note list into `folder`.
    fn drop_note(
        &mut self,
        note: &Path,
        folder: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if note.parent() != Some(folder)
            && self.open_note_with_history(note.to_path_buf(), true, window, cx)
        {
            self.finish_move(folder, window, cx);
        }
    }

    fn move_rows(
        &self,
        root: &Path,
        note: &Path,
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> Vec<Stateful<Div>> {
        let folders = std::iter::once(root).chain(
            self.entries
                .iter()
                .filter(|entry| entry.directory)
                .map(|entry| entry.path.as_path()),
        );
        folders
            .enumerate()
            .map(|(index, folder)| {
                let current = note.parent() == Some(folder);
                let occupied = !current
                    && note
                        .file_name()
                        .is_some_and(|name| folder.join(name).exists());
                let label = if folder == root {
                    "Workspace root".to_owned()
                } else {
                    folder
                        .strip_prefix(root)
                        .unwrap_or(folder)
                        .to_string_lossy()
                        .replace('\\', "/")
                };
                let folder = folder.to_path_buf();
                div()
                    .id(("move-folder", index))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .px_4()
                    .py_2()
                    .text_color(theme.text)
                    .when(!current && !occupied, |row| {
                        row.cursor_pointer().hover(|row| row.bg(theme.hover))
                    })
                    .child(div().min_w_0().truncate().child(label))
                    .child(
                        div()
                            .flex_none()
                            .text_color(theme.muted)
                            .text_size(px(11.))
                            .child(if current {
                                "Current folder"
                            } else if occupied {
                                "Name already exists"
                            } else {
                                ""
                            }),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        if !current && !occupied {
                            this.finish_move(&folder, window, cx);
                        }
                    }))
            })
            .collect()
    }

    fn move_page(&self, theme: Theme, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        if !self.move_picker {
            return None;
        }
        let (Some(root), Some(note)) = (&self.settings.folder, &self.open) else {
            return None;
        };
        let rows = self.move_rows(root, note, theme, cx);
        let panel = div()
            .id("move-panel")
            .flex()
            .flex_col()
            .w(px(440.))
            .max_w_full()
            .bg(theme.sheet)
            .border_1()
            .border_color(theme.line)
            .rounded_lg()
            .shadow_lg()
            .overflow_hidden()
            .on_click(|_, _, cx| cx.stop_propagation())
            .child(label("MOVE NOTE", theme).px_4().pt_3().pb_2())
            .child(
                div()
                    .px_4()
                    .pb_2()
                    .text_color(theme.muted)
                    .child(notes::title(note)),
            )
            .child(
                div()
                    .id("move-folders")
                    .max_h(px(340.))
                    .overflow_y_scroll()
                    .children(rows),
            )
            .child(
                div()
                    .id("cancel-move")
                    .px_4()
                    .py_3()
                    .cursor_pointer()
                    .text_color(theme.muted)
                    .hover(|button| button.text_color(theme.text))
                    .on_click(cx.listener(|this, _, window, cx| this.cancel_move(window, cx)))
                    .child("Cancel"),
            );
        Some(
            div()
                .id("move-overlay")
                .absolute()
                .size_full()
                .flex()
                .justify_center()
                .items_start()
                .pt(px(72.))
                .px_4()
                .bg(gpui::rgba(0x0000_0066))
                .on_click(cx.listener(|this, _, window, cx| this.cancel_move(window, cx)))
                .child(panel),
        )
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

    fn save_shortcuts(&mut self, bindings: Bindings, cx: &mut Context<Self>) {
        let mut settings = self.settings.clone();
        settings.shortcuts = bindings;
        match settings.save() {
            Ok(()) => {
                cx.set_global(settings.shortcuts.clone());
                self.settings = settings;
                self.shortcut_capture = None;
                self.shortcut_error = None;
            }
            Err(error) => self.shortcut_error = Some(format!("Could not save shortcuts: {error}")),
        }
        cx.notify();
    }

    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(action) = self.shortcut_capture {
            if !self.settings_open && self.settings.folder.is_some() {
                self.shortcut_capture = None;
            } else {
                if matches!(
                    event.keystroke.key.as_str(),
                    "control" | "ctrl" | "alt" | "shift" | "cmd" | "super" | "win" | "fn"
                ) {
                    cx.stop_propagation();
                    return;
                }
                if event.keystroke.key == "escape" {
                    self.shortcut_capture = None;
                    self.shortcut_error = None;
                } else {
                    match Chord::capture(&event.keystroke)
                        .and_then(|chord| self.settings.shortcuts.changed(action, chord))
                    {
                        Ok(bindings) => self.save_shortcuts(bindings, cx),
                        Err(error) => self.shortcut_error = Some(error),
                    }
                }
                cx.notify();
                cx.stop_propagation();
                return;
            }
        }
        let action = self.settings.shortcuts.action(&event.keystroke);
        if self.move_picker {
            if event.keystroke.key == "escape" {
                self.cancel_move(window, cx);
            }
            cx.stop_propagation();
            return;
        }
        if self.switcher.is_some() {
            if action == Some(Action::Switcher) || action == Some(Action::WorkspaceSearch) {
                self.close_switcher(window, cx);
            }
            cx.stop_propagation();
            return;
        }
        if self.find.is_some() && event.keystroke.key == "escape" {
            self.close_find(window, cx);
            cx.stop_propagation();
            return;
        }
        let Some(action) = action else {
            return;
        };
        match action {
            Action::Switcher => self.begin_switcher(window, cx),
            Action::WorkspaceSearch => self.begin_workspace_search(true, window, cx),
            Action::Find => self.begin_find(window, cx),
            Action::Save => {
                self.save(cx);
            }
            Action::NewNote => self.new_note(window, cx),
            Action::Split => self.toggle_split(window, cx),
            Action::Move => self.begin_move(window, cx),
            Action::NextTab => self.step_tab(false, window, cx),
            Action::PreviousTab => self.step_tab(true, window, cx),
            Action::CloseTab => {
                if let Some(path) = self.open.clone() {
                    self.close_tab(&path, window, cx);
                }
            }
            Action::Rename => self.begin_rename(window, cx),
            Action::Back => self.navigate(true, window, cx),
            Action::Forward => self.navigate(false, window, cx),
            Action::NextMatch => self.step_find(false, cx),
            Action::PreviousMatch => self.step_find(true, cx),
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
        let drop_root = cx.listener(|this, note: &DraggedNote, window, cx| {
            if let Some(root) = this.settings.folder.clone() {
                this.drop_note(&note.0, &root, window, cx);
            }
        });
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
        let list = div()
            .id("notes")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .pt_3()
            .overflow_y_scroll()
            .on_drop(drop_root)
            .child(header)
            .children(folder_input)
            .when(self.entries.is_empty(), |list| list.child(empty))
            .children(rows);
        div()
            .flex()
            .flex_col()
            .flex_none()
            .min_h_0()
            .w(px(SIDEBAR_WIDTH))
            .child(list)
            .when(!self.settings_open, |sidebar| {
                sidebar.children(self.outline_panel(theme, cx))
            })
    }

    fn outline_panel(&self, theme: Theme, cx: &mut Context<Self>) -> Option<Div> {
        let path = self.open.as_ref()?;
        let headings = markdown::outline(self.editor.read(cx).text());
        let empty = headings.is_empty();
        let editor = self.editor.clone();
        let rows = headings.into_iter().map(|heading| {
            let offset = heading.offset;
            let editor = editor.clone();
            div()
                .id(("outline-heading", offset))
                .flex_none()
                .pl(px(8. + f32::from(heading.level - 1) * 12.))
                .pr_2()
                .py_1()
                .rounded_md()
                .cursor_pointer()
                .hover(|style| style.bg(theme.hover).text_color(theme.text))
                .on_click(cx.listener(move |this, _, window, cx| {
                    if this.open.is_some() && this.editor == editor {
                        editor.update(cx, |editor, cx| editor.select_range(offset..offset, cx));
                        editor.focus_handle(cx).focus(window);
                    }
                }))
                .child(div().truncate().child(if heading.title.is_empty() {
                    "Untitled heading".to_owned()
                } else {
                    heading.title
                }))
        });
        Some(
            div()
                .flex()
                .flex_col()
                .flex_none()
                .gap_1()
                .border_t_1()
                .border_color(theme.line)
                .mt_2()
                .pt_3()
                .pb_2()
                .child(label("OUTLINE", theme).px_3())
                .child(
                    div()
                        .px_3()
                        .text_size(px(11.))
                        .text_color(theme.faint)
                        .truncate()
                        .child(notes::title(path)),
                )
                .child(
                    div()
                        .id("note-outline")
                        .mx_1()
                        .max_h(px(200.))
                        .overflow_y_scroll()
                        .text_size(px(12.))
                        .text_color(theme.muted)
                        .when(empty, |list| {
                            list.child(div().px_2().py_1().child("No headings yet"))
                        })
                        .children(rows),
                ),
        )
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
                            .id("workspace-search")
                            .cursor_pointer()
                            .font_family(ICON_FONT)
                            .text_size(px(12.))
                            .text_color(theme.muted)
                            .hover(|button| button.text_color(theme.text))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.begin_workspace_search(true, window, cx);
                            }))
                            .child("\u{e721}"),
                    )
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
        let folder = if directory {
            Some(path.clone())
        } else {
            path.parent().map(Path::to_path_buf)
        };
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
            .when(directory, |row| {
                row.drag_over::<DraggedNote>(move |style, _, _, _| style.bg(theme.hover))
            })
            .when(!directory, |row| {
                row.on_drag(
                    DraggedNote(path.clone(), gpui::Point::default()),
                    |note, offset, _, cx| cx.new(|_| DraggedNote(note.0.clone(), offset)),
                )
            })
            .on_drop(cx.listener(move |this, note: &DraggedNote, window, cx| {
                if let Some(folder) = &folder {
                    this.drop_note(&note.0, folder, window, cx);
                }
            }))
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

    fn note_title(
        &self,
        path: &Path,
        pane: usize,
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let active = pane == self.layout.active;
        if let Some(input) = self.rename_editor.as_ref().filter(|_| active) {
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
                .id(("note-title", pane))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.select_pane(pane, window, cx);
                    this.begin_rename(window, cx);
                }))
                .into_any_element()
        }
    }

    /// Returns the page a note is read and edited on.
    fn note_page(
        &self,
        path: &Path,
        editor: &Entity<Editor>,
        pane: usize,
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let active = pane == self.layout.active;
        let title = self.note_title(path, pane, theme, cx);
        let column = column()
            .min_h_full()
            .pb_8()
            .line_height(relative(1.7))
            .child(title)
            .child(rule(theme))
            .child(editor.clone());
        let scroll = div()
            .id(("note", pane))
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .w_full()
            .items_center()
            .overflow_y_scroll()
            .track_scroll(editor.read(cx).scroll_handle())
            .child(column);
        sheet(theme)
            .children(active.then(|| self.conflict_bar(theme, cx)).flatten())
            .when(active && self.find.is_some(), |sheet| {
                sheet.child(self.find_bar(theme, cx))
            })
            .child(scroll)
            .into_any_element()
    }

    fn pane_page(&self, pane: usize, theme: Theme, cx: &mut Context<Self>) -> Stateful<Div> {
        let tab = self.layout.paths[pane]
            .as_ref()
            .and_then(|path| self.tabs.iter().find(|tab| tab.path == *path));
        let content = tab.map_or_else(
            || empty_page(theme),
            |tab| self.note_page(&tab.path, &tab.editor, pane, theme, cx),
        );
        div()
            .id(("pane", pane))
            .capture_any_mouse_down(cx.listener(
                move |this, event: &gpui::MouseDownEvent, window, cx| {
                    if event.button == MouseButton::Left && this.layout.active != pane {
                        this.select_pane(pane, window, cx);
                    }
                },
            ))
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .gap_1()
            .when(self.layout.split, |view| {
                view.child(
                    div()
                        .id(("pane-header", pane))
                        .px_3()
                        .py_1()
                        .truncate()
                        .cursor_pointer()
                        .text_size(px(11.))
                        .text_color(if pane == self.layout.active {
                            theme.accent
                        } else {
                            theme.muted
                        })
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.select_pane(pane, window, cx);
                        }))
                        .child(format!(
                            "{}{}",
                            tab.map_or_else(
                                || "No note is open".to_owned(),
                                |tab| notes::title(&tab.path),
                            ),
                            if tab.is_some_and(|tab| tab.dirty || tab.conflict.is_some()) {
                                " (unsaved changes)"
                            } else {
                                ""
                            }
                        )),
                )
            })
            .when(tab.is_none(), |view| {
                view.on_click(
                    cx.listener(move |this, _, window, cx| this.select_pane(pane, window, cx)),
                )
            })
            .child(content)
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
            )
            .child(rule(theme))
            .child(label("KEYBOARD SHORTCUTS", theme))
            .child(
                div().font_family(theme.ui_font).text_sm().text_color(theme.muted)
                    .child("Click a binding, then press its new keys. Escape cancels. Changes save immediately."),
            )
            .child(
                div().id("restore-shortcuts").px_3().py_1p5().rounded_md()
                    .border_1().border_color(theme.accent).text_color(theme.accent)
                    .font_family(theme.ui_font).text_sm().cursor_pointer()
                    .hover(|style| style.bg(theme.hover))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.focus.focus(window);
                        this.save_shortcuts(Bindings::default(), cx);
                    }))
                    .child("Restore default shortcuts"),
            )
            .children(self.shortcut_error.as_ref().filter(|_| self.shortcut_capture.is_none()).map(|error| {
                div().text_color(theme.danger).font_family(theme.ui_font).text_sm().child(error.clone())
            }))
            .child(label("APP", theme))
            .children(self.shortcut_rows(false, theme, cx))
            .child(label("FORMATTING", theme))
            .children(self.shortcut_rows(true, theme, cx))
            .pb_8().flex_none();
        sheet(theme)
            .id("settings-page")
            .min_h_0()
            .overflow_y_scroll()
            .items_center()
            .child(column)
            .into_any_element()
    }

    fn shortcut_rows(
        &self,
        formatting: bool,
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> Vec<Stateful<Div>> {
        shortcuts::DEFAULTS
            .iter()
            .filter(|(action, _, _)| action.formatting().is_some() == formatting)
            .enumerate()
            .map(|(index, &(action, name, _))| {
                let capturing = self.shortcut_capture == Some(action);
                div()
                    .id((
                        if formatting {
                            "format-binding"
                        } else {
                            "app-binding"
                        },
                        index,
                    ))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .w_full()
                    .font_family(theme.ui_font)
                    .text_sm()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_3()
                            .child(name)
                            .child(
                                div()
                                    .id(("binding", action as usize))
                                    .px_3()
                                    .py_1p5()
                                    .rounded_md()
                                    .border_1()
                                    .border_color(if capturing { theme.accent } else { theme.line })
                                    .text_color(theme.accent)
                                    .cursor_pointer()
                                    .hover(|style| style.bg(theme.hover))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.shortcut_capture =
                                            if this.shortcut_capture == Some(action) {
                                                None
                                            } else {
                                                Some(action)
                                            };
                                        this.shortcut_error = None;
                                        this.focus.focus(window);
                                        cx.notify();
                                    }))
                                    .child(if capturing {
                                        "Press keys (Esc or click to cancel)".to_owned()
                                    } else {
                                        self.settings.shortcuts.chord(action).to_string()
                                    }),
                            ),
                    )
                    .children(
                        self.shortcut_error
                            .as_ref()
                            .filter(|_| capturing)
                            .map(|error| div().text_color(theme.danger).child(error.clone())),
                    )
            })
            .collect()
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
fn titlebar(
    theme: Theme,
    can_back: bool,
    can_forward: bool,
    cx: &mut Context<Wevren>,
) -> impl IntoElement {
    let [min, max, close] = CAPTION_GLYPHS;
    div()
        .flex()
        .flex_none()
        .h(px(TITLEBAR_HEIGHT))
        .children(
            [
                ("\u{e72b}", true, can_back),
                ("\u{e72a}", false, can_forward),
            ]
            .into_iter()
            .map(|(glyph, back, enabled)| {
                icon_button(glyph, 12., theme.hover, theme)
                    .when(enabled, Styled::cursor_pointer)
                    .when(!enabled, |button| button.opacity(0.35))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        if enabled {
                            this.navigate(back, window, cx);
                        }
                    }))
            }),
        )
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
                    this.shortcut_capture = None;
                    this.shortcut_error = None;
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
        .child(
            icon_button(min, 10., theme.hover, theme)
                .cursor_pointer()
                .on_click(cx.listener(|_, _, window, cx| {
                    cx.stop_propagation();
                    window.minimize_window();
                })),
        )
        .child(
            icon_button(max, 10., theme.hover, theme)
                .cursor_pointer()
                .on_click(cx.listener(|this, _, window, cx| {
                    cx.stop_propagation();
                    this.report(window_controls::toggle_maximize(window));
                    cx.notify();
                })),
        )
        .child(
            icon_button(close, 10., theme.danger, theme)
                .cursor_pointer()
                .on_click(cx.listener(|this, _, window, cx| {
                    cx.stop_propagation();
                    if this.prepare_close(window, cx) {
                        window.remove_window();
                    }
                })),
        )
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

/// Note dragged from the note list, shown under the pointer by its title.
#[derive(Clone)]
struct DraggedNote(PathBuf, gpui::Point<gpui::Pixels>);

impl Render for DraggedNote {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let folder = self
            .0
            .parent()
            .and_then(Path::file_name)
            .map(|name| name.to_string_lossy().into_owned());
        div().pl(self.1.x + px(2.)).pt(self.1.y + px(2.)).child(
            div()
                .flex()
                .items_center()
                .gap_3()
                .max_w(px(260.))
                .px_3()
                .py(px(10.))
                .rounded(px(12.))
                .border_1()
                .border_color(theme.line)
                .bg(theme.sheet)
                .shadow_md()
                .font_family(theme.ui_font)
                .text_size(px(14.))
                .line_height(px(20.))
                .text_color(theme.text)
                .child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .justify_center()
                        .size_8()
                        .rounded_md()
                        .bg(Rgba {
                            a: 0.1,
                            ..theme.accent
                        })
                        .font_family(ICON_FONT)
                        .text_size(px(16.))
                        .text_color(theme.accent)
                        .child("\u{e8a5}"),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .child(
                            div()
                                .truncate()
                                .font_family(theme.text_font)
                                .child(notes::title(&self.0)),
                        )
                        .children(folder.map(|folder| {
                            div()
                                .truncate()
                                .text_size(px(11.))
                                .line_height(px(16.))
                                .text_color(theme.muted)
                                .child(folder)
                        })),
                ),
        )
    }
}

impl Render for Wevren {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.store_active_tab();
        let theme = *cx.global::<Theme>();
        let has_folder = self.settings.folder.is_some();
        let page = if self.settings_open || !has_folder {
            self.settings_page(theme, cx)
        } else {
            div()
                .relative()
                .flex()
                .flex_1()
                .min_h_0()
                .min_w_0()
                .gap_2()
                .child(self.pane_page(0, theme, cx))
                .when(self.layout.split, |view| {
                    view.child(self.pane_page(1, theme, cx))
                })
                .when(cx.has_active_drag(), |view| {
                    view.children([0, 1].map(|pane| Self::drop_zone(pane, theme, cx)))
                })
                .into_any_element()
        };
        let error = self
            .error
            .clone()
            .map(|error| div().px_4().pb_2().text_color(theme.danger).child(error));
        div()
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .bg(theme.ground)
            .font_family(theme.ui_font)
            .text_sm()
            .text_color(theme.text)
            .on_key_down(cx.listener(Self::key_down))
            .child(titlebar(
                theme,
                self.navigation_target(true).is_some(),
                self.navigation_target(false).is_some(),
                cx,
            ))
            .child(
                div()
                    .track_focus(&self.focus)
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .gap_2()
                    .px_2()
                    .pb_2()
                    .when(has_folder, |body| body.child(self.sidebar(theme, cx)))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .min_h_0()
                            .gap_2()
                            .children(self.tab_bar(theme, cx))
                            .child(page),
                    ),
            )
            .children(error)
            .children(self.switcher_page(theme, cx))
            .children(self.move_page(theme, cx))
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
