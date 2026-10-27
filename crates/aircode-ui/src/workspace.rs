use std::sync::Arc;

use aircode_ai::{
    AiClient, ApiKey, AppSettings, ChatMessage, CredentialStore, MessageRole, ProviderConfig,
    ProviderKind, ReasoningEffort, Region, SettingsStore, StreamEvent,
};
use aircode_core::{NavigationPanel, ThemeMode, WorkspaceState};
use aircode_editor::Document;
use gpui::{
    App, Context, Entity, FocusHandle, Focusable, FontWeight, KeyBinding, Rgba, Window, actions,
    div, prelude::*, px,
};
use rfd::{FileDialog, MessageButtons, MessageDialog, MessageDialogResult};
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
        view.update(cx, |this, cx| {
            this.state.toggle_palette();
            cx.notify();
        });
    });
    let view = workspace.clone();
    cx.on_action(move |_: &CloseOverlay, cx| {
        view.update(cx, |this, cx| {
            this.state.close_palette();
            this.settings_open = false;
            cx.notify();
        });
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
        workspace.update(cx, |this, cx| {
            this.settings_open = !this.settings_open;
            cx.notify();
        });
    });
}

#[derive(Clone)]
struct ChatEntry {
    role: MessageRole,
    content: String,
    reasoning: String,
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

pub struct Workspace {
    state: WorkspaceState,
    focus_handle: FocusHandle,
    document: Entity<DocumentView>,
    chat_input: Entity<TextInput>,
    api_key_input: Entity<TextInput>,
    base_url_input: Entity<TextInput>,
    model_input: Entity<TextInput>,
    settings: AppSettings,
    settings_store: Option<SettingsStore>,
    settings_open: bool,
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
        let key_saved = match CredentialStore::load(&settings.provider.credential_id()) {
            Ok(key) => key.is_some(),
            Err(error) => {
                status = error.to_string();
                false
            }
        };
        let document = cx.new(DocumentView::new);
        cx.observe(&document, |_, _, cx| cx.notify()).detach();
        let chat_input = cx.new(|cx| TextInput::new("Ask about your code...", cx));
        let api_key_input = cx.new(|cx| {
            let mut input = TextInput::new("Paste API key", cx);
            input.set_masked(true);
            input
        });
        let base_url_input = cx.new(|cx| TextInput::new("https://server/v1", cx));
        let model_input = cx.new(|cx| TextInput::new("Model ID", cx));
        base_url_input.update(cx, |input, cx| {
            input.set_text(settings.provider.base_url.clone(), cx);
        });
        model_input.update(cx, |input, cx| {
            input.set_text(settings.provider.model.clone(), cx);
        });
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("failed to start async runtime");
        let client = Arc::new(AiClient::new().expect("failed to initialize HTTP client"));
        Self {
            state: WorkspaceState::default(),
            focus_handle: cx.focus_handle(),
            document,
            chat_input,
            api_key_input,
            base_url_input,
            model_input,
            settings,
            settings_store,
            settings_open: false,
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

    fn open_file_dialog(&mut self, cx: &mut Context<Self>) {
        if self.document.read(cx).document().is_modified() {
            let answer = MessageDialog::new()
                .set_title("Unsaved changes")
                .set_description("Discard unsaved changes and open another file? Choose Yes to discard or No to cancel.")
                .set_buttons(MessageButtons::YesNo)
                .show();
            if answer != MessageDialogResult::Yes {
                return;
            }
        }
        let Some(path) = FileDialog::new().pick_file() else {
            return;
        };
        match Document::open(&path) {
            Ok(document) => {
                self.document
                    .update(cx, |view, cx| view.set_document(document, cx));
                self.status = format!("Opened {}", path.display());
            }
            Err(error) => self.status = error.to_string(),
        }
        cx.notify();
    }

    fn save_document(&mut self, force_dialog: bool, cx: &mut Context<Self>) {
        let has_path = self.document.read(cx).document().path().is_some();
        let path = if force_dialog || !has_path {
            FileDialog::new().save_file()
        } else {
            self.document
                .read(cx)
                .document()
                .path()
                .map(|path| path.to_path_buf())
        };
        let Some(path) = path else {
            return;
        };
        let result = self.document.update(cx, |view, _| {
            let document = view.document_mut();
            if force_dialog || !has_path {
                document.save_as(&path)
            } else {
                document.save()
            }
        });
        self.status = match result {
            Ok(()) => format!("Saved {}", path.display()),
            Err(error) => error.to_string(),
        };
        cx.notify();
    }

    fn new_chat(&mut self, cx: &mut Context<Self>) {
        if let Some(cancel) = self.cancel.take() {
            cancel.cancel();
        }
        self.generation += 1;
        self.streaming = false;
        self.chat.clear();
        self.status = "New session".into();
        cx.notify();
    }

    fn stop_generation(&mut self, _: &StopGeneration, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(cancel) = &self.cancel {
            cancel.cancel();
            self.status = "Cancelling...".into();
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
                self.settings_open = true;
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
        });
        self.chat.push(ChatEntry {
            role: MessageRole::Assistant,
            content: String::new(),
            reasoning: String::new(),
        });
        self.chat_input.update(cx, |input, cx| input.clear(cx));
        self.generation += 1;
        let generation = self.generation;
        let (sender, mut receiver) = mpsc::unbounded_channel();
        let cancel = CancellationToken::new();
        self.cancel = Some(cancel.clone());
        self.streaming = true;
        self.status = format!("Connecting to {}...", self.settings.provider.kind.label());
        let config = self.settings.provider.clone();
        let client = self.client.clone();
        let worker_sender = sender.clone();
        self.runtime.spawn(async move {
            if let Err(error) = client
                .stream_chat(&config, &key, &request_messages, cancel, sender)
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
        let Some(entry) = self.chat.last_mut() else {
            return;
        };
        match event {
            StreamEvent::ReasoningDelta(delta) => entry.reasoning.push_str(&delta),
            StreamEvent::ContentDelta(delta) => {
                entry.content.push_str(&delta);
                self.status = "Receiving response...".into();
            }
            StreamEvent::Usage(usage) => {
                if let Some(tokens) = usage.get("total_tokens").and_then(|value| value.as_u64()) {
                    self.status = format!("{tokens} tokens");
                }
            }
            StreamEvent::Error(error) => {
                self.streaming = false;
                self.cancel = None;
                self.status = error;
            }
            StreamEvent::Done => {
                self.streaming = false;
                self.cancel = None;
                if self.status.starts_with("Receiving") || self.status.starts_with("Connecting") {
                    self.status = "Ready".into();
                }
            }
        }
        cx.notify();
    }

    fn select_provider(&mut self, cx: &mut Context<Self>) {
        let current = ProviderKind::ALL
            .iter()
            .position(|kind| *kind == self.settings.provider.kind)
            .unwrap_or(0);
        let kind = ProviderKind::ALL[(current + 1) % ProviderKind::ALL.len()];
        let region = self.settings.provider.region;
        self.settings.provider = ProviderConfig::new(kind).with_region(region);
        self.update_config_inputs(cx);
        self.reload_key_state();
        self.new_chat(cx);
        self.persist_settings();
    }

    fn select_region(&mut self, cx: &mut Context<Self>) {
        let region = match self.settings.provider.region {
            Region::China => Region::Overseas,
            Region::Overseas => Region::China,
        };
        self.settings.provider = self.settings.provider.clone().with_region(region);
        self.update_config_inputs(cx);
        self.reload_key_state();
        self.new_chat(cx);
        self.persist_settings();
    }

    fn select_model(&mut self, cx: &mut Context<Self>) {
        let options = self.settings.provider.model_options();
        if options.is_empty() {
            return;
        }
        let current = options
            .iter()
            .position(|model| *model == self.settings.provider.model)
            .unwrap_or(0);
        self.settings.provider.model = options[(current + 1) % options.len()].into();
        self.update_config_inputs(cx);
        self.new_chat(cx);
        self.persist_settings();
    }

    fn select_effort(&mut self, cx: &mut Context<Self>) {
        let current = ReasoningEffort::ALL
            .iter()
            .position(|effort| *effort == self.settings.provider.effort)
            .unwrap_or(1);
        self.settings.provider.effort = ReasoningEffort::ALL[(current + 1) % 3];
        self.new_chat(cx);
        self.persist_settings();
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
                    .child(div().text_xs().text_color(theme.faint).child("0.2.0"))
                    .child(div().w_4())
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.muted)
                            .child(self.document.read(cx).document().file_name().to_string()),
                    )
                    .when(self.document.read(cx).document().is_modified(), |this| {
                        this.child(div().text_color(theme.yellow).child("modified"))
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
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.state.toggle_palette();
                        cx.notify();
                    }))
                    .child("Commands and file actions")
                    .child(keycap("Ctrl P", theme)),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        action_button("Open", "open-button", false, theme)
                            .on_click(cx.listener(|this, _, _, cx| this.open_file_dialog(cx))),
                    )
                    .child(
                        action_button("Save", "save-button", false, theme)
                            .on_click(cx.listener(|this, _, _, cx| this.save_document(false, cx))),
                    )
                    .child(
                        action_button(
                            "Theme",
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
                        action_button("AI", "ai-button", self.state.ai_visible(), theme).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.state.toggle_ai();
                                cx.notify();
                            }),
                        ),
                    )
                    .child(
                        action_button("Settings", "settings-button", self.settings_open, theme)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.settings_open = !this.settings_open;
                                cx.notify();
                            })),
                    )
                    .child(
                        action_button("Focus", "focus-button", self.state.focus_mode, theme)
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
            .child(div().text_xs().text_color(theme.faint).child("0.2"))
    }

    fn navigation_panel(&self, theme: Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let document = self.document.read(cx).document();
        div()
            .flex()
            .flex_col()
            .w(px(230.0))
            .border_r_1()
            .border_color(theme.border)
            .bg(theme.surface)
            .child(panel_title("WORKSPACE", "Current document", theme))
            .child(
                div()
                    .m_2()
                    .p_3()
                    .rounded_md()
                    .bg(theme.accent_soft)
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.accent)
                            .child(document.file_name().to_string()),
                    )
                    .child(div().pt_1().text_xs().text_color(theme.muted).child(
                        if document.is_modified() {
                            "Unsaved changes"
                        } else {
                            "Saved"
                        },
                    )),
            )
            .child(
                div()
                    .p_3()
                    .text_xs()
                    .line_height(px(18.0))
                    .text_color(theme.faint)
                    .child(
                        document
                            .path()
                            .map(|path| path.display().to_string())
                            .unwrap_or_else(|| "Unsaved document".into()),
                    ),
            )
            .child(div().flex_1())
            .child(
                div()
                    .p_3()
                    .text_xs()
                    .text_color(theme.faint)
                    .child("Large files, Tree-sitter and LSP are planned for the next stage."),
            )
    }

    fn editor(&self, theme: Theme, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
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
                    .id("document-scroll")
                    .flex_1()
                    .overflow_y_scroll()
                    .py_3()
                    .font_family("monospace")
                    .text_sm()
                    .child(self.document.clone()),
            )
    }

    fn assistant_panel(&self, theme: Theme, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .w(px(360.0))
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
                    .child(badge(
                        self.settings.provider.model.clone(),
                        theme.accent_soft,
                        theme.accent,
                    )),
            )
            .child(
                div()
                    .id("chat-scroll")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .gap_4()
                    .p_4()
                    .overflow_y_scroll()
                    .when(self.chat.is_empty(), |this| this.child(empty_chat(theme)))
                    .children(
                        self.chat
                            .iter()
                            .cloned()
                            .map(|entry| chat_message(entry, theme).into_any_element()),
                    ),
            )
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
                                    "Document context: on"
                                } else {
                                    "Document context: off"
                                },
                            ))
                            .child(div().flex_1())
                            .child(div().text_xs().text_color(theme.faint).child("Ctrl+Enter"))
                            .child(
                                action_button(
                                    if self.streaming { "Stop" } else { "Send" },
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
    }

    fn settings_panel(&self, theme: Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let provider = &self.settings.provider;
        div()
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
                    .child(div().font_weight(FontWeight::SEMIBOLD).child("AI provider settings"))
                    .child(action_button("Close", "close-settings", false, theme).on_click(cx.listener(|this, _, _, cx| {
                        this.settings_open = false;
                        cx.notify();
                    }))),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .p_4()
                    .child(setting_row("Provider", provider.kind.label(), theme).on_click(cx.listener(|this, _, _, cx| this.select_provider(cx))))
                    .when(matches!(provider.kind, ProviderKind::KimiCode | ProviderKind::KimiPlatform), |this| {
                        this.child(setting_row("Region", match provider.region { Region::China => "China", Region::Overseas => "Overseas" }, theme).on_click(cx.listener(|this, _, _, cx| this.select_region(cx))))
                    })
                    .child(div().text_xs().text_color(theme.faint).child(provider.base_url.clone()))
                    .when(!provider.model_options().is_empty(), |this| {
                        this.child(setting_row("Model", provider.model.clone(), theme).on_click(cx.listener(|this, _, _, cx| this.select_model(cx))))
                    })
                    .when(provider.model_options().is_empty(), |this| {
                        this.child(field_label("Model ID", theme)).child(input_container(theme).child(self.model_input.clone()))
                    })
                    .when(provider.kind == ProviderKind::OpenAiCompatible, |this| {
                        this.child(field_label("Base URL", theme)).child(input_container(theme).child(self.base_url_input.clone()))
                    })
                    .when(provider.supports_effort(), |this| {
                        this.child(setting_row("Reasoning effort (new session)", provider.effort.label(), theme).on_click(cx.listener(|this, _, _, cx| this.select_effort(cx))))
                    })
                    .when(provider.kind == ProviderKind::KimiCode, |this| {
                        this.child(div().text_xs().line_height(px(18.0)).text_color(theme.faint).child("K3.1 Pro always uses thinking. Changing the model or effort starts a new session."))
                    })
                    .child(field_label("API key (stored only in the OS credential store)", theme))
                    .child(input_container(theme).child(self.api_key_input.clone()))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(action_button("Save key", "save-key", true, theme).on_click(cx.listener(|this, _, _, cx| this.save_api_key(cx))))
                            .child(action_button("Delete key", "delete-key", false, theme).on_click(cx.listener(|this, _, _, cx| this.delete_api_key(cx))))
                            .child(div().text_xs().text_color(if self.key_saved { theme.green } else { theme.faint }).child(if self.key_saved { "Key saved" } else { "No key saved" })),
                    )
                    .child(setting_row("Include document or selection in chat", if self.settings.include_document { "On" } else { "Off" }, theme).on_click(cx.listener(|this, _, _, cx| {
                        this.settings.include_document = !this.settings.include_document;
                        this.persist_settings();
                        cx.notify();
                    }))),
            )
    }

    fn status_bar(&self, theme: Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let (line, column) = self.document.read(cx).document().line_col();
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

    fn command_palette(&self, theme: Theme, cx: &mut Context<Self>) -> impl IntoElement {
        div()
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
                    .text_color(theme.faint)
                    .border_b_1()
                    .border_color(theme.border)
                    .child("Select a command"),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_2()
                    .child(command_row(
                        "File: open",
                        "Ctrl O",
                        PaletteCommand::Open,
                        true,
                        theme,
                        cx,
                    ))
                    .child(command_row(
                        "File: save",
                        "Ctrl S",
                        PaletteCommand::Save,
                        false,
                        theme,
                        cx,
                    ))
                    .child(command_row(
                        "AI: new chat",
                        "Ctrl Shift N",
                        PaletteCommand::NewChat,
                        false,
                        theme,
                        cx,
                    ))
                    .child(command_row(
                        "AI: provider settings",
                        "Ctrl ,",
                        PaletteCommand::Settings,
                        false,
                        theme,
                        cx,
                    ))
                    .child(command_row(
                        "Focus: toggle",
                        "Ctrl Shift F",
                        PaletteCommand::Focus,
                        false,
                        theme,
                        cx,
                    ))
                    .child(command_row(
                        "AI: toggle panel",
                        "Ctrl Shift I",
                        PaletteCommand::Ai,
                        false,
                        theme,
                        cx,
                    ))
                    .child(command_row(
                        "Theme: switch",
                        "Ctrl Shift T",
                        PaletteCommand::Theme,
                        false,
                        theme,
                        cx,
                    )),
            )
    }

    fn execute_palette_command(&mut self, command: PaletteCommand, cx: &mut Context<Self>) {
        self.state.close_palette();
        match command {
            PaletteCommand::Open => self.open_file_dialog(cx),
            PaletteCommand::Save => self.save_document(false, cx),
            PaletteCommand::NewChat => self.new_chat(cx),
            PaletteCommand::Settings => self.settings_open = true,
            PaletteCommand::Focus => self.state.toggle_focus_mode(),
            PaletteCommand::Ai => self.state.toggle_ai(),
            PaletteCommand::Theme => self.state.toggle_theme(),
        }
        cx.notify();
    }

    pub fn focus_document(&self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.document.read(cx).focus_handle(cx));
    }

    pub fn has_unsaved_changes(&self, cx: &App) -> bool {
        self.document.read(cx).document().is_modified()
    }
}

