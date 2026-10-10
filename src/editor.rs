//! A plain-text editor for the body of a note.

use std::{mem, ops::Range, panic::Location};

use gpui::{
    App, AvailableSpace, Bounds, ClipboardItem, ElementId, ElementInputHandler, Entity,
    EntityInputHandler, EventEmitter, FocusHandle, Focusable, GlobalElementId, InspectorElementId,
    KeyDownEvent, Keystroke, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, PaintQuad,
    Pixels, Point, Rgba, ScrollHandle, SharedString, Style, TextAlign, TextRun, TextStyle,
    UTF16Selection, UnderlineStyle, Window, WrappedLine, div, fill, point, prelude::*, px,
    relative, size,
};
use unicode_segmentation::UnicodeSegmentation as _;

use crate::theme::Theme;

/// Width of the caret, thin yet visible on dense displays.
const CARET_WIDTH: Pixels = px(1.5);
/// Width a selected line break is drawn with, so that empty lines show as selected.
const BREAK_WIDTH: Pixels = px(6.);
/// Opacity of the selection, letting the text stay readable through it.
const SELECTION_ALPHA: f32 = 0.25;

/// Emitted whenever editing changes the text.
#[derive(Clone, Copy, Debug)]
pub struct Edited;

/// A hard line of text laid out at the current width.
#[derive(Clone, Debug)]
struct Row {
    /// Byte offset of the first character.
    start: usize,
    /// Distance from the top of the editor.
    top: Pixels,
    line: WrappedLine,
}

/// Editable, soft-wrapped plain text.
#[derive(Debug)]
pub struct Editor {
    text: SharedString,
    /// Selected bytes, empty when there is only a caret.
    selection: Range<usize>,
    /// Whether the caret sits at the start of the selection.
    reversed: bool,
    /// Text an input method is still composing.
    marked: Option<Range<usize>>,
    /// Whether a mouse drag is extending the selection.
    selecting: bool,
    /// Whether the next frame must scroll the caret into view.
    reveal: bool,
    focus: FocusHandle,
    scroll: ScrollHandle,
    rows: Vec<Row>,
    /// Window position of the first character.
    origin: Point<Pixels>,
    line_height: Pixels,
}

impl EventEmitter<Edited> for Editor {}

