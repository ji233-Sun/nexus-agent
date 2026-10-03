use super::*;
use crate::infrastructure::attachments;

#[test]
fn persists_history_and_recovers_active_runs() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("nexus.db");
    let project_dir = directory.path().join("project");
    fs::create_dir(&project_dir).unwrap();

    let mut storage = Storage::open(&database).unwrap();
    let project = storage.open_project(&project_dir).unwrap();
    use base64::Engine as _;
    let png = base64::engine::general_purpose::STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=").unwrap();
    let image =
        attachments::save_pdf_capture(&storage.attachment_directory(), "报告.pdf", 12, &png)
            .unwrap();
    let source = directory.path().join("说明.txt");
    fs::write(&source, "original file contents").unwrap();
    let file = attachments::import_attachment(&storage.attachment_directory(), &source).unwrap();
    assert!(!file.is_image());
    fs::write(&source, "changed after import").unwrap();
    let attachments = vec![image.clone(), file.clone()];
    let (task_id, run_id) = storage
        .create_task_run(NewTaskRun {
            attachments: &attachments,
            workspace_id: None,
            permission_mode: PermissionMode::Ask,
            task_id: None,
            project_id: Some(project.id),
            title: "Test task",
            prompt: "hello",
            harness: HarnessKind::Claude,
            executable: "claude-custom",
            model: Some("sonnet"),
            effort: ThinkingEffort::High,
            harness_version: Some("1.2.3"),
        })
        .unwrap();
    storage
        .update_run_status(run_id, RunStatus::Running)
        .unwrap();
    assert!(
        storage
            .update_task_title(task_id, "Generated title")
            .unwrap()
    );
    assert!(
        !storage
            .update_task_title(task_id, "Generated title")
            .unwrap()
    );
    storage.save_run_session(run_id, "claude-session").unwrap();
    drop(storage);

    let storage = Storage::open(&database).unwrap();
    let tasks = storage.tasks(project.id).unwrap();
    assert_eq!(tasks[0].status, RunStatus::Interrupted);
    assert_eq!(tasks[0].title, "Generated title");
    let messages = storage.messages(task_id).unwrap();
    assert_eq!(messages[0].content, "hello");
    assert_eq!(messages[0].attachments, attachments);
    assert_eq!(std::fs::read(&image.path).unwrap(), png);
    assert_eq!(
        fs::read_to_string(&file.path).unwrap(),
        "original file contents"
    );
    let config = storage.conversation_config(task_id).unwrap().unwrap();
    assert_eq!(config.harness, HarnessKind::Claude);
    assert_eq!(config.session_id.as_deref(), Some("claude-session"));
    assert_eq!(config.executable, "claude-custom");
    assert_eq!(config.model, "sonnet");
    assert_eq!(config.effort, ThinkingEffort::High);
    assert_eq!(config.permission_mode, PermissionMode::Ask);
    let workspace = storage.task_workspace(task_id).unwrap().unwrap();
    assert_eq!(workspace.path, project.canonical_path);
    let cwd: String = storage
        .connection
        .query_row(
            "SELECT cwd FROM runs WHERE id = ?1",
            [run_id.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(cwd, workspace.path);
    let harness_version: String = storage
        .connection
        .query_row(
            "SELECT harness_version FROM runs WHERE id = ?1",
            [run_id.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(harness_version, "1.2.3");
}

#[test]
fn persists_completed_codex_runs_for_later_browsing() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("nexus.db");
    let project_dir = directory.path().join("project");
    fs::create_dir(&project_dir).unwrap();

    let mut storage = Storage::open(&database).unwrap();
    let project = storage.open_project(&project_dir).unwrap();
    let (task_id, run_id) = storage
        .create_task_run(NewTaskRun {
            attachments: &[],
            workspace_id: None,
            permission_mode: nexus_domain::PermissionMode::AutoEdit,
            task_id: None,
            project_id: Some(project.id),
            title: "Codex task",
            prompt: "describe this project",
            harness: HarnessKind::Codex,
            executable: "codex",
            model: None,
            effort: ThinkingEffort::Medium,
            harness_version: Some("4.5.6"),
        })
        .unwrap();
    storage
        .append_message(
            task_id,
            run_id,
            MessageRole::Assistant,
            MessageKind::Text,
            "project summary",
            None,
        )
        .unwrap();
    storage
        .update_run_status(run_id, RunStatus::Running)
        .unwrap();
    assert!(storage.completed_runs(task_id).unwrap().is_empty());
    storage
        .finish_run(run_id, RunStatus::Completed, Some(0))
        .unwrap();
    drop(storage);

    let storage = Storage::open(&database).unwrap();
    let tasks = storage.tasks(project.id).unwrap();
    assert_eq!(tasks[0].status, RunStatus::Completed);
    assert_eq!(
        storage.completed_runs(task_id).unwrap(),
        HashSet::from([run_id])
    );
    assert!(storage.completed_runs(Uuid::new_v4()).unwrap().is_empty());
    let messages = storage.messages(task_id).unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].content, "describe this project");
    assert_eq!(messages[1].content, "project summary");
    assert!(messages.iter().all(|message| message.tool.is_none()));

    // Simulate the previous schema with real messages, then migrate in place.
    storage
        .connection
        .pragma_update(None, "user_version", 9)
        .unwrap();
    storage
        .connection
        .execute("ALTER TABLE runs DROP COLUMN permission_mode", [])
        .unwrap();
    storage
        .connection
        .execute("ALTER TABLE messages DROP COLUMN tool", [])
        .unwrap();
    storage
        .connection
        .execute("ALTER TABLE tasks DROP COLUMN session_id", [])
        .unwrap();
    storage
        .connection
        .execute("ALTER TABLE tasks DROP COLUMN archived_at", [])
        .unwrap();
    storage
        .connection
        .execute("ALTER TABLE tasks DROP COLUMN workspace_id", [])
        .unwrap();
    storage
        .connection
        .execute("ALTER TABLE runs DROP COLUMN cwd", [])
        .unwrap();
    drop(storage);
    let storage = Storage::open(&database).unwrap();
    assert_eq!(
        storage.task_workspace(task_id).unwrap().unwrap().path,
        project.canonical_path
    );
    assert_eq!(
        storage.messages(task_id).unwrap()[1].content,
        "project summary"
    );
    assert_eq!(
        storage
            .conversation_config(task_id)
            .unwrap()
            .unwrap()
            .permission_mode,
        PermissionMode::AutoEdit
    );
    assert!(
        storage
            .conversation_config(task_id)
            .unwrap()
            .unwrap()
            .session_id
            .is_none()
    );
    let content = "完整工具输出\n".repeat(100);
    let metadata = ToolMetadata {
        id: "command-1".into(),
        is_error: true,
    };
    storage
        .append_message(
            task_id,
            run_id,
            MessageRole::Tool,
            MessageKind::ToolResult,
            &content,
            Some(metadata.clone()),
        )
        .unwrap();
    drop(storage);
    let storage = Storage::open(&database).unwrap();
    let messages = storage.messages(task_id).unwrap();
    assert_eq!(messages[2].content, content);
    assert_eq!(messages[2].tool.as_ref(), Some(&metadata));
}

