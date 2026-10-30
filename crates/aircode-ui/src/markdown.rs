use std::{
    fs::OpenOptions,
    io::{self, Write},
    path::{Path, PathBuf},
};

pub const DEFAULT_EXPORT_NAME: &str = "aircode-chat.md";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MarkdownMessage {
    pub speaker: String,
    pub content: String,
    pub state: Option<String>,
    pub error: Option<String>,
}

pub fn default_export_path(project_root: &Path) -> PathBuf {
    project_root.join(DEFAULT_EXPORT_NAME)
}

fn heading(value: &str) -> String {
    let mut escaped = String::new();
    for character in value.chars() {
        match character {
            '\r' | '\n' => escaped.push(' '),
            '\\' | '#' | '[' | ']' => {
                escaped.push('\\');
                escaped.push(character);
            }
            _ => escaped.push(character),
        }
    }
    escaped
}

fn inline(value: &str) -> String {
    heading(value).replace('*', "\\*").replace('`', "\\`")
}

pub fn render_markdown(project_title: &str, messages: &[MarkdownMessage]) -> String {
    let mut output = format!("# AIRcode chat — {}\n", heading(project_title));
    for message in messages {
        output.push_str(&format!("\n## {}\n", heading(&message.speaker)));
        if let Some(state) = &message.state {
            output.push_str(&format!("\n**State:** {}\n", inline(state)));
        }
        if let Some(error) = &message.error {
            output.push_str(&format!("\n**Error:** {}\n", inline(error)));
        }
        if !message.content.is_empty() {
            output.push('\n');
            output.push_str(&message.content);
            output.push('\n');
        }
    }
    output
}

/// `None` is a cancelled native picker and must not touch the filesystem.
/// New files use create_new. Existing files were explicitly confirmed by the
/// native save picker and are replaced only after a complete temporary write.
pub fn write_markdown(path: Option<&Path>, markdown: &str) -> io::Result<bool> {
    let Some(path) = path else {
        return Ok(false);
    };
    if !path.exists() {
        let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
        if let Err(error) = file
            .write_all(markdown.as_bytes())
            .and_then(|_| file.sync_all())
        {
            drop(file);
            let _ = std::fs::remove_file(path);
            return Err(error);
        }
        return Ok(true);
    }
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(DEFAULT_EXPORT_NAME);
    let mut temporary = None;
    for attempt in 0..100u32 {
        let candidate = parent.join(format!(".{name}.{}.{}.tmp", std::process::id(), attempt));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => {
                temporary = Some((candidate, file));
                break;
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    let (temporary_path, mut file) = temporary.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::AlreadyExists,
            "Could not allocate an export temporary file",
        )
    })?;
    if let Err(error) = file
        .write_all(markdown.as_bytes())
        .and_then(|_| file.sync_all())
    {
        drop(file);
        let _ = std::fs::remove_file(&temporary_path);
        return Err(error);
    }
    drop(file);
    match std::fs::rename(&temporary_path, path) {
        Ok(()) => Ok(true),
        Err(error) => {
            let _ = std::fs::remove_file(&temporary_path);
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unicode_fences_labels_and_partial_errors_stay_visible() {
        let markdown = render_markdown(
            "проект #1",
            &[
                MarkdownMessage {
                    speaker: "You".into(),
                    content: "Сделай ```rust\nfn main() {}\n```".into(),
                    state: None,
                    error: None,
                },
                MarkdownMessage {
                    speaker: "@kimi".into(),
                    content: "Частичный ответ\n````md\n```\n````".into(),
                    state: Some("Partial".into()),
                    error: Some("connection closed".into()),
                },
            ],
        );
        assert!(markdown.contains("\\#1"));
        assert!(markdown.contains("## @kimi"));
        assert!(markdown.contains("````md\n```\n````"));
        assert!(markdown.contains("**State:** Partial"));
        assert!(markdown.contains("**Error:** connection closed"));
    }

    #[test]
    fn hidden_fields_are_not_part_of_the_export_model() {
        let source = MarkdownMessage {
            speaker: "@deepseek".into(),
            content: "visible".into(),
            state: None,
            error: None,
        };
        let markdown = render_markdown("project", &[source]);
        assert!(!markdown.contains("reasoning"));
        assert!(!markdown.contains("arguments"));
    }

    #[test]
    fn cancelled_picker_does_not_write_and_default_is_project_local() {
        let root = std::env::temp_dir().join(format!("aircode-export-test-{}", std::process::id()));
        assert_eq!(default_export_path(&root), root.join(DEFAULT_EXPORT_NAME));
        assert!(!write_markdown(None, "do not write").unwrap());
    }

    #[test]
    fn replacement_is_complete_and_failed_replacement_preserves_existing_data() {
        let root = std::env::temp_dir().join(format!(
            "aircode-markdown-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("chat.md");
        std::fs::write(&path, "old").unwrap();
        assert!(write_markdown(Some(&path), "new 🦀").unwrap());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new 🦀");
        let directory = root.join("directory");
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(directory.join("keep.txt"), "preserve").unwrap();
        assert!(write_markdown(Some(&directory), "invalid target").is_err());
        assert_eq!(
            std::fs::read_to_string(directory.join("keep.txt")).unwrap(),
            "preserve"
        );
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 2);
        std::fs::remove_dir_all(root).unwrap();
    }
}
