use std::ops::Range;

use aircode_editor::Document;
use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, Element, ElementId, ElementInputHandler,
    Entity, EntityInputHandler, FocusHandle, Focusable, GlobalElementId, KeyBinding, LayoutId,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point,
    ShapedLine, Style, TextRun, UTF16Selection, UnderlineStyle, Window, actions, div, fill, point,
    prelude::*, px, relative, rgba, size,
};

actions!(
    aircode_document,
    [
        Backspace,
        Delete,
        Left,
        Right,
        Up,
        Down,
        SelectLeft,
        SelectRight,
        SelectAll,
        Home,
        End,
        Paste,
        Cut,
        Copy,
        Undo,
        Redo,
        InsertNewline,
        InsertTab,
        ShowCharacterPalette,
    ]
);

pub fn register_actions(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, Some("Document")),
        KeyBinding::new("delete", Delete, Some("Document")),
        KeyBinding::new("left", Left, Some("Document")),
        KeyBinding::new("right", Right, Some("Document")),
        KeyBinding::new("up", Up, Some("Document")),
        KeyBinding::new("down", Down, Some("Document")),
        KeyBinding::new("shift-left", SelectLeft, Some("Document")),
        KeyBinding::new("shift-right", SelectRight, Some("Document")),
        KeyBinding::new("secondary-a", SelectAll, Some("Document")),
        KeyBinding::new("home", Home, Some("Document")),
        KeyBinding::new("end", End, Some("Document")),
        KeyBinding::new("secondary-v", Paste, Some("Document")),
        KeyBinding::new("secondary-c", Copy, Some("Document")),
        KeyBinding::new("secondary-x", Cut, Some("Document")),
        KeyBinding::new("secondary-z", Undo, Some("Document")),
        KeyBinding::new("secondary-y", Redo, Some("Document")),
        KeyBinding::new("enter", InsertNewline, Some("Document")),
        KeyBinding::new("tab", InsertTab, Some("Document")),
        KeyBinding::new("ctrl-cmd-space", ShowCharacterPalette, Some("Document")),
    ]);
}

pub struct DocumentView {
    document: Document,
    focus_handle: FocusHandle,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    last_layouts: Vec<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
    is_selecting: bool,
}

