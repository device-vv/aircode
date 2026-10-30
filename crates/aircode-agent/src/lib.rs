use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Component, Path, PathBuf},
    process::Stdio,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use serde_json::{Value, json};
use similar::TextDiff;
use tokio::{
    io::AsyncReadExt,
    process::{Child, Command},
};

const DISCOVERY_LIMIT: usize = 500;
const SEARCH_MATCH_LIMIT: usize = 100;
const DEFAULT_COMMAND_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_COMMAND_TIMEOUT_SECS: u64 = 600;
const MAX_COMMAND_OUTPUT_BYTES: usize = 200_000;
const MAX_WRITE_BYTES: usize = 1_000_000;
const OUTPUT_DRAIN_GRACE: Duration = Duration::from_millis(500);
const CANCELLED_ACTION: &str = "Action cancelled by user";
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PermissionMode {
    AskEvery,
    AskImportant,
    FullAccess,
}

impl PermissionMode {
    pub fn from_code(code: &str) -> Self {
        match code {
            "ask_every" => Self::AskEvery,
            "full_access" => Self::FullAccess,
            _ => Self::AskImportant,
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Self::AskEvery => "ask_every",
            Self::AskImportant => "ask_important",
            Self::FullAccess => "full_access",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::AskEvery => Self::AskImportant,
            Self::AskImportant => Self::FullAccess,
            Self::FullAccess => Self::AskEvery,
        }
    }

