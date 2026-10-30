use std::sync::Arc;
use std::{collections::HashMap, ops::Range};
use tokio_util::sync::CancellationToken;

use aircode_editor::Document;
use aircode_i18n::{Language as UiLanguage, tr};
use aircode_syntax::{HighlightKind, Language, SyntaxHighlighter};
use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, Element, ElementId, ElementInputHandler,
    Entity, EntityInputHandler, FocusHandle, Focusable, GlobalElementId, IsZero, KeyBinding,
    LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point,
    ScrollHandle, ShapedLine, Style, TextRun, UTF16Selection, UnderlineStyle, Window, actions, div,
    fill, point, prelude::*, px, relative, rgba, size,
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
        PageUp,
        PageDown,
        DocumentStart,
        DocumentEnd,
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
        DismissCompletion,
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
        KeyBinding::new("pageup", PageUp, Some("Document")),
        KeyBinding::new("pagedown", PageDown, Some("Document")),
        KeyBinding::new("secondary-home", DocumentStart, Some("Document")),
        KeyBinding::new("secondary-end", DocumentEnd, Some("Document")),
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
        KeyBinding::new("escape", DismissCompletion, Some("Document")),
        KeyBinding::new("ctrl-cmd-space", ShowCharacterPalette, Some("Document")),
    ]);
}

pub(crate) const LINE_HEIGHT: f32 = 22.0;
const TEXT_PADDING: f32 = 24.0;

pub struct DocumentView {
    document: Document,
    language: UiLanguage,
    focus_handle: FocusHandle,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    last_layouts: HashMap<usize, ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
    is_selecting: bool,
    context_menu_open: bool,
    syntax: Option<SyntaxHighlighter>,
    scroll_handle: ScrollHandle,
    read_only: bool,
    reveal_cursor: bool,
    max_line_columns: usize,
    last_scale_factor: f32,
    revision: u64,
    completion: Option<(u64, usize, String)>,
    completion_engine: Option<CompletionEngine>,
    completion_cancel: Option<CancellationToken>,
    completion_failed: bool,
    completion_generation: u64,
    completion_refresh: bool,
    completion_focus: Option<gpui::Subscription>,
    completion_blur: Option<gpui::Subscription>,
}

#[derive(Clone)]
pub(crate) struct CompletionEngine {
    pub runtime: Arc<tokio::runtime::Runtime>,
    pub client: Arc<aircode_ai::AiClient>,
    pub config: aircode_ai::ProviderConfig,
}

