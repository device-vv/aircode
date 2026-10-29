use crate::model::HistoryRecord;
use crate::{ChatMessage, HistoryError, ProjectId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Byte-and-count budget for model input selection.
///
/// Bytes are UTF-8 content bytes, not provider tokens. The AI integrator must
/// leave additional room for its prompt template and message framing.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ContextBudget {
    pub max_messages: usize,
    /// Includes the previous summary and selected visible message content.
    pub max_input_bytes: usize,
}

/// An immutable, revision-bound input snapshot for a later AI integration.
///
/// The selected messages are a contiguous, most-recent suffix. If older
/// messages do not fit, `omitted_older_messages` is nonzero: this is a bounded
/// excerpt, not a summary of those omitted messages. Creating a plan performs
/// no model call and changes no stored data.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CompactionPlan {
    project: ProjectId,
    base_revision: u64,
    previous_summary: Option<String>,
    source_message_count: usize,
    included_messages: Vec<ChatMessage>,
    omitted_older_messages: usize,
    input_bytes: usize,
}

impl CompactionPlan {
    pub(crate) fn build(
        record: &HistoryRecord,
        budget: ContextBudget,
    ) -> Result<Option<Self>, HistoryError> {
        if record.visible_messages().is_empty() {
            return Ok(None);
        }
        if budget.max_messages == 0 || budget.max_input_bytes == 0 {
            return Err(HistoryError::InvalidLimits(
                "compaction budget values must be positive",
            ));
        }

        let previous_summary = record
            .compaction()
            .summary()
            .map(|summary| summary.text().to_owned());
        let summary_bytes = previous_summary.as_ref().map_or(0, String::len);
        if summary_bytes > budget.max_input_bytes {
            return Err(HistoryError::BudgetTooSmall {
                required_bytes: summary_bytes,
                available_bytes: budget.max_input_bytes,
            });
        }

        let mut newest_first = Vec::new();
        let mut input_bytes = summary_bytes;
        for message in record.visible_messages().iter().rev() {
            if newest_first.len() == budget.max_messages {
                break;
            }
            let Some(candidate_bytes) = input_bytes.checked_add(message.content().len()) else {
                break;
            };
            if candidate_bytes > budget.max_input_bytes {
                if newest_first.is_empty() {
                    return Err(HistoryError::BudgetTooSmall {
                        required_bytes: summary_bytes.saturating_add(message.content().len()),
                        available_bytes: budget.max_input_bytes,
                    });
                }
                break;
            }
            input_bytes = candidate_bytes;
            newest_first.push(message.clone());
        }
        newest_first.reverse();
        let omitted_older_messages = record.visible_messages().len() - newest_first.len();
        Ok(Some(Self {
            project: record.project().clone(),
            base_revision: record.revision(),
            previous_summary,
            source_message_count: record.visible_messages().len(),
            included_messages: newest_first,
            omitted_older_messages,
            input_bytes,
        }))
    }

    pub fn project(&self) -> &ProjectId {
        &self.project
    }

    pub fn base_revision(&self) -> u64 {
        self.base_revision
    }

    pub fn previous_summary(&self) -> Option<&str> {
        self.previous_summary.as_deref()
    }

    pub fn source_message_count(&self) -> usize {
        self.source_message_count
    }

    pub fn included_messages(&self) -> &[ChatMessage] {
        &self.included_messages
    }

    /// Number of older visible messages not supplied to the model in this plan.
    pub fn omitted_older_messages(&self) -> usize {
        self.omitted_older_messages
    }

    pub fn is_bounded_excerpt(&self) -> bool {
        self.omitted_older_messages != 0
    }

    pub fn input_bytes(&self) -> usize {
        self.input_bytes
    }

    pub(crate) fn matches_record(&self, record: &HistoryRecord) -> bool {
        if self.project != *record.project()
            || self.base_revision != record.revision()
            || self.source_message_count != record.visible_messages().len()
            || self.included_messages.is_empty()
            || self.included_messages.len() > self.source_message_count
            || self.omitted_older_messages
                != self.source_message_count - self.included_messages.len()
        {
            return false;
        }
        let previous_summary = record.compaction().summary().map(|summary| summary.text());
        if self.previous_summary.as_deref() != previous_summary {
            return false;
        }
        record.visible_messages()[self.omitted_older_messages..] == self.included_messages
    }
}

/// Provider-neutral output to apply to a specific [`CompactionPlan`].
///
/// The caller should construct this only after successful AI generation. On
/// provider failure, simply do not apply a result; the stored record is intact.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CompactionResult {
    summary: String,
    retained_message_ids: Vec<String>,
    metadata: BTreeMap<String, String>,
}

impl CompactionResult {
    pub fn new(
        summary: impl Into<String>,
        retained_message_ids: Vec<String>,
        metadata: BTreeMap<String, String>,
    ) -> Self {
        Self {
            summary: summary.into(),
            retained_message_ids,
            metadata,
        }
    }

    pub fn summary(&self) -> &str {
        &self.summary
    }

    pub fn retained_message_ids(&self) -> &[String] {
        &self.retained_message_ids
    }

    /// Must contain no credentials or secrets.
    pub fn metadata(&self) -> &BTreeMap<String, String> {
        &self.metadata
    }
}