#[test]
fn archives_restores_and_deletes_conversations_with_their_history() {
    let directory = tempfile::tempdir().unwrap();
    let project_dir = directory.path().join("project");
    fs::create_dir(&project_dir).unwrap();
    let mut storage = Storage::open(&directory.path().join("nexus.db")).unwrap();
    let project = storage.open_project(&project_dir).unwrap();
    let create_task = |storage: &mut Storage, title: &str| {
        storage
            .create_task_run(NewTaskRun {
                attachments: &[],
                workspace_id: None,
                permission_mode: nexus_domain::PermissionMode::AutoEdit,
                task_id: None,
                project_id: Some(project.id),
                title,
                prompt: title,
                harness: HarnessKind::Claude,
                executable: "claude",
                model: None,
                effort: ThinkingEffort::Medium,
                harness_version: None,
            })
            .unwrap()
    };
    let (first_task, _) = create_task(&mut storage, "First task");
    let (second_task, _) = create_task(&mut storage, "Second task");

    storage.archive_task(first_task).unwrap();
    assert_eq!(storage.tasks(project.id).unwrap()[0].id, second_task);
    assert_eq!(storage.archived_tasks().unwrap()[0].id, first_task);

    storage.restore_task(first_task).unwrap();
    assert_eq!(storage.tasks(project.id).unwrap().len(), 2);
    assert!(storage.archived_tasks().unwrap().is_empty());

    storage.archive_task(first_task).unwrap();
    storage.archive_task(second_task).unwrap();
    storage.delete_task(first_task).unwrap();
    assert_eq!(storage.archived_tasks().unwrap()[0].id, second_task);
    let first_run_count = storage
        .connection
        .query_row(
            "SELECT COUNT(*) FROM runs WHERE task_id = ?1",
            [first_task.to_string()],
            |row| row.get::<_, i64>(0),
        )
        .unwrap();
    let first_message_count = storage
        .connection
        .query_row(
            "SELECT COUNT(*) FROM messages WHERE task_id = ?1",
            [first_task.to_string()],
            |row| row.get::<_, i64>(0),
        )
        .unwrap();
    assert_eq!((first_run_count, first_message_count), (0, 0));

    assert_eq!(storage.delete_archived_tasks().unwrap(), 1);
    assert!(storage.archived_tasks().unwrap().is_empty());
    assert!(storage.tasks(project.id).unwrap().is_empty());
}