impl DocumentView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            document: Document::from_text("// Open a file or start typing here.\n"),
            focus_handle: cx.focus_handle(),
            selection_reversed: false,
            marked_range: None,
            last_layouts: Vec::new(),
            last_bounds: None,
            is_selecting: false,
        }
    }

    pub fn document(&self) -> &Document {
        &self.document
    }

    pub fn document_mut(&mut self) -> &mut Document {
        &mut self.document
    }

    pub fn set_document(&mut self, document: Document, cx: &mut Context<Self>) {
        self.document = document;
        self.marked_range = None;
        self.last_layouts.clear();
        cx.notify();
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.document.selection().start
        } else {
            self.document.selection().end
        }
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        let _ = self.document.set_selection(offset..offset);
        self.selection_reversed = false;
        cx.notify();
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        let mut range = self.document.selection();
        if self.selection_reversed {
            range.start = offset;
        } else {
            range.end = offset;
        }
        if range.end < range.start {
            self.selection_reversed = !self.selection_reversed;
            range = range.end..range.start;
        }
        let _ = self.document.set_selection(range);
        cx.notify();
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        let selection = self.document.selection();
        let offset = if selection.is_empty() {
            self.document.previous_boundary(self.cursor_offset())
        } else {
            selection.start
        };
        self.move_to(offset, cx);
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        let selection = self.document.selection();
        let offset = if selection.is_empty() {
            self.document.next_boundary(selection.end)
        } else {
            selection.end
        };
        self.move_to(offset, cx);
    }

    fn up(&mut self, _: &Up, _: &mut Window, cx: &mut Context<Self>) {
        let offset = self.vertical_offset(-1);
        self.move_to(offset, cx);
    }

    fn down(&mut self, _: &Down, _: &mut Window, cx: &mut Context<Self>) {
        let offset = self.vertical_offset(1);
        self.move_to(offset, cx);
    }

    fn vertical_offset(&self, direction: isize) -> usize {
        let cursor = self.cursor_offset();
        let (line, column) = self.line_column(cursor);
        let target_line = if direction < 0 {
            line.saturating_sub(1)
        } else {
            line + 1
        };
        self.offset_for_line_column(target_line, column)
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.document.previous_boundary(self.cursor_offset()), cx);
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.document.next_boundary(self.cursor_offset()), cx);
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
        self.select_to(self.document.text().len(), cx);
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        let (line, _) = self.line_column(self.cursor_offset());
        self.move_to(self.offset_for_line_column(line, 0), cx);
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        let (line, _) = self.line_column(self.cursor_offset());
        let end = self.line_ranges()[line].end;
        self.move_to(end, cx);
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.document.selection().is_empty() {
            self.select_to(self.document.previous_boundary(self.cursor_offset()), cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.document.selection().is_empty() {
            self.select_to(self.document.next_boundary(self.cursor_offset()), cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.replace_text_in_range(None, &text, window, cx);
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.document.selection().is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.document.selected_text().to_string(),
            ));
        }
    }

    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if !self.document.selection().is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.document.selected_text().to_string(),
            ));
            self.replace_text_in_range(None, "", window, cx);
        }
    }

    fn undo(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        self.document.undo();
        cx.notify();
    }

    fn redo(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        self.document.redo();
        cx.notify();
    }

    fn newline(&mut self, _: &InsertNewline, window: &mut Window, cx: &mut Context<Self>) {
        self.replace_text_in_range(None, "\n", window, cx);
    }

    fn tab(&mut self, _: &InsertTab, window: &mut Window, cx: &mut Context<Self>) {
        self.replace_text_in_range(None, "    ", window, cx);
    }

    fn show_character_palette(
        &mut self,
        _: &ShowCharacterPalette,
        window: &mut Window,
        _: &mut Context<Self>,
    ) {
        window.show_character_palette();
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle(cx));
        self.is_selecting = true;
        let offset = self.index_for_mouse_position(event.position);
        if event.modifiers.shift {
            self.select_to(offset, cx);
        } else {
            self.move_to(offset, cx);
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_selecting {
            self.select_to(self.index_for_mouse_position(event.position), cx);
        }
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        let Some(bounds) = self.last_bounds else {
            return 0;
        };
        let line_height = px(22.0);
        let line = ((position.y - bounds.top()) / line_height).floor().max(0.0) as usize;
        let line = line.min(self.last_layouts.len().saturating_sub(1));
        let Some(layout) = self.last_layouts.get(line) else {
            return 0;
        };
        let line_start = self.line_ranges()[line].start;
        line_start + layout.closest_index_for_x(position.x - bounds.left() - px(54.0))
    }

    fn line_ranges(&self) -> Vec<Range<usize>> {
        let text = self.document.text();
        let mut ranges = Vec::new();
        let mut start = 0;
        for (index, character) in text.char_indices() {
            if character == '\n' {
                ranges.push(start..index);
                start = index + 1;
            }
        }
        ranges.push(start..text.len());
        ranges
    }

    fn line_column(&self, offset: usize) -> (usize, usize) {
        let ranges = self.line_ranges();
        let line = ranges
            .iter()
            .position(|range| offset >= range.start && offset <= range.end)
            .unwrap_or(ranges.len().saturating_sub(1));
        (line, offset - ranges[line].start)
    }

    fn offset_for_line_column(&self, line: usize, column: usize) -> usize {
        let ranges = self.line_ranges();
        let line = line.min(ranges.len().saturating_sub(1));
        let range = &ranges[line];
        let line_text = &self.document.text()[range.clone()];
        let mut utf8_column = 0;
        for (index, character) in line_text.char_indices() {
            if index >= column {
                break;
            }
            utf8_column = index + character.len_utf8();
        }
        range.start + utf8_column.min(range.len())
    }

    fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf8_offset = 0;
        let mut utf16_count = 0;
        for character in self.document.text().chars() {
            if utf16_count >= offset {
                break;
            }
            utf16_count += character.len_utf16();
            utf8_offset += character.len_utf8();
        }
        utf8_offset
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        let mut utf16_offset = 0;
        let mut utf8_count = 0;
        for character in self.document.text().chars() {
            if utf8_count >= offset {
                break;
            }
            utf8_count += character.len_utf8();
            utf16_offset += character.len_utf16();
        }
        utf16_offset
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range.start)..self.offset_from_utf16(range.end)
    }
}

