use std::{
    path::PathBuf,
    sync::{Arc, mpsc as std_mpsc},
};

use aircode_agent::{AgentAction, AgentExecutor, PermissionMode, tool_definitions};
use aircode_ai::{
    AiClient, ApiKey, AppSettings, ChatMessage, CredentialStore, MessageRole, ProviderConfig,
    ProviderKind, ReasoningEffort, Region, SettingsStore, StreamEvent, ToolCall,
};
use aircode_core::{NavigationPanel, ThemeMode, WorkspaceState};
use aircode_editor::Document;
use aircode_i18n::{Language, system_is_russian, tr};
use aircode_project::{Project, RecommendedWatcher};
use gpui::{
    App, ClipboardItem, Context, CursorStyle, Entity, FocusHandle, Focusable, FontWeight,
    KeyBinding, MouseButton, PathPromptOptions, Rgba, ScrollHandle, Window, WindowControlArea,
    actions, div, point, prelude::*, px,
};
use tokio::{runtime::Runtime, sync::mpsc};
use tokio_util::sync::CancellationToken;

use crate::{document::DocumentView, text_input::TextInput, theme::Theme};

const MAX_CONTEXT_BYTES: usize = 200_000;

actions!(
    aircode,
    [
        CloseOverlay,
        NewChat,
        OpenFile,
        SaveFile,
        SaveFileAs,
        SendMessage,
        StopGeneration,
        ToggleAi,
        ToggleFocus,
        TogglePalette,
        ToggleSettings,
        ToggleTheme,
        PaletteUp,
        PaletteDown,
        PaletteConfirm,
    ]
);

pub fn register_actions(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("escape", CloseOverlay, None),
        KeyBinding::new("secondary-o", OpenFile, None),
        KeyBinding::new("secondary-s", SaveFile, None),
        KeyBinding::new("secondary-shift-s", SaveFileAs, None),
        KeyBinding::new("ctrl-shift-s", SaveFileAs, None),
        KeyBinding::new("secondary-p", TogglePalette, None),
        KeyBinding::new("secondary-shift-i", ToggleAi, None),
        KeyBinding::new("secondary-shift-f", ToggleFocus, None),
        KeyBinding::new("secondary-shift-t", ToggleTheme, None),
        KeyBinding::new("secondary-shift-n", NewChat, None),
        KeyBinding::new("secondary-,", ToggleSettings, None),
        KeyBinding::new("ctrl-enter", SendMessage, None),
        KeyBinding::new("secondary-enter", SendMessage, None),
        KeyBinding::new("up", PaletteUp, Some("Palette")),
        KeyBinding::new("down", PaletteDown, Some("Palette")),
        KeyBinding::new("enter", PaletteConfirm, Some("Palette")),
    ]);
}

pub fn register_global_actions(workspace: Entity<Workspace>, cx: &mut App) {
    let view = workspace.clone();
    cx.on_action(move |_: &OpenFile, cx| {
        view.update(cx, |this, cx| this.open_file_dialog(cx));
    });
    let view = workspace.clone();
    cx.on_action(move |_: &SaveFile, cx| {
        view.update(cx, |this, cx| this.save_document(false, cx));
    });
    let view = workspace.clone();
    cx.on_action(move |_: &SaveFileAs, cx| {
        view.update(cx, |this, cx| this.save_document(true, cx));
    });
    let view = workspace.clone();
    cx.on_action(move |_: &SendMessage, cx| {
        view.update(cx, |this, cx| this.send(cx));
    });
    let view = workspace.clone();
    cx.on_action(move |_: &StopGeneration, cx| {
        view.update(cx, |this, cx| {
            if let Some(cancel) = &this.cancel {
                cancel.cancel();
            }
            cx.notify();
        });
    });
    let view = workspace.clone();
    cx.on_action(move |_: &NewChat, cx| {
        view.update(cx, |this, cx| this.new_chat(cx));
    });
    let view = workspace.clone();
    cx.on_action(move |_: &TogglePalette, cx| {
        view.update(cx, |this, cx| this.toggle_palette(cx));
    });
    let view = workspace.clone();
    cx.on_action(move |_: &CloseOverlay, cx| {
        view.update(cx, |this, cx| this.close_overlays(cx));
    });
    let view = workspace.clone();
    cx.on_action(move |_: &ToggleAi, cx| {
        view.update(cx, |this, cx| {
            this.state.toggle_ai();
            cx.notify();
        });
    });
    let view = workspace.clone();
    cx.on_action(move |_: &ToggleFocus, cx| {
        view.update(cx, |this, cx| {
            this.state.toggle_focus_mode();
            cx.notify();
        });
    });
    let view = workspace.clone();
    cx.on_action(move |_: &ToggleTheme, cx| {
        view.update(cx, |this, cx| {
            this.state.toggle_theme();
            cx.notify();
        });
    });
    cx.on_action(move |_: &ToggleSettings, cx| {
        workspace.update(cx, |this, cx| this.toggle_settings(cx));
    });
}

#[derive(Clone)]
struct ChatEntry {
    role: MessageRole,
    content: String,
    reasoning: String,
    show_reasoning: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Dropdown {
    Provider,
    Region,
    Model,
    Effort,
    /// Model picker opened from the chat header badge.
    ChatModel,
}

#[derive(Clone, Copy)]
enum FocusTarget {
    Document,
    Palette,
    Settings,
}

/// (translation key of the label, shortcut, command)
const PALETTE_COMMANDS: [(&str, &str, PaletteCommand); 7] = [
    ("palette_open", "Ctrl O", PaletteCommand::Open),
    ("palette_save", "Ctrl S", PaletteCommand::Save),
    ("palette_new_chat", "Ctrl Shift N", PaletteCommand::NewChat),
    ("palette_settings", "Ctrl ,", PaletteCommand::Settings),
    ("palette_focus", "Ctrl Shift F", PaletteCommand::Focus),
    ("palette_ai", "Ctrl Shift I", PaletteCommand::Ai),
    ("palette_theme", "Ctrl Shift T", PaletteCommand::Theme),
];

/// Monospace face that actually exists on the platform (gpui has no generic "monospace").
fn editor_font() -> &'static str {
    if cfg!(target_os = "windows") {
        "Consolas"
    } else if cfg!(target_os = "macos") {
        "Menlo"
    } else {
        "DejaVu Sans Mono"
    }
}

#[derive(Clone, Copy)]
enum PaletteCommand {
    Open,
    Save,
    NewChat,
    Settings,
    Focus,
    Ai,
    Theme,
}

#[derive(Clone, Copy)]
enum ResizeTarget {
    Left,
    Right,
}

enum PendingDiscard {
    OpenDialog,
    OpenPath(PathBuf),
}

#[cfg(windows)]
fn start_window_drag(window: &Window) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    unsafe extern "system" {
        fn ReleaseCapture() -> i32;
        fn SendMessageW(
            window: *mut std::ffi::c_void,
            message: u32,
            wparam: usize,
            lparam: isize,
        ) -> isize;
    }

    if let Ok(handle) = HasWindowHandle::window_handle(window)
        && let RawWindowHandle::Win32(handle) = handle.as_raw()
    {
        unsafe {
            ReleaseCapture();
            SendMessageW(handle.hwnd.get() as *mut std::ffi::c_void, 0x00a1, 2, 0);
        }
    }
}

#[cfg(not(windows))]
fn start_window_drag(_: &Window) {}

#[cfg(windows)]
fn toggle_maximize(window: &Window) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    unsafe extern "system" {
        fn PostMessageW(
            window: *mut std::ffi::c_void,
            message: u32,
            wparam: usize,
            lparam: isize,
        ) -> i32;
    }

    if let Ok(handle) = HasWindowHandle::window_handle(window)
        && let RawWindowHandle::Win32(handle) = handle.as_raw()
    {
        let command = if window.is_maximized() {
            0xf120
        } else {
            0xf030
        };
        unsafe {
            PostMessageW(
                handle.hwnd.get() as *mut std::ffi::c_void,
                0x0112,
                command,
                0,
            );
        }
    }
}

#[cfg(not(windows))]
fn toggle_maximize(window: &Window) {
    window.zoom_window();
}

pub struct Workspace {
    state: WorkspaceState,
    language: Language,
    offer_russian: bool,
    dialog_open: bool,
    pending_discard: Option<PendingDiscard>,
    close_confirmation_open: bool,
    close_confirmed: bool,
    focus_handle: FocusHandle,
    resizing: Option<ResizeTarget>,
    window_origin_x: f32,
    window_width: f32,
    document: Entity<DocumentView>,
    document_generation: u64,
    project: Option<Project>,
    project_watcher: Option<RecommendedWatcher>,
    project_events: Option<std_mpsc::Receiver<()>>,
    chat_input: Entity<TextInput>,
    api_key_input: Entity<TextInput>,
    base_url_input: Entity<TextInput>,
    model_input: Entity<TextInput>,
    palette_input: Entity<TextInput>,
    palette_selected: usize,
    pending_focus: Option<FocusTarget>,
    chat_scroll: ScrollHandle,
    settings: AppSettings,
    settings_store: Option<SettingsStore>,
    permission: PermissionMode,
    agent_messages: Vec<ChatMessage>,
    current_config: ProviderConfig,
    pending_tool: Option<(ToolCall, Option<String>)>,
    tool_running: bool,
    agent_steps: u8,
    audit: Vec<String>,
    settings_open: bool,
    dropdown: Option<Dropdown>,
    key_saved: bool,
    status: String,
    chat: Vec<ChatEntry>,
    streaming: bool,
    cancel: Option<CancellationToken>,
    generation: u64,
    runtime: Arc<Runtime>,
    client: Arc<AiClient>,
}

