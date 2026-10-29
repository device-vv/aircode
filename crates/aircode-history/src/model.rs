use crate::{HistoryError, ProjectId, SCHEMA_VERSION};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Hard limits enforced on both loaded and saved records.
///
/// Exceeding a limit is an error. The store never drops or truncates visible
/// messages to make a record fit.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct HistoryLimits {
    pub max_visible_messages: usize,
    pub max_message_id_bytes: usize,
    pub max_message_bytes: usize,
    pub max_visible_content_bytes: usize,
    pub max_draft_bytes: usize,
    pub max_summary_bytes: usize,
    pub max_retained_messages: usize,
    pub max_metadata_entries: usize,
    pub max_metadata_key_bytes: usize,
    pub max_metadata_value_bytes: usize,
    pub max_record_bytes: usize,
}

impl Default for HistoryLimits {
    fn default() -> Self {
        Self {
            max_visible_messages: 4_096,
            max_message_id_bytes: 256,
            max_message_bytes: 1024 * 1024,
            max_visible_content_bytes: 8 * 1024 * 1024,
            max_draft_bytes: 1024 * 1024,
            max_summary_bytes: 1024 * 1024,
            max_retained_messages: 64,
            max_metadata_entries: 64,
            max_metadata_key_bytes: 128,
            max_metadata_value_bytes: 4 * 1024,
            max_record_bytes: 16 * 1024 * 1024,
        }
    }
}

impl HistoryLimits {
    pub(crate) fn validate(&self) -> Result<(), HistoryError> {
        let required_positive = [
            (
                self.max_visible_messages,
                "max_visible_messages must be positive",
            ),
            (
                self.max_message_id_bytes,
                "max_message_id_bytes must be positive",
            ),
            (self.max_message_bytes, "max_message_bytes must be positive"),
            (
                self.max_visible_content_bytes,
                "max_visible_content_bytes must be positive",
            ),
            (self.max_summary_bytes, "max_summary_bytes must be positive"),
            (self.max_record_bytes, "max_record_bytes must be positive"),
        ];
        for (value, message) in required_positive {
            if value == 0 {
                return Err(HistoryError::InvalidLimits(message));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    System,
    User,
    Assistant,
    Tool,
}

/// A provider-neutral visible chat message.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ChatMessage {
    id: String,
    role: MessageRole,
    content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    created_at_unix_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    state: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    reasoning: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

impl ChatMessage {
    /// `id` must be non-empty and unique within a record. It is used to retain
    /// exact messages as context without duplicating them in the summary.
    pub fn new(id: impl Into<String>, role: MessageRole, content: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            role,
            content: content.into(),
            created_at_unix_ms: None,
            state: String::new(),
            reasoning: String::new(),
            error: None,
        }
    }

    pub fn with_created_at_unix_ms(mut self, timestamp: u64) -> Self {
        self.created_at_unix_ms = Some(timestamp);
        self
    }

    pub fn with_display_state(
        mut self,
        state: impl Into<String>,
        reasoning: impl Into<String>,
        error: Option<String>,
    ) -> Self {
        self.state = state.into();
        self.reasoning = reasoning.into();
        self.error = error;
        self
    }

    pub fn state(&self) -> &str {
        &self.state
    }
    pub fn reasoning(&self) -> &str {
        &self.reasoning
    }
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn role(&self) -> MessageRole {
        self.role
    }

    pub fn content(&self) -> &str {
        &self.content
    }

    pub fn created_at_unix_ms(&self) -> Option<u64> {
        self.created_at_unix_ms
    }
}

/// Provenance for an AI-produced summary.
///
/// `omitted_older_messages` makes partial model input explicit. A compacted
/// record always retains the complete `visible_messages` collection.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Summary {
    text: String,
    based_on_revision: u64,
    source_message_count: usize,
    included_message_count: usize,
    omitted_older_messages: usize,
}

impl Summary {
    pub(crate) fn new(
        text: String,
        based_on_revision: u64,
        source_message_count: usize,
        included_message_count: usize,
        omitted_older_messages: usize,
    ) -> Self {
        Self {
            text,
            based_on_revision,
            source_message_count,
            included_message_count,
            omitted_older_messages,
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn based_on_revision(&self) -> u64 {
        self.based_on_revision
    }

    pub fn source_message_count(&self) -> usize {
        self.source_message_count
    }

    pub fn included_message_count(&self) -> usize {
        self.included_message_count
    }

    pub fn omitted_older_messages(&self) -> usize {
        self.omitted_older_messages
    }
}

/// IDs of exact visible messages to send alongside the summary on later turns.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct RetainedContext {
    message_ids: Vec<String>,
}

impl RetainedContext {
    pub fn message_ids(&self) -> &[String] {
        &self.message_ids
    }
}

/// Summary, exact retained context, and caller-defined non-secret metadata.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct CompactionState {
    summary: Option<Summary>,
    retained_context: RetainedContext,
    metadata: BTreeMap<String, String>,
}

impl CompactionState {
    pub fn summary(&self) -> Option<&Summary> {
        self.summary.as_ref()
    }

