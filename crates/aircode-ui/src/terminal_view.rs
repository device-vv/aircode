use std::{
    ffi::OsString,
    ops::Range,
    path::{Path, PathBuf},
};

use aircode_i18n::{Language, tr};
use gpui::{
    App, Bounds, Context, CursorStyle, Element, ElementId, ElementInputHandler, Entity,
    EntityInputHandler, FocusHandle, Focusable, GlobalElementId, KeyDownEvent, LayoutId,
    MouseButton, PaintQuad, Pixels, ScrollWheelEvent, ShapedLine, Style, TextRun, UTF16Selection,
    UnderlineStyle, Window, div, fill, point, prelude::*, px, relative, size,
};
use terminal_lab::{
    CellSnapshot, Key, KeyEvent, Modifiers, ProcessCommand, Terminal, TerminalColor,
    TerminalConfig, TerminalSize, TerminalSnapshot, UpdateReceiver,
};

use crate::theme::Theme;

const LINE_HEIGHT: f32 = 18.0;
const TEXT_PADDING: f32 = 12.0;
const MIN_TERMINAL_ROWS: u16 = 2;
const MIN_TERMINAL_COLUMNS: u16 = 20;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TerminalStatus {
    Idle,
    Starting,
    Running,
    Exited,
    Failed,
}

pub(crate) struct TerminalView {
    focus_handle: FocusHandle,
    terminal: Option<Terminal>,
    status: TerminalStatus,
    language: Language,
    theme: Theme,
    error: Option<String>,
    generation: u64,
    scroll_offset: usize,
    history_lines: usize,
    last_size: Option<TerminalSize>,
    last_bounds: Option<Bounds<Pixels>>,
    last_cell_width: Pixels,
    last_cursor: (u16, u16),
    ime_text: String,
}