impl Workspace {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let settings_store = SettingsStore::new().ok();
        let mut status = String::new();
        let settings = settings_store
            .as_ref()
            .map(|store| store.load())
            .unwrap_or_else(|| Ok(AppSettings::default()))
            .unwrap_or_else(|error| {
                status = error.to_string();
                AppSettings::default()
            });
        let language = Language::from_code(&settings.language);
        let offer_russian = language == Language::English
            && !settings.locale_offer_dismissed
            && system_is_russian();
        let key_saved = match CredentialStore::load(&settings.provider.credential_id()) {
            Ok(key) => key.is_some(),
            Err(error) => {
                status = error.to_string();
                false
            }
        };
        let document = cx.new(DocumentView::new);
        document.update(cx, |view, _| view.set_language(language));
        cx.observe(&document, |_, _, cx| cx.notify()).detach();
        let chat_input = cx.new(|cx| TextInput::new(tr(language, "chat_placeholder"), cx));
        chat_input.update(cx, |input, _| input.set_language(language));
        let api_key_input = cx.new(|cx| {
            let mut input = TextInput::new(tr(language, "api_key_placeholder"), cx);
            input.set_masked(true);
            input
        });
        api_key_input.update(cx, |input, _| input.set_language(language));
        let base_url_input = cx.new(|cx| TextInput::new("https://server/v1", cx));
        base_url_input.update(cx, |input, _| input.set_language(language));
        let model_input = cx.new(|cx| TextInput::new(tr(language, "model_id"), cx));
        model_input.update(cx, |input, _| input.set_language(language));
        let palette_input = cx.new(|cx| TextInput::new(tr(language, "palette_filter"), cx));
        palette_input.update(cx, |input, _| input.set_language(language));
        base_url_input.update(cx, |input, cx| {
            input.set_text(settings.provider.base_url.clone(), cx);
        });
        model_input.update(cx, |input, cx| {
            input.set_text(settings.provider.model.clone(), cx);
        });
        cx.observe(&model_input, |this, input, cx| {
            let text = input.read(cx).text().trim().to_string();
            if text != this.settings.provider.model {
                let previous = this.settings.provider.clone();
                this.settings.provider.model = text;
                this.settings_changed(&previous, cx);
            }
        })
        .detach();
        cx.observe(&base_url_input, |this, input, cx| {
            let text = input.read(cx).text().trim().to_string();
            if text != this.settings.provider.base_url {
                let previous = this.settings.provider.clone();
                this.settings.provider.base_url = text;
                this.reload_key_state();
                this.settings_changed(&previous, cx);
            }
        })
        .detach();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("failed to start async runtime");
        let client = Arc::new(AiClient::new().expect("failed to initialize HTTP client"));
        Self {
            state: WorkspaceState::default(),
            language,
            offer_russian,
            dialog_open: false,
            pending_discard: None,
            close_confirmation_open: false,
            close_confirmed: false,
            focus_handle: cx.focus_handle(),
            resizing: None,
            window_origin_x: 0.0,
            window_width: 1440.0,
            document,
            document_generation: 0,
            project: None,
            project_watcher: None,
            project_events: None,
            chat_input,
            api_key_input,
            base_url_input,
            model_input,
            permission: PermissionMode::from_code(&settings.permission_mode),
            agent_messages: Vec::new(),
            current_config: settings.provider.clone(),
            pending_tool: None,
            tool_running: false,
            agent_steps: 0,
            audit: Vec::new(),
            palette_input,
            palette_selected: 0,
            pending_focus: None,
            chat_scroll: ScrollHandle::new(),
            settings,
            settings_store,
            settings_open: false,
            dropdown: None,
            key_saved,
            status,
            chat: Vec::new(),
            streaming: false,
            cancel: None,
            generation: 0,
            runtime: Arc::new(runtime),
            client,
        }
    }

    fn overlay_open(&self) -> bool {
        self.settings_open
            || self.state.palette_open
            || self.pending_discard.is_some()
            || self.close_confirmation_open
    }

    fn toggle_palette(&mut self, cx: &mut Context<Self>) {
        if self.state.palette_open {
            self.close_overlays(cx);
            return;
        }
        self.settings_open = false;
        self.state.toggle_palette();
        self.palette_selected = 0;
        self.palette_input.update(cx, |input, cx| input.clear(cx));
        self.pending_focus = Some(FocusTarget::Palette);
        cx.notify();
    }

    fn toggle_settings(&mut self, cx: &mut Context<Self>) {
        if self.settings_open {
            self.close_overlays(cx);
        } else {
            self.open_settings(cx);
        }
    }

    fn open_settings(&mut self, cx: &mut Context<Self>) {
        self.state.close_palette();
        self.settings_open = true;
        self.pending_focus = Some(FocusTarget::Settings);
        cx.notify();
    }

    fn close_overlays(&mut self, cx: &mut Context<Self>) {
        self.state.close_palette();
        self.settings_open = false;
        self.pending_discard = None;
        self.close_confirmation_open = false;
        self.dropdown = None;
        self.pending_focus = Some(FocusTarget::Document);
        cx.notify();
    }

    fn settings_changed(&mut self, previous: &ProviderConfig, cx: &mut Context<Self>) {
        if !self.settings.provider.session_compatible_with(previous) {
            self.new_chat(cx);
        }
        self.persist_settings();
        cx.notify();
    }

    fn filtered_commands(&self, cx: &App) -> Vec<usize> {
        let query = self.palette_input.read(cx).text().trim().to_lowercase();
        PALETTE_COMMANDS
            .iter()
            .enumerate()
            .filter(|(_, (label_key, _, _))| self.tr(label_key).to_lowercase().contains(&query))
            .map(|(index, _)| index)
            .collect()
    }

    fn move_palette_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let count = self.filtered_commands(cx).len();
        if count > 0 {
            self.palette_selected =
                (self.palette_selected as isize + delta).rem_euclid(count as isize) as usize;
        }
        cx.notify();
    }

    fn confirm_palette(&mut self, cx: &mut Context<Self>) {
        let matches = self.filtered_commands(cx);
        if let Some(index) = matches.get(self.palette_selected.min(matches.len().saturating_sub(1)))
        {
            self.execute_palette_command(PALETTE_COMMANDS[*index].2, cx);
        }
    }

    fn open_file_dialog(&mut self, cx: &mut Context<Self>) {
        if self.dialog_open {
            return;
        }
        if self.document.read(cx).document().is_modified() {
            self.pending_discard = Some(PendingDiscard::OpenDialog);
            cx.notify();
            return;
        }
        self.show_open_file_dialog(cx);
    }

    fn show_open_file_dialog(&mut self, cx: &mut Context<Self>) {
        if self.dialog_open {
            return;
        }
        self.dialog_open = true;
        cx.notify();
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(self.tr("open").into()),
        });
        cx.spawn(async move |this, cx| {
            let result = receiver.await;
            let _ = this.update(cx, |this, cx| {
                this.dialog_open = false;
                match result {
                    Ok(Ok(Some(paths))) => {
                        if let Some(path) = paths.into_iter().next() {
                            this.open_document_path(path, cx);
                        }
                    }
                    Ok(Err(error)) => this.status = error.to_string(),
                    _ => {}
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn open_document_path(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.document_generation += 1;
        let generation = self.document_generation;
        let display_path = path.clone();
        let task = cx
            .background_executor()
            .spawn(async move { Document::open(&path) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if generation != this.document_generation {
                    return;
                }
                match result {
                    Ok(document) => {
                        this.document
                            .update(cx, |view, cx| view.set_document(document, cx));
                        this.status = format!("Opened {}", display_path.display());
                    }
                    Err(error) => this.status = error.to_string(),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn open_project_dialog(&mut self, cx: &mut Context<Self>) {
        if self.dialog_open {
            return;
        }
        self.dialog_open = true;
        cx.notify();
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(self.tr("open_folder").into()),
        });
        cx.spawn(async move |this, cx| {
            let result = receiver.await;
            let _ = this.update(cx, |this, cx| {
                this.dialog_open = false;
                match result {
                    Ok(Ok(Some(paths))) => {
                        if let Some(path) = paths.into_iter().next() {
                            this.open_project_path(path, cx);
                        }
                    }
                    Ok(Err(error)) => this.status = error.to_string(),
                    _ => {}
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn open_project_path(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        match Project::open(&path) {
            Ok(project) => {
                self.project = Some(project);
                self.project_watcher = None;
                self.project_events = None;
                if let Some(project) = &self.project {
                    match project.watch() {
                        Ok((watcher, receiver)) => {
                            self.project_watcher = Some(watcher);
                            self.project_events = Some(receiver);
                            self.watch_project_events(cx);
                        }
                        Err(error) => self.status = error.to_string(),
                    }
                }
                self.status = format!("Project opened: {}", path.display());
                if self.state.navigation() != Some(NavigationPanel::Project) {
                    self.state.toggle_navigation(NavigationPanel::Project);
                }
            }
            Err(error) => self.status = error.to_string(),
        }
        cx.notify();
    }

    fn watch_project_events(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(1))
                    .await;
                let active = this
                    .update(cx, |this, cx| this.refresh_project_events(cx))
                    .unwrap_or(false);
                if !active {
                    break;
                }
            }
        })
        .detach();
    }

    fn refresh_project_events(&mut self, cx: &mut Context<Self>) -> bool {
        let mut changed = false;
        if let Some(receiver) = &self.project_events {
            while receiver.try_recv().is_ok() {
                changed = true;
            }
        }
        if changed && !self.dialog_open {
            if let Some(project) = &mut self.project {
                project.refresh();
            }
            cx.notify();
        }
        self.project_events.is_some()
    }

    fn open_project_entry(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if path.is_dir() {
            if let Some(project) = &mut self.project {
                project.toggle(&path);
            }
            cx.notify();
            return;
        }
        if self.document.read(cx).document().is_modified() {
            self.pending_discard = Some(PendingDiscard::OpenPath(path));
            cx.notify();
            return;
        }
        self.open_document_path(path, cx);
    }

    fn save_document(&mut self, force_dialog: bool, cx: &mut Context<Self>) {
        let document = self.document.read(cx).document();
        let path = document.path().map(|path| path.to_path_buf());
        if force_dialog || path.is_none() {
            if self.dialog_open {
                return;
            }
            let directory = path
                .as_deref()
                .and_then(|path| path.parent())
                .map(|path| path.to_path_buf())
                .or_else(|| {
                    self.project
                        .as_ref()
                        .map(|project| project.root().to_path_buf())
                })
                .or_else(|| std::env::current_dir().ok())
                .unwrap_or_else(|| PathBuf::from("."));
            let file_name = document.file_name().to_string();
            self.dialog_open = true;
            cx.notify();
            let receiver = cx.prompt_for_new_path(&directory, Some(&file_name));
            cx.spawn(async move |this, cx| {
                let result = receiver.await;
                let _ = this.update(cx, |this, cx| {
                    this.dialog_open = false;
                    match result {
                        Ok(Ok(Some(path))) => this.save_document_to(path, true, cx),
                        Ok(Err(error)) => this.status = error.to_string(),
                        _ => {}
                    }
                    cx.notify();
                });
            })
            .detach();
            return;
        }
        if let Some(path) = path {
            self.save_document_to(path, false, cx);
        }
    }

    fn save_document_to(&mut self, path: PathBuf, save_as: bool, cx: &mut Context<Self>) {
        let result = self.document.update(cx, |view, _| {
            let document = view.document_mut();
            if save_as {
                document.save_as(&path)
            } else {
                document.save()
            }
        });
        self.status = match result {
            Ok(()) => format!("Saved {}", path.display()),
            Err(error) => error.to_string(),
        };
        if let Some(project) = &mut self.project {
            project.refresh();
        }
        cx.notify();
    }

    pub fn should_close(&mut self, cx: &mut Context<Self>) -> bool {
        if self.close_confirmed || !self.document.read(cx).document().is_modified() {
            return true;
        }
        self.close_confirmation_open = true;
        cx.notify();
        false
    }

    fn request_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.should_close(cx) {
            window.remove_window();
        }
    }

    fn confirm_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_confirmation_open = false;
        self.close_confirmed = true;
        window.remove_window();
        cx.notify();
    }

    fn cancel_unsaved_action(&mut self, cx: &mut Context<Self>) {
        self.pending_discard = None;
        self.close_confirmation_open = false;
        cx.notify();
    }

    fn confirm_discard(&mut self, cx: &mut Context<Self>) {
        match self.pending_discard.take() {
            Some(PendingDiscard::OpenDialog) => self.show_open_file_dialog(cx),
            Some(PendingDiscard::OpenPath(path)) => self.open_document_path(path, cx),
            None => {}
        }
        cx.notify();
    }

    fn new_chat(&mut self, cx: &mut Context<Self>) {
        if let Some(cancel) = self.cancel.take() {
            cancel.cancel();
        }
        self.generation += 1;
        self.streaming = false;
        self.tool_running = false;
        self.chat.clear();
        self.pending_tool = None;
        self.agent_messages.clear();
        self.agent_steps = 0;
        self.audit.clear();
        self.status = self.tr("new_session").into();
        cx.notify();
    }

    fn stop_generation(&mut self, _: &StopGeneration, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(cancel) = &self.cancel {
            cancel.cancel();
            self.status = self.tr("cancelling").into();
        }
        cx.notify();
    }

    fn sync_config_from_inputs(&mut self, cx: &mut Context<Self>) {
        let base_url = self.base_url_input.read(cx).text().trim().to_string();
        let model = self.model_input.read(cx).text().trim().to_string();
        let previous = self.settings.provider.clone();
        self.settings.provider.base_url = base_url;
        self.settings.provider.model = model;
        if !self.settings.provider.session_compatible_with(&previous) {
            self.new_chat(cx);
        }
        self.persist_settings();
    }

    fn persist_settings(&mut self) {
        if let Some(store) = &self.settings_store
            && let Err(error) = store.save(&self.settings)
        {
            self.status = error.to_string();
        }
    }

    pub fn tr(&self, key: &'static str) -> &'static str {
        tr(self.language, key)
    }

    fn set_language(&mut self, language: Language, cx: &mut Context<Self>) {
        self.language = language;
        self.offer_russian = false;
        self.settings.language = language.code().into();
        self.settings.locale_offer_dismissed = true;
        self.document
            .update(cx, |view, _| view.set_language(language));
        self.chat_input
            .update(cx, |input, _| input.set_language(language));
        self.api_key_input
            .update(cx, |input, _| input.set_language(language));
        self.base_url_input
            .update(cx, |input, _| input.set_language(language));
        self.model_input
            .update(cx, |input, _| input.set_language(language));
        self.persist_settings();
        cx.notify();
    }

    fn dismiss_language_offer(&mut self, cx: &mut Context<Self>) {
        self.offer_russian = false;
        self.settings.locale_offer_dismissed = true;
        self.persist_settings();
        cx.notify();
    }

    fn send(&mut self, cx: &mut Context<Self>) {
        if self.streaming {
            return;
        }
        self.sync_config_from_inputs(cx);
        let message = self.chat_input.read(cx).text().trim().to_string();
        if message.is_empty() {
            return;
        }
        let key = match CredentialStore::load(&self.settings.provider.credential_id()) {
            Ok(Some(key)) => key,
            Ok(None) => {
                self.open_settings(cx);
                self.status = "Add an API key for the selected provider".into();
                cx.notify();
                return;
            }
            Err(error) => {
                self.status = error.to_string();
                cx.notify();
                return;
            }
        };
        let mut request_messages: Vec<ChatMessage> = self
            .chat
            .iter()
            .filter(|entry| !(entry.role == MessageRole::Assistant && entry.content.is_empty()))
            .map(|entry| ChatMessage {
                role: entry.role,
                content: entry.content.clone(),
            })
            .collect();
        let document = self.document.read(cx).document();
        let mut outbound = message.clone();
        if self.settings.include_document && !document.text().trim().is_empty() {
            let source = if document.selection().is_empty() {
                document.text()
            } else {
                document.selected_text()
            };
            let mut end = source.len().min(MAX_CONTEXT_BYTES);
            while !source.is_char_boundary(end) {
                end -= 1;
            }
            let truncated = end < source.len();
            outbound = format!(
                "{message}\n\n<context file=\"{}\" truncated=\"{}\">\n{}\n</context>",
                document.file_name(),
                truncated,
                &source[..end]
            );
        }
        request_messages.push(ChatMessage::user(outbound));
        self.chat.push(ChatEntry {
            role: MessageRole::User,
            content: message,
            reasoning: String::new(),
            show_reasoning: false,
        });
        self.chat.push(ChatEntry {
            role: MessageRole::Assistant,
            content: String::new(),
            reasoning: String::new(),
            show_reasoning: false,
        });
        self.chat_input.update(cx, |input, cx| input.clear(cx));
        self.generation += 1;
        let generation = self.generation;
        let (sender, mut receiver) = mpsc::unbounded_channel();
        let cancel = CancellationToken::new();
        self.cancel = Some(cancel.clone());
        self.streaming = true;
        self.status = format!("Connecting to {}...", self.settings.provider.kind.label());
        self.chat_scroll.scroll_to_bottom();
        let config = self.settings.provider.clone();
        self.agent_messages = request_messages.clone();
        self.current_config = config.clone();
        let tools = self.project.is_some().then(tool_definitions);
        let client = self.client.clone();
        let worker_sender = sender.clone();
        self.runtime.spawn(async move {
            if let Err(error) = client
                .stream_chat_with_tools(&config, &key, &request_messages, tools, cancel, sender)
                .await
            {
                let _ = worker_sender.send(StreamEvent::Error(error.to_string()));
            }
        });
        cx.spawn(async move |this, cx| {
            while let Some(event) = receiver.recv().await {
                if this
                    .update(cx, |this, cx| {
                        this.handle_stream_event(generation, event, cx)
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        cx.notify();
    }

    fn handle_stream_event(&mut self, generation: u64, event: StreamEvent, cx: &mut Context<Self>) {
        if generation != self.generation {
            return;
        }
        match event {
            StreamEvent::ReasoningDelta(delta) => {
                if let Some(entry) = self.chat.last_mut() {
                    entry.reasoning.push_str(&delta);
                }
                self.chat_scroll.scroll_to_bottom();
            }
            StreamEvent::ContentDelta(delta) => {
                if let Some(entry) = self.chat.last_mut() {
                    entry.content.push_str(&delta);
                }
                self.status = "Receiving response...".into();
                self.chat_scroll.scroll_to_bottom();
            }
            StreamEvent::ToolCall(call) => {
                let diff = self.tool_diff(&call);
                let action = AgentAction {
                    name: call.name.clone(),
                    arguments: call.arguments.clone(),
                };
                if self.permission.requires_approval(&action) {
                    self.pending_tool = Some((call, diff));
                    self.streaming = false;
                    self.cancel = None;
                    self.status = self.tr("approval_required").into();
                } else {
                    self.execute_agent_action(call, cx);
                }
            }
            StreamEvent::ToolResult(result) => {
                self.tool_running = false;
                self.audit.push(result.clone());
                if self.audit.len() > 50 {
                    self.audit.remove(0);
                }
                self.start_followup(result, cx);
            }
            StreamEvent::Usage(usage) => {
                if let Some(tokens) = usage.get("total_tokens").and_then(|value| value.as_u64()) {
                    self.status = format!("{tokens} tokens");
                }
            }
            StreamEvent::Error(error) => {
                self.streaming = false;
                self.tool_running = false;
                self.cancel = None;
                // Nothing was produced: drop the empty reply and give the question back,
                // so the history never contains a dangling "Thinking..." entry.
                if self.chat.last().is_some_and(|entry| {
                    entry.role == MessageRole::Assistant && entry.content.is_empty()
                }) {
                    self.chat.pop();
                    if self
                        .chat
                        .last()
                        .is_some_and(|entry| entry.role == MessageRole::User)
                    {
                        let question = self.chat.pop().map(|entry| entry.content);
                        if let Some(question) = question {
                            self.chat_input.update(cx, |input, cx| {
                                if input.text().is_empty() {
                                    input.set_text(question, cx);
                                }
                            });
                        }
                    }
                }
                self.status = error;
            }
            StreamEvent::Done => {
                if self.tool_running {
                    cx.notify();
                    return;
                }
                self.streaming = false;
                self.cancel = None;
                if self.pending_tool.is_some() {
                    self.status = self.tr("approval_required").into();
                } else {
                    if self.status.starts_with("Receiving") || self.status.starts_with("Connecting")
                    {
                        self.status = self.tr("ready").into();
                    }
                    if let Some(entry) = self.chat.last()
                        && !entry.content.is_empty()
                    {
                        self.agent_messages
                            .push(ChatMessage::assistant(entry.content.clone()));
                    }
                }
            }
        }
        cx.notify();
    }

    fn tool_diff(&self, call: &ToolCall) -> Option<String> {
        if call.name != "write_file" {
            return None;
        }
        let root = self.project.as_ref()?.root();
        let executor = AgentExecutor::new(root).ok()?;
        let arguments: serde_json::Value = serde_json::from_str(&call.arguments).ok()?;
        let path = arguments.get("path")?.as_str()?;
        let content = arguments.get("content")?.as_str()?;
        executor.diff(path, content).ok()
    }

    fn execute_agent_action(&mut self, call: ToolCall, cx: &mut Context<Self>) {
        let Some(root) = self
            .project
            .as_ref()
            .map(|project| project.root().to_path_buf())
        else {
            self.status = self.tr("agent_project_required").into();
            self.streaming = false;
            self.cancel = None;
            return;
        };
        let Ok(executor) = AgentExecutor::new(root) else {
            self.status = self.tr("sandbox_error").into();
            self.streaming = false;
            self.cancel = None;
            return;
        };
        if self.agent_steps >= 8 {
            self.streaming = false;
            self.cancel = None;
            self.status = self.tr("agent_step_limit").into();
            return;
        }
        self.agent_steps += 1;
        let action = AgentAction {
            name: call.name.clone(),
            arguments: call.arguments.clone(),
        };
        self.audit.push(format!("{} {}", call.name, call.arguments));
        self.status = format!("Running {}...", call.name);
        self.streaming = true;
        self.tool_running = true;
        let generation = self.generation;
        let cancel = self
            .cancel
            .get_or_insert_with(CancellationToken::new)
            .clone();
        let (sender, mut receiver) = mpsc::unbounded_channel();
        let worker_sender = sender.clone();
        self.runtime.spawn(async move {
            let result = executor.execute_with_cancel(&action, &cancel).await;
            let text = match result {
                Ok(output) => output,
                Err(error) => format!("Tool error: {error}"),
            };
            let _ = worker_sender.send(StreamEvent::ToolResult(text));
        });
        cx.spawn(async move |this, cx| {
            while let Some(event) = receiver.recv().await {
                if this
                    .update(cx, |this, cx| {
                        this.handle_stream_event(generation, event, cx)
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    fn start_followup(&mut self, result: String, cx: &mut Context<Self>) {
        self.pending_tool = None;
        if self
            .cancel
            .as_ref()
            .is_some_and(CancellationToken::is_cancelled)
        {
            self.streaming = false;
            self.cancel = None;
            self.status = self.tr("cancelled").into();
            return;
        }
        let key = match CredentialStore::load(&self.current_config.credential_id()) {
            Ok(Some(key)) => key,
            _ => {
                self.streaming = false;
                self.status = "API key is not available".into();
                return;
            }
        };
        self.agent_messages.push(ChatMessage::user(format!(
            "Tool result. Return the final answer to the user or use another tool if needed:\n{result}"
        )));
        self.chat.push(ChatEntry {
            role: MessageRole::Assistant,
            content: String::new(),
            reasoning: String::new(),
            show_reasoning: false,
        });
        self.generation += 1;
        let generation = self.generation;
        let (sender, mut receiver) = mpsc::unbounded_channel();
        let cancel = CancellationToken::new();
        self.cancel = Some(cancel.clone());
        self.streaming = true;
        self.status = "Waiting for the model...".into();
        let config = self.current_config.clone();
        let messages = self.agent_messages.clone();
        let tools = self.project.is_some().then(tool_definitions);
        let client = self.client.clone();
        let worker_sender = sender.clone();
        self.runtime.spawn(async move {
            if let Err(error) = client
                .stream_chat_with_tools(&config, &key, &messages, tools, cancel, sender)
                .await
            {
                let _ = worker_sender.send(StreamEvent::Error(error.to_string()));
            }
        });
        cx.spawn(async move |this, cx| {
            while let Some(event) = receiver.recv().await {
                if this
                    .update(cx, |this, cx| {
                        this.handle_stream_event(generation, event, cx)
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    fn approve_pending_tool(&mut self, cx: &mut Context<Self>) {
        let Some((call, _)) = self.pending_tool.take() else {
            return;
        };
        self.execute_agent_action(call, cx);
    }

    fn deny_pending_tool(&mut self, cx: &mut Context<Self>) {
        if self.pending_tool.take().is_some() {
            self.audit.push("Action denied by user".into());
            self.start_followup("Action denied by user".into(), cx);
        }
    }

    fn permission_label(&self) -> &'static str {
        match self.permission {
            PermissionMode::AskEvery => self.tr("ask_all"),
            PermissionMode::AskImportant => self.tr("ask_important"),
            PermissionMode::FullAccess => self.tr("full_access"),
        }
    }

    fn toggle_dropdown(&mut self, kind: Dropdown, cx: &mut Context<Self>) {
        self.dropdown = if self.dropdown == Some(kind) {
            None
        } else {
            Some(kind)
        };
        cx.notify();
    }

    /// Applies the option at `index` of the given dropdown and closes it.
    fn apply_dropdown(&mut self, kind: Dropdown, index: usize, cx: &mut Context<Self>) {
        self.dropdown = None;
        let previous = self.settings.provider.clone();
        match kind {
            Dropdown::Provider => {
                let Some(new_kind) = ProviderKind::ALL.get(index).copied() else {
                    return;
                };
                if new_kind != previous.kind {
                    self.settings.provider =
                        ProviderConfig::new(new_kind).with_region(previous.region);
                }
            }
            Dropdown::Region => {
                let region = if index == 0 {
                    Region::China
                } else {
                    Region::Overseas
                };
                self.settings.provider = previous.clone().with_region(region);
            }
            Dropdown::Model | Dropdown::ChatModel => {
                let Some(model) = previous.model_options().get(index) else {
                    return;
                };
                self.settings.provider.model = (*model).into();
            }
            Dropdown::Effort => {
                let Some(effort) = ReasoningEffort::ALL.get(index).copied() else {
                    return;
                };
                self.settings.provider.effort = effort;
            }
        }
        if self.settings.provider != previous {
            self.update_config_inputs(cx);
            self.reload_key_state();
            self.new_chat(cx);
            self.persist_settings();
        }
        cx.notify();
    }

    fn update_config_inputs(&self, cx: &mut Context<Self>) {
        self.base_url_input.update(cx, |input, cx| {
            input.set_text(self.settings.provider.base_url.clone(), cx);
        });
        self.model_input.update(cx, |input, cx| {
            input.set_text(self.settings.provider.model.clone(), cx);
        });
    }

    fn reload_key_state(&mut self) {
        self.key_saved = match CredentialStore::load(&self.settings.provider.credential_id()) {
            Ok(key) => key.is_some(),
            Err(error) => {
                self.status = error.to_string();
                false
            }
        };
    }

    fn save_api_key(&mut self, cx: &mut Context<Self>) {
        self.sync_config_from_inputs(cx);
        let value = self.api_key_input.read(cx).text().to_string();
        match ApiKey::new(value)
            .and_then(|key| CredentialStore::save(&self.settings.provider.credential_id(), &key))
        {
            Ok(()) => {
                self.key_saved = true;
                self.status = "API key saved in the system credential store".into();
                self.api_key_input.update(cx, |input, cx| input.clear(cx));
            }
            Err(error) => self.status = error.to_string(),
        }
        cx.notify();
    }

    fn delete_api_key(&mut self, cx: &mut Context<Self>) {
        match CredentialStore::delete(&self.settings.provider.credential_id()) {
            Ok(()) => {
                self.key_saved = false;
                self.status = "API key deleted".into();
            }
            Err(error) => self.status = error.to_string(),
        }
        cx.notify();
    }

    fn window_chrome(
        &self,
        _window: &mut Window,
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .w_full()
            .h(px(34.0))
            .border_b_1()
            .border_color(theme.border)
            .bg(theme.surface)
            .child(
                div()
                    .id("window-drag-area")
                    .flex()
                    .flex_1()
                    .items_center()
                    .gap_2()
                    .min_w(px(0.0))
                    .h_full()
                    .pl_3()
                    .when(!cfg!(windows), |this| {
                        this.window_control_area(WindowControlArea::Drag)
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|_, _, window, _| start_window_drag(window)),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(px(18.0))
                            .rounded(px(4.0))
                            .bg(theme.accent)
                            .text_color(gpui::white())
                            .text_xs()
                            .child("A"),
                    )
                    .child(div().text_xs().text_color(theme.muted).child("AIRcode")),
            )
            .child(
                div()
                    .id("window-minimize")
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .w(px(42.0))
                    .h_full()
                    .text_color(theme.muted)
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.hover).text_color(theme.text))
                    .on_click(cx.listener(|_, _, window, cx| {
                        window.minimize_window();
                        cx.notify();
                    }))
                    .child("–"),
            )
            .child(
                div()
                    .id("window-maximize")
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .w(px(42.0))
                    .h_full()
                    .text_color(theme.muted)
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.hover).text_color(theme.text))
                    .on_click(cx.listener(|_, _, window, cx| {
                        toggle_maximize(window);
                        window.refresh();
                        cx.notify();
                    }))
                    .child("□"),
            )
            .child(
                div()
                    .id("window-close")
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .w(px(42.0))
                    .h_full()
                    .text_color(theme.muted)
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.red).text_color(gpui::white()))
                    .on_click(cx.listener(|this, _, window, cx| this.request_close(window, cx)))
                    .child("×"),
            )
    }

    fn header(&self, theme: Theme, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .justify_between()
            .h(px(54.0))
            .px_4()
            .border_b_1()
            .border_color(theme.border)
            .bg(theme.surface)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(logo(theme))
                    .child(div().font_weight(FontWeight::SEMIBOLD).child("AIRcode"))
                    .child(div().text_xs().text_color(theme.faint).child("0.3.0"))
                    .child(div().w_4())
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.muted)
                            .child(self.document.read(cx).document().file_name().to_string()),
                    )
                    .when(self.document.read(cx).document().is_modified(), |this| {
                        this.child(div().text_color(theme.yellow).child(self.tr("modified")))
                    }),
            )
            .child(
                div()
                    .id("command-palette-button")
                    .flex()
                    .items_center()
                    .justify_between()
                    .w(px(360.0))
                    .h(px(32.0))
                    .px_3()
                    .rounded_md()
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.raised)
                    .text_sm()
                    .text_color(theme.muted)
                    .cursor_pointer()
                    .hover(|style| style.border_color(theme.accent))
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_palette(cx)))
                    .child(self.tr("commands"))
                    .child(keycap("Ctrl P", theme)),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        action_button(
                            self.tr("folder"),
                            "open-folder-button",
                            self.project.is_some(),
                            theme,
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.open_project_dialog(cx))),
                    )
                    .child(
                        action_button(self.tr("open"), "open-button", false, theme)
                            .on_click(cx.listener(|this, _, _, cx| this.open_file_dialog(cx))),
                    )
                    .child(
                        action_button(self.tr("save"), "save-button", false, theme)
                            .on_click(cx.listener(|this, _, _, cx| this.save_document(false, cx))),
                    )
                    .child(
                        action_button(
                            self.tr("theme"),
                            "theme-button",
                            self.state.theme == ThemeMode::Light,
                            theme,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.state.toggle_theme();
                            cx.notify();
                        })),
                    )
                    .child(
                        action_button(self.language.label(), "language-button", false, theme)
                            .on_click(cx.listener(|this, _, _, cx| {
                                let language = if this.language == Language::English {
                                    Language::Russian
                                } else {
                                    Language::English
                                };
                                this.set_language(language, cx);
                            })),
                    )
                    .child(
                        action_button("AI", "ai-button", self.state.ai_visible(), theme).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.state.toggle_ai();
                                cx.notify();
                            }),
                        ),
                    )
                    .child(
                        action_button(
                            self.tr("settings"),
                            "settings-button",
                            self.settings_open,
                            theme,
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_settings(cx))),
                    )
                    .child(
                        action_button(
                            self.tr("focus"),
                            "focus-button",
                            self.state.focus_mode,
                            theme,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.state.toggle_focus_mode();
                            cx.notify();
                        })),
                    ),
            )
    }

    fn activity_rail(&self, theme: Theme, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .items_center()
            .w(px(52.0))
            .py_3()
            .gap_2()
            .border_r_1()
            .border_color(theme.border)
            .bg(theme.surface)
            .child(
                action_button(
                    "PR",
                    "project-button",
                    self.state.navigation_visible(),
                    theme,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.state.toggle_navigation(NavigationPanel::Project);
                    cx.notify();
                })),
            )
            .child(
                action_button("AI", "rail-ai-button", self.state.ai_visible(), theme).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.state.toggle_ai();
                        cx.notify();
                    }),
                ),
            )
            .child(div().flex_1())
            .child(div().text_xs().text_color(theme.faint).child("0.3"))
    }

    fn resize_handle(
        &self,
        target: ResizeTarget,
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let id = match target {
            ResizeTarget::Left => "resize-left",
            ResizeTarget::Right => "resize-right",
        };
        div()
            .id(id)
            .w(px(6.0))
            .h_full()
            .cursor(CursorStyle::ResizeLeftRight)
            .bg(theme.window)
            .hover(|style| style.bg(theme.accent_soft))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.resizing = Some(target);
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &gpui::MouseMoveEvent, _, cx| {
                let x: f32 = event.position.x.into();
                match this.resizing {
                    Some(ResizeTarget::Left) => {
                        this.settings.left_width =
                            (x - this.window_origin_x - 52.0).clamp(160.0, 450.0);
                    }
                    Some(ResizeTarget::Right) => {
                        this.settings.right_width =
                            (this.window_origin_x + this.window_width - x).clamp(260.0, 650.0);
                    }
                    None => return,
                }
                cx.notify();
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.resizing = None;
                    this.persist_settings();
                    cx.notify();
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.resizing = None;
                    this.persist_settings();
                    cx.notify();
                }),
            )
    }

    fn navigation_panel(&mut self, theme: Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self
            .project
            .as_mut()
            .map(|project| project.visible_rows())
            .unwrap_or_default();
        let current_path = self
            .document
            .read(cx)
            .document()
            .path()
            .map(|path| path.to_path_buf());
        let document_name = self.document.read(cx).document().file_name().to_string();
        div()
            .flex()
            .flex_col()
            .w(px(self.settings.left_width))
            .border_r_1()
            .border_color(theme.border)
            .bg(theme.surface)
            .child(panel_title(self.tr("project"), self.tr("files"), theme))
            .when(self.project.is_none(), |this| {
                this.child(
                    div().p_3().child(
                        action_button(self.tr("open_folder"), "sidebar-open-folder", true, theme)
                            .on_click(cx.listener(|this, _, _, cx| this.open_project_dialog(cx))),
                    ),
                )
            })
            .child(
                div()
                    .id("project-tree-scroll")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .gap(px(2.0))
                    .overflow_y_scroll()
                    .p_2()
                    .children(rows.into_iter().map(|row| {
                        let path = row.entry.path.clone();
                        let active = current_path.as_ref() == Some(&path);
                        let is_dir = row.entry.is_dir;
                        let expanded = row.expanded;
                        let depth = row.depth;
                        let name = row.entry.name;
                        div()
                            .id(gpui::SharedString::from(
                                path.to_string_lossy().into_owned(),
                            ))
                            .flex()
                            .items_center()
                            .gap_2()
                            .h(px(27.0))
                            .pr_2()
                            .pl(px(6.0 + depth as f32 * 13.0))
                            .rounded_md()
                            .cursor_pointer()
                            .text_sm()
                            .when(active, |this| this.bg(theme.accent_soft))
                            .when(!active, |this| this.hover(|style| style.bg(theme.hover)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.open_project_entry(path.clone(), cx);
                            }))
                            .child(
                                div()
                                    .w(px(14.0))
                                    .text_xs()
                                    .text_color(if is_dir { theme.accent } else { theme.faint })
                                    .child(if is_dir {
                                        if expanded { "v" } else { ">" }
                                    } else {
                                        "·"
                                    }),
                            )
                            .child(
                                div()
                                    .text_color(if active { theme.accent } else { theme.muted })
                                    .child(name),
                            )
                            .into_any_element()
                    })),
            )
            .child(
                div().m_2().p_3().rounded_md().bg(theme.accent_soft).child(
                    div()
                        .text_sm()
                        .text_color(theme.accent)
                        .child(document_name),
                ),
            )
    }

    fn editor(&self, theme: Theme, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .bg(theme.window)
            .child(
                div()
                    .flex()
                    .items_center()
                    .h(px(40.0))
                    .px_4()
                    .gap_3()
                    .border_b_1()
                    .border_color(theme.border)
                    .bg(theme.surface)
                    .child(div().text_color(theme.blue).child("TXT"))
                    .child(
                        div()
                            .text_sm()
                            .child(self.document.read(cx).document().file_name().to_string()),
                    )
                    .when(self.document.read(cx).document().is_modified(), |this| {
                        this.child(div().text_xs().text_color(theme.yellow).child("unsaved"))
                    }),
            )
            .child(
                div()
                    .flex_1()
                    // Without this the flex item grows to the text height and never scrolls.
                    .min_h(px(0.0))
                    .relative()
                    .py_3()
                    .font_family(editor_font())
                    .text_sm()
                    .child(self.document.clone())
                    .child(document_scrollbar(
                        self.document.read(cx).scroll_handle(),
                        theme,
                    )),
            )
    }

    fn assistant_panel(&self, theme: Theme, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .relative()
            .flex()
            .flex_col()
            .w(px(self.settings.right_width))
            .border_l_1()
            .border_color(theme.border)
            .bg(theme.surface)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .h(px(48.0))
                    .px_4()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("AIR assistant"),
                    )
                    .child(
                        div()
                            .id("model-badge")
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| {
                                if this.settings.provider.model_options().is_empty() {
                                    this.open_settings(cx);
                                } else {
                                    this.toggle_dropdown(Dropdown::ChatModel, cx);
                                }
                            }))
                            .child(badge(
                                format!(
                                    "{}  {}",
                                    self.settings.provider.model,
                                    if self.dropdown == Some(Dropdown::ChatModel) {
                                        "\u{25B4}"
                                    } else {
                                        "\u{25BE}"
                                    }
                                ),
                                theme.accent_soft,
                                theme.accent,
                            )),
                    ),
            )
            .child(
                div()
                    .id("chat-scroll")
                    .track_scroll(&self.chat_scroll)
                    .flex()
                    .flex_col()
                    .flex_1()
                    .gap_4()
                    .p_4()
                    .overflow_y_scroll()
                    .when(self.chat.is_empty(), |this| {
                        this.child(empty_chat(theme, self.language))
                    })
                    .children({
                        let last = self.chat.len().saturating_sub(1);
                        let streaming = self.streaming;
                        self.chat
                            .iter()
                            .cloned()
                            .enumerate()
                            .map(|(index, entry)| {
                                let waiting = streaming && index == last;
                                chat_message(index, entry, waiting, theme, self.language, cx)
                                    .into_any_element()
                            })
                            .collect::<Vec<_>>()
                    }),
            )
            .when(!self.audit.is_empty(), |this| {
                this.child(self.audit_panel(theme))
            })
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .m_3()
                    .p_3()
                    .rounded_lg()
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.window)
                    .child(self.chat_input.clone())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(div().text_xs().text_color(theme.faint).child(
                                if self.settings.include_document {
                                    self.tr("context_on")
                                } else {
                                    self.tr("context_off")
                                },
                            ))
                            .child(div().flex_1())
                            .child(div().text_xs().text_color(theme.faint).child("Ctrl+Enter"))
                            .child(
                                action_button(
                                    if self.streaming {
                                        self.tr("stop")
                                    } else {
                                        self.tr("send")
                                    },
                                    "send-button",
                                    true,
                                    theme,
                                )
                                .on_click(cx.listener(
                                    |this, _, window, cx| {
                                        if this.streaming {
                                            this.stop_generation(&StopGeneration, window, cx);
                                        } else {
                                            this.send(cx);
                                        }
                                    },
                                )),
                            ),
                    ),
            )
            .when(self.dropdown == Some(Dropdown::ChatModel), |this| {
                let provider = &self.settings.provider;
                this.child(
                    div()
                        .absolute()
                        .top(px(52.0))
                        .right(px(12.0))
                        .w(px(230.0))
                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                            this.dropdown = None;
                            cx.notify();
                        }))
                        .child(dropdown_list(
                            Dropdown::ChatModel,
                            provider
                                .model_options()
                                .iter()
                                .map(|model| model.to_string())
                                .collect(),
                            provider
                                .model_options()
                                .iter()
                                .position(|model| *model == provider.model),
                            theme,
                            cx,
                        )),
                )
            })
    }

    fn backdrop(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("overlay-backdrop")
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .occlude()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.close_overlays(cx)),
            )
    }

    /// A settings row that expands into a list of options (instead of cycling on click).
    #[allow(clippy::too_many_arguments)]
    fn dropdown_row(
        &self,
        kind: Dropdown,
        label: &'static str,
        value: String,
        options: Vec<String>,
        selected: Option<usize>,
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let open = self.dropdown == Some(kind);
        let row = setting_row(
            label,
            format!("{value}  {}", if open { "\u{25B4}" } else { "\u{25BE}" }),
            theme,
        )
        .on_click(cx.listener(move |this, _, _, cx| this.toggle_dropdown(kind, cx)));
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(row)
            .when(open, |this| {
                this.child(dropdown_list(kind, options, selected, theme, cx))
            })
            .into_any_element()
    }

    fn settings_panel(&self, theme: Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let provider = &self.settings.provider;
        div()
            .id("settings-panel")
            .occlude()
            .absolute()
            .top(px(72.0))
            .left_0()
            .right_0()
            .mx_auto()
            .w(px(620.0))
            .rounded_xl()
            .border_1()
            .border_color(theme.border)
            .bg(theme.surface)
            .shadow_lg()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .p_4()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(self.tr("settings_title")),
                    )
                    .child(
                        action_button(self.tr("close"), "close-settings", false, theme)
                            .on_click(cx.listener(|this, _, _, cx| this.close_overlays(cx))),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .p_4()
                    .child(
                        self.dropdown_row(
                            Dropdown::Provider,
                            self.tr("provider"),
                            provider.kind.label().to_string(),
                            ProviderKind::ALL
                                .iter()
                                .map(|kind| kind.label().to_string())
                                .collect(),
                            ProviderKind::ALL
                                .iter()
                                .position(|kind| *kind == provider.kind),
                            theme,
                            cx,
                        ),
                    )
                    .when(
                        matches!(
                            provider.kind,
                            ProviderKind::KimiCode | ProviderKind::KimiPlatform
                        ),
                        |this| {
                            this.child(
                                self.dropdown_row(
                                    Dropdown::Region,
                                    self.tr("region"),
                                    match provider.region {
                                        Region::China => "China",
                                        Region::Overseas => "Overseas",
                                    }
                                    .to_string(),
                                    vec!["China".to_string(), "Overseas".to_string()],
                                    Some(if provider.region == Region::China {
                                        0
                                    } else {
                                        1
                                    }),
                                    theme,
                                    cx,
                                ),
                            )
                        },
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.faint)
                            .child(provider.base_url.clone()),
                    )
                    .when(!provider.model_options().is_empty(), |this| {
                        this.child(
                            self.dropdown_row(
                                Dropdown::Model,
                                self.tr("model"),
                                provider.model.clone(),
                                provider
                                    .model_options()
                                    .iter()
                                    .map(|model| model.to_string())
                                    .collect(),
                                provider
                                    .model_options()
                                    .iter()
                                    .position(|model| *model == provider.model),
                                theme,
                                cx,
                            ),
                        )
                    })
                    .when(provider.model_options().is_empty(), |this| {
                        this.child(field_label(self.tr("model_id"), theme))
                            .child(input_container(theme).child(self.model_input.clone()))
                    })
                    .when(provider.kind == ProviderKind::OpenAiCompatible, |this| {
                        this.child(field_label(self.tr("base_url"), theme))
                            .child(input_container(theme).child(self.base_url_input.clone()))
                    })
                    .when(provider.supports_effort(), |this| {
                        this.child(
                            self.dropdown_row(
                                Dropdown::Effort,
                                self.tr("effort"),
                                provider.effort.label().to_string(),
                                ReasoningEffort::ALL
                                    .iter()
                                    .map(|effort| effort.label().to_string())
                                    .collect(),
                                ReasoningEffort::ALL
                                    .iter()
                                    .position(|effort| *effort == provider.effort),
                                theme,
                                cx,
                            ),
                        )
                    })
                    .when(provider.kind == ProviderKind::KimiCode, |this| {
                        this.child(
                            div()
                                .text_xs()
                                .line_height(px(18.0))
                                .text_color(theme.faint)
                                .child(self.tr("kimi_thinking")),
                        )
                    })
                    .child(field_label(self.tr("api_key_label"), theme))
                    .child(input_container(theme).child(self.api_key_input.clone()))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                action_button(self.tr("save_key"), "save-key", true, theme)
                                    .on_click(cx.listener(|this, _, _, cx| this.save_api_key(cx))),
                            )
                            .child(
                                action_button(self.tr("delete_key"), "delete-key", false, theme)
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.delete_api_key(cx)),
                                    ),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(if self.key_saved {
                                        theme.green
                                    } else {
                                        theme.faint
                                    })
                                    .child(if self.key_saved {
                                        self.tr("key_saved")
                                    } else {
                                        self.tr("no_key_saved")
                                    }),
                            ),
                    )
                    .child(
                        setting_row(self.tr("permission"), self.permission_label(), theme)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.permission = this.permission.next();
                                this.settings.permission_mode = this.permission.code().into();
                                this.pending_tool = None;
                                this.persist_settings();
                                cx.notify();
                            })),
                    )
                    .child(
                        setting_row(
                            self.tr("include_document"),
                            if self.settings.include_document {
                                self.tr("on")
                            } else {
                                self.tr("off")
                            },
                            theme,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.settings.include_document = !this.settings.include_document;
                            this.persist_settings();
                            cx.notify();
                        })),
                    ),
            )
    }

    fn approval_panel(&self, theme: Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let (name, arguments, diff) = self
            .pending_tool
            .as_ref()
            .map(|(call, diff)| (call.name.clone(), call.arguments.clone(), diff.clone()))
            .unwrap_or_default();
        div()
            .absolute()
            .top(px(90.0))
            .left_0()
            .right_0()
            .mx_auto()
            .w(px(620.0))
            .rounded_xl()
            .border_1()
            .border_color(theme.yellow)
            .bg(theme.surface)
            .shadow_lg()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .p_4()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(self.tr("approval_required")),
                    )
                    .child(badge(
                        self.permission_label().into(),
                        theme.accent_soft,
                        theme.yellow,
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .p_4()
                    .child(div().text_sm().text_color(theme.accent).child(name))
                    .child(
                        div()
                            .text_xs()
                            .line_height(px(18.0))
                            .text_color(theme.muted)
                            .child(arguments),
                    )
                    .when_some(diff, |this, diff| {
                        this.child(
                            div()
                                .p_3()
                                .rounded_md()
                                .bg(theme.window)
                                .font_family("monospace")
                                .text_xs()
                                .line_height(px(18.0))
                                .child(diff),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                action_button(self.tr("allow"), "allow-tool", true, theme)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.approve_pending_tool(cx);
                                        cx.notify();
                                    })),
                            )
                            .child(
                                action_button(self.tr("deny"), "deny-tool", false, theme).on_click(
                                    cx.listener(|this, _, _, cx| {
                                        this.deny_pending_tool(cx);
                                        cx.notify();
                                    }),
                                ),
                            ),
                    ),
            )
    }

    fn audit_panel(&self, theme: Theme) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_1()
            .mx_3()
            .mb_2()
            .p_3()
            .rounded_md()
            .bg(theme.raised)
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.muted)
                    .child(self.tr("audit")),
            )
            .children(self.audit.iter().rev().take(3).rev().cloned().map(|entry| {
                div()
                    .text_xs()
                    .line_height(px(17.0))
                    .text_color(theme.faint)
                    .child(entry)
                    .into_any_element()
            }))
    }

    fn status_bar(&self, theme: Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let (line, column) = self.document.read(cx).cursor_line_col();
        div()
            .flex()
            .items_center()
            .h(px(28.0))
            .px_3()
            .gap_4()
            .border_t_1()
            .border_color(theme.border)
            .bg(theme.surface)
            .text_xs()
            .text_color(theme.muted)
            .when(self.status.to_lowercase().contains("error"), |this| {
                this.child(div().size(px(6.0)).rounded_full().bg(theme.red))
            })
            .child(self.status.clone())
            .child(div().flex_1())
            .child(if self.state.focus_mode {
                "Focus mode".into()
            } else {
                format!("Ln {line}, Col {column}")
            })
            .child("UTF-8")
            .child(self.settings.provider.kind.label())
    }

    fn language_offer(&self, theme: Theme, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .absolute()
            .top(px(94.0))
            .left_0()
            .right_0()
            .mx_auto()
            .w(px(520.0))
            .flex()
            .items_center()
            .gap_3()
            .p_4()
            .rounded_xl()
            .border_1()
            .border_color(theme.accent)
            .bg(theme.surface)
            .shadow_lg()
            .child(div().flex_1().text_sm().child(self.tr("language_offer")))
            .child(
                action_button("Русский", "use-russian", true, theme).on_click(
                    cx.listener(|this, _, _, cx| this.set_language(Language::Russian, cx)),
                ),
            )
            .child(
                action_button("Keep English", "keep-english", false, theme)
                    .on_click(cx.listener(|this, _, _, cx| this.dismiss_language_offer(cx))),
            )
    }

    fn unsaved_panel(&self, theme: Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let closing = self.close_confirmation_open;
        div()
            .occlude()
            .absolute()
            .top(px(94.0))
            .left_0()
            .right_0()
            .mx_auto()
            .w(px(520.0))
            .flex()
            .flex_col()
            .gap_4()
            .p_4()
            .rounded_xl()
            .border_1()
            .border_color(theme.yellow)
            .bg(theme.surface)
            .shadow_lg()
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(self.tr("unsaved_title")),
            )
            .child(
                div()
                    .text_sm()
                    .line_height(px(21.0))
                    .text_color(theme.muted)
                    .child(if closing {
                        self.tr("unsaved_close")
                    } else {
                        self.tr("unsaved_open")
                    }),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        action_button(self.tr("keep_editing"), "keep-editing", false, theme)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.cancel_unsaved_action(cx);
                            })),
                    )
                    .when(closing, |this| {
                        this.child(
                            action_button(self.tr("discard"), "confirm-close", true, theme)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.confirm_close(window, cx);
                                })),
                        )
                    })
                    .when(!closing, |this| {
                        this.child(
                            action_button(self.tr("discard"), "confirm-discard", true, theme)
                                .on_click(cx.listener(|this, _, _, cx| this.confirm_discard(cx))),
                        )
                    }),
            )
    }

    fn command_palette(&self, theme: Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let matches = self.filtered_commands(cx);
        let selected = self.palette_selected.min(matches.len().saturating_sub(1));
        div()
            .id("command-palette")
            .key_context("Palette")
            .occlude()
            .on_action(
                cx.listener(|this, _: &PaletteUp, _, cx| this.move_palette_selection(-1, cx)),
            )
            .on_action(
                cx.listener(|this, _: &PaletteDown, _, cx| this.move_palette_selection(1, cx)),
            )
            .on_action(cx.listener(|this, _: &PaletteConfirm, _, cx| this.confirm_palette(cx)))
            .absolute()
            .top(px(72.0))
            .left_0()
            .right_0()
            .mx_auto()
            .w(px(620.0))
            .rounded_xl()
            .border_1()
            .border_color(theme.border)
            .bg(theme.surface)
            .shadow_lg()
            .child(
                div()
                    .h(px(48.0))
                    .px_4()
                    .flex()
                    .items_center()
                    .border_b_1()
                    .border_color(theme.border)
                    .text_color(theme.text)
                    .child(div().flex_1().child(self.palette_input.clone())),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_2()
                    .children(matches.iter().enumerate().map(|(position, index)| {
                        let (label_key, shortcut, command) = PALETTE_COMMANDS[*index];
                        command_row(
                            self.tr(label_key),
                            shortcut,
                            command,
                            position == selected,
                            theme,
                            cx,
                        )
                        .into_any_element()
                    }))
                    .when(matches.is_empty(), |this| {
                        this.child(
                            div()
                                .h(px(38.0))
                                .px_3()
                                .flex()
                                .items_center()
                                .text_sm()
                                .text_color(theme.faint)
                                .child(self.tr("no_commands")),
                        )
                    }),
            )
    }

    fn execute_palette_command(&mut self, command: PaletteCommand, cx: &mut Context<Self>) {
        self.state.close_palette();
        match command {
            PaletteCommand::Open => self.open_file_dialog(cx),
            PaletteCommand::Save => self.save_document(false, cx),
            PaletteCommand::NewChat => self.new_chat(cx),
            PaletteCommand::Settings => self.open_settings(cx),
            PaletteCommand::Focus => self.state.toggle_focus_mode(),
            PaletteCommand::Ai => self.state.toggle_ai(),
            PaletteCommand::Theme => self.state.toggle_theme(),
        }
        if !self.settings_open {
            self.pending_focus = Some(FocusTarget::Document);
        }
        cx.notify();
    }

    pub fn focus_document(&self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.document.read(cx).focus_handle(cx));
    }
}

