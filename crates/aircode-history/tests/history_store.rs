use project_chat_history::{
    ChatMessage, ClearOutcome, CompactionResult, ContextBudget, HistoryError, HistoryLimits,
    HistoryRecord, HistoryStore, MessageRole, ProjectId,
};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static TEST_DIRECTORY_COUNTER: AtomicU64 = AtomicU64::new(0);

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new(name: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let sequence = TEST_DIRECTORY_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("history-tests")
            .join(format!("{name}-{}-{nonce}-{sequence}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn project(root: &Path, relative: &str) -> ProjectId {
    let path = root.join(relative);
    fs::create_dir_all(&path).unwrap();
    ProjectId::from_canonical_path(fs::canonicalize(path).unwrap()).unwrap()
}

fn message(id: &str, content: &str) -> ChatMessage {
    ChatMessage::new(id, MessageRole::User, content)
}

fn save_one(store: &HistoryStore, project: &ProjectId, text: &str, draft: &str) -> HistoryRecord {
    let mut record = HistoryRecord::new(project.clone());
    record.replace_contents(vec![message("message-1", text)], draft);
    store.save(&record).unwrap()
}

#[test]
fn restores_message_state_reasoning_and_error() {
    let temp = TestDirectory::new("message-state");
    let store = HistoryStore::new(temp.path().join("app-data"));
    let project = project(temp.path(), "project");
    let mut record = HistoryRecord::new(project.clone());
    record.set_visible_messages(vec![ChatMessage::new(
        "1",
        MessageRole::Assistant,
        "partial",
    )
    .with_display_state("failed", "reason", Some("network error".into()))]);
    store.save(&record).unwrap();
    let restored = store.load(&project).unwrap().unwrap();
    let message = &restored.visible_messages()[0];
    assert_eq!(message.state(), "failed");
    assert_eq!(message.reasoning(), "reason");
    assert_eq!(message.error(), Some("network error"));
}

#[test]
fn same_basename_projects_are_isolated() {
    let temp = TestDirectory::new("same-basename");
    let store = HistoryStore::new(temp.path().join("app-data"));
    let first = project(temp.path(), "first/project");
    let second = project(temp.path(), "second/project");

    assert_ne!(first.storage_key(), second.storage_key());
    assert_ne!(store.record_path(&first), store.record_path(&second));
    save_one(&store, &first, "first project", "first draft");
    save_one(&store, &second, "second project", "second draft");

    let first_loaded = store.load(&first).unwrap().unwrap();
    let second_loaded = store.load(&second).unwrap().unwrap();
    assert_eq!(
        first_loaded.visible_messages()[0].content(),
        "first project"
    );
    assert_eq!(first_loaded.input_draft(), "first draft");
    assert_eq!(
        second_loaded.visible_messages()[0].content(),
        "second project"
    );
    assert_eq!(second_loaded.input_draft(), "second draft");
    for project_id in [&first, &second] {
        let filename = store
            .record_path(project_id)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert_eq!(filename, format!("{}.json", project_id.storage_key()));
        assert!(!filename.contains("project"));
    }
}

#[test]
fn store_reconstruction_restores_messages_and_draft() {
    let temp = TestDirectory::new("restart");
    let root = temp.path().join("app-data");
    let project = project(temp.path(), "project");
    let saved = save_one(
        &HistoryStore::new(&root),
        &project,
        "persisted",
        "unfinished",
    );
    assert_eq!(saved.revision(), 1);

    let reopened_store = HistoryStore::new(&root);
    let reopened_project = ProjectId::from_canonical_path(project.normalized_path()).unwrap();
    let loaded = reopened_store.load(&reopened_project).unwrap().unwrap();
    assert_eq!(loaded, saved);
    assert_eq!(loaded.visible_messages()[0].content(), "persisted");
    assert_eq!(loaded.input_draft(), "unfinished");
}

#[test]
fn clear_requires_confirmation_and_does_not_cross_projects() {
    let temp = TestDirectory::new("confirmed-clear");
    let store = HistoryStore::new(temp.path().join("app-data"));
    let first = project(temp.path(), "first/project");
    let second = project(temp.path(), "second/project");
    save_one(&store, &first, "first", "");
    save_one(&store, &second, "second", "");

    assert_eq!(
        store.clear(&first, false).unwrap(),
        ClearOutcome::NotConfirmed
    );
    assert!(store.load(&first).unwrap().is_some());
    assert!(store.load(&second).unwrap().is_some());

    assert_eq!(store.clear(&first, true).unwrap(), ClearOutcome::Cleared);
    assert!(store.load(&first).unwrap().is_none());
    assert_eq!(
        store.load(&second).unwrap().unwrap().visible_messages()[0].content(),
        "second"
    );
    assert_eq!(store.clear(&first, true).unwrap(), ClearOutcome::NotFound);
}

#[test]
fn corruption_is_explicit_and_save_does_not_overwrite_it() {
    let temp = TestDirectory::new("corruption");
    let store = HistoryStore::new(temp.path().join("app-data"));
    let project = project(temp.path(), "project");
    let saved = save_one(&store, &project, "valid before corruption", "");
    let path = store.record_path(&project);
    let corrupt_bytes = b"{ definitely not valid JSON".to_vec();
    fs::write(&path, &corrupt_bytes).unwrap();

    assert!(matches!(
        store.load(&project),
        Err(HistoryError::CorruptRecord { .. })
    ));
    let mut attempted_save = saved.clone();
    attempted_save.set_input_draft("must not replace corrupt content");
    assert!(matches!(
        store.save(&attempted_save),
        Err(HistoryError::CorruptRecord { .. })
    ));
    assert_eq!(fs::read(&path).unwrap(), corrupt_bytes);

    assert_eq!(store.clear(&project, true).unwrap(), ClearOutcome::Cleared);
    assert!(store.load(&project).unwrap().is_none());
}

#[test]
fn failed_validation_preserves_last_valid_bytes() {
    let temp = TestDirectory::new("safe-write");
    let limits = HistoryLimits {
        max_draft_bytes: 3,
        ..HistoryLimits::default()
    };
    let store = HistoryStore::with_limits(temp.path().join("app-data"), limits).unwrap();
    let project = project(temp.path(), "project");
    let saved = save_one(&store, &project, "valid", "old");
    let path = store.record_path(&project);
    let valid_bytes = fs::read(&path).unwrap();

    let mut invalid = saved.clone();
    invalid.set_input_draft("four");
    assert!(matches!(
        store.save(&invalid),
        Err(HistoryError::LimitExceeded {
            resource: "input draft",
            ..
        })
    ));
    assert_eq!(fs::read(&path).unwrap(), valid_bytes);
    assert_eq!(store.load(&project).unwrap().unwrap(), saved);

    let mut valid = saved;
    valid.set_input_draft("new");
    store.save(&valid).unwrap();
    let temp_files = fs::read_dir(path.parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("tmp"))
        .count();
    assert_eq!(temp_files, 0);
}

#[test]
fn stale_compaction_cannot_replace_newer_history() {
    let temp = TestDirectory::new("stale-compaction");
    let store = HistoryStore::new(temp.path().join("app-data"));
    let project = project(temp.path(), "project");
    let mut record = HistoryRecord::new(project.clone());
    record.replace_contents(
        vec![
            message("m1", "one"),
            message("m2", "two"),
            message("m3", "three"),
            message("m4", "four"),
        ],
        "draft before compaction",
    );
    let saved = store.save(&record).unwrap();
    let budget = ContextBudget {
        max_messages: 2,
        max_input_bytes: 9,
    };
    let stale_plan = store.compaction_plan(&project, budget).unwrap().unwrap();
    assert_eq!(stale_plan.base_revision(), saved.revision());
    assert_eq!(stale_plan.included_messages().len(), 2);
    assert_eq!(stale_plan.included_messages()[0].id(), "m3");
    assert_eq!(stale_plan.omitted_older_messages(), 2);
    assert!(stale_plan.is_bounded_excerpt());

    let mut newer = saved.clone();
    let mut visible = newer.visible_messages().to_vec();
    visible.push(message("m5", "five"));
    newer.replace_contents(visible, "newer draft");
    let newer = store.save(&newer).unwrap();
    let stale_result = CompactionResult::new("stale summary", vec!["m4".into()], BTreeMap::new());
    assert!(matches!(
        store.apply_compaction(&project, &stale_plan, &stale_result),
        Err(HistoryError::StaleCompaction {
            base_revision: 1,
            current_revision: Some(2),
        })
    ));
    let after_stale = store.load(&project).unwrap().unwrap();
    assert_eq!(after_stale, newer);
    assert!(after_stale.compaction().summary().is_none());

    let current_plan = store.compaction_plan(&project, budget).unwrap().unwrap();
    let invalid_result = CompactionResult::new("", Vec::new(), BTreeMap::new());
    assert!(store
        .apply_compaction(&project, &current_plan, &invalid_result)
        .is_err());
    assert_eq!(store.load(&project).unwrap().unwrap(), newer);

    let mut metadata = BTreeMap::new();
    metadata.insert("model".into(), "configured-test-double".into());
    let valid_result = CompactionResult::new(
        "summary of only the supplied suffix",
        vec!["m5".into()],
        metadata,
    );
    let compacted = store
        .apply_compaction(&project, &current_plan, &valid_result)
        .unwrap();
    assert_eq!(compacted.revision(), newer.revision() + 1);
    assert_eq!(compacted.visible_messages(), newer.visible_messages());
    assert_eq!(compacted.input_draft(), "newer draft");
    let summary = compacted.compaction().summary().unwrap();
    assert_eq!(summary.based_on_revision(), newer.revision());
    assert_eq!(summary.included_message_count(), 2);
    assert_eq!(summary.omitted_older_messages(), 3);
    assert_eq!(
        compacted.compaction().retained_context().message_ids(),
        &["m5".to_owned()]
    );
    assert_eq!(
        compacted.compaction().metadata().get("model").unwrap(),
        "configured-test-double"
    );
}