impl TerminalView {
    pub fn new(language: Language, cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            terminal: None,
            status: TerminalStatus::Idle,
            language,
            theme: Theme::from_mode(aircode_core::ThemeMode::Dark),
            error: None,
            generation: 0,
            scroll_offset: 0,
            history_lines: 0,
            last_size: None,
            last_bounds: None,
            last_cell_width: px(8.0),
            last_cursor: (0, 0),
            ime_text: String::new(),
        }
    }

    pub fn set_language(&mut self, language: Language) {
        self.language = language;
    }

    pub fn set_theme(&mut self, theme: Theme) {
        self.theme = theme;
    }

    pub fn can_restart(&self) -> bool {
        matches!(
            self.status,
            TerminalStatus::Idle | TerminalStatus::Exited | TerminalStatus::Failed
        )
    }

    #[cfg(test)]
    pub fn status(&self) -> TerminalStatus {
        self.status
    }

    pub fn start(&mut self, project_root: Option<PathBuf>, cx: &mut Context<Self>) {
        if matches!(
            self.status,
            TerminalStatus::Starting | TerminalStatus::Running
        ) {
            return;
        }
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        self.terminal.take();
        self.error = None;
        self.scroll_offset = 0;
        self.history_lines = 0;
        self.last_size = None;
        self.ime_text.clear();

        let Some(cwd) = project_root else {
            self.status = TerminalStatus::Failed;
            self.error = Some(tr(self.language, "terminal_project_required").to_owned());
            cx.notify();
            return;
        };
        let command = match system_shell_command(&cwd) {
            Ok(command) => command,
            Err(error) => {
                self.status = TerminalStatus::Failed;
                self.error = Some(error);
                cx.notify();
                return;
            }
        };
        let config = TerminalConfig::new(command);
        self.status = TerminalStatus::Starting;
        cx.notify();

        let spawn = cx
            .background_executor()
            .spawn(async move { Terminal::spawn(config) });
        cx.spawn(async move |this, cx| {
            let result = spawn.await;
            let updates =
                match this.update(cx, |this, cx| this.finish_start(generation, result, cx)) {
                    Ok(Some(updates)) => updates,
                    _ => return,
                };

            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(50))
                    .await;
                match updates.try_recv() {
                    Ok(()) => {}
                    Err(std::sync::mpsc::TryRecvError::Empty) => continue,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
                }
                let active = this
                    .update(cx, |this, cx| this.refresh(generation, cx))
                    .unwrap_or(false);
                if !active {
                    break;
                }
            }
        })
        .detach();
    }

    fn finish_start(
        &mut self,
        generation: u64,
        result: Result<(Terminal, UpdateReceiver), terminal_lab::TerminalError>,
        cx: &mut Context<Self>,
    ) -> Option<UpdateReceiver> {
        if generation != self.generation {
            return None;
        }
        match result {
            Ok((terminal, updates)) => {
                self.terminal = Some(terminal);
                self.status = TerminalStatus::Running;
                self.error = None;
                cx.notify();
                Some(updates)
            }
            Err(error) => {
                self.status = TerminalStatus::Failed;
                self.error = Some(error.to_string());
                cx.notify();
                None
            }
        }
    }

    fn refresh(&mut self, generation: u64, cx: &mut Context<Self>) -> bool {
        if generation != self.generation {
            return false;
        }
        if let Some(terminal) = &self.terminal
            && terminal.is_reader_closed()
        {
            self.status = TerminalStatus::Exited;
            self.error = terminal.last_io_error();
        }
        cx.notify();
        true
    }

    fn status_text(&self) -> Option<String> {
        match self.status {
            TerminalStatus::Idle => Some(tr(self.language, "terminal_idle").to_owned()),
            TerminalStatus::Starting => Some(tr(self.language, "terminal_starting").to_owned()),
            TerminalStatus::Failed => Some(format!(
                "{}: {}",
                tr(self.language, "terminal_start_failed"),
                self.error.as_deref().unwrap_or("unknown error")
            )),
            TerminalStatus::Exited => {
                let mut message = tr(self.language, "terminal_exited").to_owned();
                if let Some(error) = &self.error {
                    message.push_str(": ");
                    message.push_str(error);
                }
                Some(message)
            }
            TerminalStatus::Running => None,
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let modifiers = Modifiers {
            shift: keystroke.modifiers.shift,
            alt: keystroke.modifiers.alt,
            ctrl: keystroke.modifiers.control,
        };

        if keystroke.key.eq_ignore_ascii_case("v")
            && ((modifiers.ctrl && modifiers.shift)
                || (cfg!(target_os = "macos") && keystroke.modifiers.platform))
        {
            if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                self.send_text(&text, cx);
            }
            cx.stop_propagation();
            return;
        }

        if modifiers.shift && keystroke.key == "pageup" {
            self.scroll_by(visible_page_lines(self.last_bounds), cx);
            cx.stop_propagation();
            return;
        }
        if modifiers.shift && keystroke.key == "pagedown" {
            self.scroll_by(-visible_page_lines(self.last_bounds), cx);
            cx.stop_propagation();
            return;
        }

        let Some(key_event) = map_key(&keystroke.key, modifiers) else {
            return;
        };
        if matches!(key_event.key, Key::Char(_)) && !modifiers.ctrl && !modifiers.alt {
            // Printable text, including Unicode, arrives through the input
            // handler so it follows the platform IME commit path.
            return;
        }
        if let Some(terminal) = &self.terminal
            && terminal.send_key(key_event).is_ok()
        {
            self.scroll_offset = 0;
            self.ime_text.clear();
            cx.notify();
        }
        cx.stop_propagation();
    }

    fn on_mouse_down(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle);
        cx.notify();
    }

    fn on_scroll_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let delta = event.delta.pixel_delta(px(LINE_HEIGHT));
        let lines = (-f32::from(delta.y) / LINE_HEIGHT).round() as isize;
        self.scroll_by(lines, cx);
        cx.stop_propagation();
    }

    fn scroll_by(&mut self, lines: isize, cx: &mut Context<Self>) {
        self.scroll_offset = next_scroll_offset(self.scroll_offset, lines, self.history_lines);
        cx.notify();
    }

    fn send_text(&mut self, text: &str, cx: &mut Context<Self>) {
        if text.is_empty() {
            return;
        }
        if let Some(terminal) = &self.terminal
            && terminal.send_input(text.as_bytes()).is_ok()
        {
            self.scroll_offset = 0;
            cx.notify();
        }
    }
}