#[test]
fn project_deletion_is_atomic_persistent_and_scoped_to_its_records() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("nexus.db");
    let project_dir = directory.path().join("project");
    let other_dir = directory.path().join("other");
    fs::create_dir(&project_dir).unwrap();
    fs::create_dir(&other_dir).unwrap();
    fs::write(project_dir.join("keep.txt"), "local changes").unwrap();
    let mut storage = Storage::open(&database).unwrap();
    let project = storage.open_project(&project_dir).unwrap();
    let other = storage.open_project(&other_dir).unwrap();
    let workspace_mode = format!("workspace_mode:{}", project.id);
    storage.set_setting(&workspace_mode, "worktree").unwrap();
    storage.set_setting("language", "en").unwrap();
    let tasks = [project.id, project.id, other.id].map(|project_id| {
        storage
            .create_task_run(NewTaskRun {
                attachments: &[],
                task_id: None,
                workspace_id: None,
                project_id: Some(project_id),
                title: "Conversation",
                prompt: "Saved message",
                harness: HarnessKind::Claude,
                executable: "claude",
                model: None,
                effort: ThinkingEffort::Default,
                permission_mode: PermissionMode::AutoEdit,
                harness_version: None,
            })
            .unwrap()
            .0
    });
    storage.archive_task(tasks[0]).unwrap();

    storage
        .connection
        .execute_batch(
            "CREATE TEMP TRIGGER reject_project_delete BEFORE DELETE ON projects
             BEGIN SELECT RAISE(ABORT, 'test deletion failure'); END;",
        )
        .unwrap();
    assert!(storage.delete_project(project.id).is_err());
    assert!(storage.project(project.id).unwrap().is_some());
    assert_eq!(storage.tasks(project.id).unwrap()[0].id, tasks[1]);
    assert_eq!(storage.archived_tasks().unwrap()[0].id, tasks[0]);
    for task_id in tasks {
        assert_eq!(
            storage.messages(task_id).unwrap()[0].content,
            "Saved message"
        );
        assert!(storage.conversation_config(task_id).unwrap().is_some());
        assert!(storage.task_workspace(task_id).unwrap().is_some());
    }
    assert_eq!(
        storage.setting(&workspace_mode).unwrap().as_deref(),
        Some("worktree")
    );
    storage
        .connection
        .execute_batch("DROP TRIGGER reject_project_delete;")
        .unwrap();

    storage.delete_project(project.id).unwrap();
    drop(storage);
    let mut storage = Storage::open(&database).unwrap();
    assert!(storage.project(project.id).unwrap().is_none());
    assert_eq!(storage.projects().unwrap()[0].id, other.id);
    assert_eq!(storage.projects().unwrap().len(), 1);
    assert!(storage.tasks(project.id).unwrap().is_empty());
    assert!(storage.archived_tasks().unwrap().is_empty());
    assert!(storage.workspaces(project.id).unwrap().is_empty());
    assert!(storage.setting(&workspace_mode).unwrap().is_none());
    for task_id in &tasks[..2] {
        assert!(storage.messages(*task_id).unwrap().is_empty());
        assert!(storage.conversation_config(*task_id).unwrap().is_none());
    }
    assert_eq!(storage.tasks(other.id).unwrap()[0].id, tasks[2]);
    assert_eq!(
        storage.messages(tasks[2]).unwrap()[0].content,
        "Saved message"
    );
    assert!(storage.conversation_config(tasks[2]).unwrap().is_some());
    assert!(storage.task_workspace(tasks[2]).unwrap().is_some());
    assert_eq!(storage.setting("language").unwrap().as_deref(), Some("en"));
    assert_eq!(
        fs::read_to_string(project_dir.join("keep.txt")).unwrap(),
        "local changes"
    );

    assert!(storage.delete_project(project.id).is_err());
    let reopened = storage.open_project(&project_dir).unwrap();
    assert_ne!(reopened.id, project.id);
    assert!(storage.tasks(reopened.id).unwrap().is_empty());
    storage.delete_project(reopened.id).unwrap();
    assert_eq!(storage.projects().unwrap().len(), 1);
}