impl DocumentView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            document: Document::from_text("// Open a file or start typing here.\n"),
            language: UiLanguage::English,
            focus_handle: cx.focus_handle(),
            selection_reversed: false,
            marked_range: None,
            last_layouts: HashMap::new(),
            last_bounds: None,
            is_selecting: false,
            context_menu_open: false,
            syntax: None,
            scroll_handle: ScrollHandle::new(),
            read_only: false,
            reveal_cursor: false,
            max_line_columns: 40,
            last_scale_factor: 0.0,
            revision: 0,
            completion: None,
            completion_engine: None,
            completion_cancel: None,
            completion_failed: false,
            completion_generation: 0,
            completion_refresh: false,
            completion_focus: None,
            completion_blur: None,
        }
    }

    pub fn scroll_handle(&self) -> ScrollHandle {
        self.scroll_handle.clone()
    }

    pub fn set_read_only(&mut self, read_only: bool) {
        self.read_only = read_only;
        if read_only {
            self.cancel_completion();
        }
    }

    pub(crate) fn configure_completion(&mut self, engine: Option<CompletionEngine>) {
        if self.completion_engine.as_ref().map(|engine| &engine.config)
            == engine.as_ref().map(|engine| &engine.config)
        {
            self.completion_engine = engine;
            return;
        }
        self.cancel_completion();
        self.completion_engine = engine;
        self.completion_failed = false;
        self.completion_refresh = self.completion_engine.is_some();
    }

    pub(crate) fn completion_failed(&self) -> bool {
        self.completion_failed
    }

    fn cancel_completion(&mut self) {
        self.completion_generation = self.completion_generation.wrapping_add(1);
        if let Some(cancel) = self.completion_cancel.take() {
            cancel.cancel();
        }
        self.completion = None;
    }

    fn watch_completion_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.completion_focus.is_none() {
            let focus = self.focus_handle.clone();
            self.completion_focus = Some(cx.on_focus(&focus, window, |view, _, cx| {
                view.completion_refresh = true;
                cx.notify();
            }));
        }
        if self.completion_blur.is_none() {
            let focus = self.focus_handle.clone();
            self.completion_blur = Some(cx.on_blur(&focus, window, |view, _, cx| {
                view.completion_refresh = false;
                view.revision = view.revision.wrapping_add(1);
                view.cancel_completion();
                cx.notify();
            }));
        }
    }

    fn schedule_completion(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.watch_completion_focus(window, cx);
        self.cancel_completion();
        self.completion_refresh = false;
        let Some(engine) = self.completion_engine.clone() else {
            return;
        };
        let Some((revision, cursor, messages)) = self.completion_context() else {
            return;
        };
        if !self.focus_handle.is_focused(window) {
            return;
        }
        self.completion_failed = false;
        let generation = self.completion_generation;
        let cancel = CancellationToken::new();
        self.completion_cancel = Some(cancel.clone());
        cx.spawn_in(window, async move |view, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(650))
                .await;
            if cancel.is_cancelled() {
                return;
            }
            let should_request = view
                .update_in(cx, |view, window, _| {
                    let current = view.focus_handle.is_focused(window)
                        && view.completion_is_current(revision, cursor);
                    if !current && view.completion_generation == generation {
                        view.completion_cancel = None;
                    }
                    current
                })
                .unwrap_or(false);
            if !should_request {
                return;
            }
            let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
            let request_cancel = cancel.clone();
            engine.runtime.spawn(async move {
                let result = match aircode_ai::CredentialStore::load(&engine.config.credential_id())
                {
                    Ok(Some(key)) => aircode_ai::request_completion(
                        &engine.client,
                        &engine.config,
                        &key,
                        &messages,
                        request_cancel,
                    )
                    .await
                    .map(Some),
                    Ok(None) => Ok(None),
                    Err(error) => Err(error),
                };
                let _ = sender.send(result);
            });
            let result = receiver.recv().await;
            let _ = view.update_in(cx, |view, window, cx| {
                if view.completion_generation != generation {
                    return;
                }
                view.completion_cancel = None;
                if cancel.is_cancelled()
                    || !view.focus_handle.is_focused(window)
                    || !view.completion_is_current(revision, cursor)
                {
                    return;
                }
                match result {
                    Some(Ok(Some(text))) => {
                        view.set_completion(revision, cursor, &text, cx);
                    }
                    Some(Ok(None)) => {
                        view.completion_failed = true;
                        cx.notify();
                    }
                    Some(Err(_)) => {
                        view.completion_failed = true;
                        cx.notify();
                    }
                    None => {
                        view.completion_failed = true;
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    pub fn cursor_line_col(&self) -> (usize, usize) {
        let (line, column) = self.document.line_col_at(self.cursor_offset());
        (line + 1, column + 1)
    }

    pub fn document(&self) -> &Document {
        &self.document
    }

    pub fn document_mut(&mut self) -> &mut Document {
        self.revision = self.revision.wrapping_add(1);
        self.cancel_completion();
        &mut self.document
    }

    pub fn completion_context(&self) -> Option<(u64, usize, Vec<aircode_ai::ChatMessage>)> {
        let cursor = self.completion_cursor()?;
        let language = self
            .detected_language()
            .map_or("text", |language| language.label());
        let messages = aircode_ai::completion_messages(self.document.text(), cursor, language)?;
        Some((self.revision, cursor, messages))
    }

    fn completion_cursor(&self) -> Option<usize> {
        let selection = self.document.selection();
        if self.read_only || self.marked_range.is_some() || !selection.is_empty() {
            return None;
        }
        let cursor = selection.start;
        let line = self.document.line_of_offset(cursor);
        if cursor != self.document.line_range(line).end {
            return None;
        }
        Some(cursor)
    }

    fn completion_is_current(&self, revision: u64, cursor: usize) -> bool {
        self.revision == revision && self.completion_cursor() == Some(cursor)
    }

    pub fn set_completion(
        &mut self,
        revision: u64,
        cursor: usize,
        text: &str,
        cx: &mut Context<Self>,
    ) {
        if !self.completion_is_current(revision, cursor) {
            return;
        }
        if let Some(line) = inline_completion_line(text) {
            let mut end = line.len().min(4096);
            while !line.is_char_boundary(end) {
                end -= 1;
            }
            self.completion = Some((revision, cursor, line[..end].to_owned()));
            cx.notify();
        }
    }

    fn dismiss_completion(
        &mut self,
        _: &DismissCompletion,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.completion.is_some() || self.completion_cancel.is_some() {
            self.revision = self.revision.wrapping_add(1);
            self.cancel_completion();
            cx.notify();
        } else {
            cx.propagate();
        }
    }

    pub fn detected_language(&self) -> Option<Language> {
        self.document.path().and_then(Language::from_path)
    }

    pub fn syntax_highlighting_ready(&self) -> bool {
        self.syntax.is_some()
    }

    pub fn refresh_syntax(&mut self) {
        self.syntax = self
            .document
            .path()
            .and_then(Language::from_path)
            .and_then(|language| SyntaxHighlighter::new(language).ok());
        if let Some(syntax) = &mut self.syntax {
            syntax.set_text(self.document.text());
        }
    }

    pub(crate) fn reload_clean(&mut self, cx: &mut Context<Self>) -> Result<bool, String> {
        let reloaded = self
            .document
            .reload_clean()
            .map_err(|error| error.to_string())?;
        if reloaded {
            self.revision = self.revision.wrapping_add(1);
            self.cancel_completion();
            self.max_line_columns = max_line_columns(&self.document);
            self.last_layouts.clear();
            self.last_bounds = None;
            self.refresh_syntax();
            cx.notify();
        }
        Ok(reloaded)
    }

    pub fn set_document(&mut self, mut document: Document, cx: &mut Context<Self>) {
        self.revision = self.revision.wrapping_add(1);
        self.cancel_completion();
        let _ = document.set_selection(0..0);
        self.max_line_columns = max_line_columns(&document);
        self.document = document;
        self.refresh_syntax();
        self.marked_range = None;
        self.selection_reversed = false;
        self.last_layouts.clear();
        self.last_bounds = None;
        self.reveal_cursor = false;
        self.scroll_handle.set_offset(point(px(0.0), px(0.0)));
        cx.notify();
    }

    fn gutter_width(&self) -> f32 {
        let digits = self.document.line_count().max(1).ilog10() as usize + 1;
        22.0 + digits.max(4) as f32 * 8.0
    }

    fn refresh_content_metrics(&mut self) {
        self.max_line_columns = max_line_columns(&self.document);
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.document.selection().start
        } else {
            self.document.selection().end
        }
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.revision = self.revision.wrapping_add(1);
        self.cancel_completion();
        let _ = self.document.set_selection(offset..offset);
        self.selection_reversed = false;
        self.reveal_cursor = true;
        cx.notify();
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.revision = self.revision.wrapping_add(1);
        self.cancel_completion();
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
        self.reveal_cursor = true;
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
        let (line, column) = self.document.line_col_at(self.cursor_offset());
        let target_line = if direction < 0 {
            line.saturating_sub(direction.unsigned_abs())
        } else {
            line.saturating_add(direction as usize)
                .min(self.document.line_count().saturating_sub(1))
        };
        self.document.offset_for_line_col(target_line, column)
    }

    fn page_up(&mut self, _: &PageUp, _: &mut Window, cx: &mut Context<Self>) {
        let pages = self.visible_page_lines();
        self.move_to(self.vertical_offset(-(pages as isize)), cx);
    }

    fn page_down(&mut self, _: &PageDown, _: &mut Window, cx: &mut Context<Self>) {
        let pages = self.visible_page_lines();
        self.move_to(self.vertical_offset(pages as isize), cx);
    }

    fn visible_page_lines(&self) -> usize {
        (f32::from(self.scroll_handle.bounds().size.height) / LINE_HEIGHT)
            .floor()
            .max(1.0) as usize
    }

    fn document_start(&mut self, _: &DocumentStart, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
    }

    fn document_end(&mut self, _: &DocumentEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.document.text().len(), cx);
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
        let line = self.document.line_of_offset(self.cursor_offset());
        self.move_to(self.document.line_range(line).start, cx);
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        let line = self.document.line_of_offset(self.cursor_offset());
        self.move_to(self.document.line_range(line).end, cx);
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
        if self.read_only {
            return;
        }
        self.revision = self.revision.wrapping_add(1);
        self.cancel_completion();
        self.document.undo();
        self.marked_range = None;
        self.selection_reversed = false;
        self.reveal_cursor = true;
        self.refresh_content_metrics();
        if let Some(syntax) = &mut self.syntax {
            syntax.set_text(self.document.text());
        }
        self.last_layouts.clear();
        cx.notify();
    }

    fn redo(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        self.revision = self.revision.wrapping_add(1);
        self.cancel_completion();
        self.document.redo();
        self.marked_range = None;
        self.selection_reversed = false;
        self.reveal_cursor = true;
        self.refresh_content_metrics();
        if let Some(syntax) = &mut self.syntax {
            syntax.set_text(self.document.text());
        }
        self.last_layouts.clear();
        cx.notify();
    }

    fn newline(&mut self, _: &InsertNewline, window: &mut Window, cx: &mut Context<Self>) {
        self.replace_text_in_range(None, "\n", window, cx);
    }

    fn tab(&mut self, _: &InsertTab, window: &mut Window, cx: &mut Context<Self>) {
        let suggestion = self
            .completion
            .take()
            .filter(|(revision, cursor, _)| self.completion_is_current(*revision, *cursor));
        self.replace_text_in_range(
            None,
            suggestion
                .as_ref()
                .map_or("    ", |(_, _, text)| text.as_str()),
            window,
            cx,
        );
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
        self.context_menu_open = false;
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
        if event.pressed_button != Some(MouseButton::Left) {
            self.is_selecting = false;
        }
        if self.is_selecting {
            self.select_to(self.index_for_mouse_position(event.position), cx);
        }
    }

    fn on_right_mouse_down(
        &mut self,
        _: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle(cx));
        self.context_menu_open = true;
        cx.notify();
    }

    fn context_menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("document-context-menu")
            .absolute()
            .top(px(24.0))
            .left(px(60.0))
            .w(px(150.0))
            .flex()
            .flex_col()
            .rounded_md()
            .border_1()
            .border_color(gpui::rgba(0x7c8cff60))
            .bg(gpui::rgba(0x1a1e26f0))
            .shadow_lg()
            .child(
                div()
                    .id("document-context-cut")
                    .px_3()
                    .py_2()
                    .cursor_pointer()
                    .hover(|style| style.bg(gpui::rgba(0x7c8cff30)))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.cut(&Cut, window, cx);
                        this.context_menu_open = false;
                    }))
                    .child(tr(self.language, "cut")),
            )
            .child(
                div()
                    .id("document-context-copy")
                    .px_3()
                    .py_2()
                    .cursor_pointer()
                    .hover(|style| style.bg(gpui::rgba(0x7c8cff30)))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.copy(&Copy, window, cx);
                        this.context_menu_open = false;
                    }))
                    .child(tr(self.language, "copy")),
            )
            .child(
                div()
                    .id("document-context-paste")
                    .px_3()
                    .py_2()
                    .cursor_pointer()
                    .hover(|style| style.bg(gpui::rgba(0x7c8cff30)))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.paste(&Paste, window, cx);
                        this.context_menu_open = false;
                    }))
                    .child(tr(self.language, "paste")),
            )
            .child(
                div()
                    .id("document-context-select-all")
                    .px_3()
                    .py_2()
                    .cursor_pointer()
                    .hover(|style| style.bg(gpui::rgba(0x7c8cff30)))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.select_all(&SelectAll, window, cx);
                        this.context_menu_open = false;
                    }))
                    .child(tr(self.language, "select_all")),
            )
    }

    pub fn set_language(&mut self, language: UiLanguage) {
        self.language = language;
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        let Some(bounds) = self.last_bounds else {
            return 0;
        };
        let line = ((position.y - bounds.top()) / px(LINE_HEIGHT))
            .floor()
            .max(0.0) as usize;
        let line = line.min(self.document.line_count().saturating_sub(1));
        let range = self.document.line_range(line);
        let Some(layout) = self.last_layouts.get(&line) else {
            return range.start;
        };
        let x = position.x - bounds.left() - px(self.gutter_width());
        (range.start + layout.closest_index_for_x(x)).min(range.end)
    }

    fn offset_from_utf16(&self, offset: usize) -> usize {
        utf16_to_byte(self.document.text(), offset)
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        byte_to_utf16(self.document.text(), offset)
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range.start)..self.offset_from_utf16(range.end)
    }
}