impl Focusable for TerminalView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EntityInputHandler for TerminalView {
    fn character_index_for_point(
        &mut self,
        _: gpui::Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }

    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        if range_utf16.start > range_utf16.end {
            return None;
        }
        let range = utf16_range_to_bytes(&self.ime_text, range_utf16);
        actual_range.replace(byte_range_to_utf16(&self.ime_text, range.clone()));
        Some(self.ime_text[range].to_owned())
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let end = self.ime_text.len();
        Some(UTF16Selection {
            range: byte_range_to_utf16(&self.ime_text, end..end),
            reversed: false,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        (!self.ime_text.is_empty())
            .then(|| byte_range_to_utf16(&self.ime_text, 0..self.ime_text.len()))
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.ime_text.clear();
    }

    fn replace_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        new_text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.ime_text.clear();
        self.send_text(new_text, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        new_text: &str,
        _: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Composition is kept locally for IME positioning. Only the final
        // replace_text_in_range commit is written to the PTY.
        self.ime_text = new_text.to_owned();
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let bounds = self.last_bounds.unwrap_or(element_bounds);
        let (row, column) = self.last_cursor;
        let origin = point(
            bounds.left() + px(TEXT_PADDING) + self.last_cell_width * f32::from(column),
            bounds.top() + px(TEXT_PADDING + LINE_HEIGHT * f32::from(row)),
        );
        Some(Bounds::new(
            origin,
            size(self.last_cell_width, px(LINE_HEIGHT)),
        ))
    }
}

impl Render for TerminalView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme;
        let status = self.status_text();
        let status_is_error = matches!(self.status, TerminalStatus::Failed);
        div()
            .id("terminal-grid")
            .debug_selector(|| "qa-terminal_view".to_owned())
            .key_context("Terminal")
            .track_focus(&self.focus_handle)
            .relative()
            .size_full()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .overflow_hidden()
            .bg(theme.window)
            .font_family(terminal_font())
            .text_sm()
            .cursor(CursorStyle::IBeam)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| this.on_mouse_down(window, cx)),
            )
            .on_scroll_wheel(cx.listener(Self::on_scroll_wheel))
            .child(TerminalElement {
                terminal: cx.entity(),
                theme,
            })
            .when_some(status, |this, message| {
                this.child(
                    div()
                        .absolute()
                        .top(px(6.0))
                        .left(px(12.0))
                        .right(px(12.0))
                        .text_xs()
                        .text_color(if status_is_error {
                            theme.red
                        } else {
                            theme.muted
                        })
                        .child(message),
                )
            })
    }
}

struct TerminalElement {
    terminal: Entity<TerminalView>,
    theme: Theme,
}

struct PrepaintState {
    lines: Vec<(u16, ShapedLine)>,
    cursor: Option<PaintQuad>,
}