impl Focusable for Workspace {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::from_mode(self.state.theme);
        self.window_origin_x = window.bounds().origin.x.into();
        self.window_width = window.bounds().size.width.into();

        // While an overlay is open the text editor must not accept keystrokes.
        let overlay = self.overlay_open();
        self.document
            .update(cx, |view, _| view.set_read_only(overlay));
        if let Some(target) = self.pending_focus.take() {
            let handle = match target {
                FocusTarget::Document => self.document.read(cx).focus_handle(cx),
                FocusTarget::Palette => self.palette_input.read(cx).focus_handle(cx),
                FocusTarget::Settings => {
                    if self.settings.provider.model_options().is_empty() {
                        self.model_input.read(cx).focus_handle(cx)
                    } else {
                        self.api_key_input.read(cx).focus_handle(cx)
                    }
                }
            };
            window.focus(&handle);
        }
        div()
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .overflow_hidden()
            .bg(theme.window)
            .text_color(theme.text)
            .track_focus(&self.focus_handle)
            .child(self.window_chrome(window, theme, cx))
            .child(self.header(theme, cx))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h(px(0.0))
                    .when(!self.state.focus_mode, |this| {
                        this.child(self.activity_rail(theme, cx))
                    })
                    .when(self.state.navigation_visible(), |this| {
                        this.child(self.navigation_panel(theme, cx))
                            .child(self.resize_handle(ResizeTarget::Left, theme, cx))
                    })
                    .child(self.editor(theme, cx))
                    .when(self.state.ai_visible(), |this| {
                        this.child(self.resize_handle(ResizeTarget::Right, theme, cx))
                            .child(self.assistant_panel(theme, cx))
                    }),
            )
            .child(self.status_bar(theme, cx))
            .when(self.offer_russian, |this| {
                this.child(self.language_offer(theme, cx))
            })
            .when(overlay, |this| this.child(self.backdrop(cx)))
            .when(self.state.palette_open, |this| {
                this.child(self.command_palette(theme, cx))
            })
            .when(self.settings_open, |this| {
                this.child(self.settings_panel(theme, cx))
            })
            .when(self.pending_tool.is_some(), |this| {
                this.child(self.approval_panel(theme, cx))
            })
            .when(
                self.pending_discard.is_some() || self.close_confirmation_open,
                |this| this.child(self.unsaved_panel(theme, cx)),
            )
    }
}

