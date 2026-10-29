use crate::compaction::CompactionPlan;
use crate::model::{check_limit, Summary};
use crate::{
    CompactionResult, ContextBudget, HistoryError, HistoryLimits, HistoryRecord, ProjectId,
};
use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

static PROCESS_HISTORY_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClearOutcome {
    /// The caller did not confirm; no filesystem operation was attempted.
    NotConfirmed,
    Cleared,
    NotFound,
}

/// File-backed project history rooted at a caller-provided directory.
///
/// Operations are serialized process-wide, including across multiple store
/// instances. Coordinate external processes separately: this crate does not
/// provide an inter-process writer lock.
#[derive(Clone, Debug)]
pub struct HistoryStore {
    application_data_root: PathBuf,
    limits: HistoryLimits,
}

impl HistoryStore {
    pub fn new(application_data_root: impl Into<PathBuf>) -> Self {
        Self {
            application_data_root: application_data_root.into(),
            limits: HistoryLimits::default(),
        }
    }

    pub fn with_limits(
        application_data_root: impl Into<PathBuf>,
        limits: HistoryLimits,
    ) -> Result<Self, HistoryError> {
        limits.validate()?;
        Ok(Self {
            application_data_root: application_data_root.into(),
            limits,
        })
    }

    pub fn application_data_root(&self) -> &Path {
        &self.application_data_root
    }

    pub fn limits(&self) -> &HistoryLimits {
        &self.limits
    }

    /// The exact record path. The filename is SHA-256 hex, never a project path.
    pub fn record_path(&self, project: &ProjectId) -> PathBuf {
        self.projects_directory()
            .join(format!("{}.json", project.storage_key()))
    }

    /// Loads and validates a record. Malformed, inconsistent, oversized, and
    /// wrong-version files return explicit errors and are never auto-replaced.
    pub fn load(&self, project: &ProjectId) -> Result<Option<HistoryRecord>, HistoryError> {
        let _guard = acquire_history_lock()?;
        self.load_unlocked(project)
    }

    pub fn load_or_default(&self, project: &ProjectId) -> Result<HistoryRecord, HistoryError> {
        let _guard = acquire_history_lock()?;
        Ok(self
            .load_unlocked(project)?
            .unwrap_or_else(|| HistoryRecord::new(project.clone())))
    }

    /// Compare-and-set save of visible messages and draft.
    ///
    /// Save a revision-zero record only when no record exists, or save a loaded
    /// snapshot only while its revision is current. The returned record has the
    /// incremented revision. Validation happens before any write, and corrupt
    /// existing content blocks saving rather than being overwritten.
    pub fn save(&self, record: &HistoryRecord) -> Result<HistoryRecord, HistoryError> {
        let _guard = acquire_history_lock()?;
        let project = record.project();
        let current = self.load_unlocked(project)?;
        let current_revision = current.as_ref().map(HistoryRecord::revision);
        if current_revision != expected_revision_for_save(record.revision()) {
            return Err(HistoryError::RevisionConflict {
                expected: record.revision(),
                actual: current_revision,
            });
        }

        let mut candidate = record.clone();
        let next_revision = candidate.next_revision()?;
        candidate.set_revision(next_revision);
        self.write_unlocked(&candidate)
    }

    /// Builds a bounded model-input snapshot without modifying the record.
    /// Returns `Ok(None)` when there are no visible messages.
    pub fn compaction_plan(
        &self,
        project: &ProjectId,
        budget: ContextBudget,
    ) -> Result<Option<CompactionPlan>, HistoryError> {
        let _guard = acquire_history_lock()?;
        match self.load_unlocked(project)? {
            Some(record) => CompactionPlan::build(&record, budget),
            None => Ok(None),
        }
    }

    /// Applies an asynchronously produced summary only if the plan's project
    /// and base revision still match. Visible messages and input draft are
    /// copied unchanged. Stale plans return an error before any write.
    pub fn apply_compaction(
        &self,
        project: &ProjectId,
        plan: &CompactionPlan,
        result: &CompactionResult,
    ) -> Result<HistoryRecord, HistoryError> {
        if plan.project() != project {
            return Err(HistoryError::ProjectMismatch);
        }
        let _guard = acquire_history_lock()?;
        let Some(mut candidate) = self.load_unlocked(project)? else {
            return Err(HistoryError::StaleCompaction {
                base_revision: plan.base_revision(),
                current_revision: None,
            });
        };
        if candidate.revision() != plan.base_revision() {
            return Err(HistoryError::StaleCompaction {
                base_revision: plan.base_revision(),
                current_revision: Some(candidate.revision()),
            });
        }
        if !plan.matches_record(&candidate) {
            return Err(HistoryError::InvalidRecord(
                "compaction plan does not match its claimed snapshot".into(),
            ));
        }

        let allowed_ids: BTreeSet<&str> = plan
            .included_messages()
            .iter()
            .map(|message| message.id())
            .collect();
        let mut unique_retained = BTreeSet::new();
        for id in result.retained_message_ids() {
            if !allowed_ids.contains(id.as_str()) {
                return Err(HistoryError::InvalidRecord(format!(
                    "retained message ID was not in the compaction plan: {id}"
                )));
            }
            if !unique_retained.insert(id.as_str()) {
                return Err(HistoryError::InvalidRecord(format!(
                    "duplicate retained message ID: {id}"
                )));
            }
        }

        let summary = Summary::new(
            result.summary().to_owned(),
            plan.base_revision(),
            plan.source_message_count(),
            plan.included_messages().len(),
            plan.omitted_older_messages(),
        );
        let next_revision = candidate.next_revision()?;
        candidate.replace_compaction(
            summary,
            result.retained_message_ids().to_vec(),
            result.metadata().clone(),
        );
        candidate.set_revision(next_revision);
        self.write_unlocked(&candidate)
    }

