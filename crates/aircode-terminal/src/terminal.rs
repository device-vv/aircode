use crate::error::TerminalError;
use crate::grid::{Grid, TerminalSnapshot};
use crate::input::{encode_or_error, KeyEvent};
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::Duration;

pub const DEFAULT_SCROLLBACK_LINES: usize = 2_000;
pub const MAX_SCROLLBACK_LINES: usize = 50_000;
pub const MAX_INPUT_CHUNK: usize = 64 * 1024;
const INPUT_QUEUE_CHUNKS: usize = 64;
const MAX_ROWS: u16 = 500;
const MAX_COLUMNS: u16 = 1_000;

/// Coalesced output/lifecycle notifications. Receivers must call `snapshot()`
/// to obtain state; a notification never contains terminal output itself.
pub type UpdateReceiver = Receiver<()>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TerminalSize {
    rows: u16,
    columns: u16,
}

impl TerminalSize {
    pub fn new(rows: u16, columns: u16) -> Result<Self, TerminalError> {
        if rows == 0 || columns == 0 {
            return Err(TerminalError::InvalidSize);
        }
        if rows > MAX_ROWS || columns > MAX_COLUMNS {
            return Err(TerminalError::SizeLimitExceeded);
        }
        Ok(Self { rows, columns })
    }

    pub const fn rows(self) -> u16 {
        self.rows
    }

    pub const fn columns(self) -> u16 {
        self.columns
    }

    fn pty_size(self) -> PtySize {
        PtySize {
            rows: self.rows,
            cols: self.columns,
            pixel_width: 0,
            pixel_height: 0,
        }
    }
}

impl Default for TerminalSize {
    fn default() -> Self {
        Self {
            rows: 24,
            columns: 80,
        }
    }
}

/// An exact executable, argument list, and mandatory working directory.
/// No shell, profile switch, or command is implied by this type.
#[derive(Clone, Debug)]
pub struct ProcessCommand {
    program: OsString,
    args: Vec<OsString>,
    cwd: PathBuf,
    environment: Vec<(OsString, OsString)>,
}

impl ProcessCommand {
    pub fn new(program: impl Into<OsString>, cwd: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            cwd: cwd.into(),
            environment: Vec::new(),
        }
    }

    pub fn arg(mut self, argument: impl Into<OsString>) -> Self {
        self.args.push(argument.into());
        self
    }

    pub fn args<I, S>(mut self, arguments: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        self.args.extend(arguments.into_iter().map(Into::into));
        self
    }

    /// Add or override one inherited environment variable in the child.
    pub fn env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.environment.push((key.into(), value.into()));
        self
    }
}

#[derive(Clone, Debug)]
pub struct TerminalConfig {
    pub command: ProcessCommand,
    pub size: TerminalSize,
    pub scrollback_lines: usize,
}

impl TerminalConfig {
    pub fn new(command: ProcessCommand) -> Self {
        Self {
            command,
            size: TerminalSize::default(),
            scrollback_lines: DEFAULT_SCROLLBACK_LINES,
        }
    }

    pub fn with_size(mut self, size: TerminalSize) -> Self {
        self.size = size;
        self
    }

    pub fn with_scrollback_lines(mut self, scrollback_lines: usize) -> Self {
        self.scrollback_lines = scrollback_lines;
        self
    }
}

struct Shared {
    grid: Mutex<Grid>,
    closing: AtomicBool,
    reader_closed: AtomicBool,
    io_error: Mutex<Option<String>>,
}

impl Shared {
    fn grid(&self) -> MutexGuard<'_, Grid> {
        self.grid.lock().unwrap_or_else(|error| error.into_inner())
    }

    fn record_io_error(&self, operation: &str, error: impl std::fmt::Display) {
        let mut slot = self
            .io_error
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if slot.is_none() {
            *slot = Some(format!("{operation}: {error}"));
        }
    }
}

type SharedMaster = Arc<Mutex<Option<Box<dyn MasterPty + Send>>>>;

struct CleanupRequest {
    result: Option<SyncSender<Result<(), TerminalError>>>,
}

/// A running terminal. Blocking reads, writes, waiting, and process cleanup do
/// not run on the caller's thread, except when the caller explicitly invokes
/// `close`/`close_timeout` and waits for the cleanup result.
pub struct Terminal {
    shared: Arc<Shared>,
    master: SharedMaster,
    input: Option<SyncSender<Vec<u8>>>,
    cleanup: Option<SyncSender<CleanupRequest>>,
    cleanup_thread: Option<JoinHandle<()>>,
    process_id: Option<u32>,
}

