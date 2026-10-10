//! A plain-text editor for the body of a note.

use std::{collections::VecDeque, mem, ops::Range, panic::Location};

use gpui::{
    App, AvailableSpace, Bounds, ClipboardItem, ElementId, ElementInputHandler, Entity,
    EntityInputHandler, EventEmitter, FocusHandle, Focusable, GlobalElementId, Hsla,
    InspectorElementId, KeyDownEvent, Keystroke, LayoutId, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point, Rgba, ScrollHandle, SharedString,
    Style, TextAlign, TextRun, TextStyle, UTF16Selection, UnderlineStyle, Window, WrappedLine, div,
    fill, point, prelude::*, px, relative, size,
};
use unicode_segmentation::UnicodeSegmentation as _;

use crate::{
    completion::{self, Completion},
    markdown,
    theme::Theme,
};

/// Width of the caret, thin yet visible on dense displays.
const CARET_WIDTH: Pixels = px(1.5);
/// Width a selected line break is drawn with, so that empty lines show as selected.
const BREAK_WIDTH: Pixels = px(6.);
/// Opacity of the selection, letting the text stay readable through it.
const SELECTION_ALPHA: f32 = 0.25;

const HISTORY_LIMIT: usize = 200;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Snapshot {
    text: SharedString,
    selection: Range<usize>,
    reversed: bool,
}

#[derive(Debug, Default)]
struct History {
    undo: VecDeque<Snapshot>,
    redo: Vec<Snapshot>,
}

impl History {
    fn record(&mut self, state: Snapshot) {
        if self.undo.len() == HISTORY_LIMIT {
            self.undo.pop_front();
        }
        self.undo.push_back(state);
        self.redo.clear();
    }

    fn undo(&mut self, current: Snapshot) -> Option<Snapshot> {
        let previous = self.undo.pop_back()?;
        self.redo.push(current);
        Some(previous)
    }

    fn redo(&mut self, current: Snapshot) -> Option<Snapshot> {
        let next = self.redo.pop()?;
        self.undo.push_back(current);
        Some(next)
    }
}

/// Emitted whenever editing changes the text.
#[derive(Clone, Copy, Debug)]
pub struct Edited;

/// Emitted with the title of the linked note when a link is followed.
#[derive(Clone, Debug)]
pub struct Followed(pub String);

#[derive(Clone, Copy, Debug)]
pub struct Submitted(pub bool);

#[derive(Clone, Copy, Debug)]
pub struct Cancelled;

#[derive(Clone, Copy, Debug)]
pub struct Navigated(pub bool);

/// A hard line of text laid out at the current width.
#[derive(Clone, Debug)]
struct Row {
    /// Byte offset of the first character.
    start: usize,
    /// Distance from the top of the editor.
    top: Pixels,
    line: WrappedLine,
    line_height: Pixels,
}