    pub fn requires_approval(self, action: &AgentAction) -> bool {
        match self {
            Self::AskEvery => true,
            Self::AskImportant => !action.is_read_only(),
            Self::FullAccess => false,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentAction {
    pub name: String,
    pub arguments: String,
}

impl AgentAction {
    pub fn is_read_only(&self) -> bool {
        matches!(
            self.name.as_str(),
            "list_files" | "read_file" | "search_text"
        )
    }
}

pub struct AgentExecutor {
    root: PathBuf,
}

impl AgentExecutor {
    pub fn new(root: impl AsRef<Path>) -> Result<Self, String> {
        let root = root
            .as_ref()
            .canonicalize()
            .map_err(|error| error.to_string())?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub async fn execute(&self, action: &AgentAction) -> Result<String, String> {
        self.execute_with_cancel(action, &tokio_util::sync::CancellationToken::new())
            .await
    }

    pub async fn execute_with_cancel(
        &self,
        action: &AgentAction,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> Result<String, String> {
        if cancel.is_cancelled() {
            return Err(CANCELLED_ACTION.into());
        }
        match action.name.as_str() {
            "list_files" => {
                let path = self.resolve(&action_path(&action.arguments, "path", ".")?, false)?;
                self.list_files(&path)
            }
            "read_file" => {
                let path = self.resolve(&action_path(&action.arguments, "path", "")?, false)?;
                let text = fs::read_to_string(&path).map_err(|error| error.to_string())?;
                Ok(truncate(&text, 200_000))
            }
            "search_text" => {
                let query = action_string(&action.arguments, "query")?;
                let base = self.resolve(&action_path(&action.arguments, "path", ".")?, false)?;
                self.search(&base, &query)
            }
            "write_file" => {
                let path = self.resolve(&action_path(&action.arguments, "path", "")?, true)?;
                let content = action_string(&action.arguments, "content")?;
                if content.len() > MAX_WRITE_BYTES {
                    return Err("write_file content exceeds 1 MB".into());
                }
                if cancel.is_cancelled() {
                    return Err(CANCELLED_ACTION.into());
                }
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
                }
                self.atomic_write(&path, content.as_bytes(), cancel)?;
                Ok(format!(
                    "Wrote {} bytes to {}",
                    content.len(),
                    self.relative(&path)
                ))
            }
            "create_directory" => {
                let path = self.resolve(&action_path(&action.arguments, "path", "")?, true)?;
                if cancel.is_cancelled() {
                    return Err(CANCELLED_ACTION.into());
                }
                fs::create_dir_all(&path).map_err(|error| error.to_string())?;
                Ok(format!("Created directory {}", self.relative(&path)))
            }
            "move_file" => {
                let from = self.resolve(&action_path(&action.arguments, "from", "")?, false)?;
                let to = self.resolve(&action_path(&action.arguments, "to", "")?, true)?;
                if cancel.is_cancelled() {
                    return Err(CANCELLED_ACTION.into());
                }
                fs::rename(&from, &to).map_err(|error| error.to_string())?;
                Ok(format!(
                    "Moved {} to {}",
                    self.relative(&from),
                    self.relative(&to)
                ))
            }
            "delete_file" => {
                let path = self.resolve(&action_path(&action.arguments, "path", "")?, false)?;
                if !path.is_file() {
                    return Err("Only files can be deleted".into());
                }
                if cancel.is_cancelled() {
                    return Err(CANCELLED_ACTION.into());
                }
                fs::remove_file(&path).map_err(|error| error.to_string())?;
                Ok(format!("Deleted {}", self.relative(&path)))
            }
            "run_command" => self.run_command(&action.arguments, cancel).await,
            _ => Err(format!("Unknown tool: {}", action.name)),
        }
    }

    pub fn diff(&self, path: &str, new_content: &str) -> Result<String, String> {
        let path = self.resolve(path, true)?;
        let old = fs::read_to_string(&path).unwrap_or_default();
        Ok(truncate(
            &TextDiff::from_lines(&old, &new_content.to_string())
                .unified_diff()
                .to_string(),
            50_000,
        ))
    }

    // Path validation for file tools, not an OS sandbox. Commands still run
    // with the permissions of the user running AIRcode.
    fn resolve(&self, value: &str, create: bool) -> Result<PathBuf, String> {
        let requested = Path::new(value);
        if requested
            .components()
            .any(|component| matches!(component, Component::ParentDir))
        {
            return Err("Path traversal is not allowed".into());
        }
        let candidate = if requested.is_absolute() {
            requested.to_path_buf()
        } else {
            self.root.join(requested)
        };
        #[cfg(windows)]
        let candidate = {
            let normal_root = windows_shell_path(&self.root)?;
            let normal_path = windows_shell_path(&candidate)?;
            match normal_path.strip_prefix(&normal_root) {
                Ok(relative) => self.root.join(relative),
                Err(_) => candidate,
            }
        };
        self.ensure_inside(&candidate)?;
        self.check_protected_relative(&candidate)?;
        let resolved = if create {
            self.resolve_for_create(&candidate)?
        } else {
            let resolved = candidate
                .canonicalize()
                .map_err(|error| error.to_string())?;
            self.ensure_inside(&resolved)?;
            self.check_protected_relative(&resolved)?;
            resolved
        };
        self.ensure_inside(&resolved)?;
        self.check_protected_relative(&resolved)?;
        Ok(resolved)
    }

    fn resolve_for_create(&self, candidate: &Path) -> Result<PathBuf, String> {
        let mut existing = candidate.to_path_buf();
        let mut missing = Vec::new();
        loop {
            match fs::symlink_metadata(&existing) {
                Ok(metadata) => {
                    if metadata.file_type().is_symlink() {
                        return Err("Access through symbolic links is not allowed".into());
                    }
                    if !missing.is_empty() && !metadata.is_dir() {
                        return Err("A path component is not a directory".into());
                    }
                    let canonical = existing.canonicalize().map_err(|error| error.to_string())?;
                    self.ensure_inside(&canonical)?;
                    self.check_protected_relative(&canonical)?;
                    let mut resolved = canonical;
                    for component in missing.iter().rev() {
                        resolved.push(component);
                    }
                    self.ensure_inside(&resolved)?;
                    self.check_protected_relative(&resolved)?;
                    return Ok(resolved);
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    let parent = existing.parent().ok_or("Invalid target path")?;
                    let name = existing.file_name().ok_or("Invalid file name")?;
                    missing.push(name.to_os_string());
                    existing = parent.to_path_buf();
                }
                Err(error) => return Err(error.to_string()),
            }
        }
    }

    fn ensure_inside(&self, path: &Path) -> Result<(), String> {
        if path.starts_with(&self.root) {
            Ok(())
        } else {
            Err("Path is outside the project sandbox".into())
        }
    }

    fn check_protected_relative(&self, path: &Path) -> Result<(), String> {
        let relative = path
            .strip_prefix(&self.root)
            .map_err(|_| "Path is outside the project sandbox")?;
        for component in relative.components() {
            let Component::Normal(part) = component else {
                continue;
            };
            let part = part.to_string_lossy();
            if protected(&part) {
                return Err(format!("Protected path is not accessible: {part}"));
            }
        }
        Ok(())
    }

    fn atomic_write(
        &self,
        path: &Path,
        content: &[u8],
        cancel: &tokio_util::sync::CancellationToken,
    ) -> Result<(), String> {
        if content.len() > MAX_WRITE_BYTES {
            return Err("write_file content exceeds 1 MB".into());
        }
        if cancel.is_cancelled() {
            return Err(CANCELLED_ACTION.into());
        }
        let parent = path.parent().ok_or("Invalid target path")?;
        let (mut file, temporary) = create_temporary_file(parent)?;
        let write_result = file.write_all(content).and_then(|_| file.flush());
        drop(file);
        if let Err(error) = write_result {
            let _ = fs::remove_file(&temporary);
            return Err(error.to_string());
        }
        if cancel.is_cancelled() {
            let _ = fs::remove_file(&temporary);
            return Err(CANCELLED_ACTION.into());
        }
        if let Err(error) = self.recheck_replace_target(path) {
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }
        if cancel.is_cancelled() {
            let _ = fs::remove_file(&temporary);
            return Err(CANCELLED_ACTION.into());
        }
        if let Err(error) = fs::rename(&temporary, path) {
            let _ = fs::remove_file(&temporary);
            return Err(error.to_string());
        }
        Ok(())
    }

    fn recheck_replace_target(&self, path: &Path) -> Result<(), String> {
        let parent = path.parent().ok_or("Invalid target path")?;
        let canonical_parent = parent.canonicalize().map_err(|error| error.to_string())?;
        self.ensure_inside(&canonical_parent)?;
        match fs::symlink_metadata(path) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() {
                    return Err("Refusing to replace a symbolic link".into());
                }
                let canonical = path.canonicalize().map_err(|error| error.to_string())?;
                self.ensure_inside(&canonical)?;
                self.check_protected_relative(&canonical)?;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let resolved = canonical_parent.join(path.file_name().ok_or("Invalid file name")?);
                self.ensure_inside(&resolved)?;
                self.check_protected_relative(&resolved)?;
            }
            Err(error) => return Err(error.to_string()),
        }
        Ok(())
    }

    fn relative(&self, path: &Path) -> String {
        path.strip_prefix(&self.root)
            .unwrap_or(path)
            .display()
            .to_string()
    }

    fn list_files(&self, base: &Path) -> Result<String, String> {
        let mut paths = Vec::new();
        self.walk(base, &mut paths);
        let truncated = paths.len() > DISCOVERY_LIMIT;
        let mut output: Vec<_> = paths.iter().map(|path| self.relative(path)).collect();
        output.sort();
        output.truncate(DISCOVERY_LIMIT);
        if truncated {
            output.push(format!(
                "... [file list truncated after {DISCOVERY_LIMIT} entries; project inspection is incomplete]"
            ));
        }
        Ok(output.join("\n"))
    }

    fn walk(&self, base: &Path, output: &mut Vec<PathBuf>) {
        if output.len() > DISCOVERY_LIMIT {
            return;
        }
        if base.is_file() {
            output.push(base.to_path_buf());
            return;
        }
        let Ok(entries) = fs::read_dir(base) else {
            return;
        };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if protected(&name) || kind.is_symlink() {
                continue;
            }
            output.push(path.clone());
            if output.len() > DISCOVERY_LIMIT {
                break;
            }
            if kind.is_dir() {
                self.walk(&path, output);
            }
        }
    }

    fn search(&self, base: &Path, query: &str) -> Result<String, String> {
        if query.is_empty() {
            return Err("Search query cannot be empty".into());
        }
        let mut files = Vec::new();
        self.walk(base, &mut files);
        let files_truncated = files.len() > DISCOVERY_LIMIT;
        let mut matches = Vec::new();
        for path in files.into_iter().take(DISCOVERY_LIMIT) {
            if !path.is_file() {
                continue;
            }
            let Ok(text) = fs::read_to_string(&path) else {
                continue;
            };
            for (index, line) in text.lines().enumerate() {
                if line.contains(query) {
                    matches.push(format!(
                        "{}:{}: {}",
                        self.relative(&path),
                        index + 1,
                        line.trim()
                    ));
                    if matches.len() >= SEARCH_MATCH_LIMIT {
                        matches.push(format!(
                            "... [search truncated after {SEARCH_MATCH_LIMIT} matches; project inspection is incomplete]"
                        ));
                        return Ok(matches.join("\n"));
                    }
                }
            }
        }
        if files_truncated {
            matches.push(format!(
                "... [search covered only the first {DISCOVERY_LIMIT} discovered files; project inspection is incomplete]"
            ));
        }
        Ok(matches.join("\n"))
    }

    async fn run_command(
        &self,
        arguments: &str,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> Result<String, String> {
        let command = action_string(arguments, "command")?;
        if command.trim().is_empty() {
            return Err("Command cannot be empty".into());
        }
        let timeout = command_timeout(arguments)?;
        let (mut process, cwd) = shell_command(&command, &self.root)?;
        process
            .current_dir(&cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(windows)]
        configure_hidden(&mut process);
        #[cfg(unix)]
        configure_process_group(&mut process);
        let mut child = process.spawn().map_err(|error| error.to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or("Could not capture command stdout")?;
        let stderr = child
            .stderr
            .take()
            .ok_or("Could not capture command stderr")?;
        let mut stdout_task = tokio::spawn(drain_output(stdout));
        let mut stderr_task = tokio::spawn(drain_output(stderr));

        let outcome = tokio::select! {
            () = cancel.cancelled() => CommandOutcome::Cancelled,
            _ = tokio::time::sleep(timeout) => CommandOutcome::TimedOut,
            status = child.wait() => CommandOutcome::Exited(status.map_err(|error| error.to_string())?),
        };
        if !matches!(outcome, CommandOutcome::Exited(_)) {
            terminate_process_tree(&mut child).await;
        }
        let stdout = finish_drain(&mut stdout_task).await;
        let stderr = finish_drain(&mut stderr_task).await;
        match outcome {
            CommandOutcome::Cancelled => Err("Command cancelled by user".into()),
            CommandOutcome::TimedOut => Err(format!(
                "Command timed out after {} seconds. The process tree was stopped.{}",
                timeout.as_secs(),
                partial_output(&stdout, &stderr)
            )),
            CommandOutcome::Exited(status) => Ok(truncate(
                &format!(
                    "stdout:\n{}\nstderr:\n{}\nexit status: {status}{}",
                    stdout.text(),
                    stderr.text(),
                    if stdout.truncated || stderr.truncated {
                        "\n[command output was truncated; the process continued running and its remaining output was drained]"
                    } else {
                        ""
                    }
                ),
                MAX_COMMAND_OUTPUT_BYTES,
            )),
        }
    }
}

enum CommandOutcome {
    Exited(std::process::ExitStatus),
    Cancelled,
    TimedOut,
}

#[derive(Default)]
struct CapturedOutput {
    bytes: Vec<u8>,
    total: usize,
    truncated: bool,
}

impl CapturedOutput {
    fn push(&mut self, bytes: &[u8]) {
        self.total = self.total.saturating_add(bytes.len());
        let remaining = MAX_COMMAND_OUTPUT_BYTES.saturating_sub(self.bytes.len());
        if remaining > 0 {
            self.bytes
                .extend_from_slice(&bytes[..bytes.len().min(remaining)]);
        }
        if bytes.len() > remaining {
            self.truncated = true;
        }
    }

    fn text(&self) -> String {
        let mut text = String::from_utf8_lossy(&self.bytes).into_owned();
        if self.truncated {
            text.push_str(&format!(
                "\n...[output truncated: {} of {} bytes captured]",
                self.bytes.len(),
                self.total
            ));
        }
        text
    }
}

async fn drain_output(mut reader: impl tokio::io::AsyncRead + Unpin) -> CapturedOutput {
    let mut output = CapturedOutput::default();
    let mut buffer = [0u8; 8192];
    loop {
        match reader.read(&mut buffer).await {
            Ok(0) => break,
            Ok(count) => output.push(&buffer[..count]),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
    output
}

async fn finish_drain(task: &mut tokio::task::JoinHandle<CapturedOutput>) -> CapturedOutput {
    match tokio::time::timeout(OUTPUT_DRAIN_GRACE, &mut *task).await {
        Ok(Ok(output)) => output,
        _ => {
            task.abort();
            CapturedOutput::default()
        }
    }
}

fn partial_output(stdout: &CapturedOutput, stderr: &CapturedOutput) -> String {
    let combined = format!("stdout:\n{}\nstderr:\n{}", stdout.text(), stderr.text());
    if combined.trim().is_empty() {
        String::new()
    } else {
        format!("\nPartial output:\n{}", truncate(&combined, 8_000))
    }
}

fn command_timeout(arguments: &str) -> Result<Duration, String> {
    let value: Value = serde_json::from_str(arguments).map_err(|error| error.to_string())?;
    let seconds = value
        .get("timeout_seconds")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_COMMAND_TIMEOUT.as_secs())
        .clamp(1, MAX_COMMAND_TIMEOUT_SECS);
    Ok(Duration::from_secs(seconds))
}

#[cfg(windows)]
fn configure_hidden(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    command.as_std_mut().creation_flags(CREATE_NO_WINDOW);
}

#[cfg(unix)]
fn configure_process_group(command: &mut Command) {
    use std::os::unix::process::CommandExt;

    unsafe extern "C" {
        fn setpgid(pid: i32, pgid: i32) -> i32;
    }
    // Put the shell and all descendants in one group so cancellation can stop
    // the entire command tree, not just the immediate shell process.
    unsafe {
        command.as_std_mut().pre_exec(|| {
            let _ = setpgid(0, 0);
            Ok(())
        });
    }
}

#[cfg(windows)]
fn shell_command(command: &str, root: &Path) -> Result<(Command, PathBuf), String> {
    use std::ffi::OsString;

    let cwd = windows_shell_path(root)?;
    if cwd.as_os_str().to_string_lossy().starts_with(r"\\") {
        // cmd.exe cannot use a UNC working directory. Start PowerShell from a
        // local directory, change to the UNC provider path, then invoke cmd.
        let path = cwd.to_str().ok_or("Project path is not valid Unicode")?;
        let cmd_command = format!("pushd \"{path}\" && {command}");
        let script = format!("& cmd.exe /D /S /C '{}'", cmd_command.replace('\'', "''"));
        let local = std::env::var_os("SystemRoot")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\"));
        let mut process = Command::new("powershell.exe");
        process
            .args(["-NoLogo", "-NoProfile", "-NonInteractive", "-Command"])
            .arg(script);
        return Ok((process, local));
    }
    let shell = std::env::var_os("COMSPEC")
        .filter(|shell| !shell.is_empty())
        .unwrap_or_else(|| OsString::from("cmd.exe"));
    let mut process = Command::new(shell);
    process.args(["/D", "/S", "/C", command]);
    Ok((process, cwd))
}

#[cfg(not(windows))]
fn shell_command(command: &str, root: &Path) -> Result<(Command, PathBuf), String> {
    let shell = std::env::var_os("SHELL")
        .filter(|shell| !shell.is_empty())
        .unwrap_or_else(|| "/bin/sh".into());
    let mut process = Command::new(shell);
    process.args(["-c", command]);
    Ok((process, root.to_path_buf()))
}

#[cfg(windows)]
fn windows_shell_path(path: &Path) -> Result<PathBuf, String> {
    use std::ffi::OsString;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};

    let wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    let prefix: Vec<u16> = r"\\?\".encode_utf16().collect();
    if !wide.starts_with(&prefix) {
        return Ok(path.to_owned());
    }
    let suffix = &wide[prefix.len()..];
    let unc: Vec<u16> = "UNC\\".encode_utf16().collect();
    if suffix.starts_with(&unc) {
        let mut normal = vec![b'\\' as u16, b'\\' as u16];
        normal.extend_from_slice(&suffix[unc.len()..]);
        return Ok(PathBuf::from(OsString::from_wide(&normal)));
    }
    if suffix.len() >= 3 && suffix[1] == b':' as u16 && suffix[2] == b'\\' as u16 {
        return Ok(PathBuf::from(OsString::from_wide(suffix)));
    }
    Err("Unsupported Windows project path for command execution".into())
}

async fn terminate_process_tree(child: &mut Child) {
    #[cfg(windows)]
    if let Some(pid) = child.id() {
        let mut taskkill = Command::new("taskkill.exe");
        taskkill
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        configure_hidden(&mut taskkill);
        let _ = taskkill.status().await;
    }
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        unsafe extern "C" {
            fn kill(pid: i32, signal: i32) -> i32;
        }
        const SIGKILL: i32 = 9;
        unsafe {
            kill(-(pid as i32), SIGKILL);
        }
    }
    let _ = child.start_kill();
    let _ = tokio::time::timeout(Duration::from_secs(1), child.wait()).await;
}

fn action_string(arguments: &str, key: &str) -> Result<String, String> {
    let value: Value = serde_json::from_str(arguments).map_err(|error| error.to_string())?;
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("Missing string argument: {key}"))
}

fn action_path(arguments: &str, key: &str, default: &str) -> Result<String, String> {
    let arguments: Value = serde_json::from_str(arguments).map_err(|error| error.to_string())?;
    let value = match arguments.get(key) {
        None => default.to_owned(),
        Some(Value::String(value)) => value.clone(),
        Some(_) => return Err(format!("Expected string argument: {key}")),
    };
    if value.is_empty() {
        Ok(default.into())
    } else {
        Ok(value)
    }
}

fn create_temporary_file(parent: &Path) -> Result<(fs::File, PathBuf), String> {
    let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    for attempt in 0..100 {
        let temporary = parent.join(format!(
            ".aircode-write-{}-{counter}-{attempt}.tmp",
            std::process::id()
        ));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(file) => return Ok((file, temporary)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.to_string()),
        }
    }
    Err("Could not create a temporary file".into())
}