impl EntityInputHandler for DocumentView {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.document.text()[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.document.selection()),
            reversed: self.selection_reversed,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked_range
            .as_ref()
            .map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.marked_range = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range| self.range_from_utf16(range))
            .or(self.marked_range.clone())
            .unwrap_or(self.document.selection());
        if self.document.replace_range(range, new_text).is_ok() {
            self.selection_reversed = false;
            self.marked_range = None;
            self.last_layouts.clear();
            cx.notify();
        }
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let insertion_start = range_utf16
            .as_ref()
            .map(|range| self.range_from_utf16(range))
            .or(self.marked_range.clone())
            .unwrap_or(self.document.selection())
            .start;
        self.replace_text_in_range(range_utf16, new_text, window, cx);
        if !new_text.is_empty() {
            self.marked_range = Some(insertion_start..insertion_start + new_text.len());
        }
        if let Some(selection) = new_selected_range_utf16 {
            let selection = self.range_from_utf16(&selection);
            let _ = self
                .document
                .set_selection(insertion_start + selection.start..insertion_start + selection.end);
        }
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let range = self.range_from_utf16(&range_utf16);
        let (start_line, start_column) = self.line_column(range.start);
        let (end_line, end_column) = self.line_column(range.end);
        let start_layout = self.last_layouts.get(start_line)?;
        let end_layout = self.last_layouts.get(end_line)?;
        Some(Bounds::from_corners(
            point(
                bounds.left() + px(54.0) + start_layout.x_for_index(start_column),
                bounds.top() + px(22.0 * start_line as f32),
            ),
            point(
                bounds.left() + px(54.0) + end_layout.x_for_index(end_column),
                bounds.top() + px(22.0 * (end_line + 1) as f32),
            ),
        ))
    }

    fn character_index_for_point(
        &mut self,
        position: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        Some(self.offset_to_utf16(self.index_for_mouse_position(position)))
    }
}

struct DocumentElement {
    input: Entity<DocumentView>,
}

struct PrepaintState {
    lines: Vec<ShapedLine>,
    numbers: Vec<ShapedLine>,
    selection: Vec<PaintQuad>,
    cursor: Option<PaintQuad>,
}

