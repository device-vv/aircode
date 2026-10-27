use std::{
    fs,
    path::{Component, Path, PathBuf},
    time::Duration,
};

use serde_json::{Value, json};
use similar::TextDiff;

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
                if content.len() > 1_000_000 {
                    return Err("write_file content exceeds 1 MB".into());
                }
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
                }
                fs::write(&path, &content).map_err(|error| error.to_string())?;
                Ok(format!(
                    "Wrote {} bytes to {}",
                    content.len(),
                    self.relative(&path)
                ))
            }
            "create_directory" => {
                let path = self.resolve(&action_path(&action.arguments, "path", "")?, true)?;
                fs::create_dir_all(&path).map_err(|error| error.to_string())?;
                Ok(format!("Created directory {}", self.relative(&path)))
            }
            "move_file" => {
                let from = self.resolve(&action_path(&action.arguments, "from", "")?, false)?;
                let to = self.resolve(&action_path(&action.arguments, "to", "")?, true)?;
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

    fn resolve(&self, value: &str, create: bool) -> Result<PathBuf, String> {
        let candidate = {
            let path = Path::new(value);
            if path.is_absolute() {
                path.to_path_buf()
            } else {
                self.root.join(path)
            }
        };
        let resolved = if create {
            let parent = candidate.parent().ok_or("Invalid target path")?;
            let parent = parent.canonicalize().map_err(|error| error.to_string())?;
            parent.join(candidate.file_name().ok_or("Invalid file name")?)
        } else {
            candidate
                .canonicalize()
                .map_err(|error| error.to_string())?
        };
        if !resolved.starts_with(&self.root) {
            return Err("Path is outside the project sandbox".into());
        }
        let relative = resolved
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
        Ok(resolved)
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
        let mut output: Vec<_> = paths.iter().map(|path| self.relative(path)).collect();
        output.sort();
        output.truncate(500);
        Ok(output.join("\n"))
    }

    fn walk(&self, base: &Path, output: &mut Vec<PathBuf>) {
        if output.len() >= 500 {
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
            if output.len() >= 500 {
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
        let mut matches = Vec::new();
        for path in files.into_iter().take(500) {
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
                    if matches.len() >= 100 {
                        return Ok(matches.join("\n"));
                    }
                }
            }
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
        let mut process = if cfg!(windows) {
            let mut process = tokio::process::Command::new("cmd");
            process.args(["/C", &command]);
            process
        } else {
            let mut process = tokio::process::Command::new("sh");
            process.args(["-c", &command]);
            process
        };
        process.current_dir(&self.root).kill_on_drop(true);
        let output = tokio::select! {
            () = cancel.cancelled() => return Err("Command cancelled by user".into()),
            output = tokio::time::timeout(Duration::from_secs(30), process.output()) => {
                output.map_err(|_| "Command timed out after 30 seconds".to_string())?
                    .map_err(|error| error.to_string())?
            }
        };
        let text = format!(
            "{}\n{}\nexit status: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
            output.status
        );
        Ok(truncate(&text, 200_000))
    }
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
    let value = action_string(arguments, key)?;
    if value.is_empty() {
        Ok(default.into())
    } else {
        Ok(value)
    }
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
    ) || name.ends_with(".env")
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
    json!([
        {"type":"function","function":{"name":"list_files","description":"List project files","parameters":{"type":"object","properties":{"path":{"type":"string"}},"required":[]}}},
        {"type":"function","function":{"name":"read_file","description":"Read a UTF-8 project file","parameters":{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}}},
        {"type":"function","function":{"name":"search_text","description":"Search text in project files","parameters":{"type":"object","properties":{"query":{"type":"string"},"path":{"type":"string"}},"required":["query"]}}},
        {"type":"function","function":{"name":"write_file","description":"Create or replace a project file","parameters":{"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},"required":["path","content"]}}},
        {"type":"function","function":{"name":"create_directory","description":"Create a project directory","parameters":{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}}},
        {"type":"function","function":{"name":"move_file","description":"Move or rename a project file","parameters":{"type":"object","properties":{"from":{"type":"string"},"to":{"type":"string"}},"required":["from","to"]}}},
        {"type":"function","function":{"name":"delete_file","description":"Delete one project file","parameters":{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}}},
        {"type":"function","function":{"name":"run_command","description":"Run a command in the project root with a 30 second timeout","parameters":{"type":"object","properties":{"command":{"type":"string"}},"required":["command"]}}}
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("aircode-agent-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
        root
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

        for path in ["../outside.txt", ".env", ".git/config", "src/private.pem"] {
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
}