/// Editable, soft-wrapped plain text.
#[derive(Debug)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "the flags are independent of each other"
)]
pub struct Editor {
    text: SharedString,
    history: History,
    matches: Vec<Range<usize>>,
    active_match: Option<usize>,
    note_titles: Vec<String>,
    completion: Option<Completion>,
    single_line: bool,
    /// Selected bytes, empty when there is only a caret.
    selection: Range<usize>,
    /// Whether the caret sits at the start of the selection.
    reversed: bool,
    /// Text an input method is still composing.
    marked: Option<Range<usize>>,
    /// Whether a mouse drag is extending the selection.
    selecting: bool,
    /// Whether the pointer is over a link.
    over_link: bool,
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

impl EventEmitter<Followed> for Editor {}

impl EventEmitter<Submitted> for Editor {}

impl EventEmitter<Cancelled> for Editor {}

impl EventEmitter<Navigated> for Editor {}

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
            history: History::default(),
            matches: Vec::new(),
            active_match: None,
            note_titles: Vec::new(),
            completion: None,
            single_line: false,
            selection: 0..0,
            reversed: false,
            marked: None,
            selecting: false,
            over_link: false,
            reveal: false,
            focus: cx.focus_handle(),
            scroll: ScrollHandle::new(),
            rows: Vec::new(),
            origin: Point::default(),
            line_height: Pixels::ZERO,
        }
    }

    pub fn single_line(text: String, cx: &mut Context<Self>) -> Self {
        let mut editor = Self::new(cx);
        editor.single_line = true;
        editor.set_text(text, cx);
        editor.selection = 0..editor.text.len();
        editor
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
        self.history = History::default();
        self.matches.clear();
        self.active_match = None;
        self.completion = None;
        self.selection = 0..0;
        self.reversed = false;
        self.marked = None;
        self.scroll.set_offset(Point::default());
        cx.notify();
    }

    pub fn set_note_titles(&mut self, titles: Vec<String>, cx: &mut Context<Self>) {
        self.note_titles = titles;
        self.refresh_completion();
        cx.notify();
    }

    pub fn reload_text(&mut self, text: String, cx: &mut Context<Self>) {
        let selection = self.selection.clone();
        let scroll = self.scroll.offset();
        self.set_text(text, cx);
        let boundary = |offset: usize| {
            let mut offset = offset.min(self.text.len());
            while !self.text.is_char_boundary(offset) {
                offset -= 1;
            }
            offset
        };
        self.selection = boundary(selection.start)..boundary(selection.end);
        self.scroll.set_offset(scroll);
    }

    fn refresh_completion(&mut self) {
        if self.single_line || !self.selection.is_empty() || self.marked.is_some() {
            self.completion = None;
            return;
        }
        let selected = self
            .completion
            .as_ref()
            .and_then(|completion| completion.items.get(completion.selected).cloned());
        self.completion = completion::link(&self.text, self.caret(), &self.note_titles);
        if let Some(completion) = &mut self.completion
            && let Some(selected) = selected
            && let Some(index) = completion.items.iter().position(|title| *title == selected)
        {
            completion.selected = index;
        }
    }

    fn accept_completion(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(completion) = self.completion.take() else {
            return;
        };
        let Some(title) = completion.items.get(index) else {
            return;
        };
        let text = if completion.close {
            format!("{title}]]")
        } else {
            title.clone()
        };
        if self.text[completion.range.clone()] == text {
            self.move_to(completion.range.end, false, cx);
        } else {
            self.replace(completion.range, &text, cx);
        }
        self.completion = None;
    }

    fn completion_key(&mut self, key: &str, cx: &mut Context<Self>) -> bool {
        let Some(completion) = &mut self.completion else {
            return false;
        };
        match key {
            "up" => {
                completion.selected =
                    (completion.selected + completion.items.len() - 1) % completion.items.len();
            }
            "down" => completion.selected = (completion.selected + 1) % completion.items.len(),
            "enter" | "tab" => {
                let selected = completion.selected;
                self.accept_completion(selected, cx);
            }
            "escape" => self.completion = None,
            _ => return false,
        }
        cx.notify();
        true
    }

    fn completion_menu(&self, cx: &mut Context<Self>) -> Option<gpui::Stateful<gpui::Div>> {
        let completion = self.completion.as_ref()?;
        let theme = *cx.global::<Theme>();
        let caret = self.position(self.caret());
        let height = completion
            .items
            .iter()
            .fold(Pixels::ZERO, |height, _| height + px(32.));
        let below = caret.y + self.height_at(self.caret());
        let top = if self.origin.y + below + height > self.scroll.bounds().bottom() {
            (caret.y - height).max(Pixels::ZERO)
        } else {
            below
        };
        let items = completion.items.iter().enumerate().map(|(index, title)| {
            div()
                .id(index)
                .flex()
                .items_center()
                .h(px(32.))
                .px_3()
                .truncate()
                .cursor_pointer()
                .when(index == completion.selected, |item| {
                    item.bg(theme.hover).text_color(theme.text)
                })
                .hover(|item| item.bg(theme.hover).text_color(theme.text))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.accept_completion(index, cx);
                    this.focus.focus(window);
                }))
                .child(title.clone())
        });
        Some(
            div()
                .id("link-completion")
                .absolute()
                .left_0()
                .top(top)
                .w_full()
                .max_w(px(320.))
                .bg(theme.sheet)
                .border_1()
                .border_color(theme.line)
                .rounded_md()
                .shadow_md()
                .overflow_hidden()
                .font_family(theme.ui_font)
                .text_size(px(14.))
                .text_color(theme.muted)
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_mouse_up(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .children(items),
        )
    }

    pub fn selected_text(&self) -> &str {
        &self.text[self.selection.clone()]
    }

    pub fn selection_start(&self) -> usize {
        self.selection.start
    }

    pub fn set_matches(
        &mut self,
        matches: Vec<Range<usize>>,
        active: Option<usize>,
        select: bool,
        cx: &mut Context<Self>,
    ) {
        self.matches = matches;
        self.active_match = active;
        if select && let Some(range) = active.and_then(|index| self.matches.get(index)) {
            self.selection = range.clone();
            self.reversed = false;
            self.reveal = true;
            self.completion = None;
        }
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
        self.refresh_completion();
        self.reveal = true;
        cx.notify();
    }

    /// Replaces `range` with `text` and places the caret after it.
    fn replace(&mut self, range: Range<usize>, text: &str, cx: &mut Context<Self>) {
        let normalized;
        let text = if self.single_line {
            normalized = text.replace(['\r', '\n', '\t'], " ");
            normalized.as_str()
        } else {
            text
        };
        if self.text[range.clone()] == *text {
            return;
        }
        if self.marked.is_none() {
            self.history.record(self.snapshot());
        }
        let caret = range.start + text.len();
        let mut edited = self.text.to_string();
        edited.replace_range(range, text);
        self.text = edited.into();
        self.selection = caret..caret;
        self.reversed = false;
        self.marked = None;
        self.reveal = true;
        self.completion = None;
        self.refresh_completion();
        cx.emit(Edited);
        cx.notify();
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            text: self.text.clone(),
            selection: self.selection.clone(),
            reversed: self.reversed,
        }
    }

    fn restore_history(&mut self, redo: bool, cx: &mut Context<Self>) {
        let current = self.snapshot();
        let state = if redo {
            self.history.redo(current)
        } else {
            self.history.undo(current)
        };
        if let Some(state) = state {
            self.text = state.text;
            self.selection = state.selection;
            self.reversed = state.reversed;
            self.marked = None;
            self.selecting = false;
            self.over_link = false;
            self.reveal = true;
            self.refresh_completion();
            cx.emit(Edited);
            cx.notify();
        }
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
                .position_for_index(offset - row.start, row.line_height)?;
            Some(point(local.x, local.y + row.top))
        })
        .unwrap_or_default()
    }

    fn height_at(&self, offset: usize) -> Pixels {
        self.rows
            .iter()
            .rfind(|row| row.start <= offset)
            .map_or(self.line_height, |row| row.line_height)
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
        let last_line = row.line.size(row.line_height).height - row.line_height;
        let y = (position.y - row.top).clamp(Pixels::ZERO, last_line);
        let local = row
            .line
            .closest_index_for_position(point(position.x, y), row.line_height);
        row.start + local.unwrap_or_else(|index| index)
    }

    /// Returns the offset `lines` below the caret, or at `x` on the caret's line.
    fn near_caret(&self, x: Option<Pixels>, lines: f32) -> usize {
        let caret = self.position(self.caret());
        let height = self.height_at(self.caret());
        let y = if lines < 0. {
            caret.y - px(1.)
        } else if lines > 0. {
            caret.y + height + px(1.)
        } else {
            caret.y + height * 0.5
        };
        self.offset_at(point(x.unwrap_or(caret.x), y))
    }

    /// Returns the rectangles covering the selection in an editor of `width`.
    fn range_quads(&self, range: Range<usize>, width: Pixels, color: Rgba) -> Vec<PaintQuad> {
        let Range { start, end } = range;
        let quad = |from: Point<Pixels>, to: Point<Pixels>| {
            fill(
                Bounds::from_corners(self.origin + from, self.origin + to),
                color,
            )
        };
        let mut quads = Vec::new();
        for row in &self.rows {
            let height = row.line_height;
            let row_end = row.start + row.line.len();
            if start == end || end < row.start || start > row_end {
                continue;
            }
            let local_position = |offset| {
                let local = row
                    .line
                    .position_for_index(offset - row.start, height)
                    .unwrap_or_default();
                point(local.x, local.y + row.top)
            };
            let from = local_position(start.max(row.start));
            let mut to = local_position(end.min(row_end));
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
    fn layout(
        &mut self,
        rows: Vec<Row>,
        origin: Point<Pixels>,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let previous = (
            self.position(self.caret()),
            self.height_at(self.caret()),
            self.origin,
        );
        self.origin = origin;
        self.line_height = window.line_height();
        self.rows = rows;
        if self.completion.is_some()
            && previous
                != (
                    self.position(self.caret()),
                    self.height_at(self.caret()),
                    self.origin,
                )
        {
            cx.notify();
            window.request_animation_frame();
        }
        if !mem::take(&mut self.reveal) {
            return;
        }
        let caret = origin.y + self.position(self.caret()).y;
        let height = self.height_at(self.caret());
        let view = self.scroll.bounds();
        let shift = if caret < view.top() {
            view.top() - caret
        } else if caret + height > view.bottom() {
            view.bottom() - caret - height
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
        if modifiers.alt && matches!(key.as_str(), "left" | "right") {
            return;
        }
        let (caret, select) = (self.caret(), modifiers.shift);
        let step = select || self.selection.is_empty();
        // AltGr reports as control and alt together, and must keep typing characters.
        let command = modifiers.secondary() && !modifiers.alt;
        if !command && !modifiers.alt && self.completion_key(key, cx) {
            cx.stop_propagation();
            return;
        }
        match key.as_str() {
            "up" if self.single_line => cx.emit(Navigated(true)),
            "down" if self.single_line => cx.emit(Navigated(false)),
            "enter" if self.single_line => cx.emit(Submitted(modifiers.shift)),
            "escape" if self.single_line => cx.emit(Cancelled),
            "z" if command => self.restore_history(modifiers.shift, cx),
            "y" if command => self.restore_history(true, cx),
            "left" if command => self.move_to(previous_word(&self.text, caret), select, cx),
            "right" if command => self.move_to(next_word(&self.text, caret), select, cx),
            "home" if command => self.move_to(0, select, cx),
            "end" if command => self.move_to(self.text.len(), select, cx),
            "backspace" if command => self.delete(previous_word(&self.text, caret)..caret, cx),
            "delete" if command => self.delete(caret..next_word(&self.text, caret), cx),
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
            "enter" if command => {
                if let Some(target) = self.link_at(caret) {
                    cx.emit(Followed(target.to_owned()));
                }
            }
            "enter" => self.insert("\n", cx),
            "tab" => self.insert("\t", cx),
            "a" if command => {
                self.selection = 0..self.text.len();
                self.reversed = false;
                self.completion = None;
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

    /// Returns the ranges to decorate and their colors: composed text, or else the links.
    fn spans(&self, style: &TextStyle, accent: Rgba) -> Vec<(Range<usize>, Hsla)> {
        if self.single_line && self.marked.is_none() {
            return Vec::new();
        }
        match &self.marked {
            Some(marked) => vec![(marked.clone(), style.color)],
            None => links(&self.text)
                .map(|link| (link, accent.into()))
                .collect(),
        }
    }

    /// Returns the title of the note linked at `offset`, the link's edges included.
    fn link_at(&self, offset: usize) -> Option<&str> {
        if self.single_line {
            return None;
        }
        let link = links(&self.text).find(|link| link.start <= offset && offset <= link.end)?;
        let target = self.text[link.start + 2..link.end - 2]
            .split(['|', '#'])
            .next()?;
        Some(target.trim()).filter(|target| !target.is_empty())
    }

    /// Returns the title of the note linked under `position`, relative to the first character.
    fn link_under(&self, position: Point<Pixels>) -> Option<&str> {
        let row = self.rows.iter().rfind(|row| row.top <= position.y)?;
        let local = point(position.x, position.y - row.top);
        if local.y >= row.line.size(row.line_height).height {
            return None;
        }
        let index = row.line.index_for_position(local, row.line_height).ok()?;
        self.link_at(row.start + index)
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

    /// Extends the selection while the button stays down and notes links under the pointer.
    fn mouse_move(&mut self, event: &MouseMoveEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let position = event.position - self.origin;
        self.selecting &= event.pressed_button == Some(MouseButton::Left);
        if self.selecting {
            self.move_to(self.offset_at(position), true, cx);
        }
        let over_link = self.link_under(position).is_some();
        if over_link != self.over_link {
            self.over_link = over_link;
            cx.notify();
        }
    }

    /// Follows the link under the pointer when a click ends without selecting anything.
    fn mouse_up(&mut self, event: &MouseUpEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if self.selection.is_empty()
            && let Some(target) = self.link_under(event.position - self.origin)
        {
            cx.emit(Followed(target.to_owned()));
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

/// The kinds of character that word movement tells apart.
#[derive(Debug, PartialEq, Eq)]
enum Kind {
    Space,
    Word,
    Symbol,
}

/// Returns the kind of `character`.
fn kind(character: char) -> Kind {
    match character {
        _ if character.is_whitespace() => Kind::Space,
        _ if character.is_alphanumeric() || character == '_' => Kind::Word,
        _ => Kind::Symbol,
    }
}

/// Returns the start of the word or run of symbols before `offset` in `text`.
fn previous_word(text: &str, offset: usize) -> usize {
    let before = text[..offset].trim_end();
    let Some(last) = before.chars().next_back() else {
        return 0;
    };
    let other = before.rfind(|character| kind(character) != kind(last));
    other.map_or(0, |index| {
        index + before[index..].chars().next().map_or(0, char::len_utf8)
    })
}

/// Returns the start of the word or run of symbols after `offset` in `text`.
fn next_word(text: &str, offset: usize) -> usize {
    let after = &text[offset..];
    let Some(first) = after.chars().next() else {
        return offset;
    };
    let run = after.find(|character| kind(character) != kind(first));
    let rest = run.map_or("", |run| &after[run..]);
    text.len() - rest.trim_start().len()
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

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.marked = None;
        self.refresh_completion();
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.replace(self.input_range(range.as_ref()), text, cx);
        self.marked = None;
        self.refresh_completion();
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
        self.refresh_completion();
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
        end.y += self.height_at(range.end);
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let menu = if self.focus.is_focused(window) {
            self.completion_menu(cx)
        } else {
            None
        };
        div()
            .relative()
            .flex_grow()
            .track_focus(&self.focus)
            .map(|editor| {
                if self.over_link {
                    editor.cursor_pointer()
                } else {
                    editor.cursor_text()
                }
            })
            .on_key_down(cx.listener(Self::key_down))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_move(cx.listener(Self::mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_up))
            .child(EditorElement(cx.entity()))
            .children(menu)
    }
}

/// Returns the ranges of the `[[links]]` in `text`, brackets included.
fn links(text: &str) -> impl Iterator<Item = Range<usize>> {
    let mut from = 0;
    std::iter::from_fn(move || {
        loop {
            let open = from + text[from..].find("[[")?;
            let close = open + text[open..].find("]]")?;
            if close > open + 2 && !text[open + 2..close].contains(['[', '\n']) {
                from = close + 2;
                return Some(open..from);
            }
            from = open + 1;
        }
    })
}

/// Lays `text` out in hard lines wrapped at `width`, underlining `spans` in their colors.
fn shape_line(
    text: SharedString,
    spans: &[(Range<usize>, Hsla)],
    emphasis: &[markdown::Emphasis],
    style: &TextStyle,
    width: Option<Pixels>,
    window: &Window,
) -> Vec<WrappedLine> {
    let mut boundaries = vec![0, text.len()];
    for (span, _) in spans {
        boundaries.extend([span.start, span.end]);
    }
    for span in emphasis {
        boundaries.extend([span.range.start, span.range.end]);
    }
    boundaries.sort_unstable();
    boundaries.dedup();
    let runs: Vec<_> = boundaries
        .windows(2)
        .map(|range| {
            let mut font = style.font();
            for span in emphasis
                .iter()
                .filter(|span| span.range.contains(&range[0]))
            {
                font = if span.bold {
                    font.bold()
                } else {
                    font.italic()
                };
            }
            let decoration = spans
                .iter()
                .find(|(span, _)| span.contains(&range[0]))
                .map(|(_, color)| *color);
            TextRun {
                len: range[1] - range[0],
                font,
                color: decoration.unwrap_or(style.color),
                background_color: None,
                underline: decoration.map(|color| UnderlineStyle {
                    thickness: px(1.),
                    color: Some(color),
                    wavy: false,
                }),
                strikethrough: None,
            }
        })
        .collect();
    let font_size = style.font_size.to_pixels(window.rem_size());
    let lines = window
        .text_system()
        .shape_text(text, font_size, &runs, width, None);
    lines.map_or_else(|_| Vec::new(), Vec::from_iter)
}

fn shape(
    text: &str,
    spans: &[(Range<usize>, Hsla)],
    style: &TextStyle,
    width: Option<Pixels>,
    markdown: bool,
    window: &Window,
) -> Vec<Row> {
    let styles = if markdown {
        markdown::styles(text)
    } else {
        Vec::new()
    };
    let mut rows = Vec::new();
    let (mut start, mut top) = (0, Pixels::ZERO);
    for (index, text) in text.split('\n').enumerate() {
        let end = start + text.len();
        let spans: Vec<_> = spans
            .iter()
            .filter(|(span, _)| span.start < end && span.end > start)
            .map(|(span, color)| {
                (
                    span.start.max(start) - start..span.end.min(end) - start,
                    *color,
                )
            })
            .collect();
        let formatting = styles.get(index);
        let level = formatting.and_then(|style| style.heading);
        let emphasis = formatting.map_or(&[][..], |style| style.emphasis.as_slice());
        let scale = level.map_or(1., markdown::heading_scale);
        let mut line_style = style.clone();
        if level.is_some() {
            line_style.font_size = (style.font_size.to_pixels(window.rem_size()) * scale).into();
            line_style.font_weight = gpui::FontWeight::BOLD;
        }
        let line_height = window.line_height() * scale;
        for line in shape_line(
            text.to_owned().into(),
            &spans,
            emphasis,
            &line_style,
            width,
            window,
        ) {
            let row = Row {
                start,
                top,
                line,
                line_height,
            };
            top += row.line.size(line_height).height;
            rows.push(row);
        }
        start = end + 1;
    }
    rows
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
        let (style, line_height) = (window.text_style(), window.line_height());
        let spans = editor.spans(&style, cx.global::<Theme>().accent);
        let text = editor.text.clone();
        let markdown = !editor.single_line;
        let mut layout = Style::default();
        layout.size.width = relative(1.).into();
        let id = window.request_measured_layout(layout, move |known, available, window, _cx| {
            let width = known.width.or(match available.width {
                AvailableSpace::Definite(width) => Some(width),
                AvailableSpace::MinContent | AvailableSpace::MaxContent => None,
            });
            let lines = shape(&text, &spans, &style, width, markdown, window);
            let height = lines.iter().fold(Pixels::ZERO, |sum, line| {
                sum + line.line.size(line.line_height).height
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
        let accent = cx.global::<Theme>().accent;
        self.0.update(cx, |editor, cx| {
            let width = Some(bounds.size.width);
            let style = window.text_style();
            let spans = editor.spans(&style, accent);
            let lines = shape(
                &editor.text,
                &spans,
                &style,
                width,
                !editor.single_line,
                window,
            );
            editor.layout(lines, bounds.origin, window, cx);
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
        let selection = Rgba {
            a: SELECTION_ALPHA,
            ..accent
        };
        let mut quads = Vec::new();
        for (index, range) in editor.matches.iter().enumerate() {
            let highlight = Rgba {
                a: if Some(index) == editor.active_match {
                    0.4
                } else {
                    0.15
                },
                ..accent
            };
            quads.extend(editor.range_quads(range.clone(), bounds.size.width, highlight));
        }
        if focus.is_focused(window) || editor.active_match.is_none() {
            quads.extend(editor.range_quads(
                editor.selection.clone(),
                bounds.size.width,
                selection,
            ));
        }
        if editor.selection.is_empty() && focus.is_focused(window) {
            let caret = Bounds::new(
                origin + editor.position(editor.caret()),
                size(CARET_WIDTH, editor.height_at(editor.caret())),
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
                .paint(at, row.line_height, TextAlign::Left, None, window, cx);
        }
    }
}
