//! Standalone terminal backend prototype using `portable-pty` and `vt100`.
//!
//! The library owns no UI state and never invents a shell command. Callers
//! provide an executable, arguments, and working directory explicitly.

mod error;
mod grid;
mod input;
mod terminal;

pub use error::TerminalError;
pub use grid::{CellSnapshot, CursorSnapshot, TerminalColor, TerminalSnapshot};
pub use input::{encode_key, Key, KeyEvent, Modifiers};
pub use terminal::{
    ProcessCommand, Terminal, TerminalConfig, TerminalSize, UpdateReceiver,
    DEFAULT_SCROLLBACK_LINES, MAX_INPUT_CHUNK, MAX_SCROLLBACK_LINES,
};