impl Focusable for Editor {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Editor {
    /// Creates an empty editor.
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            text: SharedString::default(),
            selection: 0..0,
            reversed: false,
            marked: None,
            selecting: false,
            reveal: false,
            focus: cx.focus_handle(),
            scroll: ScrollHandle::new(),
            rows: Vec::new(),
            origin: Point::default(),
            line_height: Pixels::ZERO,
        }
    }

    /// Returns the text being edited.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Returns the handle of the scroll area the editor keeps its caret visible in.
    #[must_use]
    pub fn scroll_handle(&self) -> &ScrollHandle {
        &self.scroll
    }

    /// Replaces the whole text without emitting [`Edited`], as when a note is opened.
    pub fn set_text(&mut self, text: String, cx: &mut Context<Self>) {
        self.text = text.into();
        self.selection = 0..0;
        self.reversed = false;
        self.marked = None;
        self.scroll.set_offset(Point::default());
        cx.notify();
    }

    /// Returns the byte offset of the caret.
    fn caret(&self) -> usize {
        if self.reversed {
            self.selection.start
        } else {
            self.selection.end
        }
    }

    /// Moves the caret to `offset`, extending the selection when `select` is set.
    fn move_to(&mut self, offset: usize, select: bool, cx: &mut Context<Self>) {
        let anchor = match (select, self.reversed) {
            (false, _) => offset,
            (true, false) => self.selection.start,
            (true, true) => self.selection.end,
        };
        self.selection = anchor.min(offset)..anchor.max(offset);
        self.reversed = offset < anchor;
        self.reveal = true;
        cx.notify();
    }

    /// Replaces `range` with `text` and places the caret after it.
    fn replace(&mut self, range: Range<usize>, text: &str, cx: &mut Context<Self>) {
        let caret = range.start + text.len();
        let mut edited = self.text.to_string();
        edited.replace_range(range, text);
        self.text = edited.into();
        self.selection = caret..caret;
        self.reversed = false;
        self.marked = None;
        self.reveal = true;
        cx.emit(Edited);
        cx.notify();
    }

    /// Replaces the selection with `text`.
    fn insert(&mut self, text: &str, cx: &mut Context<Self>) {
        self.replace(self.selection.clone(), text, cx);
    }

    /// Deletes the selection, or `range` when nothing is selected.
    fn delete(&mut self, range: Range<usize>, cx: &mut Context<Self>) {
        let range = if self.selection.is_empty() {
            range
        } else {
            self.selection.clone()
        };
        self.replace(range, "", cx);
    }

    /// Returns the start of the character before `offset`.
    fn previous(&self, offset: usize) -> usize {
        let before = self.text[..offset].grapheme_indices(true).next_back();
        before.map_or(0, |(index, _)| index)
    }

    /// Returns the end of the character after `offset`.
    fn next(&self, offset: usize) -> usize {
        let after = self.text[offset..].graphemes(true).next();
        offset + after.map_or(0, str::len)
    }

    /// Returns where `offset` is drawn, relative to the first character.
    fn position(&self, offset: usize) -> Point<Pixels> {
        let row = self
            .rows
            .partition_point(|row| row.start <= offset)
            .checked_sub(1);
        row.and_then(|row| {
            let row = &self.rows[row];
            let local = row
                .line
                .position_for_index(offset - row.start, self.line_height)?;
            Some(point(local.x, local.y + row.top))
        })
        .unwrap_or_default()
    }

    /// Returns the offset drawn closest to `position`, relative to the first character.
    fn offset_at(&self, position: Point<Pixels>) -> usize {
        let row = self
            .rows
            .partition_point(|row| row.top <= position.y)
            .saturating_sub(1);
        let Some(row) = self.rows.get(row) else {
            return 0;
        };
        let last_line = row.line.size(self.line_height).height - self.line_height;
        let y = (position.y - row.top).clamp(Pixels::ZERO, last_line);
        let local = row
            .line
            .closest_index_for_position(point(position.x, y), self.line_height);
        row.start + local.unwrap_or_else(|index| index)
    }

    /// Returns the offset `lines` below the caret, or at `x` on the caret's line.
    fn near_caret(&self, x: Option<Pixels>, lines: f32) -> usize {
        let caret = self.position(self.caret());
        let y = caret.y + self.line_height * (lines + 0.5);
        self.offset_at(point(x.unwrap_or(caret.x), y))
    }

    /// Returns the rectangles covering the selection in an editor of `width`.
    fn selection_quads(&self, width: Pixels, color: Rgba) -> Vec<PaintQuad> {
        let Range { start, end } = self.selection;
        let height = self.line_height;
        let quad = |from: Point<Pixels>, to: Point<Pixels>| {
            fill(
                Bounds::from_corners(self.origin + from, self.origin + to),
                color,
            )
        };
        let mut quads = Vec::new();
        for row in &self.rows {
            let row_end = row.start + row.line.len();
            if start == end || end < row.start || start > row_end {
                continue;
            }
            let from = self.position(start.max(row.start));
            let mut to = self.position(end.min(row_end));
            if end > row_end {
                to.x += BREAK_WIDTH;
            }
            to.y += height;
            if from.y + height == to.y {
                quads.push(quad(from, to));
            } else {
                quads.push(quad(from, point(width, from.y + height)));
                quads.push(quad(
                    point(Pixels::ZERO, from.y + height),
                    point(width, to.y - height),
                ));
                quads.push(quad(point(Pixels::ZERO, to.y - height), to));
            }
        }
        quads
    }

    /// Stores the laid out `lines` and scrolls the caret into view when asked to.
    fn layout(&mut self, lines: Vec<WrappedLine>, origin: Point<Pixels>, window: &Window) {
        self.origin = origin;
        self.line_height = window.line_height();
        let (mut start, mut top) = (0, Pixels::ZERO);
        self.rows = lines
            .into_iter()
            .map(|line| {
                let row = Row { start, top, line };
                start += row.line.len() + 1;
                top += row.line.size(self.line_height).height;
                row
            })
            .collect();
        if !mem::take(&mut self.reveal) {
            return;
        }
        let caret = origin.y + self.position(self.caret()).y;
        let view = self.scroll.bounds();
        let shift = if caret < view.top() {
            view.top() - caret
        } else if caret + self.line_height > view.bottom() {
            view.bottom() - caret - self.line_height
        } else {
            return;
        };
        let mut offset = self.scroll.offset();
        offset.y += shift;
        self.scroll.set_offset(offset);
        window.request_animation_frame();
    }

    /// Handles the keys that move the caret, delete, or use the clipboard.
    fn key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let Keystroke { modifiers, key, .. } = &event.keystroke;
        let (caret, select) = (self.caret(), modifiers.shift);
        let step = select || self.selection.is_empty();
        // AltGr reports as control and alt together, and must keep typing characters.
        let command = modifiers.secondary() && !modifiers.alt;
        match key.as_str() {
            "left" if step => self.move_to(self.previous(caret), select, cx),
            "left" => self.move_to(self.selection.start, false, cx),
            "right" if step => self.move_to(self.next(caret), select, cx),
            "right" => self.move_to(self.selection.end, false, cx),
            "up" => self.move_to(self.near_caret(None, -1.), select, cx),
            "down" => self.move_to(self.near_caret(None, 1.), select, cx),
            "home" => self.move_to(self.near_caret(Some(Pixels::ZERO), 0.), select, cx),
            "end" => self.move_to(self.near_caret(Some(Pixels::MAX), 0.), select, cx),
            "backspace" => self.delete(self.previous(caret)..caret, cx),
            "delete" => self.delete(caret..self.next(caret), cx),
            "enter" => self.insert("\n", cx),
            "tab" => self.insert("\t", cx),
            "a" if command => {
                self.selection = 0..self.text.len();
                self.reversed = false;
                cx.notify();
            }
            "c" | "x" if command => {
                let selected = self.text[self.selection.clone()].to_owned();
                cx.write_to_clipboard(ClipboardItem::new_string(selected));
                if key == "x" {
                    self.insert("", cx);
                }
            }
            "v" if command => {
                if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                    self.insert(&text.replace("\r\n", "\n"), cx);
                }
            }
            _ => return,
        }
        cx.stop_propagation();
    }

    /// Places the caret under the pointer and starts a drag selection.
    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.selecting = true;
        window.focus(&self.focus);
        self.move_to(
            self.offset_at(event.position - self.origin),
            event.modifiers.shift,
            cx,
        );
    }

    /// Extends the selection to the pointer while the button stays down.
    fn mouse_move(&mut self, event: &MouseMoveEvent, _window: &mut Window, cx: &mut Context<Self>) {
        self.selecting &= event.pressed_button == Some(MouseButton::Left);
        if self.selecting {
            self.move_to(self.offset_at(event.position - self.origin), true, cx);
        }
    }

    /// Returns the byte range for a UTF-16 `range`.
    fn range_from_utf16(&self, range: &Range<usize>) -> Range<usize> {
        from_utf16(&self.text, range.start)..from_utf16(&self.text, range.end)
    }

    /// Returns the UTF-16 range for a byte `range`.
    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        to_utf16(&self.text, range.start)..to_utf16(&self.text, range.end)
    }

    /// Returns the bytes an input method means to replace.
    fn input_range(&self, range_utf16: Option<&Range<usize>>) -> Range<usize> {
        range_utf16
            .map(|range| self.range_from_utf16(range))
            .or_else(|| self.marked.clone())
            .unwrap_or_else(|| self.selection.clone())
    }
}