/// Vertical scrollbar for the editor (gpui draws none by itself). Click or drag on the
/// track to scroll; the thumb follows the mouse wheel because the window is redrawn on scroll.
fn document_scrollbar(handle: ScrollHandle, theme: Theme) -> gpui::AnyElement {
    const TRACK_WIDTH: f32 = 12.0;
    const MIN_THUMB: f32 = 28.0;
    let viewport = handle.bounds().size.height;
    let max_offset = handle.max_offset().height;
    if viewport <= px(1.0) || max_offset <= px(0.5) {
        return div().into_any_element();
    }
    let thumb_height = (viewport * (viewport / (viewport + max_offset))).max(px(MIN_THUMB));
    let travel = (viewport - thumb_height).max(px(1.0));
    let progress = (-handle.offset().y / max_offset).clamp(0.0, 1.0);
    let thumb_top = travel * progress;
    let scroll_to = move |handle: &ScrollHandle, y: gpui::Pixels| {
        let origin = handle.bounds().origin.y;
        let fraction = ((y - origin - thumb_height / 2.0) / travel).clamp(0.0, 1.0);
        let offset = handle.offset();
        handle.set_offset(point(offset.x, -(max_offset * fraction)));
    };
    let down_handle = handle.clone();
    let move_handle = handle;
    div()
        .id("document-scrollbar")
        .absolute()
        .top(px(12.0))
        .bottom(px(12.0))
        .right_0()
        .w(px(TRACK_WIDTH))
        .cursor_pointer()
        .occlude()
        .on_mouse_down(MouseButton::Left, move |event, window, _| {
            scroll_to(&down_handle, event.position.y);
            window.refresh();
        })
        .on_mouse_move(move |event, window, _| {
            if event.pressed_button == Some(MouseButton::Left) {
                scroll_to(&move_handle, event.position.y);
                window.refresh();
            }
        })
        .child(
            div()
                .absolute()
                .top(thumb_top)
                .right(px(2.0))
                .w(px(8.0))
                .h(thumb_height)
                .rounded_full()
                .bg(gpui::Hsla::from(theme.muted).opacity(0.35))
                .hover(|style| style.bg(gpui::Hsla::from(theme.muted).opacity(0.6))),
        )
        .into_any_element()
}