#[test]
fn settings_round_trip() {
    let directory = tempfile::tempdir().unwrap();
    let storage = Storage::open(&directory.path().join("nexus.db")).unwrap();
    storage.set_setting("model", "opus").unwrap();
    assert_eq!(storage.setting("model").unwrap().as_deref(), Some("opus"));
}

#[test]
fn project_order_persists_across_restarts_and_new_projects() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("nexus.db");
    let storage = Storage::open(&database).unwrap();
    let mut projects = Vec::new();
    for name in ["first", "second", "third"] {
        let path = directory.path().join(name);
        fs::create_dir(&path).unwrap();
        projects.push(storage.open_project(&path).unwrap());
    }
    let ids = |storage: &Storage| {
        storage
            .projects()
            .unwrap()
            .iter()
            .map(|project| project.id)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        ids(&storage),
        vec![projects[2].id, projects[1].id, projects[0].id]
    );
    let order = vec![projects[0].id, projects[2].id, projects[1].id];
    storage.save_project_order(&order).unwrap();
    for project in &projects {
        assert_eq!(
            storage.project(project.id).unwrap().unwrap().last_opened_at,
            project.last_opened_at,
        );
    }
    drop(storage);

    let storage = Storage::open(&database).unwrap();
    assert_eq!(ids(&storage), order);
    storage
        .open_project(Path::new(&projects[1].canonical_path))
        .unwrap();
    assert_eq!(ids(&storage), order);
    let path = directory.path().join("new-project");
    fs::create_dir(&path).unwrap();
    let new_project = storage.open_project(&path).unwrap();
    assert_eq!(ids(&storage), [order, vec![new_project.id]].concat());

    storage
        .set_setting("project_order", "invalid JSON")
        .unwrap();
    assert_eq!(
        ids(&storage),
        vec![
            new_project.id,
            projects[1].id,
            projects[2].id,
            projects[0].id
        ],
    );
}

#[test]
fn provider_profile_metadata_round_trips_without_a_secret() {
    let directory = tempfile::tempdir().unwrap();
    let storage = Storage::open(&directory.path().join("nexus.db")).unwrap();
    let profile = ProviderProfile {
        id: Uuid::new_v4(),
        name: "DeepSeek".into(),
        harness: HarnessKind::Omp,
        api_key_env: "DEEPSEEK_API_KEY".into(),
        base_url_env: None,
        base_url: None,
        model: Some("deepseek/deepseek-v4-pro".into()),
        credential_configured: true,
    };

    storage
        .set_provider_profiles(std::slice::from_ref(&profile))
        .unwrap();

    let mut expected = profile;
    expected.credential_configured = false;
    assert_eq!(storage.provider_profiles().unwrap(), vec![expected]);
    let metadata = storage.setting("provider_profiles").unwrap().unwrap();
    assert!(!metadata.contains("secret"));
    assert!(!metadata.contains("credential_configured"));
}