pub(crate) fn utf16_to_byte(text: &str, offset: usize) -> usize {
    let mut utf16_offset = 0;
    for (index, character) in text.char_indices() {
        if utf16_offset >= offset {
            return index;
        }
        utf16_offset += character.len_utf16();
    }
    text.len()
}

fn byte_to_utf16(text: &str, offset: usize) -> usize {
    let offset = offset.min(text.len());
    text.char_indices()
        .take_while(|(index, _)| *index < offset)
        .map(|(_, character)| character.len_utf16())
        .sum()
}

fn display_columns(text: &str) -> usize {
    text.chars().fold(0usize, |columns, character| {
        let width = match character {
            '\t' => 4,
            character if character.is_ascii() => 1,
            _ => 2,
        };
        columns.saturating_add(width)
    })
}

fn max_line_columns(document: &Document) -> usize {
    (0..document.line_count())
        .map(|line| display_columns(&document.text()[document.line_range(line)]))
        .max()
        .unwrap_or(1)
        .max(1)
}

fn visible_line_range(scroll_offset: f32, viewport_height: f32, line_count: usize) -> Range<usize> {
    if line_count == 0 {
        return 0..0;
    }
    let scroll_offset = if scroll_offset.is_finite() {
        scroll_offset.max(0.0)
    } else {
        0.0
    };
    let viewport_height = if viewport_height.is_finite() {
        viewport_height.max(0.0)
    } else {
        0.0
    };
    let first = ((scroll_offset / LINE_HEIGHT).floor() as usize).min(line_count - 1);
    let visible = ((viewport_height / LINE_HEIGHT).ceil().max(1.0) as usize).saturating_add(2);
    first..first.saturating_add(visible).min(line_count)
}