/// Returns the byte offset of the UTF-16 `offset` in `text`.
fn from_utf16(text: &str, offset: usize) -> usize {
    let mut units = 0;
    let found = text.char_indices().find(|(_, character)| {
        let reached = units >= offset;
        units += character.len_utf16();
        reached
    });
    found.map_or(text.len(), |(index, _)| index)
}

/// Returns the UTF-16 offset of the byte `offset` in `text`.
fn to_utf16(text: &str, offset: usize) -> usize {
    text[..offset].encode_utf16().count()
}

impl EntityInputHandler for Editor {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range);
        *adjusted_range = Some(self.range_to_utf16(&range));
        Some(self.text[range].to_owned())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selection),
            reversed: self.reversed,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.marked.as_ref().map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.marked = None;
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.replace(self.input_range(range.as_ref()), text, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        new_text: &str,
        new_selected_range: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = self.input_range(range.as_ref());
        let start = range.start;
        self.replace(range, new_text, cx);
        if !new_text.is_empty() {
            self.marked = Some(start..start + new_text.len());
        }
        if let Some(selected) = new_selected_range {
            self.selection = start + from_utf16(new_text, selected.start)
                ..start + from_utf16(new_text, selected.end);
        }
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        _element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let range = self.range_from_utf16(&range_utf16);
        let mut end = self.position(range.end);
        end.y += self.line_height;
        Some(Bounds::from_corners(
            self.origin + self.position(range.start),
            self.origin + end,
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        Some(to_utf16(&self.text, self.offset_at(point - self.origin)))
    }
}

impl Render for Editor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex_grow()
            .track_focus(&self.focus)
            .cursor_text()
            .on_key_down(cx.listener(Self::key_down))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_move(cx.listener(Self::mouse_move))
            .child(EditorElement(cx.entity()))
    }
}