#[test]
fn migrates_legacy_project_history_and_workspaces_and_allows_projectless_tasks() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("nexus.db");
    let connection = Connection::open(&database).unwrap();
    let project = Project {
        id: Uuid::new_v4(),
        display_name: "Legacy project".into(),
        canonical_path: directory
            .path()
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .into_owned(),
        created_at: Utc::now(),
        last_opened_at: Utc::now(),
    };
    let task_id = Uuid::new_v4();
    let run_id = Uuid::new_v4();
    let mut workspace = Workspace::local(&project);
    workspace.id = task_id;
    workspace.task_id = Some(task_id);
    workspace.kind = WorkspaceKind::Worktree;
    workspace.managed = true;
    workspace.branch = Some("feat/legacy".into());
    let record = serde_json::to_string(&workspace).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE projects (
                id TEXT PRIMARY KEY, display_name TEXT NOT NULL,
                canonical_path TEXT NOT NULL UNIQUE, created_at TEXT NOT NULL,
                last_opened_at TEXT NOT NULL
            );
            CREATE TABLE workspaces (
                id TEXT PRIMARY KEY, project_id TEXT NOT NULL REFERENCES projects(id),
                record TEXT NOT NULL
            );
            CREATE TABLE tasks (
                id TEXT PRIMARY KEY, project_id TEXT NOT NULL REFERENCES projects(id),
                title TEXT NOT NULL, status TEXT NOT NULL, created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL, archived_at TEXT, session_id TEXT,
                workspace_id TEXT REFERENCES workspaces(id)
            );
            CREATE TABLE messages (
                id TEXT PRIMARY KEY, task_id TEXT NOT NULL REFERENCES tasks(id),
                run_id TEXT NOT NULL REFERENCES runs(id), sequence INTEGER NOT NULL,
                role TEXT NOT NULL, kind TEXT NOT NULL, content TEXT NOT NULL,
                created_at TEXT NOT NULL, UNIQUE(task_id, sequence)
            );
            CREATE TABLE runs (
                id TEXT PRIMARY KEY,
                task_id TEXT NOT NULL REFERENCES tasks(id),
                status TEXT NOT NULL,
                model TEXT NOT NULL,
                effort TEXT NOT NULL,
                harness_version TEXT,
                started_at TEXT NOT NULL,
                ended_at TEXT,
                exit_code INTEGER,
                failure_code TEXT
            );",
        )
        .unwrap();
    connection.pragma_update(None, "user_version", 9).unwrap();
    let now = Utc::now().to_rfc3339();
    connection
        .execute(
            "INSERT INTO projects VALUES(?1, ?2, ?3, ?4, ?4)",
            params![
                project.id.to_string(),
                project.display_name,
                project.canonical_path,
                now
            ],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO workspaces VALUES(?1, ?2, ?3)",
            params![task_id.to_string(), project.id.to_string(), record],
        )
        .unwrap();
    connection.execute("INSERT INTO tasks VALUES(?1, ?2, 'Legacy task', 'completed', ?3, ?3, ?3, 'legacy-session', ?1)",
        params![task_id.to_string(), project.id.to_string(), now]).unwrap();
    connection.execute("INSERT INTO runs(id, task_id, status, model, effort, started_at) VALUES(?1, ?2, 'completed', 'sonnet', 'high', ?3)",
        params![run_id.to_string(), task_id.to_string(), now]).unwrap();
    connection
        .execute(
            "INSERT INTO messages VALUES(?1, ?2, ?3, 1, 'assistant', 'text', 'Saved reply', ?4)",
            params![
                Uuid::new_v4().to_string(),
                task_id.to_string(),
                run_id.to_string(),
                now
            ],
        )
        .unwrap();
    drop(connection);

    let mut storage = Storage::open(&database).unwrap();
    assert_eq!(storage.archived_tasks().unwrap()[0].id, task_id);
    assert_eq!(
        storage
            .connection
            .query_row("PRAGMA user_version", [], |row| row.get::<_, u32>(0))
            .unwrap(),
        migrations::CURRENT_VERSION
    );
    assert_eq!(storage.messages(task_id).unwrap()[0].content, "Saved reply");
    let config = storage.conversation_config(task_id).unwrap().unwrap();
    assert_eq!(config.harness, HarnessKind::Claude);
    assert_eq!(config.session_id.as_deref(), Some("legacy-session"));
    assert_eq!(config.model, "sonnet");
    assert_eq!(config.effort, ThinkingEffort::High);
    assert_eq!(
        serde_json::to_string(&storage.task_workspace(task_id).unwrap().unwrap()).unwrap(),
        record
    );
    assert!(
        !storage
            .connection
            .prepare("PRAGMA foreign_key_check")
            .unwrap()
            .exists([])
            .unwrap()
    );
    let foreign_keys: bool = storage
        .connection
        .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
        .unwrap();
    assert!(foreign_keys);
    let chat = storage
        .prepare_projectless_workspace(Uuid::new_v4())
        .unwrap();
    let (chat_id, _) = storage
        .create_task_run(NewTaskRun {
            attachments: &[],
            task_id: None,
            workspace_id: Some(chat.id),
            project_id: None,
            title: "Hello",
            prompt: "Hello",
            harness: HarnessKind::Claude,
            executable: "claude",
            model: None,
            effort: ThinkingEffort::Default,
            permission_mode: PermissionMode::Ask,
            harness_version: None,
        })
        .unwrap();
    storage.restore_task(task_id).unwrap();
    drop(storage);
    let storage = Storage::open(&database).unwrap();
    assert_eq!(storage.tasks(None).unwrap()[0].id, chat_id);
    assert_eq!(storage.tasks(project.id).unwrap()[0].id, task_id);
    assert_eq!(
        storage.task_workspace(chat_id).unwrap().unwrap().path,
        chat.path
    );
    assert!(
        storage
            .task_workspace(chat_id)
            .unwrap()
            .unwrap()
            .project_id
            .is_none()
    );
}