    /// Removes only the exact project's hashed record after explicit `true`.
    /// `false` is a no-op. This can remove a corrupt record after confirmation,
    /// providing explicit recovery without automatic overwrite or quarantine.
    pub fn clear(
        &self,
        project: &ProjectId,
        confirmed: bool,
    ) -> Result<ClearOutcome, HistoryError> {
        if !confirmed {
            return Ok(ClearOutcome::NotConfirmed);
        }
        let _guard = acquire_history_lock()?;
        let path = self.record_path(project);
        match fs::remove_file(&path) {
            Ok(()) => {
                sync_parent_directory(path.parent())?;
                Ok(ClearOutcome::Cleared)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(ClearOutcome::NotFound),
            Err(source) => Err(HistoryError::Io {
                operation: "clear project history",
                path,
                source,
            }),
        }
    }

    /// Clear only the revision the caller has actually loaded. A stale or
    /// unreadable record is never removed by this operation.
    pub fn clear_if_revision(
        &self,
        project: &ProjectId,
        expected: u64,
    ) -> Result<ClearOutcome, HistoryError> {
        let _guard = acquire_history_lock()?;
        let current = self.load_unlocked(project)?;
        let actual = current.as_ref().map(HistoryRecord::revision);
        if actual != expected_revision_for_save(expected) {
            return Err(HistoryError::RevisionConflict { expected, actual });
        }
        if current.is_none() {
            return Ok(ClearOutcome::NotFound);
        }
        let path = self.record_path(project);
        fs::remove_file(&path).map_err(|source| HistoryError::Io {
            operation: "clear project history",
            path: path.clone(),
            source,
        })?;
        sync_parent_directory(path.parent())?;
        Ok(ClearOutcome::Cleared)
    }

    fn projects_directory(&self) -> PathBuf {
        self.application_data_root
            .join("project-chat-history")
            .join("v1")
            .join("projects")
    }

    fn load_unlocked(&self, project: &ProjectId) -> Result<Option<HistoryRecord>, HistoryError> {
        project.validate()?;
        let path = self.record_path(project);
        let mut file = match File::open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(source) => {
                return Err(HistoryError::Io {
                    operation: "open project history",
                    path,
                    source,
                })
            }
        };
        let metadata = file.metadata().map_err(|source| HistoryError::Io {
            operation: "inspect project history",
            path: path.clone(),
            source,
        })?;
        let file_bytes = usize::try_from(metadata.len()).unwrap_or(usize::MAX);
        check_limit(
            "serialized history record",
            file_bytes,
            self.limits.max_record_bytes,
        )?;

        let read_cap = (self.limits.max_record_bytes as u64).saturating_add(1);
        let mut bytes = Vec::new();
        std::io::Read::by_ref(&mut file)
            .take(read_cap)
            .read_to_end(&mut bytes)
            .map_err(|source| HistoryError::Io {
                operation: "read project history",
                path: path.clone(),
                source,
            })?;
        check_limit(
            "serialized history record",
            bytes.len(),
            self.limits.max_record_bytes,
        )?;

        let record: HistoryRecord =
            serde_json::from_slice(&bytes).map_err(|error| HistoryError::CorruptRecord {
                path: path.clone(),
                reason: error.to_string(),
            })?;
        if record.schema_version() != crate::SCHEMA_VERSION {
            return Err(HistoryError::UnsupportedSchemaVersion {
                found: record.schema_version(),
            });
        }
        if let Err(error) = record.validate(&self.limits) {
            return Err(match error {
                HistoryError::UnsupportedSchemaVersion { .. }
                | HistoryError::LimitExceeded { .. } => error,
                other => HistoryError::CorruptRecord {
                    path,
                    reason: other.to_string(),
                },
            });
        }
        if record.project() != project {
            return Err(HistoryError::CorruptRecord {
                path,
                reason: "record project identity does not match its SHA-256 filename".into(),
            });
        }
        if record.revision() == 0 {
            return Err(HistoryError::CorruptRecord {
                path,
                reason: "persisted records must have a nonzero revision".into(),
            });
        }
        Ok(Some(record))
    }