fn reveal_delta(start: f32, end: f32, visible_start: f32, visible_end: f32) -> f32 {
    if !start.is_finite()
        || !end.is_finite()
        || !visible_start.is_finite()
        || !visible_end.is_finite()
    {
        return 0.0;
    }
    if start < visible_start {
        visible_start - start
    } else if end > visible_end {
        visible_end - end
    } else {
        0.0
    }
}

fn clamp_scroll_offset(offset: f32, maximum: f32) -> f32 {
    if !offset.is_finite() || !maximum.is_finite() {
        return 0.0;
    }
    offset.clamp(-maximum.max(0.0), 0.0)
}

fn line_number_text(line_index: usize, width: usize) -> String {
    format!("{:>width$}", line_index + 1)
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
        if range.start > range.end {
            return None;
        }
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

    fn unmark_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.marked_range = None;
        self.schedule_completion(window, cx);
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.read_only {
            return;
        }
        let mut range = range_utf16
            .as_ref()
            .map(|range| self.range_from_utf16(range))
            .or(self.marked_range.clone())
            .unwrap_or(self.document.selection());
        if range.is_empty()
            && range.start > 0
            && self.document.text().as_bytes()[range.start - 1] == b'\r'
            && self.document.text().as_bytes().get(range.start) == Some(&b'\n')
        {
            range.start -= 1;
            range.end -= 1;
        }
        let old_text = self.document.text().to_string();
        if self.document.replace_range(range.clone(), new_text).is_ok() {
            self.revision = self.revision.wrapping_add(1);
            self.completion = None;
            if let Some(syntax) = &mut self.syntax {
                syntax.replace_range(&old_text, self.document.text(), range, new_text);
            }
            self.selection_reversed = false;
            self.marked_range = None;
            self.last_layouts.clear();
            self.reveal_cursor = true;
            self.refresh_content_metrics();
            cx.notify();
            self.schedule_completion(window, cx);
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
        if self.read_only {
            return;
        }
        self.replace_text_in_range(range_utf16, new_text, window, cx);
        let insertion_start = self
            .document
            .selection()
            .start
            .saturating_sub(new_text.len());
        if !new_text.is_empty() {
            self.marked_range = Some(insertion_start..insertion_start + new_text.len());
        }
        if let Some(selection) = new_selected_range_utf16 {
            // IME selection offsets are relative to the newly composed text.
            let start = utf16_to_byte(new_text, selection.start);
            let end = utf16_to_byte(new_text, selection.end);
            let _ = self
                .document
                .set_selection(insertion_start + start..insertion_start + end);
        }
        // replace_text_in_range runs before the marked range is installed.
        // Do not let that intermediate state start an IME completion request.
        self.cancel_completion();
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
        let start_line = self.document.line_of_offset(range.start);
        let end_line = self.document.line_of_offset(range.end);
        let start_range = self.document.line_range(start_line);
        let end_range = self.document.line_range(end_line);
        let start_column = range
            .start
            .saturating_sub(start_range.start)
            .min(start_range.len());
        let end_column = range
            .end
            .saturating_sub(end_range.start)
            .min(end_range.len());
        let start_layout = self.last_layouts.get(&start_line)?;
        let end_layout = self.last_layouts.get(&end_line)?;
        let gutter = px(self.gutter_width());
        Some(Bounds::from_corners(
            point(
                bounds.left() + gutter + start_layout.x_for_index(start_column),
                bounds.top() + px(LINE_HEIGHT * start_line as f32),
            ),
            point(
                bounds.left() + gutter + end_layout.x_for_index(end_column),
                bounds.top() + px(LINE_HEIGHT * (end_line + 1) as f32),
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
    lines: Vec<(usize, ShapedLine)>,
    numbers: Vec<(usize, ShapedLine)>,
    selection: Vec<PaintQuad>,
    cursor: Option<PaintQuad>,
    completion: Option<(Point<Pixels>, ShapedLine)>,
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
        let input = self.input.read(cx);
        let line_count = input.document.line_count();
        let text_style = window.text_style();
        let font_size = f32::from(text_style.font_size.to_pixels(window.rem_size()));
        // Consolas advances are below 0.65em. Non-ASCII columns are conservatively
        // counted as two cells so long lines remain reachable without shaping the file.
        let content_width =
            input.gutter_width() + input.max_line_columns as f32 * font_size * 0.65 + TEXT_PADDING;
        let mut style = Style::default();
        style.size.width = px(content_width).into();
        style.min_size.width = relative(1.0).into();
        style.size.height = px(LINE_HEIGHT * line_count as f32).into();
        style.min_size.height = relative(1.0).into();
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
        let scale_factor = window.scale_factor();
        let (first, last, geometry_changed) = {
            let input = self.input.read(cx);
            let scroll_offset = -input.scroll_handle.offset().y;
            let viewport_height = input
                .scroll_handle
                .bounds()
                .size
                .height
                .max(window.content_mask().bounds.size.height);
            let visible = visible_line_range(
                scroll_offset.into(),
                viewport_height.into(),
                input.document.line_count(),
            );
            let first = visible.start;
            let last = visible.end;
            let geometry_changed = input.last_bounds.is_some_and(|old| old.size != bounds.size)
                || input.last_scale_factor != scale_factor;
            (first, last, geometry_changed)
        };
        self.input.update(cx, |input, _| {
            // Invalidate before shaping, and retain only the virtualized window.
            if geometry_changed {
                input.last_layouts.clear();
            } else {
                input
                    .last_layouts
                    .retain(|line, _| *line >= first && *line < last);
            }
            input.last_scale_factor = scale_factor;
        });
        let input = self.input.read(cx);
        let text = input.document.text();
        let selection = input.document.selection();
        let cursor = input.cursor_offset();
        let style = window.text_style();
        let font_size = style.font_size.to_pixels(window.rem_size());
        let gutter = px(input.gutter_width());
        let number_width = input.document.line_count().max(1).ilog10() as usize + 1;
        let mut lines = Vec::new();
        let mut numbers = Vec::new();
        let mut selection_quads = Vec::new();
        let mut cursor_quad = None;
        let mut completion = None;

        for line_index in first..last {
            let range = input.document.line_range(line_index);
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
                highlight_runs(input, &range, run)
            };
            let line = window.text_system().shape_line(
                line_text.to_owned().into(),
                font_size,
                &runs,
                None,
            );
            let number_text = line_number_text(line_index, number_width);
            let number_run = TextRun {
                len: number_text.len(),
                font: style.font(),
                color: style.color.opacity(0.4),
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let number =
                window
                    .text_system()
                    .shape_line(number_text.into(), font_size, &[number_run], None);
            let top = bounds.top() + px(LINE_HEIGHT * line_index as f32);

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
                        point(bounds.left() + gutter + start_x, top),
                        size((end_x - start_x).max(px(4.0)), px(LINE_HEIGHT)),
                    ),
                    rgba(0x7c8cff40),
                ));
            }
            if cursor >= range.start && cursor <= range.end {
                let cursor_x = line.x_for_index(cursor - range.start);
                if let Some((revision, offset, suggestion)) = &input.completion
                    && input.completion_is_current(*revision, *offset)
                {
                    let run = TextRun {
                        len: suggestion.len(),
                        font: style.font(),
                        color: style.color.opacity(0.45),
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    };
                    let shaped = window.text_system().shape_line(
                        suggestion.clone().into(),
                        font_size,
                        &[run],
                        None,
                    );
                    completion = Some((point(bounds.left() + gutter + cursor_x, top), shaped));
                }
                cursor_quad = Some(fill(
                    Bounds::new(
                        point(bounds.left() + gutter + cursor_x, top),
                        size(px(2.0), px(LINE_HEIGHT)),
                    ),
                    style.color,
                ));
            }
            lines.push((line_index, line));
            numbers.push((line_index, number));
        }
        PrepaintState {
            lines,
            numbers,
            selection: selection_quads,
            cursor: cursor_quad,
            completion,
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
        let (focus_handle, gutter) = {
            let input = self.input.read(cx);
            (input.focus_handle.clone(), px(input.gutter_width()))
        };
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );
        for selection in prepaint.selection.drain(..) {
            window.paint_quad(selection);
        }
        for (index, line) in prepaint.lines.drain(..) {
            let origin = point(
                bounds.left() + gutter,
                bounds.top() + px(LINE_HEIGHT * index as f32),
            );
            let _ = line.paint(origin, px(LINE_HEIGHT), window, cx);
            self.input.update(cx, |input, _| {
                input.last_layouts.insert(index, line.clone());
            });
        }
        for (index, number) in prepaint.numbers.drain(..) {
            let _ = number.paint(
                point(
                    bounds.left() + px(8.0),
                    bounds.top() + px(LINE_HEIGHT * index as f32),
                ),
                px(LINE_HEIGHT),
                window,
                cx,
            );
        }
        if focus_handle.is_focused(window)
            && let Some(cursor) = prepaint.cursor.take()
        {
            window.paint_quad(cursor);
        }
        if focus_handle.is_focused(window)
            && let Some((origin, line)) = prepaint.completion.take()
        {
            let _ = line.paint(origin, px(LINE_HEIGHT), window, cx);
        }
        let visible = window.content_mask().bounds;
        self.input.update(cx, |input, _| {
            input.last_bounds = Some(bounds);
            if std::mem::take(&mut input.reveal_cursor) {
                input.scroll_cursor_into_view(bounds, visible, window);
            }
        });
    }
}