impl IntoElement for TerminalElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TerminalElement {
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
        let mut style = Style::default();
        style.size.width = relative(1.0).into();
        style.size.height = relative(1.0).into();
        style.min_size.width = relative(1.0).into();
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
        let text_style = window.text_style();
        let font_size = text_style.font_size.to_pixels(window.rem_size());
        let probe_run = TextRun {
            len: 1,
            font: text_style.font(),
            color: text_style.color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let probe = window
            .text_system()
            .shape_line("0".into(), font_size, &[probe_run], None);
        let cell_width = probe.width.max(px(1.0));
        let target_size = terminal_size_for_bounds(
            bounds.size.width.into(),
            bounds.size.height.into(),
            cell_width.into(),
        );

        let mut snapshot = None;
        let mut running = false;
        self.terminal.update(cx, |view, _| {
            view.last_cell_width = cell_width;
            if let Some(terminal) = &view.terminal {
                if view.last_size != Some(target_size) {
                    terminal.resize(target_size).ok();
                    view.last_size = Some(target_size);
                }
                let current = terminal.snapshot_at(view.scroll_offset);
                view.history_lines = current.scrollback;
                view.scroll_offset = current.scrollback_offset;
                view.last_cursor = (current.cursor.row, current.cursor.column);
                running = view.status == TerminalStatus::Running;
                snapshot = Some(current);
            }
        });

        let Some(snapshot) = snapshot else {
            return PrepaintState {
                lines: Vec::new(),
                cursor: None,
            };
        };
        let mut lines = Vec::with_capacity(usize::from(snapshot.size.rows()));
        let mut cursor = None;
        for row in 0..snapshot.size.rows() {
            let (text, runs) = shape_row(&snapshot, row, &text_style, self.theme);
            let line = window
                .text_system()
                .shape_line(text.into(), font_size, &runs, None);
            if running
                && snapshot.cursor.visible
                && snapshot.cursor.row == row
                && snapshot.scrollback_offset == 0
            {
                cursor = Some(fill(
                    Bounds::new(
                        point(
                            bounds.left()
                                + px(TEXT_PADDING)
                                + cell_width * f32::from(snapshot.cursor.column),
                            bounds.top() + px(TEXT_PADDING + LINE_HEIGHT * f32::from(row)),
                        ),
                        size(px(2.0), px(LINE_HEIGHT)),
                    ),
                    self.theme.accent,
                ));
            }
            lines.push((row, line));
        }
        PrepaintState { lines, cursor }
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
        let focus_handle = self.terminal.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.terminal.clone()),
            cx,
        );
        for (row, line) in prepaint.lines.drain(..) {
            let origin = point(
                bounds.left() + px(TEXT_PADDING),
                bounds.top() + px(TEXT_PADDING + LINE_HEIGHT * f32::from(row)),
            );
            let _ = line.paint(origin, px(LINE_HEIGHT), window, cx);
        }
        if focus_handle.is_focused(window)
            && let Some(cursor) = prepaint.cursor.take()
        {
            window.paint_quad(cursor);
        }
        self.terminal.update(cx, |view, _| {
            view.last_bounds = Some(bounds);
        });
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
struct CellStyle {
    foreground: TerminalColor,
    background: TerminalColor,
    bold: bool,
    italic: bool,
    underline: bool,
    inverse: bool,
}

impl From<&CellSnapshot> for CellStyle {
    fn from(cell: &CellSnapshot) -> Self {
        Self {
            foreground: cell.foreground,
            background: cell.background,
            bold: cell.bold,
            italic: cell.italic,
            underline: cell.underline,
            inverse: cell.inverse,
        }
    }
}

fn shape_row(
    snapshot: &TerminalSnapshot,
    row: u16,
    text_style: &gpui::TextStyle,
    theme: Theme,
) -> (String, Vec<TextRun>) {
    let mut text = String::new();
    let mut runs = Vec::new();
    let mut run_start = 0;
    let mut current_style = None;
    for column in 0..snapshot.size.columns() {
        let cell = snapshot.cell(row, column).expect("cell within snapshot");
        let style = CellStyle::from(cell);
        if current_style.is_some_and(|current| current != style) {
            runs.push(text_run(
                text.len() - run_start,
                current_style.expect("checked above"),
                text_style,
                theme,
            ));
            run_start = text.len();
        }
        current_style = Some(style);
        if cell.contents.is_empty() {
            text.push(' ');
        } else {
            text.push_str(&cell.contents);
        }
    }
    if text.len() > run_start
        && let Some(style) = current_style
    {
        runs.push(text_run(text.len() - run_start, style, text_style, theme));
    }
    (text, runs)
}