impl IntoElement for DocumentElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for DocumentElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let line_count = self.input.read(cx).line_ranges().len();
        let mut style = Style::default();
        style.size.width = relative(1.0).into();
        style.size.height = px(22.0 * line_count as f32).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let input = self.input.read(cx);
        let text = input.document.text();
        let selection = input.document.selection();
        let cursor = input.cursor_offset();
        let style = window.text_style();
        let font_size = style.font_size.to_pixels(window.rem_size());
        let line_ranges = input.line_ranges();
        let mut lines = Vec::new();
        let mut numbers = Vec::new();
        let mut selection_quads = Vec::new();
        let mut cursor_quad = None;

        for (line_index, range) in line_ranges.iter().enumerate() {
            let line_text = &text[range.clone()];
            let run = TextRun {
                len: line_text.len(),
                font: style.font(),
                color: style.color,
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let runs = if let Some(marked) = input
                .marked_range
                .as_ref()
                .filter(|marked| marked.start >= range.start && marked.end <= range.end)
            {
                let local = marked.start - range.start..marked.end - range.start;
                vec![
                    TextRun {
                        len: local.start,
                        ..run.clone()
                    },
                    TextRun {
                        len: local.len(),
                        underline: Some(UnderlineStyle {
                            color: Some(run.color),
                            thickness: px(1.0),
                            wavy: false,
                        }),
                        ..run.clone()
                    },
                    TextRun {
                        len: line_text.len() - local.end,
                        ..run
                    },
                ]
                .into_iter()
                .filter(|run| run.len > 0)
                .collect()
            } else {
                vec![run]
            };
            let line = window.text_system().shape_line(
                line_text.to_owned().into(),
                font_size,
                &runs,
                None,
            );
            let number_run = TextRun {
                len: 4,
                font: style.font(),
                color: style.color.opacity(0.4),
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let number = window.text_system().shape_line(
                format!("{:>4}", line_index + 1).into(),
                font_size,
                &[number_run],
                None,
            );
            let top = bounds.top() + px(22.0 * line_index as f32);

            let selected_start = selection.start.max(range.start);
            let selected_end = selection.end.min(range.end);
            if selected_start < selected_end
                || (selection.start <= range.end
                    && selection.end > range.end
                    && selection.end > selection.start)
            {
                let start_x = line.x_for_index(selected_start - range.start);
                let end_x = line.x_for_index(selected_end.saturating_sub(range.start));
                selection_quads.push(fill(
                    Bounds::new(
                        point(bounds.left() + px(54.0) + start_x, top),
                        size((end_x - start_x).max(px(4.0)), px(22.0)),
                    ),
                    rgba(0x7c8cff40),
                ));
            }
            if cursor >= range.start && cursor <= range.end {
                let cursor_x = line.x_for_index(cursor - range.start);
                cursor_quad = Some(fill(
                    Bounds::new(
                        point(bounds.left() + px(54.0) + cursor_x, top),
                        size(px(2.0), px(22.0)),
                    ),
                    style.color,
                ));
            }
            lines.push(line);
            numbers.push(number);
        }
        PrepaintState {
            lines,
            numbers,
            selection: selection_quads,
            cursor: cursor_quad,
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.input.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );
        for selection in prepaint.selection.drain(..) {
            window.paint_quad(selection);
        }
        for (index, line) in prepaint.lines.drain(..).enumerate() {
            let origin = point(
                bounds.left() + px(54.0),
                bounds.top() + px(22.0 * index as f32),
            );
            line.paint(origin, px(22.0), window, cx).unwrap();
            self.input.update(cx, |input, _| {
                if input.last_layouts.len() <= index {
                    input.last_layouts.push(line.clone());
                } else {
                    input.last_layouts[index] = line.clone();
                }
            });
        }
        for (index, number) in prepaint.numbers.drain(..).enumerate() {
            number
                .paint(
                    point(bounds.left(), bounds.top() + px(22.0 * index as f32)),
                    px(22.0),
                    window,
                    cx,
                )
                .unwrap();
        }
        if focus_handle.is_focused(window)
            && let Some(cursor) = prepaint.cursor.take()
        {
            window.paint_quad(cursor);
        }
        self.input.update(cx, |input, _| {
            input.last_bounds = Some(bounds);
        });
    }
}

impl Render for DocumentView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .key_context("Document")
            .track_focus(&self.focus_handle(cx))
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::up))
            .on_action(cx.listener(Self::down))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::undo))
            .on_action(cx.listener(Self::redo))
            .on_action(cx.listener(Self::newline))
            .on_action(cx.listener(Self::tab))
            .on_action(cx.listener(Self::show_character_palette))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .child(DocumentElement { input: cx.entity() })
    }
}

impl Focusable for DocumentView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}