fn logo(theme: Theme) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .justify_center()
        .size(px(28.0))
        .rounded_md()
        .bg(theme.accent)
        .text_color(gpui::white())
        .font_weight(FontWeight::SEMIBOLD)
        .child("A")
}

fn action_button(
    label: impl Into<gpui::SharedString>,
    id: &'static str,
    active: bool,
    theme: Theme,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .h(px(30.0))
        .px_2()
        .rounded_md()
        .text_xs()
        .cursor_pointer()
        .when(active, |this| {
            this.bg(theme.accent_soft).text_color(theme.accent)
        })
        .when(!active, |this| {
            this.text_color(theme.muted)
                .hover(|style| style.bg(theme.hover).text_color(theme.text))
        })
        .child(label.into())
}

fn keycap(label: &'static str, theme: Theme) -> impl IntoElement {
    div()
        .px(px(6.0))
        .py(px(2.0))
        .rounded(px(4.0))
        .border_1()
        .border_color(theme.border)
        .text_xs()
        .text_color(theme.faint)
        .child(label)
}

fn badge(text: String, background: Rgba, foreground: Rgba) -> impl IntoElement {
    div()
        .px_2()
        .py_1()
        .rounded_md()
        .bg(background)
        .text_xs()
        .text_color(foreground)
        .child(text)
}