impl DocumentView {
    fn scroll_cursor_into_view(
        &mut self,
        bounds: Bounds<Pixels>,
        visible: Bounds<Pixels>,
        window: &mut Window,
    ) {
        let cursor = self.cursor_offset();
        let line = self.document.line_of_offset(cursor);
        let range = self.document.line_range(line);
        let column = cursor.saturating_sub(range.start).min(range.len());
        let top = bounds.top() + px(LINE_HEIGHT * line as f32);
        let bottom = top + px(LINE_HEIGHT);
        let mut delta_x = px(0.0);
        let delta_y = px(reveal_delta(
            top.into(),
            bottom.into(),
            visible.top().into(),
            visible.bottom().into(),
        ));
        if let Some(layout) = self.last_layouts.get(&line) {
            let left = bounds.left() + px(self.gutter_width()) + layout.x_for_index(column);
            let right = left + px(2.0);
            delta_x = px(reveal_delta(
                left.into(),
                right.into(),
                visible.left().into(),
                visible.right().into(),
            ));
        } else if !delta_y.is_zero() {
            // The caret line was offscreen. Reveal vertically now and horizontally
            // after its layout has been shaped on the next frame.
            self.reveal_cursor = true;
        }
        if delta_x.is_zero() && delta_y.is_zero() {
            return;
        }
        let offset = self.scroll_handle.offset();
        let maximum = self.scroll_handle.max_offset();
        let next = point(offset.x + delta_x, offset.y + delta_y);
        self.scroll_handle.set_offset(point(
            px(clamp_scroll_offset(next.x.into(), maximum.width.into())),
            px(clamp_scroll_offset(next.y.into(), maximum.height.into())),
        ));
        window.on_next_frame(|window, _| window.refresh());
    }
}