fn protected(name: &str) -> bool {
    let name = name.to_lowercase();
    matches!(
        name.as_str(),
        ".git"
            | ".env"
            | ".venv"
            | "target"
            | "node_modules"
            | "dist"
            | "build"
            | ".idea"
            | ".vscode"
    ) || name.starts_with(".env.")
        || name.ends_with(".env")
        || name.ends_with(".pem")
        || name.ends_with(".key")
        || name.contains("credential")
}

fn truncate(text: &str, max: usize) -> String {
    let mut end = text.len().min(max);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    if end < text.len() {
        format!("{}\n...[truncated]", &text[..end])
    } else {
        text.into()
    }
}

pub fn tool_definitions() -> Value {
    let mut definitions = json!([
        {"type":"function","function":{"name":"list_files","description":"List up to 500 project files; truncated results mean inspection is incomplete","parameters":{"type":"object","properties":{"path":{"type":"string"}},"required":[]}}},
        {"type":"function","function":{"name":"read_file","description":"Read a bounded excerpt of a UTF-8 project file","parameters":{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}}},
        {"type":"function","function":{"name":"search_text","description":"Search a bounded set of project files and matches; truncated results mean inspection is incomplete","parameters":{"type":"object","properties":{"query":{"type":"string"},"path":{"type":"string"}},"required":["query"]}}},
        {"type":"function","function":{"name":"write_file","description":"Create or replace a project file","parameters":{"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},"required":["path","content"]}}},
        {"type":"function","function":{"name":"create_directory","description":"Create a project directory","parameters":{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}}},
        {"type":"function","function":{"name":"move_file","description":"Move or rename a project file","parameters":{"type":"object","properties":{"from":{"type":"string"},"to":{"type":"string"}},"required":["from","to"]}}},
        {"type":"function","function":{"name":"delete_file","description":"Delete one project file","parameters":{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}}},
        {"type":"function","function":{"name":"run_command","description":"Run a non-interactive command in the project root. Default timeout 120 seconds; timeout_seconds is clamped to 1-600 seconds. Output is bounded and drained. Killing on cancellation/timeout stops the process tree. This is not an OS sandbox; the command runs with the user's permissions.","parameters":{"type":"object","properties":{"command":{"type":"string"},"timeout_seconds":{"type":"integer","minimum":1,"maximum":600}},"required":["command"]}}}
    ]);
    let shell = if cfg!(windows) {
        "cmd.exe /D /S /C"
    } else {
        "the configured SHELL with -c"
    };
    let command = &mut definitions[7]["function"]["description"];
    *command = Value::String(format!(
        "{} Host OS: {}; shell: {shell}. Use shell syntax for this OS and project-relative paths.",
        command.as_str().unwrap_or_default(),
        std::env::consts::OS,
    ));
    definitions
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn optional_paths_match_the_tool_schema() {
        assert_eq!(action_path("{}", "path", ".").unwrap(), ".");
        assert_eq!(
            action_path(r#"{"query":"value"}"#, "path", ".").unwrap(),
            "."
        );
        assert!(action_path(r#"{"path":12}"#, "path", ".").is_err());
        let tools = tool_definitions();
        assert!(
            tools[7]["function"]["description"]
                .as_str()
                .unwrap()
                .contains(std::env::consts::OS)
        );
    }

    #[tokio::test]
    async fn discovery_reports_truncation_and_search_can_target_a_file() {
        let root = project("discovery-sentinel");
        for index in 0..=DISCOVERY_LIMIT {
            fs::write(root.join(format!("file-{index}.rs")), "needle").unwrap();
        }
        let executor = AgentExecutor::new(&root).unwrap();
        let listing = executor
            .execute(&AgentAction {
                name: "list_files".into(),
                arguments: "{}".into(),
            })
            .await
            .unwrap();
        assert!(listing.contains("file list truncated"));
        assert_eq!(listing.lines().count(), DISCOVERY_LIMIT + 1);
        let search = executor
            .execute(&AgentAction {
                name: "search_text".into(),
                arguments: r#"{"path":"file-0.rs","query":"needle"}"#.into(),
            })
            .await
            .unwrap();
        assert!(search.contains("needle"));
        cleanup_path(&root);
    }

    fn project(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("aircode-agent-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
        root
    }

    fn outside_path(root: &Path, name: &str) -> PathBuf {
        root.parent().unwrap().join(format!(
            "{}-{name}",
            root.file_name().unwrap().to_string_lossy()
        ))
    }

    fn cleanup_path(path: &Path) {
        let _ = fs::remove_dir_all(path);
        let _ = fs::remove_file(path);
    }

    fn read_action(path: &Path) -> AgentAction {
        AgentAction {
            name: "read_file".into(),
            arguments: serde_json::json!({"path": path.to_string_lossy()}).to_string(),
        }
    }

    fn write_action(path: &Path, content: &str) -> AgentAction {
        AgentAction {
            name: "write_file".into(),
            arguments: serde_json::json!({
                "path": path.to_string_lossy(),
                "content": content
            })
            .to_string(),
        }
    }

    fn delete_action(path: &Path) -> AgentAction {
        AgentAction {
            name: "delete_file".into(),
            arguments: serde_json::json!({"path": path.to_string_lossy()}).to_string(),
        }
    }

    fn create_file_symlink(target: &Path, link: &Path) -> io::Result<()> {
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(target, link)
        }
        #[cfg(windows)]
        {
            std::os::windows::fs::symlink_file(target, link)
        }
        #[cfg(not(any(unix, windows)))]
        {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "symbolic links are unsupported on this platform",
            ))
        }
    }

    fn create_dir_symlink(target: &Path, link: &Path) -> io::Result<()> {
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(target, link)
        }
        #[cfg(windows)]
        {
            std::os::windows::fs::symlink_dir(target, link)
        }
        #[cfg(not(any(unix, windows)))]
        {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "symbolic links are unsupported on this platform",
            ))
        }
    }

    fn symlink_created(result: io::Result<()>, context: &str) -> bool {
        match result {
            Ok(()) => true,
            Err(error)
                if cfg!(windows)
                    && (error.kind() == io::ErrorKind::PermissionDenied
                        || error.raw_os_error() == Some(1314)) =>
            {
                eprintln!(
                    "Windows denied symlink creation for {context}: {error}; \
                     skipping only this symlink-specific assertion"
                );
                false
            }
            Err(error) => panic!("Could not create symlink for {context}: {error}"),
        }
    }

    #[test]
    fn permission_modes_match_the_matrix() {
        let read = AgentAction {
            name: "read_file".into(),
            arguments: "{}".into(),
        };
        let write = AgentAction {
            name: "write_file".into(),
            arguments: "{}".into(),
        };
        assert!(PermissionMode::AskEvery.requires_approval(&read));
        assert!(!PermissionMode::AskImportant.requires_approval(&read));
        assert!(PermissionMode::AskImportant.requires_approval(&write));
        assert!(!PermissionMode::FullAccess.requires_approval(&write));
    }

    #[tokio::test]
    async fn sandbox_allows_project_and_blocks_escape_and_secrets() {
        let root = project("sandbox");
        let executor = AgentExecutor::new(&root).unwrap();
        let read = AgentAction {
            name: "read_file".into(),
            arguments: "{\"path\":\"src/main.rs\"}".into(),
        };
        assert!(executor.execute(&read).await.unwrap().contains("fn main"));

        for path in [
            "../outside.txt",
            ".env",
            ".env.production",
            "src/.env.local",
            ".git/config",
            "src/private.pem",
        ] {
            let action = AgentAction {
                name: "read_file".into(),
                arguments: format!("{{\"path\":\"{path}\"}}"),
            };
            assert!(executor.execute(&action).await.is_err(), "allowed {path}");
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn protected_parent_directory_does_not_block_project() {
        let root = project("parent").join("target");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
        let executor = AgentExecutor::new(&root).unwrap();
        let action = AgentAction {
            name: "write_file".into(),
            arguments: "{\"path\":\"src/main.rs\",\"content\":\"fn updated() {}\\n\"}".into(),
        };
        executor.execute(&action).await.unwrap();
        assert!(
            fs::read_to_string(root.join("src/main.rs"))
                .unwrap()
                .contains("updated")
        );
        fs::remove_dir_all(root.parent().unwrap()).unwrap();
    }

    #[tokio::test]
    async fn write_move_and_delete_stay_in_sandbox() {
        let root = project("write");
        let executor = AgentExecutor::new(&root).unwrap();
        let write = AgentAction {
            name: "write_file".into(),
            arguments: "{\"path\":\"src/new.rs\",\"content\":\"pub fn new() {}\\n\"}".into(),
        };
        executor.execute(&write).await.unwrap();
        let rename = AgentAction {
            name: "move_file".into(),
            arguments: "{\"from\":\"src/new.rs\",\"to\":\"src/renamed.rs\"}".into(),
        };
        executor.execute(&rename).await.unwrap();
        let delete = AgentAction {
            name: "delete_file".into(),
            arguments: "{\"path\":\"src/renamed.rs\"}".into(),
        };
        executor.execute(&delete).await.unwrap();
        assert!(!root.join("src/renamed.rs").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn search_resolves_paths_relative_to_project_root() {
        let root = project("search");
        let executor = AgentExecutor::new(&root).unwrap();
        let action = AgentAction {
            name: "search_text".into(),
            arguments: "{\"query\":\"main\",\"path\":\"src\"}".into(),
        };
        let result = executor.execute(&action).await.unwrap();
        let expected = format!("{}:1", Path::new("src").join("main.rs").display());
        assert!(result.contains(&expected), "{result}");
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn command_can_be_cancelled() {
        let root = project("cancel");
        let executor = AgentExecutor::new(&root).unwrap();
        let action = AgentAction {
            name: "run_command".into(),
            arguments: if cfg!(windows) {
                "{\"command\":\"for /l %i in (1,1,100000000) do @set x=%i\"}".into()
            } else {
                "{\"command\":\"sleep 10\"}".into()
            },
        };
        let cancel = tokio_util::sync::CancellationToken::new();
        let task = tokio::spawn({
            let cancel = cancel.clone();
            async move { executor.execute_with_cancel(&action, &cancel).await }
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
        cancel.cancel();
        let result = tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .expect("cancellation did not finish promptly")
            .unwrap();
        assert_eq!(result.unwrap_err(), "Command cancelled by user");
        for attempt in 0..40 {
            if fs::remove_dir_all(&root).is_ok() {
                return;
            }
            assert!(attempt < 39, "command still holds the project directory");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    #[test]
    fn output_capture_is_bounded_and_reports_truncation() {
        let mut output = CapturedOutput::default();
        output.push(&vec![b'a'; MAX_COMMAND_OUTPUT_BYTES + 500]);
        assert_eq!(output.bytes.len(), MAX_COMMAND_OUTPUT_BYTES);
        assert!(output.truncated);
        assert_eq!(output.total, MAX_COMMAND_OUTPUT_BYTES + 500);
        let text = output.text();
        assert!(text.contains("output truncated"));
    }

    #[test]
    fn command_timeout_is_longer_than_the_old_fixed_limit_and_still_bounded() {
        assert_eq!(
            command_timeout("{\"command\":\"echo ok\"}").unwrap(),
            DEFAULT_COMMAND_TIMEOUT
        );
        assert_eq!(
            command_timeout("{\"command\":\"echo ok\",\"timeout_seconds\":300}").unwrap(),
            Duration::from_secs(300)
        );
        assert_eq!(
            command_timeout("{\"command\":\"echo ok\",\"timeout_seconds\":99999}").unwrap(),
            Duration::from_secs(MAX_COMMAND_TIMEOUT_SECS)
        );
    }

    #[tokio::test]
    async fn timed_out_command_is_stopped_and_releases_the_project() {
        let root = project("timeout");
        let executor = AgentExecutor::new(&root).unwrap();
        let action = AgentAction {
            name: "run_command".into(),
            arguments: if cfg!(windows) {
                "{\"command\":\"ping 127.0.0.1 -n 30 >NUL\",\"timeout_seconds\":1}".into()
            } else {
                "{\"command\":\"sleep 30\",\"timeout_seconds\":1}".into()
            },
        };
        let started = std::time::Instant::now();
        let error = executor.execute(&action).await.unwrap_err();
        assert!(error.contains("timed out"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(8));
        for attempt in 0..40 {
            if fs::remove_dir_all(&root).is_ok() {
                return;
            }
            assert!(attempt < 39, "timed-out command still holds the project");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    #[tokio::test]
    async fn symlink_to_outside_cannot_be_read_written_diffed_or_deleted() {
        let root = project("symlink-target");
        let outside = outside_path(&root, "target.txt");
        cleanup_path(&outside);
        fs::write(&outside, "outside-original").unwrap();
        let link = root.join("link.txt");
        if !symlink_created(
            create_file_symlink(&outside, &link),
            "a file outside the project",
        ) {
            cleanup_path(&root);
            cleanup_path(&outside);
            return;
        }
        let executor = AgentExecutor::new(&root).unwrap();
        assert!(executor.execute(&read_action(&link)).await.is_err());
        assert!(executor.execute(&delete_action(&link)).await.is_err());
        assert!(executor.diff(&link.to_string_lossy(), "changed").is_err());
        assert!(
            executor
                .execute(&write_action(&link, "changed"))
                .await
                .is_err()
        );
        assert_eq!(fs::read_to_string(&outside).unwrap(), "outside-original");
        cleanup_path(&root);
        cleanup_path(&outside);
    }

    #[tokio::test]
    async fn dangling_symlink_is_blocked() {
        let root = project("dangling-symlink");
        let missing_target = root.join("missing-target.txt");
        let link = root.join("dangling.txt");
        if !symlink_created(
            create_file_symlink(&missing_target, &link),
            "a dangling file link",
        ) {
            cleanup_path(&root);
            return;
        }
        let executor = AgentExecutor::new(&root).unwrap();
        assert!(executor.execute(&read_action(&link)).await.is_err());
        assert!(
            executor
                .execute(&write_action(&link, "created"))
                .await
                .is_err()
        );
        assert!(executor.diff(&link.to_string_lossy(), "created").is_err());
        assert!(executor.execute(&delete_action(&link)).await.is_err());
        assert!(!missing_target.exists());
        cleanup_path(&root);
    }

    #[tokio::test]
    async fn symlinked_ancestor_outside_project_is_blocked() {
        let root = project("symlink-ancestor");
        let outside = outside_path(&root, "outside-dir");
        cleanup_path(&outside);
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("secret.txt"), "outside-secret").unwrap();
        let link = root.join("linked-dir");
        if !symlink_created(
            create_dir_symlink(&outside, &link),
            "a directory outside the project",
        ) {
            cleanup_path(&root);
            cleanup_path(&outside);
            return;
        }
        let executor = AgentExecutor::new(&root).unwrap();
        assert!(
            executor
                .execute(&read_action(&link.join("secret.txt")))
                .await
                .is_err()
        );
        assert!(
            executor
                .execute(&write_action(&link.join("new.txt"), "new"))
                .await
                .is_err()
        );
        assert_eq!(
            fs::read_to_string(outside.join("secret.txt")).unwrap(),
            "outside-secret"
        );
        assert!(!outside.join("new.txt").exists());
        cleanup_path(&root);
        cleanup_path(&outside);
    }

    #[tokio::test]
    async fn protected_env_target_is_blocked_through_alias() {
        let root = project("protected-alias");
        let env = root.join(".env");
        fs::write(&env, "TOKEN=secret\n").unwrap();
        let alias = root.join("alias.txt");
        if !symlink_created(create_file_symlink(&env, &alias), "a protected .env file") {
            cleanup_path(&root);
            return;
        }
        let executor = AgentExecutor::new(&root).unwrap();
        assert!(executor.execute(&read_action(&alias)).await.is_err());
        assert!(
            executor
                .execute(&write_action(&alias, "TOKEN=changed\n"))
                .await
                .is_err()
        );
        assert!(
            executor
                .diff(&alias.to_string_lossy(), "TOKEN=changed\n")
                .is_err()
        );
        assert_eq!(fs::read_to_string(&env).unwrap(), "TOKEN=secret\n");
        cleanup_path(&root);
    }

    #[tokio::test]
    async fn writing_a_hardlink_replaces_the_project_entry_only() {
        let root = project("hardlink");
        let outside = outside_path(&root, "hardlink-target.txt");
        cleanup_path(&outside);
        fs::write(&outside, "outside-original").unwrap();
        let linked = root.join("linked.txt");
        fs::hard_link(&outside, &linked).unwrap();
        let executor = AgentExecutor::new(&root).unwrap();
        executor
            .execute(&write_action(&linked, "project-replacement"))
            .await
            .unwrap();
        assert_eq!(fs::read_to_string(&linked).unwrap(), "project-replacement");
        assert_eq!(fs::read_to_string(&outside).unwrap(), "outside-original");
        cleanup_path(&root);
        cleanup_path(&outside);
    }

    #[tokio::test]
    async fn write_file_creates_nested_parent_directories_after_validation() {
        let root = project("nested-write");
        let executor = AgentExecutor::new(&root).unwrap();
        let target = root.join("src/generated/deep/foo.rs");
        executor
            .execute(&write_action(&target, "pub fn generated() {}\n"))
            .await
            .unwrap();
        assert_eq!(
            fs::read_to_string(&target).unwrap(),
            "pub fn generated() {}\n"
        );
        cleanup_path(&root);
    }

    #[tokio::test]
    async fn traversal_and_absolute_outside_paths_are_rejected() {
        let root = project("traversal");
        let executor = AgentExecutor::new(&root).unwrap();
        let traversal = Path::new("../escape.txt");
        assert!(
            executor
                .execute(&write_action(traversal, "escape"))
                .await
                .is_err()
        );
        let absolute = outside_path(&root, "absolute.txt");
        cleanup_path(&absolute);
        assert!(
            executor
                .execute(&write_action(&absolute, "escape"))
                .await
                .is_err()
        );
        assert!(executor.execute(&read_action(&absolute)).await.is_err());
        assert!(!absolute.exists());
        cleanup_path(&root);
    }

    #[tokio::test]
    async fn cancelled_write_does_not_modify_or_create_files() {
        let root = project("cancelled-write");
        let executor = AgentExecutor::new(&root).unwrap();
        let existing = root.join("src/main.rs");
        let original = fs::read_to_string(&existing).unwrap();
        let cancel = tokio_util::sync::CancellationToken::new();
        cancel.cancel();
        let error = executor
            .execute_with_cancel(&write_action(&existing, "changed"), &cancel)
            .await
            .unwrap_err();
        assert!(error.to_lowercase().contains("cancel"));
        assert_eq!(fs::read_to_string(&existing).unwrap(), original);
        let new_target = root.join("src/cancelled/new.txt");
        let error = executor
            .execute_with_cancel(&write_action(&new_target, "created"), &cancel)
            .await
            .unwrap_err();
        assert!(error.to_lowercase().contains("cancel"));
        assert!(!new_target.exists());
        assert!(!root.join("src/cancelled").exists());
        cleanup_path(&root);
    }

    #[tokio::test]
    async fn atomic_write_error_keeps_original_and_cleans_temporary_file() {
        let root = project("atomic-error");
        let executor = AgentExecutor::new(&root).unwrap();
        let original = fs::read_to_string(root.join("src/main.rs")).unwrap();
        let error = executor
            .execute(&write_action(&root.join("src"), "not a file"))
            .await
            .unwrap_err();
        assert!(!error.is_empty());
        assert_eq!(
            fs::read_to_string(root.join("src/main.rs")).unwrap(),
            original
        );
        let leftovers: Vec<_> = fs::read_dir(&root)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".aircode-write-")
            })
            .collect();
        assert!(
            leftovers.is_empty(),
            "temporary files remain: {leftovers:?}"
        );
        cleanup_path(&root);
    }

    #[cfg(windows)]
    #[test]
    fn windows_commands_use_hidden_normalised_cwd_and_unc_fallback() {
        assert_eq!(CREATE_NO_WINDOW, 0x0800_0000);
        assert_eq!(
            windows_shell_path(Path::new(r"\\?\D:\project")).unwrap(),
            PathBuf::from(r"D:\project")
        );
        assert_eq!(
            windows_shell_path(Path::new(r"\\?\UNC\server\share\project")).unwrap(),
            PathBuf::from(r"\\server\share\project")
        );
        let (drive, cwd) = shell_command("echo ok", Path::new(r"\\?\D:\project")).unwrap();
        assert_eq!(cwd, PathBuf::from(r"D:\project"));
        let program = drive
            .as_std()
            .get_program()
            .to_string_lossy()
            .to_lowercase();
        assert!(program.contains("cmd"));

        let (unc, local) =
            shell_command("echo ok", Path::new(r"\\?\UNC\server\share\project")).unwrap();
        assert!(!local.to_string_lossy().starts_with(r"\\"));
        assert_eq!(
            unc.as_std().get_program().to_string_lossy().to_lowercase(),
            "powershell.exe"
        );
    }
}
