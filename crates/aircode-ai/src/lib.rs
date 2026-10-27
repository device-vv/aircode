mod client;
mod credentials;
mod provider;
mod settings;
mod sse;

pub use client::{AiClient, AiError, StreamEvent};
pub use credentials::{ApiKey, CredentialStore};
pub use provider::{
    ChatMessage, MessageRole, ProviderConfig, ProviderKind, ReasoningEffort, Region,
};
pub use settings::{AppSettings, SettingsStore};