impl Focusable for Workspace {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Workspace {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::from_mode(self.state.theme);
        div()
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .overflow_hidden()
            .bg(theme.window)
            .text_color(theme.text)
            .track_focus(&self.focus_handle)
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
                    })
                    .child(self.editor(theme, cx))
                    .when(self.state.ai_visible(), |this| {
                        this.child(self.assistant_panel(theme, cx))
                    }),
            )
            .child(self.status_bar(theme, cx))
            .when(self.state.palette_open, |this| {
                this.child(self.command_palette(theme, cx))
            })
            .when(self.settings_open, |this| {
                this.child(self.settings_panel(theme, cx))
            })
    }
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

fn empty_chat(theme: Theme) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_3()
        .p_4()
        .rounded_lg()
        .bg(theme.raised)
        .child(div().font_weight(FontWeight::SEMIBOLD).child("Native AI chat"))
        .child(div().text_sm().line_height(px(21.0)).text_color(theme.muted).child("Choose Kimi Code and k3.1-pro, save your membership API key, then send a message. Your document is included only when context is enabled."))
        .child(div().text_xs().line_height(px(18.0)).text_color(theme.faint).child("No CLI, no browser proxy, no keys in settings files."))
}

fn chat_message(entry: ChatEntry, theme: Theme) -> impl IntoElement {
    let is_user = entry.role == MessageRole::User;
    div()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.muted)
                .child(if is_user { "You" } else { "AIR" }),
        )
        .when(!entry.reasoning.is_empty(), |this| {
            this.child(
                div()
                    .p_3()
                    .rounded_md()
                    .bg(theme.window)
                    .text_xs()
                    .line_height(px(18.0))
                    .text_color(theme.faint)
                    .child(div().pb_1().text_color(theme.purple).child("Reasoning"))
                    .child(entry.reasoning),
            )
        })
        .child(
            div()
                .p_3()
                .rounded_lg()
                .when(is_user, |this| this.bg(theme.raised))
                .when(!is_user, |this| this.bg(theme.window))
                .text_sm()
                .line_height(px(21.0))
                .text_color(theme.text)
                .child(if entry.content.is_empty() {
                    "Thinking...".into()
                } else {
                    entry.content
                }),
        )
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