impl Terminal {
    /// Spawn only the explicitly configured executable. No input is sent.
    pub fn spawn(config: TerminalConfig) -> Result<(Self, UpdateReceiver), TerminalError> {
        validate_config(&config)?;

        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(config.size.pty_size())
            .map_err(|error| TerminalError::Pty(format!("openpty: {error}")))?;

        let mut command = CommandBuilder::new(&config.command.program);
        for argument in &config.command.args {
            command.arg(argument);
        }
        command.cwd(&config.command.cwd);
        for (key, value) in &config.command.environment {
            command.env(key, value);
        }

        let child = pair.slave.spawn_command(command).map_err(|error| {
            TerminalError::Pty(format!(
                "spawn {}: {error}",
                Path::new(&config.command.program).display()
            ))
        })?;
        let process_id = child.process_id();
        let mut child_guard = ChildGuard(Some(child));

        // The master owns the host side. Dropping the slave is required on
        // Unix and is harmless for ConPTY.
        drop(pair.slave);
        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|error| TerminalError::Pty(format!("clone PTY reader: {error}")))?;
        let mut writer = pair
            .master
            .take_writer()
            .map_err(|error| TerminalError::Pty(format!("take PTY writer: {error}")))?;

        let shared = Arc::new(Shared {
            grid: Mutex::new(Grid::new(config.size, config.scrollback_lines)),
            closing: AtomicBool::new(false),
            reader_closed: AtomicBool::new(false),
            io_error: Mutex::new(None),
        });
        let master: SharedMaster = Arc::new(Mutex::new(Some(pair.master)));
        let (update_sender, update_receiver) = mpsc::sync_channel::<()>(1);
        let (input_sender, input_receiver) = mpsc::sync_channel::<Vec<u8>>(INPUT_QUEUE_CHUNKS);
        let (cleanup_sender, cleanup_receiver) = mpsc::sync_channel::<CleanupRequest>(1);

        let reader_shared = Arc::clone(&shared);
        let reader_updates = update_sender.clone();
        let reader_thread = thread::Builder::new()
            .name("terminal-pty-reader".to_owned())
            .spawn(move || {
                let mut buffer = [0_u8; 16 * 1024];
                loop {
                    match reader.read(&mut buffer) {
                        Ok(0) => break,
                        Ok(count) => {
                            reader_shared.grid().process(&buffer[..count]);
                            notify(&reader_updates);
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                        Err(error) => {
                            if !reader_shared.closing.load(Ordering::Acquire) {
                                reader_shared.record_io_error("PTY read", error);
                            }
                            break;
                        }
                    }
                }
                reader_shared.reader_closed.store(true, Ordering::Release);
                notify(&reader_updates);
            })
            .map_err(|error| TerminalError::Pty(format!("spawn reader thread: {error}")))?;

        let writer_shared = Arc::clone(&shared);
        let writer_updates = update_sender.clone();
        let writer_thread = thread::Builder::new()
            .name("terminal-pty-writer".to_owned())
            .spawn(move || {
                while let Ok(bytes) = input_receiver.recv() {
                    if let Err(error) = writer.write_all(&bytes).and_then(|_| writer.flush()) {
                        if !writer_shared.closing.load(Ordering::Acquire) {
                            writer_shared.record_io_error("PTY write", error);
                        }
                        break;
                    }
                }
                writer_updates.try_send(()).ok();
            })
            .map_err(|error| TerminalError::Pty(format!("spawn writer thread: {error}")))?;

        let cleanup_shared = Arc::clone(&shared);
        let cleanup_master = Arc::clone(&master);
        let cleanup_thread = thread::Builder::new()
            .name("terminal-pty-cleanup".to_owned())
            .spawn(move || {
                let Ok(request) = cleanup_receiver.recv() else {
                    return;
                };
                let result = {
                    let child = child_guard
                        .0
                        .as_mut()
                        .expect("cleanup child guard must contain a child");
                    cleanup_process(
                        &cleanup_shared,
                        &cleanup_master,
                        child,
                        writer_thread,
                        reader_thread,
                    )
                };
                child_guard.0.take();
                notify(&update_sender);
                if let Some(result_sender) = request.result {
                    result_sender.send(result).ok();
                }
            })
            .map_err(|error| TerminalError::Pty(format!("spawn cleanup thread: {error}")))?;

        Ok((
            Self {
                shared,
                master,
                input: Some(input_sender),
                cleanup: Some(cleanup_sender),
                cleanup_thread: Some(cleanup_thread),
                process_id,
            },
            update_receiver,
        ))
    }