#[test]
fn migration_failure_rolls_back_schema_and_version() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("broken.db");
    let connection = Connection::open(&database).unwrap();
    connection
        .execute_batch(include_str!("schema.sql"))
        .unwrap();
    connection
        .pragma_update(None, "foreign_keys", false)
        .unwrap();
    connection.pragma_update(None, "user_version", 9).unwrap();
    connection
        .execute(
            "INSERT INTO tasks VALUES(?1, ?2, 'Legacy task', 'completed', ?3, ?3, NULL)",
            params![
                Uuid::new_v4().to_string(),
                Uuid::new_v4().to_string(),
                Utc::now().to_rfc3339()
            ],
        )
        .unwrap();
    drop(connection);

    assert!(Storage::open(&database).is_err());
    let connection = Connection::open(&database).unwrap();
    assert_eq!(
        connection
            .query_row("PRAGMA user_version", [], |row| row.get::<_, u32>(0))
            .unwrap(),
        9
    );
    assert_eq!(
        connection
            .query_row("SELECT count(*) FROM tasks", [], |row| row.get::<_, u32>(0))
            .unwrap(),
        1
    );
    assert!(
        !connection
            .prepare("SELECT 1 FROM pragma_table_info('tasks') WHERE name = 'workspace_id'")
            .unwrap()
            .exists([])
            .unwrap()
    );
}

#[test]
fn opening_a_newer_database_preserves_its_version_and_data() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("future.db");
    let storage = Storage::open(&database).unwrap();
    storage.set_setting("sentinel", "keep").unwrap();
    storage
        .connection
        .pragma_update(None, "user_version", migrations::CURRENT_VERSION + 1)
        .unwrap();
    drop(storage);

    assert!(Storage::open(&database).is_err());
    let connection = Connection::open(&database).unwrap();
    assert_eq!(
        connection
            .query_row("PRAGMA user_version", [], |row| row.get::<_, u32>(0))
            .unwrap(),
        migrations::CURRENT_VERSION + 1
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT value FROM settings WHERE key = 'sentinel'",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
        "keep"
    );
}
