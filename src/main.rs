//! Wevren, a note-taking app.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod settings;
mod theme;

use std::path::PathBuf;

use gpui::{
    App, Application, Bounds, Div, PathPromptOptions, Rgba, SharedString, Stateful,
    TitlebarOptions, Window, WindowBounds, WindowControlArea, WindowOptions, div, prelude::*, px,
    size,
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

/// The root view.
#[derive(Debug)]
struct Wevren {
    settings: Settings,
    /// Whether the settings page covers the open note.
    settings_open: bool,
    /// Why the last change to the settings could not be saved.
    error: Option<SharedString>,
}

impl Wevren {
    /// Creates the view from the saved settings.
    fn new() -> Self {
        Self {
            settings: Settings::load(),
            settings_open: false,
            error: None,
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
                this.set_folder(folder);
                cx.notify();
            })
        })
        .detach_and_log_err(cx);
    }

    /// Stores `folder` as the notes folder, keeping the settings open on failure.
    fn set_folder(&mut self, folder: PathBuf) {
        self.settings.folder = Some(folder);
        self.error = self
            .settings
            .save()
            .err()
            .map(|error| format!("{error:#}").into());
        self.settings_open |= self.error.is_some();
    }

    /// Returns the settings page, which doubles as the welcome page.
    fn settings_page(&self, theme: Theme, cx: &mut Context<Self>) -> Div {
        let folder = self.settings.folder.as_deref();
        let (title, action) = if folder.is_some() {
            ("Settings", "Change folder")
        } else {
            ("Welcome", "Choose folder")
        };
        let column = div()
            .flex()
            .flex_col()
            .items_start()
            .gap_5()
            .w_full()
            .max_w(px(COLUMN_WIDTH))
            .px_8()
            .pt(px(72.))
            .child(
                div()
                    .font_family(theme.display_font)
                    .text_size(px(38.))
                    .child(title),
            )
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
            .children(
                self.error
                    .clone()
                    .map(|error| div().text_color(theme.danger).child(error)),
            );
        sheet(theme).items_center().child(column)
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

/// Returns the short accent line that sets off a heading.
fn rule(theme: Theme) -> Div {
    div().w(px(36.)).h(px(2.)).rounded_full().bg(theme.accent)
}

/// Returns the note list.
fn sidebar(theme: Theme) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .flex_none()
        .gap_2()
        .w(px(SIDEBAR_WIDTH))
        .px_3()
        .pt_3()
        .text_color(theme.faint)
        .child(label("NOTES", theme))
        .child(div().font_family(theme.text_font).child("No notes yet"))
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

/// Returns the page shown while no note is open.
fn empty_page(theme: Theme) -> Div {
    sheet(theme)
        .items_center()
        .justify_center()
        .gap_4()
        .text_color(theme.muted)
        .child(rule(theme))
        .child("No note is open")
}

impl Render for Wevren {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let has_folder = self.settings.folder.is_some();
        let page = if self.settings_open || !has_folder {
            self.settings_page(theme, cx)
        } else {
            empty_page(theme)
        };
        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(theme.ground)
            .font_family(theme.ui_font)
            .text_sm()
            .text_color(theme.text)
            .child(titlebar(theme, cx))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .gap_2()
                    .px_2()
                    .pb_2()
                    .when(has_folder, |body| body.child(sidebar(theme)))
                    .child(page),
            )
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
            cx.new(|_| Wevren::new())
        })
        .expect("the main window must open at startup");
        cx.activate(true);
    });
}
