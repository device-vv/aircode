use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
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

pub struct Project {
    root: PathBuf,
    expanded: BTreeSet<PathBuf>,
    cache: BTreeMap<PathBuf, Vec<ProjectEntry>>,
}

impl Project {
    pub fn open(path: impl AsRef<Path>) -> std::io::Result<Self> {
        let root = path.as_ref().canonicalize()?;
        let mut project = Self {
            expanded: BTreeSet::from([root.clone()]),
            root,
            cache: BTreeMap::new(),
        };
        project.children(&project.root.clone())?;
        Ok(project)
    }

    pub fn root(&self) -> &Path {
        &self.root
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
}