fn text_run(len: usize, style: CellStyle, text_style: &gpui::TextStyle, theme: Theme) -> TextRun {
    let default_foreground = theme.text.into();
    let default_background = theme.window.into();
    let (foreground, background) = if style.inverse {
        (style.background, style.foreground)
    } else {
        (style.foreground, style.background)
    };
    let foreground = terminal_color(foreground, default_foreground);
    let background = match background {
        TerminalColor::Default if !style.inverse => None,
        color => Some(terminal_color(color, default_background)),
    };
    let mut font = text_style.font();
    if style.bold {
        font.weight = gpui::FontWeight::BOLD;
    }
    if style.italic {
        font.style = gpui::FontStyle::Italic;
    }
    TextRun {
        len,
        font,
        color: foreground,
        background_color: background,
        underline: style.underline.then_some(UnderlineStyle {
            color: Some(foreground),
            thickness: px(1.0),
            wavy: false,
        }),
        strikethrough: None,
    }
}

fn terminal_color(color: TerminalColor, default: gpui::Hsla) -> gpui::Hsla {
    match color {
        TerminalColor::Default => default,
        TerminalColor::Rgb(red, green, blue) => gpui::rgba(
            (u32::from(red) << 24) | (u32::from(green) << 16) | (u32::from(blue) << 8) | 0xff,
        )
        .into(),
        TerminalColor::Indexed(index) => {
            const BASIC: [u32; 16] = [
                0x000000, 0xcd3131, 0x0dbc79, 0xe5e510, 0x2472c8, 0xbc3fbc, 0x11a8cd, 0xe5e5e5,
                0x666666, 0xf14c4c, 0x23d18b, 0xf5f543, 0x3b8eea, 0xd670d6, 0x29b8db, 0xffffff,
            ];
            let rgb = match index {
                0..=15 => BASIC[usize::from(index)],
                16..=231 => {
                    let value = index - 16;
                    let red = value / 36;
                    let green = (value % 36) / 6;
                    let blue = value % 6;
                    let level = |part: u8| if part == 0 { 0 } else { 55 + 40 * part };
                    (u32::from(level(red)) << 16)
                        | (u32::from(level(green)) << 8)
                        | u32::from(level(blue))
                }
                232..=255 => {
                    let gray = 8 + 10 * (index - 232);
                    (u32::from(gray) << 16) | (u32::from(gray) << 8) | u32::from(gray)
                }
            };
            gpui::rgb(rgb).into()
        }
    }
}

fn system_shell_command(cwd: &Path) -> Result<ProcessCommand, String> {
    #[cfg(windows)]
    {
        let cwd = windows_shell_path(cwd)?;
        if cwd.as_os_str().to_string_lossy().starts_with(r"\\") {
            // cmd.exe cannot use a UNC path as its current directory. Start
            // PowerShell from a local directory and switch its provider path.
            let path = cwd.to_str().ok_or("Project path is not valid Unicode")?;
            let command = format!("Set-Location -LiteralPath '{}'", path.replace('\'', "''"));
            let local = std::env::var_os("SystemRoot")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(r"C:\"));
            return Ok(ProcessCommand::new("powershell.exe", local)
                .args(["-NoLogo", "-NoExit", "-Command"])
                .arg(command));
        }
        let shell = std::env::var_os("COMSPEC")
            .filter(|shell| !shell.is_empty())
            .unwrap_or_else(|| OsString::from("cmd.exe"));
        Ok(ProcessCommand::new(shell, cwd))
    }
    #[cfg(not(windows))]
    {
        let shell = std::env::var_os("SHELL")
            .filter(|shell| !shell.is_empty())
            .unwrap_or_else(|| OsString::from("/bin/sh"));
        Ok(ProcessCommand::new(shell, cwd))
    }
}

#[cfg(windows)]
fn windows_shell_path(path: &Path) -> Result<PathBuf, String> {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    let wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    let prefix: Vec<u16> = r"\\?\".encode_utf16().collect();
    if !wide.starts_with(&prefix) {
        return Ok(path.to_owned());
    }
    let suffix = &wide[prefix.len()..];
    let unc: Vec<u16> = "UNC\\".encode_utf16().collect();
    if suffix.starts_with(&unc) {
        let mut normal = vec![b'\\' as u16, b'\\' as u16];
        normal.extend_from_slice(&suffix[unc.len()..]);
        return Ok(PathBuf::from(OsString::from_wide(&normal)));
    }
    if suffix.len() >= 3 && suffix[1] == b':' as u16 && suffix[2] == b'\\' as u16 {
        return Ok(PathBuf::from(OsString::from_wide(suffix)));
    }
    Err("Unsupported Windows project path for terminal".into())
}

fn terminal_font() -> &'static str {
    if cfg!(target_os = "windows") {
        "Consolas"
    } else if cfg!(target_os = "macos") {
        "Menlo"
    } else {
        "DejaVu Sans Mono"
    }
}