    pub fn retained_context(&self) -> &RetainedContext {
        &self.retained_context
    }

    /// Intended for provider/model names and prompt versions. Never place API
    /// keys, access tokens, or other secrets in this map.
    pub fn metadata(&self) -> &BTreeMap<String, String> {
        &self.metadata
    }
}

/// Versioned persistent state for exactly one project.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct HistoryRecord {
    schema_version: u32,
    project: ProjectId,
    revision: u64,
    visible_messages: Vec<ChatMessage>,
    compaction: CompactionState,
    input_draft: String,
}

impl HistoryRecord {
    /// Creates an unsaved revision-zero record.
    pub fn new(project: ProjectId) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            project,
            revision: 0,
            visible_messages: Vec::new(),
            compaction: CompactionState::default(),
            input_draft: String::new(),
        }
    }

    pub fn schema_version(&self) -> u32 {
        self.schema_version
    }

    pub fn project(&self) -> &ProjectId {
        &self.project
    }

    /// Zero means unsaved. Every successful save or summary application
    /// increments this value by one.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn visible_messages(&self) -> &[ChatMessage] {
        &self.visible_messages
    }

    pub fn compaction(&self) -> &CompactionState {
        &self.compaction
    }

    pub fn input_draft(&self) -> &str {
        &self.input_draft
    }

    pub fn set_input_draft(&mut self, draft: impl Into<String>) {
        self.input_draft = draft.into();
    }

    /// Replaces visible messages without changing their order. Retained IDs
    /// that no longer exist are removed; visible history is never truncated.
    pub fn set_visible_messages(&mut self, messages: Vec<ChatMessage>) {
        self.visible_messages = messages;
        self.prune_retained_context();
    }

    pub fn replace_contents(&mut self, messages: Vec<ChatMessage>, draft: impl Into<String>) {
        self.visible_messages = messages;
        self.input_draft = draft.into();
        self.prune_retained_context();
    }

    fn prune_retained_context(&mut self) {
        let visible_ids: BTreeSet<&str> = self
            .visible_messages
            .iter()
            .map(|message| message.id.as_str())
            .collect();
        self.compaction
            .retained_context
            .message_ids
            .retain(|id| visible_ids.contains(id.as_str()));
    }

    pub(crate) fn next_revision(&self) -> Result<u64, HistoryError> {
        self.revision
            .checked_add(1)
            .ok_or(HistoryError::RevisionOverflow)
    }

    pub(crate) fn set_revision(&mut self, revision: u64) {
        self.revision = revision;
    }

    pub(crate) fn replace_compaction(
        &mut self,
        summary: Summary,
        retained_message_ids: Vec<String>,
        metadata: BTreeMap<String, String>,
    ) {
        self.compaction = CompactionState {
            summary: Some(summary),
            retained_context: RetainedContext {
                message_ids: retained_message_ids,
            },
            metadata,
        };
    }

    pub(crate) fn validate(&self, limits: &HistoryLimits) -> Result<(), HistoryError> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(HistoryError::UnsupportedSchemaVersion {
                found: self.schema_version,
            });
        }
        self.project.validate()?;
        check_limit(
            "visible message count",
            self.visible_messages.len(),
            limits.max_visible_messages,
        )?;
        check_limit(
            "input draft",
            self.input_draft.len(),
            limits.max_draft_bytes,
        )?;

        let mut visible_ids = BTreeSet::new();
        let mut visible_content_bytes = 0usize;
        for message in &self.visible_messages {
            if message.id.is_empty() {
                return Err(HistoryError::InvalidRecord(
                    "visible message ID must not be empty".into(),
                ));
            }
            check_limit(
                "visible message ID",
                message.id.len(),
                limits.max_message_id_bytes,
            )?;
            check_limit(
                "visible message content",
                message.content.len(),
                limits.max_message_bytes,
            )?;
            check_limit("message state", message.state.len(), 64)?;
            check_limit(
                "message reasoning",
                message.reasoning.len(),
                limits.max_message_bytes,
            )?;
            check_limit(
                "message error",
                message.error.as_ref().map_or(0, String::len),
                limits.max_message_bytes,
            )?;
            visible_content_bytes = visible_content_bytes
                .saturating_add(message.content.len())
                .saturating_add(message.reasoning.len())
                .saturating_add(message.error.as_ref().map_or(0, String::len));
            if !visible_ids.insert(message.id.as_str()) {
                return Err(HistoryError::InvalidRecord(format!(
                    "duplicate visible message ID: {}",
                    message.id
                )));
            }
        }
        check_limit(
            "visible message content bytes",
            visible_content_bytes,
            limits.max_visible_content_bytes,
        )?;

        if let Some(summary) = &self.compaction.summary {
            if summary.text.trim().is_empty() {
                return Err(HistoryError::InvalidRecord(
                    "summary must not be empty".into(),
                ));
            }
            check_limit("summary", summary.text.len(), limits.max_summary_bytes)?;
            if summary.based_on_revision == 0 || summary.based_on_revision > self.revision {
                return Err(HistoryError::InvalidRecord(
                    "summary base revision is outside the record history".into(),
                ));
            }
            if summary.source_message_count == 0
                || summary.included_message_count == 0
                || summary.omitted_older_messages > summary.source_message_count
                || summary.included_message_count
                    != summary.source_message_count - summary.omitted_older_messages
            {
                return Err(HistoryError::InvalidRecord(
                    "summary message coverage counts are inconsistent".into(),
                ));
            }
        } else if !self.compaction.retained_context.message_ids.is_empty()
            || !self.compaction.metadata.is_empty()
        {
            return Err(HistoryError::InvalidRecord(
                "retained context and metadata require a summary".into(),
            ));
        }

        let retained_ids = &self.compaction.retained_context.message_ids;
        check_limit(
            "retained message count",
            retained_ids.len(),
            limits.max_retained_messages,
        )?;
        let mut unique_retained = BTreeSet::new();
        for id in retained_ids {
            if !visible_ids.contains(id.as_str()) {
                return Err(HistoryError::InvalidRecord(format!(
                    "retained message ID is not visible: {id}"
                )));
            }
            if !unique_retained.insert(id.as_str()) {
                return Err(HistoryError::InvalidRecord(format!(
                    "duplicate retained message ID: {id}"
                )));
            }
        }
        validate_metadata(&self.compaction.metadata, limits)?;
        Ok(())
    }
}

pub(crate) fn validate_metadata(
    metadata: &BTreeMap<String, String>,
    limits: &HistoryLimits,
) -> Result<(), HistoryError> {
    check_limit(
        "compaction metadata entries",
        metadata.len(),
        limits.max_metadata_entries,
    )?;
    for (key, value) in metadata {
        if key.is_empty() {
            return Err(HistoryError::InvalidRecord(
                "compaction metadata key must not be empty".into(),
            ));
        }
        check_limit(
            "compaction metadata key",
            key.len(),
            limits.max_metadata_key_bytes,
        )?;
        check_limit(
            "compaction metadata value",
            value.len(),
            limits.max_metadata_value_bytes,
        )?;
    }
    Ok(())
}

pub(crate) fn check_limit(
    resource: &'static str,
    actual: usize,
    limit: usize,
) -> Result<(), HistoryError> {
    if actual > limit {
        Err(HistoryError::LimitExceeded {
            resource,
            actual,
            limit,
        })
    } else {
        Ok(())
    }
}