fn panel_title(title: &'static str, subtitle: &'static str, theme: Theme) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .p_4()
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.muted)
                .child(title),
        )
        .child(div().text_xs().text_color(theme.faint).child(subtitle))
}

fn empty_chat(theme: Theme, language: Language) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_3()
        .p_4()
        .rounded_lg()
        .bg(theme.raised)
        .child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .child(tr(language, "native_chat")),
        )
        .child(
            div()
                .text_sm()
                .line_height(px(21.0))
                .text_color(theme.muted)
                .child(tr(language, "chat_hint")),
        )
        .child(
            div()
                .text_xs()
                .line_height(px(18.0))
                .text_color(theme.faint)
                .child(tr(language, "chat_security")),
        )
}

fn chat_message(
    index: usize,
    entry: ChatEntry,
    waiting: bool,
    theme: Theme,
    language: Language,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    let is_user = entry.role == MessageRole::User;
    let content = entry.content.clone();
    div()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.muted)
                        .child(if is_user { tr(language, "you") } else { "AIR" }),
                )
                .child(div().flex_1())
                .child(
                    div()
                        .id(gpui::SharedString::from(format!("copy-message-{index}")))
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .text_xs()
                        .text_color(theme.muted)
                        .cursor_pointer()
                        .hover(|style| style.bg(theme.hover).text_color(theme.text))
                        .on_click(cx.listener(move |_, _, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(content.clone()));
                        }))
                        .child(tr(language, "copy")),
                )
                .when(!entry.reasoning.is_empty(), |this| {
                    this.child(
                        div()
                            .id(gpui::SharedString::from(format!(
                                "toggle-reasoning-{index}"
                            )))
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .text_xs()
                            .text_color(theme.purple)
                            .cursor_pointer()
                            .hover(|style| style.bg(theme.hover))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(entry) = this.chat.get_mut(index) {
                                    entry.show_reasoning = !entry.show_reasoning;
                                    cx.notify();
                                }
                            }))
                            .child(if entry.show_reasoning {
                                tr(language, "hide_reasoning")
                            } else {
                                tr(language, "show_reasoning")
                            }),
                    )
                }),
        )
        .when(
            entry.show_reasoning && !entry.reasoning.is_empty(),
            |this| {
                this.child(
                    div()
                        .p_3()
                        .rounded_md()
                        .bg(theme.window)
                        .text_xs()
                        .line_height(px(18.0))
                        .text_color(theme.faint)
                        .child(
                            div()
                                .pb_1()
                                .text_color(theme.purple)
                                .child(tr(language, "reasoning")),
                        )
                        .child(entry.reasoning),
                )
            },
        )
        .child(
            div()
                .p_3()
                .rounded_lg()
                .when(is_user, |this| this.bg(theme.raised))
                .when(!is_user, |this| this.bg(theme.window))
                .text_sm()
                .line_height(px(21.0))
                .text_color(theme.text)
                .child(if !entry.content.is_empty() {
                    entry.content
                } else if waiting {
                    tr(language, "thinking").into()
                } else {
                    "(no response)".into()
                }),
        )
}