fn map_key(key: &str, modifiers: Modifiers) -> Option<KeyEvent> {
    let key = match key {
        "enter" => Key::Enter,
        "backspace" => Key::Backspace,
        "tab" => Key::Tab,
        "up" => Key::Up,
        "down" => Key::Down,
        "left" => Key::Left,
        "right" => Key::Right,
        "home" => Key::Home,
        "end" => Key::End,
        "insert" => Key::Insert,
        "delete" => Key::Delete,
        "pageup" | "page-up" => Key::PageUp,
        "pagedown" | "page-down" => Key::PageDown,
        "escape" | "esc" => Key::Escape,
        "space" if modifiers.ctrl || modifiers.alt => Key::Char(' '),
        _ => {
            if let Some(number) = key.strip_prefix('f').and_then(|value| value.parse().ok()) {
                Key::F(number)
            } else {
                let mut characters = key.chars();
                let character = characters.next()?;
                if characters.next().is_some() {
                    return None;
                }
                Key::Char(character)
            }
        }
    };
    Some(KeyEvent::with_modifiers(key, modifiers))
}

fn next_scroll_offset(current: usize, delta: isize, available: usize) -> usize {
    let current = current.min(available);
    if delta < 0 {
        current.saturating_sub(delta.unsigned_abs())
    } else {
        current.saturating_add(delta as usize).min(available)
    }
}

fn visible_page_lines(bounds: Option<Bounds<Pixels>>) -> isize {
    bounds
        .map(|bounds| {
            (f32::from(bounds.size.height) / LINE_HEIGHT)
                .floor()
                .max(1.0) as isize
        })
        .unwrap_or(1)
}

fn terminal_size_for_bounds(width: f32, height: f32, cell_width: f32) -> TerminalSize {
    let cell_width = if cell_width.is_finite() && cell_width > 0.0 {
        cell_width
    } else {
        8.0
    };
    let width = if width.is_finite() {
        width.max(0.0)
    } else {
        0.0
    };
    let height = if height.is_finite() {
        height.max(0.0)
    } else {
        0.0
    };
    let columns = ((width - 2.0 * TEXT_PADDING) / cell_width).floor() as u16;
    let rows = ((height - 2.0 * TEXT_PADDING) / LINE_HEIGHT).floor() as u16;
    TerminalSize::new(
        rows.clamp(MIN_TERMINAL_ROWS, 500),
        columns.clamp(MIN_TERMINAL_COLUMNS, 1000),
    )
    .expect("clamped terminal dimensions are valid")
}

fn utf16_range_to_bytes(text: &str, range: Range<usize>) -> Range<usize> {
    utf16_to_byte(text, range.start)..utf16_to_byte(text, range.end)
}