    fn write_unlocked(&self, record: &HistoryRecord) -> Result<HistoryRecord, HistoryError> {
        record.validate(&self.limits)?;
        if record.revision() == 0 {
            return Err(HistoryError::InvalidRecord(
                "cannot persist a revision-zero record".into(),
            ));
        }
        let bytes = serde_json::to_vec(record).map_err(HistoryError::Serialization)?;
        check_limit(
            "serialized history record",
            bytes.len(),
            self.limits.max_record_bytes,
        )?;
        let path = self.record_path(record.project());
        atomic_write(&path, &bytes)?;
        Ok(record.clone())
    }
}

fn expected_revision_for_save(snapshot_revision: u64) -> Option<u64> {
    if snapshot_revision == 0 {
        None
    } else {
        Some(snapshot_revision)
    }
}

fn acquire_history_lock() -> Result<MutexGuard<'static, ()>, HistoryError> {
    PROCESS_HISTORY_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| HistoryError::LockPoisoned)
}

fn atomic_write(destination: &Path, bytes: &[u8]) -> Result<(), HistoryError> {
    let directory = destination
        .parent()
        .ok_or_else(|| HistoryError::InvalidRecord("record path has no parent".into()))?;
    fs::create_dir_all(directory).map_err(|source| HistoryError::Io {
        operation: "create project history directory",
        path: directory.to_path_buf(),
        source,
    })?;

    let (temporary_path, mut temporary_file) = create_temporary_file(directory)?;
    let result = (|| {
        temporary_file
            .write_all(bytes)
            .map_err(|source| HistoryError::Io {
                operation: "write temporary project history",
                path: temporary_path.clone(),
                source,
            })?;
        temporary_file
            .sync_all()
            .map_err(|source| HistoryError::Io {
                operation: "flush temporary project history",
                path: temporary_path.clone(),
                source,
            })?;
        drop(temporary_file);
        replace_file(&temporary_path, destination).map_err(|source| HistoryError::Io {
            operation: "atomically replace project history",
            path: destination.to_path_buf(),
            source,
        })?;
        sync_parent_directory(Some(directory))?;
        Ok(())
    })();

    if result.is_err() {
        // A crash can leave a unique temp file. Loads ignore it; a later save
        // never reuses or mistakes it for the last valid committed record.
        let _ = fs::remove_file(&temporary_path);
    }
    result
}

fn create_temporary_file(directory: &Path) -> Result<(PathBuf, File), HistoryError> {
    for _ in 0..128 {
        let counter = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let path = directory.join(format!(".{}.{}.{}.tmp", std::process::id(), nanos, counter));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(source) => {
                return Err(HistoryError::Io {
                    operation: "create temporary project history",
                    path,
                    source,
                })
            }
        }
    }
    Err(HistoryError::Io {
        operation: "create unique temporary project history",
        path: directory.to_path_buf(),
        source: io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not allocate a unique temporary filename",
        ),
    })
}

#[cfg(not(windows))]
fn replace_file(temporary: &Path, destination: &Path) -> io::Result<()> {
    // On Unix, rename atomically replaces an existing file in the same directory.
    fs::rename(temporary, destination)
}

#[cfg(windows)]
fn replace_file(temporary: &Path, destination: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;

    const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x8;

    #[link(name = "kernel32")]
    extern "system" {
        fn MoveFileExW(existing: *const u16, replacement: *const u16, flags: u32) -> i32;
    }

    // Canonical Windows paths carry the extended-length prefix required by
    // MoveFileExW when application-data directories exceed MAX_PATH.
    let temporary = fs::canonicalize(temporary)?;
    let parent = fs::canonicalize(destination.parent().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "destination has no parent")
    })?)?;
    let destination = parent.join(destination.file_name().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "destination has no filename")
    })?);
    let existing: Vec<u16> = temporary.as_os_str().encode_wide().chain(Some(0)).collect();
    let replacement: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    // SAFETY: both UTF-16 buffers are NUL-terminated and remain alive for the
    // call. Windows replaces an existing destination with these flags, unlike
    // std::fs::rename, and both paths are in the same directory/filesystem.
    let succeeded = unsafe {
        MoveFileExW(
            existing.as_ptr(),
            replacement.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if succeeded == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(unix)]
fn sync_parent_directory(directory: Option<&Path>) -> Result<(), HistoryError> {
    let Some(directory) = directory else {
        return Ok(());
    };
    File::open(directory)
        .and_then(|directory_file| directory_file.sync_all())
        .map_err(|source| HistoryError::Io {
            operation: "flush project history directory",
            path: directory.to_path_buf(),
            source,
        })
}

#[cfg(not(unix))]
fn sync_parent_directory(_directory: Option<&Path>) -> Result<(), HistoryError> {
    // MoveFileExW uses MOVEFILE_WRITE_THROUGH on Windows. Other platforms use
    // the atomic rename and file flush above.
    Ok(())
}