fn dropdown_list(
    kind: Dropdown,
    options: Vec<String>,
    selected: Option<usize>,
    theme: Theme,
    cx: &mut Context<Workspace>,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(("dropdown-list", kind as usize))
        .occlude()
        .flex()
        .flex_col()
        .p_1()
        .rounded_md()
        .border_1()
        .border_color(theme.border)
        .bg(theme.raised)
        .shadow_lg()
        .max_h(px(260.0))
        .overflow_y_scroll()
        .children(options.into_iter().enumerate().map(|(index, option)| {
            let is_selected = selected == Some(index);
            div()
                .id((
                    match kind {
                        Dropdown::Provider => "dd-provider",
                        Dropdown::Region => "dd-region",
                        Dropdown::Model => "dd-model",
                        Dropdown::Effort => "dd-effort",
                        Dropdown::ChatModel => "dd-chat-model",
                    },
                    index,
                ))
                .flex()
                .items_center()
                .h(px(30.0))
                .px_3()
                .rounded_md()
                .text_sm()
                .cursor_pointer()
                .text_color(if is_selected {
                    theme.accent
                } else {
                    theme.text
                })
                .when(is_selected, |this| this.bg(theme.accent_soft))
                .hover(|style| style.bg(theme.hover))
                .child(option)
                .on_click(cx.listener(move |this, _, _, cx| this.apply_dropdown(kind, index, cx)))
        }))
}