fn byte_range_to_utf16(text: &str, range: Range<usize>) -> Range<usize> {
    text[..range.start].encode_utf16().count()..text[..range.end].encode_utf16().count()
}

fn utf16_to_byte(text: &str, offset: usize) -> usize {
    let mut utf16_offset = 0;
    for (index, character) in text.char_indices() {
        if utf16_offset >= offset {
            return index;
        }
        utf16_offset += character.len_utf16();
    }
    text.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn windows_shell_uses_the_project_directory_after_canonicalization() {
        let project = std::env::current_dir().unwrap().canonicalize().unwrap();
        let expected = windows_shell_path(&project)
            .unwrap()
            .to_string_lossy()
            .to_lowercase();
        let config = TerminalConfig::new(system_shell_command(&project).unwrap())
            .with_size(TerminalSize::new(24, 200).unwrap());
        let (terminal, _updates) = Terminal::spawn(config).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            let snapshot = terminal.snapshot();
            let text = (0..snapshot.size.rows())
                .filter_map(|row| snapshot.row_text(row))
                .collect::<Vec<_>>()
                .join("\n")
                .to_lowercase();
            if text.contains(&expected) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "terminal did not open in the project directory: {text}"
            );
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
        terminal
            .close_timeout(std::time::Duration::from_secs(10))
            .unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn windows_shell_paths_handle_extended_drive_and_network_prefixes() {
        assert_eq!(
            windows_shell_path(Path::new(r"\\?\D:\project")).unwrap(),
            PathBuf::from(r"D:\project")
        );
        assert_eq!(
            windows_shell_path(Path::new(r"\\?\UNC\server\share\project")).unwrap(),
            PathBuf::from(r"\\server\share\project")
        );
    }

    const CTRL: Modifiers = Modifiers {
        shift: false,
        alt: false,
        ctrl: true,
    };
    const NONE: Modifiers = Modifiers {
        shift: false,
        alt: false,
        ctrl: false,
    };

    #[test]
    fn key_mapping_preserves_unicode_and_control_sequences() {
        assert_eq!(
            map_key("c", CTRL).and_then(|key| terminal_lab::encode_key(key, false)),
            Some(vec![3])
        );
        assert_eq!(
            map_key("Ж", NONE).and_then(|key| terminal_lab::encode_key(key, false)),
            Some("Ж".as_bytes().to_vec())
        );
        assert_eq!(
            map_key("up", NONE).and_then(|key| terminal_lab::encode_key(key, false)),
            Some(b"\x1b[A".to_vec())
        );
    }

    #[test]
    fn history_scrolling_is_bounded_and_reversible() {
        assert_eq!(next_scroll_offset(0, 3, 20), 3);
        assert_eq!(next_scroll_offset(18, 10, 20), 20);
        assert_eq!(next_scroll_offset(3, -8, 20), 0);
        assert_eq!(next_scroll_offset(50, 0, 20), 20);
    }

    #[test]
    fn panel_geometry_produces_valid_pty_sizes() {
        let size = terminal_size_for_bounds(800.0, 240.0, 8.0);
        assert_eq!(size.columns(), 97);
        assert_eq!(size.rows(), 12);
        assert_eq!(terminal_size_for_bounds(0.0, 0.0, 0.0).rows(), 2);
        assert!(terminal_size_for_bounds(f32::NAN, f32::INFINITY, 8.0).columns() >= 20);
    }

    #[test]
    fn ime_offsets_do_not_split_unicode() {
        let text = "aЖ🦀b";
        let range = utf16_range_to_bytes(text, 1..4);
        assert_eq!(&text[range.clone()], "Ж🦀");
        assert_eq!(byte_range_to_utf16(text, range), 1..4);
    }

    #[test]
    fn indexed_colors_cover_basic_cube_and_grayscale_ranges() {
        for index in 0..=255 {
            let _ = terminal_color(TerminalColor::Indexed(index), gpui::rgb(0xffffff).into());
        }
    }
}
