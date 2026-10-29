//! Portable project-scoped chat history storage and compaction planning.
//!
//! This crate deliberately has no UI, AI provider, credential, or global
//! configuration integration. The caller supplies both the application data
//! root and each project's canonical path.

mod compaction;
mod error;
mod model;
mod project;
mod store;

pub use compaction::{CompactionPlan, CompactionResult, ContextBudget};
pub use error::HistoryError;
pub use model::{
    ChatMessage, CompactionState, HistoryLimits, HistoryRecord, MessageRole, RetainedContext,
    Summary,
};
pub use project::ProjectId;
pub use store::{ClearOutcome, HistoryStore};

/// The only record version this crate currently reads and writes.
pub const SCHEMA_VERSION: u32 = 1;