    pub fn process_id(&self) -> Option<u32> {
        self.process_id
    }

    /// Queue raw input bytes without performing blocking I/O on the UI thread.
    /// Bytes are copied and writes from one queue chunk are not interleaved.
    pub fn send_input(&self, bytes: &[u8]) -> Result<(), TerminalError> {
        if self.shared.closing.load(Ordering::Acquire)
            || self.shared.reader_closed.load(Ordering::Acquire)
        {
            return Err(TerminalError::Closed);
        }
        if bytes.is_empty() {
            return Ok(());
        }
        if bytes.len() > MAX_INPUT_CHUNK {
            return Err(TerminalError::InputTooLarge {
                size: bytes.len(),
                maximum: MAX_INPUT_CHUNK,
            });
        }

        let sender = self.input.as_ref().ok_or(TerminalError::Closed)?;
        match sender.try_send(bytes.to_vec()) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => Err(TerminalError::InputQueueFull),
            Err(TrySendError::Disconnected(_)) => Err(TerminalError::Closed),
        }
    }

    /// Encode and queue one key using the grid's current DECCKM mode.
    pub fn send_key(&self, event: KeyEvent) -> Result<(), TerminalError> {
        let application_cursor_keys = self.shared.grid().application_cursor_keys();
        let bytes = encode_or_error(event, application_cursor_keys)?;
        self.send_input(&bytes)
    }

    /// Resize both the OS PTY and parser grid.
    pub fn resize(&self, size: TerminalSize) -> Result<(), TerminalError> {
        if self.shared.closing.load(Ordering::Acquire) {
            return Err(TerminalError::Closed);
        }

        // Hold the grid lock so output cannot be parsed between the two size
        // changes. The cleanup thread never acquires grid while holding master.
        let mut grid = self.shared.grid();
        let mut master = self
            .master
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let master = master.as_mut().ok_or(TerminalError::Closed)?;
        master
            .resize(size.pty_size())
            .map_err(|error| TerminalError::Pty(format!("resize PTY: {error}")))?;
        grid.resize(size);
        Ok(())
    }

    pub fn snapshot(&self) -> TerminalSnapshot {
        self.shared.grid().snapshot()
    }

    /// Capture the grid at a number of rows above the live screen. The offset
    /// is clamped to available history and the parser's live viewport is
    /// restored before the grid lock is released.
    pub fn snapshot_at(&self, scrollback_offset: usize) -> TerminalSnapshot {
        self.shared.grid().snapshot_at(scrollback_offset)
    }

    pub fn is_reader_closed(&self) -> bool {
        self.shared.reader_closed.load(Ordering::Acquire)
    }

    pub fn is_closing(&self) -> bool {
        self.shared.closing.load(Ordering::Acquire)
    }

    /// First background I/O error, if any. Normal close-induced errors are
    /// suppressed after cleanup begins.
    pub fn last_io_error(&self) -> Option<String> {
        self.shared
            .io_error
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone()
    }

    /// Terminate if needed, reap the direct child, close the PTY, and join I/O
    /// threads. This waits and therefore should be called off the UI thread.
    pub fn close(self) -> Result<(), TerminalError> {
        self.close_inner(None)
    }

    /// Bounded-wait variant. On timeout, the existing cleanup thread remains
    /// responsible for termination and reaping in the background.
    pub fn close_timeout(self, timeout: Duration) -> Result<(), TerminalError> {
        self.close_inner(Some(timeout))
    }

    fn close_inner(mut self, timeout: Option<Duration>) -> Result<(), TerminalError> {
        self.shared.closing.store(true, Ordering::Release);
        self.input.take();
        let cleanup = self.cleanup.take().ok_or(TerminalError::Closed)?;
        let (result_sender, result_receiver) = mpsc::sync_channel(1);
        cleanup
            .send(CleanupRequest {
                result: Some(result_sender),
            })
            .map_err(|_| TerminalError::Cleanup("cleanup thread disconnected".to_owned()))?;

        let result = match timeout {
            Some(duration) => {
                result_receiver
                    .recv_timeout(duration)
                    .map_err(|error| match error {
                        mpsc::RecvTimeoutError::Timeout => TerminalError::CleanupTimeout,
                        mpsc::RecvTimeoutError::Disconnected => {
                            TerminalError::Cleanup("cleanup thread disconnected".to_owned())
                        }
                    })?
            }
            None => result_receiver
                .recv()
                .map_err(|_| TerminalError::Cleanup("cleanup thread disconnected".to_owned()))?,
        };

        if let Some(handle) = self.cleanup_thread.take() {
            handle
                .join()
                .map_err(|_| TerminalError::Cleanup("cleanup thread panicked".to_owned()))?;
        }
        result
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        self.shared.closing.store(true, Ordering::Release);
        self.input.take();
        if let Some(cleanup) = self.cleanup.take() {
            // The channel has capacity for the only cleanup request. A full or
            // disconnected channel means cleanup was already requested/finished.
            cleanup.try_send(CleanupRequest { result: None }).ok();
        }
        // Deliberately detach: dropping a UI handle must not wait on ConPTY I/O.
        self.cleanup_thread.take();
    }
}