fn highlight_runs(input: &DocumentView, line_range: &Range<usize>, base: TextRun) -> Vec<TextRun> {
    let Some(syntax) = &input.syntax else {
        return vec![base];
    };
    let mut runs = Vec::new();
    let mut cursor = 0usize;
    for span in syntax.spans_in_range(line_range.clone()) {
        let start = span.range.start.max(line_range.start) - line_range.start;
        let end = span.range.end.min(line_range.end) - line_range.start;
        if start < cursor || end <= start {
            continue;
        }
        if start > cursor {
            runs.push(TextRun {
                len: start - cursor,
                ..base.clone()
            });
        }
        runs.push(TextRun {
            len: end - start,
            color: highlight_color(span.kind),
            ..base.clone()
        });
        cursor = end;
    }
    if cursor < line_range.len() {
        runs.push(TextRun {
            len: line_range.len() - cursor,
            ..base.clone()
        });
    }
    if runs.is_empty() { vec![base] } else { runs }
}

fn highlight_color(kind: HighlightKind) -> gpui::Hsla {
    match kind {
        HighlightKind::Keyword => gpui::rgb(0xc678dd).into(),
        HighlightKind::String => gpui::rgb(0x78c6a3).into(),
        HighlightKind::Comment => gpui::rgb(0x596275).into(),
        HighlightKind::Number => gpui::rgb(0xe5c07b).into(),
        HighlightKind::Function => gpui::rgb(0x61afef).into(),
        HighlightKind::Type => gpui::rgb(0xe5c07b).into(),
        HighlightKind::Macro => gpui::rgb(0xe06c75).into(),
        HighlightKind::Attribute => gpui::rgb(0xd19a66).into(),
    }
}

