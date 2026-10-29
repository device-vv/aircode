use crate::HistoryError;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;

/// Stable, portable identity derived from a caller-canonicalized project path.
///
/// Windows drive and UNC paths use `\\` separators, have no extended-length
/// prefix, and are ASCII-case-folded. Other paths retain their case. The
/// original normalized path is stored inside the record for validation; only
/// its SHA-256 digest is used as the on-disk filename.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct ProjectId {
    normalized_path: String,
    sha256: String,
}

impl ProjectId {
    /// Creates an identity from an absolute path already canonicalized by the
    /// caller (normally with [`std::fs::canonicalize`]).
    ///
    /// This method performs lexical normalization but does not access the
    /// filesystem. Non-UTF-8 paths are rejected to keep records portable.
    pub fn from_canonical_path(path: impl AsRef<Path>) -> Result<Self, HistoryError> {
        let path = path.as_ref();
        let raw = path.to_str().ok_or(HistoryError::NonUtf8ProjectPath)?;
        let normalized_path = normalize_project_path(raw)?;
        let digest = Sha256::digest(normalized_path.as_bytes());
        let mut sha256 = String::with_capacity(digest.len() * 2);
        const HEX: &[u8; 16] = b"0123456789abcdef";
        for byte in digest {
            sha256.push(HEX[(byte >> 4) as usize] as char);
            sha256.push(HEX[(byte & 0x0f) as usize] as char);
        }
        Ok(Self {
            normalized_path,
            sha256,
        })
    }

    pub fn normalized_path(&self) -> &str {
        &self.normalized_path
    }

    /// Lowercase SHA-256 hex. This is the only project-derived filename key.
    pub fn storage_key(&self) -> &str {
        &self.sha256
    }

    pub(crate) fn validate(&self) -> Result<(), HistoryError> {
        let recomputed = Self::from_canonical_path(&self.normalized_path)
            .map_err(|error| HistoryError::InvalidRecord(error.to_string()))?;
        if recomputed != *self {
            return Err(HistoryError::InvalidRecord(
                "project path and SHA-256 identity are inconsistent".into(),
            ));
        }
        Ok(())
    }
}

fn normalize_project_path(raw: &str) -> Result<String, HistoryError> {
    if raw.is_empty() {
        return Err(HistoryError::InvalidProjectPath(
            "path must not be empty".into(),
        ));
    }
    if raw.contains('\0') {
        return Err(HistoryError::InvalidProjectPath(
            "path contains a NUL character".into(),
        ));
    }

    let slash_normalized = raw.replace('/', "\\");
    if looks_like_windows_path(&slash_normalized) {
        normalize_windows(&slash_normalized)
    } else {
        if !raw.starts_with('/') {
            return Err(HistoryError::InvalidProjectPath(
                "expected an absolute canonical path".into(),
            ));
        }
        let normalized = if raw.len() > 1 {
            raw.trim_end_matches('/')
        } else {
            raw
        };
        if normalized.is_empty() {
            Ok("/".into())
        } else {
            Ok(normalized.into())
        }
    }
}

fn looks_like_windows_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    (bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':')
        || path.starts_with("\\\\")
}

fn normalize_windows(path: &str) -> Result<String, HistoryError> {
    let mut path = path.to_owned();
    if starts_with_ascii_case_insensitive(&path, "\\\\?\\UNC\\") {
        path = format!("\\\\{}", &path[8..]);
    } else if path.starts_with("\\\\?\\") {
        path = path[4..].to_owned();
    } else if path.starts_with("\\\\.\\") {
        return Err(HistoryError::InvalidProjectPath(
            "device namespace paths are not supported".into(),
        ));
    }

    if let Some(path) = path.strip_prefix("\\\\") {
        let components = normalized_components(path)?;
        if components.len() < 2 {
            return Err(HistoryError::InvalidProjectPath(
                "UNC path must include a server and share".into(),
            ));
        }
        return Ok(format!("\\\\{}", components.join("\\")).to_ascii_lowercase());
    }

    let bytes = path.as_bytes();
    if bytes.len() < 3 || !bytes[0].is_ascii_alphabetic() || bytes[1] != b':' || bytes[2] != b'\\' {
        return Err(HistoryError::InvalidProjectPath(
            "Windows path must be drive-absolute or UNC".into(),
        ));
    }
    let drive = (bytes[0] as char).to_ascii_uppercase();
    let components = normalized_components(&path[3..])?;
    let suffix = if components.is_empty() {
        String::new()
    } else {
        format!("\\{}", components.join("\\"))
    };
    Ok(format!("{drive}:{suffix}").to_ascii_lowercase())
}

fn normalized_components(path: &str) -> Result<Vec<String>, HistoryError> {
    let mut result = Vec::new();
    for component in path.split('\\').filter(|part| !part.is_empty()) {
        match component {
            "." => {}
            ".." => {
                if result.pop().is_none() {
                    return Err(HistoryError::InvalidProjectPath(
                        "path escapes its filesystem root".into(),
                    ));
                }
            }
            _ => result.push(component.to_owned()),
        }
    }
    Ok(result)
}

fn starts_with_ascii_case_insensitive(value: &str, prefix: &str) -> bool {
    value
        .get(..prefix.len())
        .map(|head| head.eq_ignore_ascii_case(prefix))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::ProjectId;

    #[test]
    fn normalizes_windows_drive_and_extended_paths() {
        let extended = ProjectId::from_canonical_path(r"\\?\C:\Work\My Project\").unwrap();
        let ordinary = ProjectId::from_canonical_path(r"c:/work/my project").unwrap();
        assert_eq!(extended, ordinary);
        assert_eq!(extended.normalized_path(), r"c:\work\my project");
        assert_eq!(extended.storage_key().len(), 64);
    }

    #[test]
    fn normalizes_windows_unc_paths() {
        let extended = ProjectId::from_canonical_path(r"\\?\UNC\Server\Share\Repo").unwrap();
        let ordinary = ProjectId::from_canonical_path(r"\\server\share\repo").unwrap();
        assert_eq!(extended, ordinary);
        assert_eq!(extended.normalized_path(), r"\\server\share\repo");
    }
}
