use std::fmt;
use std::path::PathBuf;

/// Errors reported by the terminal API.
#[derive(Debug)]
pub enum TerminalError {
    InvalidSize,
    SizeLimitExceeded,
    ScrollbackLimitExceeded,
    EmptyProgram,
    InvalidWorkingDirectory { path: PathBuf, reason: String },
    Pty(String),
    Closed,
    InputQueueFull,
    InputTooLarge { size: usize, maximum: usize },
    UnsupportedKey,
    Cleanup(String),
    CleanupTimeout,
}

impl fmt::Display for TerminalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSize => write!(f, "terminal rows and columns must be non-zero"),
            Self::SizeLimitExceeded => write!(f, "terminal size exceeds the prototype limit"),
            Self::ScrollbackLimitExceeded => {
                write!(f, "scrollback exceeds the prototype limit")
            }
            Self::EmptyProgram => write!(f, "terminal program must not be empty"),
            Self::InvalidWorkingDirectory { path, reason } => write!(
                f,
                "invalid terminal working directory {}: {reason}",
                path.display()
            ),
            Self::Pty(message) => write!(f, "PTY error: {message}"),
            Self::Closed => write!(f, "terminal is closed"),
            Self::InputQueueFull => write!(f, "terminal input queue is full"),
            Self::InputTooLarge { size, maximum } => {
                write!(f, "input chunk has {size} bytes; the maximum is {maximum}")
            }
            Self::UnsupportedKey => write!(f, "key cannot be encoded for a terminal"),
            Self::Cleanup(message) => write!(f, "terminal cleanup failed: {message}"),
            Self::CleanupTimeout => write!(
                f,
                "terminal cleanup timed out; the cleanup thread remains responsible for reaping it"
            ),
        }
    }
}

impl std::error::Error for TerminalError {}