fn inline_completion_line(text: &str) -> Option<&str> {
    let mut lines = text.lines();
    let first = lines.next()?;
    let line = if first.trim_start().starts_with("```") {
        lines.next()?
    } else {
        first
    };
    if line.trim().is_empty() || line.trim_start().starts_with("```") {
        None
    } else {
        Some(line)
    }
}

impl Render for DocumentView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.watch_completion_focus(window, cx);
        if std::mem::take(&mut self.completion_refresh) {
            self.schedule_completion(window, cx);
        }
        div()
            .relative()
            .size_full()
            .id("document-editor-scroll")
            .key_context("Document")
            .track_focus(&self.focus_handle(cx))
            .track_scroll(&self.scroll_handle)
            .overflow_scroll()
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::up))
            .on_action(cx.listener(Self::down))
            .on_action(cx.listener(Self::page_up))
            .on_action(cx.listener(Self::page_down))
            .on_action(cx.listener(Self::document_start))
            .on_action(cx.listener(Self::document_end))
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
            .on_action(cx.listener(Self::dismiss_completion))
            .on_action(cx.listener(Self::show_character_palette))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::on_right_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .child(DocumentElement { input: cx.entity() })
            .when(self.context_menu_open, |this| {
                this.child(self.context_menu(cx))
            })
    }
}