fn cleanup_process(
    shared: &Shared,
    master: &SharedMaster,
    child: &mut Box<dyn Child + Send + Sync>,
    writer_thread: JoinHandle<()>,
    reader_thread: JoinHandle<()>,
) -> Result<(), TerminalError> {
    let mut errors = Vec::new();
    let mut needs_wait = true;

    match child.try_wait() {
        Ok(Some(_)) => needs_wait = false,
        Ok(None) => {
            if let Err(error) = child.kill() {
                // A normal exit can race with kill. Do not report that race as
                // a cleanup failure if the child can now be reaped.
                match child.try_wait() {
                    Ok(Some(_)) => needs_wait = false,
                    _ => errors.push(format!("kill child: {error}")),
                }
            }
        }
        Err(error) => {
            errors.push(format!("query child: {error}"));
            if let Err(kill_error) = child.kill() {
                errors.push(format!("kill child: {kill_error}"));
            }
        }
    }

    if needs_wait {
        if let Err(error) = child.wait() {
            errors.push(format!("wait for child: {error}"));
        }
    }

    master
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .take();

    if writer_thread.join().is_err() {
        errors.push("writer thread panicked".to_owned());
    }
    if reader_thread.join().is_err() {
        errors.push("reader thread panicked".to_owned());
    }
    shared.reader_closed.store(true, Ordering::Release);

    if errors.is_empty() {
        Ok(())
    } else {
        Err(TerminalError::Cleanup(errors.join("; ")))
    }
}

fn notify(sender: &SyncSender<()>) {
    // Capacity one is a dirty flag: a pending wake-up always observes the
    // latest snapshot, so dropping duplicate notifications cannot lose output.
    sender.try_send(()).ok();
}

/// Reaps a child on every partially initialized spawn path. The cleanup thread
/// takes the child out only after its normal cleanup has completed.
struct ChildGuard(Option<Box<dyn Child + Send + Sync>>);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let Some(child) = self.0.as_mut() else {
            return;
        };
        match child.try_wait() {
            Ok(Some(_)) => {}
            _ => {
                child.kill().ok();
                child.wait().ok();
            }
        }
    }
}

fn validate_config(config: &TerminalConfig) -> Result<(), TerminalError> {
    if config.command.program.is_empty() {
        return Err(TerminalError::EmptyProgram);
    }
    if config.scrollback_lines > MAX_SCROLLBACK_LINES {
        return Err(TerminalError::ScrollbackLimitExceeded);
    }

    let metadata = std::fs::metadata(&config.command.cwd).map_err(|error| {
        TerminalError::InvalidWorkingDirectory {
            path: config.command.cwd.clone(),
            reason: error.to_string(),
        }
    })?;
    if !metadata.is_dir() {
        return Err(TerminalError::InvalidWorkingDirectory {
            path: config.command.cwd.clone(),
            reason: "path is not a directory".to_owned(),
        });
    }

    // Validate again in case TerminalSize gains unchecked constructors later.
    TerminalSize::new(config.size.rows, config.size.columns)?;
    Ok(())
}