fn setting_row(
    label: &'static str,
    value: impl Into<gpui::SharedString>,
    theme: Theme,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(label)
        .flex()
        .items_center()
        .justify_between()
        .h(px(34.0))
        .px_3()
        .rounded_md()
        .bg(theme.raised)
        .cursor_pointer()
        .hover(|style| style.bg(theme.hover))
        .child(div().text_sm().text_color(theme.muted).child(label))
        .child(div().text_sm().text_color(theme.accent).child(value.into()))
}

fn field_label(label: &'static str, theme: Theme) -> impl IntoElement {
    div().text_xs().text_color(theme.muted).child(label)
}

fn input_container(theme: Theme) -> gpui::Div {
    div()
        .h(px(36.0))
        .px_3()
        .py_2()
        .rounded_md()
        .border_1()
        .border_color(theme.border)
        .bg(theme.window)
        .text_sm()
}

fn command_row(
    label: &'static str,
    shortcut: &'static str,
    command: PaletteCommand,
    selected: bool,
    theme: Theme,
    cx: &mut Context<Workspace>,
) -> impl IntoElement {
    div()
        .id(label)
        .flex()
        .items_center()
        .h(px(38.0))
        .px_3()
        .rounded_md()
        .cursor_pointer()
        .when(selected, |this| this.bg(theme.accent_soft))
        .when(!selected, |this| this.hover(|style| style.bg(theme.hover)))
        .on_click(cx.listener(move |this, _, _, cx| this.execute_palette_command(command, cx)))
        .child(div().text_sm().text_color(theme.text).child(label))
        .child(div().flex_1())
        .child(keycap(shortcut, theme))
}