impl Focusable for DocumentView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Drop for DocumentView {
    fn drop(&mut self) {
        if let Some(cancel) = self.completion_cancel.take() {
            cancel.cancel();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn completion_accepts_only_current_revision_and_tab_is_undoable(cx: &mut gpui::TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, cx| DocumentView::new(cx));
        cx.update(|window, app| {
            view.update(app, |document, cx| {
                document.set_document(Document::from_text("fn "), cx);
                document.document_mut().set_selection(3..3).unwrap();
                let (revision, cursor, _) = document.completion_context().unwrap();
                document.move_to(2, cx);
                document.move_to(3, cx);
                document.set_completion(revision, cursor, "stale", cx);
                assert!(document.completion.is_none());
                let (revision, cursor, _) = document.completion_context().unwrap();
                document.set_completion(revision, cursor, "main() {}\nnot shown", cx);
                document.tab(&InsertTab, window, cx);
                assert_eq!(document.document().text(), "fn main() {}");
                document.undo(&Undo, window, cx);
                assert_eq!(document.document().text(), "fn ");
            })
        });
    }

    #[gpui::test]
    fn completion_rejects_midline_ime_and_accepts_fenced_code(cx: &mut gpui::TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, cx| DocumentView::new(cx));
        cx.update(|_, app| {
            view.update(app, |document, cx| {
                document.set_document(Document::from_text("fn main() {}"), cx);
                document.document_mut().set_selection(2..2).unwrap();
                let revision = document.revision;
                assert!(document.completion_context().is_none());
                document.set_completion(revision, 2, "stale", cx);
                assert!(document.completion.is_none());

                document.move_to(12, cx);
                let (revision, cursor, _) = document.completion_context().unwrap();
                document.marked_range = Some(0..2);
                document.set_completion(revision, cursor, "stale", cx);
                assert!(document.completion.is_none());

                document.marked_range = None;
                document.set_completion(revision, cursor, "```rust\n// done\n```", cx);
                assert_eq!(
                    document
                        .completion
                        .as_ref()
                        .map(|(_, _, text)| text.as_str()),
                    Some("// done")
                );
            })
        });
    }

    #[test]
    fn inline_completion_is_a_nonempty_single_safe_line() {
        assert_eq!(inline_completion_line("value\nignored"), Some("value"));
        assert_eq!(
            inline_completion_line("  ```rust\nvalue\n```"),
            Some("value")
        );
        assert_eq!(inline_completion_line("\nvalue"), None);
        assert_eq!(inline_completion_line("   \nvalue"), None);
        assert_eq!(inline_completion_line("```rust\n```"), None);
        assert_eq!(inline_completion_line(""), None);
    }

    #[test]
    fn utf16_offsets_are_relative_and_never_split_unicode() {
        let text = "a🦀b";
        assert_eq!(utf16_to_byte(text, 0), 0);
        assert_eq!(utf16_to_byte(text, 1), 1);
        assert_eq!(utf16_to_byte(text, 3), 5);
        assert_eq!(utf16_to_byte(text, 4), 6);
        assert_eq!(byte_to_utf16(text, 5), 3);
        assert!(text.is_char_boundary(utf16_to_byte(text, 2)));
    }

    #[test]
    fn virtualizes_and_formats_lines_past_four_digits() {
        let visible = visible_line_range(9999.0 * LINE_HEIGHT, 44.0, 12_000);
        assert_eq!(visible, 9999..10_003);
        let number = line_number_text(9999, 5);
        assert_eq!(number, "10000");
        assert_eq!(number.len(), 5);
    }

    #[test]
    fn caret_reveal_and_scrollbar_offsets_are_clamped() {
        assert_eq!(reveal_delta(20.0, 42.0, 0.0, 100.0), 0.0);
        assert_eq!(reveal_delta(-20.0, 2.0, 0.0, 100.0), 20.0);
        assert_eq!(reveal_delta(90.0, 112.0, 0.0, 100.0), -12.0);
        assert_eq!(clamp_scroll_offset(-500.0, 120.0), -120.0);
        assert_eq!(clamp_scroll_offset(20.0, 120.0), 0.0);
        assert_eq!(clamp_scroll_offset(f32::NAN, 120.0), 0.0);
    }
}
