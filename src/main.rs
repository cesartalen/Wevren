//! Wevren, a note-taking app.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod theme;

use gpui::{
    App, Application, Bounds, Rgba, TitlebarOptions, Window, WindowBounds, WindowControlArea,
    WindowOptions, div, prelude::*, px, size,
};
use mimalloc::MiMalloc;
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
/// Icon typeface that ships with Windows 11.
const ICON_FONT: &str = "Segoe Fluent Icons";
/// Minimize, maximize and close glyphs in [`ICON_FONT`].
const CAPTION_GLYPHS: [&str; 3] = ["\u{e921}", "\u{e922}", "\u{e8bb}"];

/// The root view.
#[derive(Debug)]
struct Wevren;

/// Returns a titlebar button that performs the native action for `area`.
fn caption_button(
    glyph: &'static str,
    area: WindowControlArea,
    hover: Rgba,
    theme: Theme,
) -> impl IntoElement {
    div()
        .id(glyph)
        .flex()
        .items_center()
        .justify_center()
        .w(px(CAPTION_WIDTH))
        .h_full()
        .font_family(ICON_FONT)
        .text_size(px(10.))
        .text_color(theme.muted)
        .hover(|s| s.bg(hover).text_color(theme.text))
        .window_control_area(area)
        .child(glyph)
}

/// Returns a titlebar button that switches between the light and dark theme.
fn theme_toggle(theme: Theme, cx: &mut Context<Wevren>) -> impl IntoElement {
    div()
        .id("theme")
        .flex()
        .items_center()
        .justify_center()
        .w(px(CAPTION_WIDTH))
        .h_full()
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
        )
}

/// Returns the custom titlebar: a draggable wordmark and the window buttons.
fn titlebar(theme: Theme, cx: &mut Context<Wevren>) -> impl IntoElement {
    let [min, max, close] = CAPTION_GLYPHS;
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
        .child(theme_toggle(theme, cx))
        .child(caption_button(
            min,
            WindowControlArea::Min,
            theme.hover,
            theme,
        ))
        .child(caption_button(
            max,
            WindowControlArea::Max,
            theme.hover,
            theme,
        ))
        .child(caption_button(
            close,
            WindowControlArea::Close,
            theme.danger,
            theme,
        ))
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
        .child(div().text_size(px(11.)).child("NOTES"))
        .child(div().font_family(theme.text_font).child("No notes yet"))
}

/// Returns the inset sheet that shows the open note.
fn sheet(theme: Theme) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .flex_1()
        .min_w_0()
        .items_center()
        .justify_center()
        .gap_4()
        .bg(theme.sheet)
        .border_1()
        .border_color(theme.line)
        .rounded(px(14.))
        .font_family(theme.text_font)
        .text_size(px(16.))
        .text_color(theme.muted)
        .child(div().w(px(36.)).h(px(2.)).rounded_full().bg(theme.accent))
        .child("No note is open")
}

impl Render for Wevren {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
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
                    .child(sidebar(theme))
                    .child(sheet(theme)),
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
            cx.new(|_| Wevren)
        })
        .expect("the main window must open at startup");
        cx.activate(true);
    });
}