/// Lays `text` out in hard lines wrapped at `width`, underlining the `marked` range.
fn shape(
    text: SharedString,
    marked: Option<&Range<usize>>,
    style: &TextStyle,
    width: Option<Pixels>,
    window: &Window,
) -> Vec<WrappedLine> {
    let run = |len, underline| TextRun {
        len,
        font: style.font(),
        color: style.color,
        background_color: None,
        underline,
        strikethrough: None,
    };
    let composing = UnderlineStyle {
        thickness: px(1.),
        color: Some(style.color),
        wavy: false,
    };
    let runs = match marked {
        Some(marked) => vec![
            run(marked.start, None),
            run(marked.len(), Some(composing)),
            run(text.len() - marked.end, None),
        ],
        None => vec![run(text.len(), None)],
    };
    let font_size = style.font_size.to_pixels(window.rem_size());
    let lines = window
        .text_system()
        .shape_text(text, font_size, &runs, width, None);
    lines.map_or_else(|_| Vec::new(), Vec::from_iter)
}

/// Draws the text, selection and caret of an [`Editor`] and routes typing to it.
struct EditorElement(Entity<Editor>);

impl IntoElement for EditorElement {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for EditorElement {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let editor = self.0.read(cx);
        let (text, marked) = (editor.text.clone(), editor.marked.clone());
        let (style, line_height) = (window.text_style(), window.line_height());
        let mut layout = Style::default();
        layout.size.width = relative(1.).into();
        let id = window.request_measured_layout(layout, move |known, available, window, _cx| {
            let width = known.width.or(match available.width {
                AvailableSpace::Definite(width) => Some(width),
                AvailableSpace::MinContent | AvailableSpace::MaxContent => None,
            });
            let lines = shape(text.clone(), marked.as_ref(), &style, width, window);
            let height = lines.iter().fold(Pixels::ZERO, |sum, line| {
                sum + line.size(line_height).height
            });
            size(width.unwrap_or_default(), line_height.max(height))
        });
        (id, ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.0.update(cx, |editor, _cx| {
            let width = Some(bounds.size.width);
            let style = window.text_style();
            let lines = shape(
                editor.text.clone(),
                editor.marked.as_ref(),
                &style,
                width,
                window,
            );
            editor.layout(lines, bounds.origin, window);
        });
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        _prepaint: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let accent = cx.global::<Theme>().accent;
        let editor = self.0.read(cx);
        let (focus, rows, origin) = (editor.focus.clone(), editor.rows.clone(), editor.origin);
        let line_height = editor.line_height;
        let selection = Rgba {
            a: SELECTION_ALPHA,
            ..accent
        };
        let mut quads = editor.selection_quads(bounds.size.width, selection);
        if editor.selection.is_empty() && focus.is_focused(window) {
            let caret = Bounds::new(
                origin + editor.position(editor.caret()),
                size(CARET_WIDTH, line_height),
            );
            quads.push(fill(caret, accent));
        }
        window.handle_input(&focus, ElementInputHandler::new(bounds, self.0.clone()), cx);
        for quad in quads {
            window.paint_quad(quad);
        }
        for row in rows {
            let at = origin + point(Pixels::ZERO, row.top);
            // A line that cannot be painted is left out; there is nothing to fall back to.
            let _ = row
                .line
                .paint(at, line_height, TextAlign::Left, None, window, cx);
        }
    }
}
