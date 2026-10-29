use std::fmt;
use std::io;
use std::path::PathBuf;

/// Errors produced by project history storage and compaction.
#[derive(Debug)]
pub enum HistoryError {
    InvalidProjectPath(String),
    NonUtf8ProjectPath,
    InvalidLimits(&'static str),
    InvalidRecord(String),
    UnsupportedSchemaVersion {
        found: u32,
    },
    LimitExceeded {
        resource: &'static str,
        actual: usize,
        limit: usize,
    },
    RevisionConflict {
        expected: u64,
        actual: Option<u64>,
    },
    StaleCompaction {
        base_revision: u64,
        current_revision: Option<u64>,
    },
    ProjectMismatch,
    BudgetTooSmall {
        required_bytes: usize,
        available_bytes: usize,
    },
    CorruptRecord {
        path: PathBuf,
        reason: String,
    },
    Serialization(serde_json::Error),
    Io {
        operation: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    RevisionOverflow,
    LockPoisoned,
}

impl fmt::Display for HistoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidProjectPath(reason) => write!(f, "invalid project path: {reason}"),
            Self::NonUtf8ProjectPath => {
                write!(f, "project path is not valid UTF-8; portable IDs require UTF-8")
            }
            Self::InvalidLimits(reason) => write!(f, "invalid history limits: {reason}"),
            Self::InvalidRecord(reason) => write!(f, "invalid history record: {reason}"),
            Self::UnsupportedSchemaVersion { found } => {
                write!(f, "unsupported history schema version {found}")
            }
            Self::LimitExceeded {
                resource,
                actual,
                limit,
            } => write!(
                f,
                "{resource} exceeds its limit: {actual} bytes/items > {limit}"
            ),
            Self::RevisionConflict { expected, actual } => write!(
                f,
                "history revision conflict: snapshot is {expected}, stored revision is {actual:?}"
            ),
            Self::StaleCompaction {
                base_revision,
                current_revision,
            } => write!(
                f,
                "stale compaction: base revision is {base_revision}, stored revision is {current_revision:?}"
            ),
            Self::ProjectMismatch => write!(f, "project identity does not match the operation"),
            Self::BudgetTooSmall {
                required_bytes,
                available_bytes,
            } => write!(
                f,
                "compaction budget is too small: needs at least {required_bytes} bytes, has {available_bytes}"
            ),
            Self::CorruptRecord { path, reason } => {
                write!(f, "corrupt history record {}: {reason}", path.display())
            }
            Self::Serialization(error) => write!(f, "could not serialize history: {error}"),
            Self::Io {
                operation,
                path,
                source,
            } => write!(f, "could not {operation} {}: {source}", path.display()),
            Self::RevisionOverflow => write!(f, "history revision counter exhausted"),
            Self::LockPoisoned => write!(f, "process-local history lock is poisoned"),
        }
    }
}

impl std::error::Error for HistoryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Serialization(error) => Some(error),
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}
