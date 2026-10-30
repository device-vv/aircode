mod client;
mod completion;
mod credentials;
mod provider;
mod settings;
mod sse;

pub use client::{AiClient, AiError, StreamEvent};
pub use completion::{
    completion_messages, request_completion, summarize_conversation, summary_messages,
};
pub use credentials::{ApiKey, CredentialStore};
pub use provider::{
    AssistantToolCall, AssistantToolFunction, ChatMessage, MessageRole, ProviderConfig,
    ProviderKind, ReasoningEffort, Region, ToolCall, ToolCallKind,
};
pub use settings::{AgentProfile, AppSettings, MAX_AGENT_PROFILES, SettingsStore, normalize_alias};
