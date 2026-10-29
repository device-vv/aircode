use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read,
    path::{Path, PathBuf},
};

pub use notify::RecommendedWatcher;
use notify::Watcher;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectEntry {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectRow {
    pub entry: ProjectEntry,
    pub depth: usize,
    pub expanded: bool,
}

const MAX_PUBSPEC_BYTES: u64 = 256 * 1024;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ProjectKind {
    Rust,
    Python,
    Dart,
    Flutter,
    #[default]
    Unknown,
}

impl ProjectKind {
    pub fn detect(root: &Path) -> Self {
        // Only inspect fixed marker names at the opened root. Do not follow
        // marker symlinks or execute package-manager/project code.
        if regular_marker(root, "pubspec.yaml") {
            return if read_marker(root, "pubspec.yaml")
                .is_some_and(|source| pubspec_uses_flutter_sdk(&source))
            {
                Self::Flutter
            } else {
                Self::Dart
            };
        }
        if regular_marker(root, "Cargo.toml") {
            return Self::Rust;
        }
        if regular_marker(root, "pyproject.toml") {
            return Self::Python;
        }
        Self::Unknown
    }
}

pub struct Project {
    root: PathBuf,
    kind: ProjectKind,
    expanded: BTreeSet<PathBuf>,
    cache: BTreeMap<PathBuf, Vec<ProjectEntry>>,
}

impl Project {
    pub fn open(path: impl AsRef<Path>) -> std::io::Result<Self> {
        let root = path.as_ref().canonicalize()?;
        let mut project = Self {
            expanded: BTreeSet::from([root.clone()]),
            kind: ProjectKind::detect(&root),
            root,
            cache: BTreeMap::new(),
        };
        project.children(&project.root.clone())?;
        Ok(project)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn kind(&self) -> ProjectKind {
        self.kind
    }

    pub fn watch(
        &self,
    ) -> std::io::Result<(notify::RecommendedWatcher, std::sync::mpsc::Receiver<()>)> {
        let (sender, receiver) = std::sync::mpsc::channel();
        let mut watcher = notify::RecommendedWatcher::new(
            move |_| {
                let _ = sender.send(());
            },
            notify::Config::default(),
        )
        .map_err(std::io::Error::other)?;
        watcher
            .watch(&self.root, notify::RecursiveMode::Recursive)
            .map_err(std::io::Error::other)?;
        Ok((watcher, receiver))
    }

    pub fn children(&mut self, directory: &Path) -> std::io::Result<&[ProjectEntry]> {
        if !self.cache.contains_key(directory) {
            let mut entries = fs::read_dir(directory)?
                .filter_map(Result::ok)
                .filter_map(|entry| {
                    let path = entry.path();
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if ignored(&name) {
                        return None;
                    }
                    let is_dir = entry.file_type().ok()?.is_dir();
                    Some(ProjectEntry { path, name, is_dir })
                })
                .collect::<Vec<_>>();
            entries.sort_by(|left, right| {
                right
                    .is_dir
                    .cmp(&left.is_dir)
                    .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
            });
            self.cache.insert(directory.to_path_buf(), entries);
        }
        Ok(self.cache.get(directory).map(Vec::as_slice).unwrap_or(&[]))
    }

    pub fn toggle(&mut self, path: &Path) {
        let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        if !self.expanded.remove(&path) {
            self.expanded.insert(path);
        }
    }

    pub fn visible_rows(&mut self) -> Vec<ProjectRow> {
        let mut rows = Vec::new();
        let root = self.root.clone();
        let children = self.children(&root).map(Vec::from).unwrap_or_default();
        for entry in children {
            self.append_rows(entry, 0, &mut rows);
            if rows.len() >= 10_000 {
                break;
            }
        }
        rows
    }

    pub fn refresh(&mut self) {
        self.cache.clear();
        self.kind = ProjectKind::detect(&self.root);
    }

    fn append_rows(&mut self, entry: ProjectEntry, depth: usize, rows: &mut Vec<ProjectRow>) {
        let expanded = self.expanded.contains(&entry.path);
        rows.push(ProjectRow {
            entry: entry.clone(),
            depth,
            expanded,
        });
        if entry.is_dir && expanded && rows.len() < 10_000 {
            let children = self
                .children(&entry.path)
                .map(Vec::from)
                .unwrap_or_default();
            for child in children {
                self.append_rows(child, depth + 1, rows);
                if rows.len() >= 10_000 {
                    break;
                }
            }
        }
    }
}

fn regular_marker(root: &Path, name: &str) -> bool {
    fs::symlink_metadata(root.join(name)).is_ok_and(|metadata| metadata.file_type().is_file())
}

fn read_marker(root: &Path, name: &str) -> Option<String> {
    let file = fs::File::open(root.join(name)).ok()?;
    let mut bytes = Vec::new();
    file.take(MAX_PUBSPEC_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > MAX_PUBSPEC_BYTES as usize {
        return None;
    }
    String::from_utf8(bytes).ok()
}

fn pubspec_uses_flutter_sdk(source: &str) -> bool {
    let mut dependency_indent = None;
    let mut direct_dependency_indent = None;
    let mut in_flutter_dependency = false;

    for raw_line in source.lines() {
        let line = yaml_without_comment(raw_line);
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        if indent == 0 {
            dependency_indent = yaml_key(trimmed)
                .is_some_and(|(key, value)| {
                    matches!(key, "dependencies" | "dev_dependencies") && value.is_empty()
                })
                .then_some(0);
            direct_dependency_indent = None;
            in_flutter_dependency = false;
            continue;
        }
        let Some(section_indent) = dependency_indent else {
            continue;
        };
        if indent <= section_indent {
            dependency_indent = None;
            direct_dependency_indent = None;
            in_flutter_dependency = false;
            continue;
        }

        let direct_indent = *direct_dependency_indent.get_or_insert(indent);
        let Some((key, value)) = yaml_key(trimmed) else {
            in_flutter_dependency = false;
            continue;
        };
        if indent == direct_indent {
            in_flutter_dependency = key == "flutter";
            if in_flutter_dependency && flow_flutter_sdk(value) {
                return true;
            }
        } else if indent > direct_indent
            && in_flutter_dependency
            && key == "sdk"
            && unquote_yaml(value) == "flutter"
        {
            return true;
        }
    }
    false
}

fn yaml_key(line: &str) -> Option<(&str, &str)> {
    let (key, value) = line.split_once(':')?;
    Some((unquote_yaml(key), value.trim()))
}

fn unquote_yaml(value: &str) -> &str {
    let value = value.trim();
    if value.len() >= 2
        && ((value.starts_with('"') && value.ends_with('"'))
            || (value.starts_with('\'') && value.ends_with('\'')))
    {
        &value[1..value.len() - 1]
    } else {
        value
    }
}

fn flow_flutter_sdk(value: &str) -> bool {
    let value = value.trim();
    value.starts_with('{')
        && value.ends_with('}')
        && value[1..value.len() - 1].split(',').any(|entry| {
            yaml_key(entry)
                .is_some_and(|(key, value)| key == "sdk" && unquote_yaml(value) == "flutter")
        })
}

fn yaml_without_comment(line: &str) -> &str {
    let mut single_quoted = false;
    let mut double_quoted = false;
    let mut escaped = false;
    let mut previous_was_space = true;
    for (index, character) in line.char_indices() {
        match character {
            '\\' if double_quoted => escaped = !escaped,
            '"' if !single_quoted && !escaped => double_quoted = !double_quoted,
            '\'' if !double_quoted => single_quoted = !single_quoted,
            '#' if !single_quoted && !double_quoted && previous_was_space => {
                return &line[..index];
            }
            _ => {}
        }
        if character != '\\' {
            escaped = false;
        }
        previous_was_space = character.is_whitespace();
    }
    line
}

fn ignored(name: &str) -> bool {
    matches!(
        name,
        ".git"
            | ".idea"
            | ".vscode"
            | "target"
            | "node_modules"
            | "dist"
            | "build"
            | ".env"
            | ".venv"
    ) || name.ends_with(".env")
        || name.ends_with(".pem")
        || name.ends_with(".key")
        || name.contains("credential")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tree_is_lazy_sorted_and_expandable() {
        let root = std::env::temp_dir().join(format!("aircode-project-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("src/nested")).unwrap();
        fs::create_dir_all(root.join("target")).unwrap();
        fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
        fs::write(root.join("src/nested/lib.rs"), "pub fn value() {}\n").unwrap();
        fs::write(root.join("target/ignored.rs"), "ignored").unwrap();
        let mut project = Project::open(&root).unwrap();

        let rows = project.visible_rows();
        assert!(rows.iter().any(|row| row.entry.name == "src"));
        assert!(!rows.iter().any(|row| row.entry.name == "target"));
        assert!(!rows.iter().any(|row| row.entry.name == "main.rs"));

        project.toggle(&root.join("src"));
        let rows = project.visible_rows();
        assert!(rows.iter().any(|row| row.entry.name == "main.rs"));
        assert!(!rows.iter().any(|row| row.entry.name == "lib.rs"));

        project.toggle(&root.join("src/nested"));
        assert!(
            project
                .visible_rows()
                .iter()
                .any(|row| row.entry.name == "lib.rs")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn credential_files_are_not_listed() {
        let root =
            std::env::temp_dir().join(format!("aircode-project-secrets-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join(".env"), "secret").unwrap();
        fs::write(root.join("private.pem"), "secret").unwrap();
        fs::write(root.join("main.rs"), "fn main() {}").unwrap();
        let mut project = Project::open(&root).unwrap();

        let names = project
            .visible_rows()
            .into_iter()
            .map(|row| row.entry.name)
            .collect::<Vec<_>>();
        assert_eq!(names, ["main.rs"]);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn removed_expanded_directories_do_not_break_refresh() {
        let root =
            std::env::temp_dir().join(format!("aircode-project-refresh-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("src/nested")).unwrap();
        fs::write(root.join("src/nested/lib.rs"), "pub fn value() {}\n").unwrap();
        let mut project = Project::open(&root).unwrap();
        project.toggle(&root.join("src"));
        project.toggle(&root.join("src/nested"));
        assert!(!project.visible_rows().is_empty());

        fs::remove_dir_all(root.join("src")).unwrap();
        project.refresh();
        project.toggle(&root.join("src/nested"));

        assert!(project.visible_rows().is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    fn detection_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "aircode-project-detect-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn detects_only_regular_safe_project_markers() {
        let root = detection_root("markers");
        assert_eq!(ProjectKind::detect(&root), ProjectKind::Unknown);

        fs::create_dir(root.join("Cargo.toml")).unwrap();
        assert_eq!(ProjectKind::detect(&root), ProjectKind::Unknown);
        fs::remove_dir(root.join("Cargo.toml")).unwrap();

        fs::write(root.join("Cargo.toml"), "[package]\nname = 'demo'\n").unwrap();
        assert_eq!(ProjectKind::detect(&root), ProjectKind::Rust);
        let project = Project::open(&root).unwrap();
        assert_eq!(project.kind(), ProjectKind::Rust);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn detects_python_and_plain_dart_projects() {
        let python = detection_root("python");
        fs::write(python.join("pyproject.toml"), "[project]\nname = 'demo'\n").unwrap();
        assert_eq!(ProjectKind::detect(&python), ProjectKind::Python);
        fs::remove_dir_all(python).unwrap();

        let dart = detection_root("dart");
        fs::write(
            dart.join("pubspec.yaml"),
            "name: demo\ndependencies:\n  http: ^1.0.0\n",
        )
        .unwrap();
        assert_eq!(ProjectKind::detect(&dart), ProjectKind::Dart);
        fs::remove_dir_all(dart).unwrap();
    }

    #[test]
    fn flutter_requires_an_uncommented_sdk_dependency() {
        let root = detection_root("flutter");
        let marker = root.join("pubspec.yaml");
        fs::write(
            &marker,
            r#"
name: counter_app
# dependencies:
#   flutter:
#     sdk: flutter
dependencies:
  flutter:
    sdk: flutter
  cupertino_icons: ^1.0.8
"#,
        )
        .unwrap();
        assert_eq!(ProjectKind::detect(&root), ProjectKind::Flutter);

        fs::write(
            &marker,
            "name: demo\ndependencies:\n  flutter:\n    version: any\n",
        )
        .unwrap();
        assert_eq!(ProjectKind::detect(&root), ProjectKind::Dart);

        fs::write(
            &marker,
            "name: demo\ndependencies: {flutter: {sdk: flutter}}\n",
        )
        .unwrap();
        assert_eq!(ProjectKind::detect(&root), ProjectKind::Dart);

        fs::write(
            &marker,
            "name: demo\ndependencies:\n  flutter: {sdk: flutter}\n",
        )
        .unwrap();
        assert_eq!(ProjectKind::detect(&root), ProjectKind::Flutter);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn flutter_scan_is_bounded_and_ignores_invalid_utf8() {
        let root = detection_root("bounded");
        let marker = root.join("pubspec.yaml");
        fs::write(&marker, [0xff, 0xfe, 0xfd]).unwrap();
        assert_eq!(ProjectKind::detect(&root), ProjectKind::Dart);

        let mut source = "name: demo\ndependencies:\n".to_string();
        source.push_str(&"  # padding\n".repeat(MAX_PUBSPEC_BYTES as usize / 11));
        source.push_str("  flutter:\n    sdk: flutter\n");
        fs::write(&marker, source).unwrap();
        assert_eq!(ProjectKind::detect(&root), ProjectKind::Dart);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn refresh_updates_the_cached_project_kind() {
        let root = detection_root("refresh-kind");
        fs::write(root.join("Cargo.toml"), "[workspace]\n").unwrap();
        let mut project = Project::open(&root).unwrap();
        assert_eq!(project.kind(), ProjectKind::Rust);
        fs::remove_file(root.join("Cargo.toml")).unwrap();
        fs::write(root.join("pyproject.toml"), "[project]\n").unwrap();
        project.refresh();
        assert_eq!(project.kind(), ProjectKind::Python);
        fs::remove_dir_all(root).unwrap();
    }
}
