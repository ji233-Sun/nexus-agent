use std::{fs, path::Path, str::FromStr as _};

use anyhow::{Context as _, Result, anyhow};
use chrono::{DateTime, Utc};
use nexus_domain::{
    HarnessKind, Message, MessageKind, MessageRole, Project, ProviderProfile, RunStatus,
    TaskSummary, ThinkingEffort,
};
use rusqlite::{Connection, OptionalExtension as _, params};
use uuid::Uuid;

pub struct Storage {
    connection: Connection,
}

pub struct ConversationConfig {
    pub harness: HarnessKind,
    pub executable: String,
    pub model: String,
    pub effort: ThinkingEffort,
}

pub struct NewTaskRun<'a> {
    pub project_id: Uuid,
    pub title: &'a str,
    pub prompt: &'a str,
    pub harness: HarnessKind,
    pub executable: &'a str,
    pub model: Option<&'a str>,
    pub effort: ThinkingEffort,
    pub harness_version: Option<&'a str>,
}

impl Storage {
    pub fn open_default() -> Result<Self> {
        let base = super::paths::data_directory()?;
        fs::create_dir_all(&base).context("创建应用数据目录")?;
        Self::open(&base.join("nexus.db"))
    }

    pub fn open(path: &Path) -> Result<Self> {
        let connection = Connection::open(path).context("打开 SQLite 数据库")?;
        connection.execute_batch(
            "PRAGMA foreign_keys = ON;
             PRAGMA journal_mode = WAL;
             CREATE TABLE IF NOT EXISTS projects (
                 id TEXT PRIMARY KEY,
                 display_name TEXT NOT NULL,
                 canonical_path TEXT NOT NULL UNIQUE,
                 created_at TEXT NOT NULL,
                 last_opened_at TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS tasks (
                 id TEXT PRIMARY KEY,
                 project_id TEXT NOT NULL REFERENCES projects(id),
                 title TEXT NOT NULL,
                 status TEXT NOT NULL,
                 created_at TEXT NOT NULL,
                 updated_at TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS runs (
                 id TEXT PRIMARY KEY,
                 task_id TEXT NOT NULL REFERENCES tasks(id),
                 status TEXT NOT NULL,
                 harness_kind TEXT NOT NULL DEFAULT 'claude',
                 executable TEXT NOT NULL DEFAULT '',
                 model TEXT NOT NULL,
                 effort TEXT NOT NULL,
                 harness_version TEXT,
                 started_at TEXT NOT NULL,
                 ended_at TEXT,
                 exit_code INTEGER,
                 failure_code TEXT
             );
             CREATE UNIQUE INDEX IF NOT EXISTS one_active_run_per_task
             ON runs(task_id) WHERE status IN ('starting', 'running', 'cancelling');
             CREATE TABLE IF NOT EXISTS messages (
                 id TEXT PRIMARY KEY,
                 task_id TEXT NOT NULL REFERENCES tasks(id),
                 run_id TEXT NOT NULL REFERENCES runs(id),
                 sequence INTEGER NOT NULL,
                 role TEXT NOT NULL,
                 kind TEXT NOT NULL,
                 content TEXT NOT NULL,
                 created_at TEXT NOT NULL,
                 UNIQUE(task_id, sequence)
             );
             CREATE TABLE IF NOT EXISTS settings (
                 key TEXT PRIMARY KEY,
                 value TEXT NOT NULL
             );
             PRAGMA user_version = 1;",
        )?;
        if !table_has_column(&connection, "runs", "harness_kind")? {
            connection.execute(
                "ALTER TABLE runs ADD COLUMN harness_kind TEXT NOT NULL DEFAULT 'claude'",
                [],
            )?;
        }
        if !table_has_column(&connection, "runs", "executable")? {
            connection.execute(
                "ALTER TABLE runs ADD COLUMN executable TEXT NOT NULL DEFAULT ''",
                [],
            )?;
        }
        connection.execute_batch("PRAGMA user_version = 3;")?;
        let storage = Self { connection };
        storage.recover_interrupted()?;
        Ok(storage)
    }

    fn recover_interrupted(&self) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        self.connection.execute(
            "UPDATE runs SET status = 'interrupted', ended_at = ?1
             WHERE status IN ('starting', 'running', 'cancelling')",
            [&now],
        )?;
        self.connection.execute(
            "UPDATE tasks SET status = 'interrupted', updated_at = ?1
             WHERE status IN ('starting', 'running', 'cancelling')",
            [&now],
        )?;
        Ok(())
    }

    pub fn open_project(&self, path: &Path) -> Result<Project> {
        let canonical = path.canonicalize().context("规范化项目路径")?;
        if !canonical.is_dir() {
            return Err(anyhow!("选择的路径不是目录"));
        }
        let canonical_path = canonical.to_string_lossy().into_owned();
        let now = Utc::now();
        let display_name = canonical
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(&canonical_path)
            .to_owned();
        let existing_id: Option<String> = self
            .connection
            .query_row(
                "SELECT id FROM projects WHERE canonical_path = ?1",
                [&canonical_path],
                |row| row.get(0),
            )
            .optional()?;
        let id = existing_id
            .as_deref()
            .map(Uuid::parse_str)
            .transpose()?
            .unwrap_or_else(Uuid::new_v4);
        self.connection.execute(
            "INSERT INTO projects(id, display_name, canonical_path, created_at, last_opened_at)
             VALUES(?1, ?2, ?3, ?4, ?4)
             ON CONFLICT(canonical_path) DO UPDATE SET
                 display_name = excluded.display_name,
                 last_opened_at = excluded.last_opened_at",
            params![
                id.to_string(),
                display_name,
                canonical_path,
                now.to_rfc3339()
            ],
        )?;
        self.project(id)?.ok_or_else(|| anyhow!("项目保存失败"))
    }

    pub fn projects(&self) -> Result<Vec<Project>> {
        let mut statement = self.connection.prepare(
            "SELECT id, display_name, canonical_path, created_at, last_opened_at
             FROM projects ORDER BY last_opened_at DESC LIMIT 20",
        )?;
        let rows = statement.query_map([], project_from_row)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    fn project(&self, id: Uuid) -> Result<Option<Project>> {
        self.connection
            .query_row(
                "SELECT id, display_name, canonical_path, created_at, last_opened_at
                 FROM projects WHERE id = ?1",
                [id.to_string()],
                project_from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn tasks(&self, project_id: Uuid) -> Result<Vec<TaskSummary>> {
        let mut statement = self.connection.prepare(
            "SELECT id, project_id, title, status, created_at
             FROM tasks WHERE project_id = ?1 ORDER BY created_at DESC",
        )?;
        let rows = statement.query_map([project_id.to_string()], |row| {
            Ok(TaskSummary {
                id: parse_uuid(row.get::<_, String>(0)?)?,
                project_id: parse_uuid(row.get::<_, String>(1)?)?,
                title: row.get(2)?,
                status: parse_status(row.get::<_, String>(3)?)?,
                created_at: parse_date(row.get::<_, String>(4)?)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    pub fn create_task_run(&mut self, request: NewTaskRun<'_>) -> Result<(Uuid, Uuid)> {
        let NewTaskRun {
            project_id,
            title,
            prompt,
            harness,
            executable,
            model,
            effort,
            harness_version,
        } = request;
        let task_id = Uuid::new_v4();
        let run_id = Uuid::new_v4();
        let message_id = Uuid::new_v4();
        let now = Utc::now().to_rfc3339();
        let transaction = self.connection.transaction()?;
        transaction.execute(
            "INSERT INTO tasks(id, project_id, title, status, created_at, updated_at)
             VALUES(?1, ?2, ?3, 'starting', ?4, ?4)",
            params![task_id.to_string(), project_id.to_string(), title, now],
        )?;
        transaction.execute(
            "INSERT INTO runs(
                 id, task_id, status, harness_kind, executable, model, effort,
                 harness_version, started_at
             ) VALUES(?1, ?2, 'starting', ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                run_id.to_string(),
                task_id.to_string(),
                harness.as_str(),
                executable,
                model.unwrap_or("default"),
                effort.as_str(),
                harness_version,
                now
            ],
        )?;
        transaction.execute(
            "INSERT INTO messages(id, task_id, run_id, sequence, role, kind, content, created_at)
             VALUES(?1, ?2, ?3, 1, 'user', 'text', ?4, ?5)",
            params![
                message_id.to_string(),
                task_id.to_string(),
                run_id.to_string(),
                prompt,
                now
            ],
        )?;
        transaction.commit()?;
        Ok((task_id, run_id))
    }

    pub fn conversation_config(&self, task_id: Uuid) -> Result<Option<ConversationConfig>> {
        self.connection
            .query_row(
                "SELECT harness_kind, executable, model, effort
                 FROM runs WHERE task_id = ?1 ORDER BY started_at DESC LIMIT 1",
                [task_id.to_string()],
                |row| {
                    Ok(ConversationConfig {
                        harness: HarnessKind::from_str(&row.get::<_, String>(0)?)
                            .map_err(to_sql_data_error)?,
                        executable: row.get(1)?,
                        model: row.get(2)?,
                        effort: ThinkingEffort::from_str(&row.get::<_, String>(3)?)
                            .map_err(to_sql_data_error)?,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn update_run_status(&self, run_id: Uuid, status: RunStatus) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        self.connection.execute(
            "UPDATE runs SET status = ?2 WHERE id = ?1",
            params![run_id.to_string(), status.to_string()],
        )?;
        self.connection.execute(
            "UPDATE tasks SET status = ?2, updated_at = ?3
             WHERE id = (SELECT task_id FROM runs WHERE id = ?1)",
            params![run_id.to_string(), status.to_string(), now],
        )?;
        Ok(())
    }

    pub fn finish_run(
        &self,
        run_id: Uuid,
        status: RunStatus,
        exit_code: Option<i32>,
    ) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        self.connection.execute(
            "UPDATE runs SET status = ?2, ended_at = ?3, exit_code = ?4 WHERE id = ?1",
            params![run_id.to_string(), status.to_string(), now, exit_code],
        )?;
        self.connection.execute(
            "UPDATE tasks SET status = ?2, updated_at = ?3
             WHERE id = (SELECT task_id FROM runs WHERE id = ?1)",
            params![run_id.to_string(), status.to_string(), now],
        )?;
        Ok(())
    }

    pub fn append_message(
        &self,
        task_id: Uuid,
        run_id: Uuid,
        role: MessageRole,
        kind: MessageKind,
        content: &str,
    ) -> Result<Message> {
        let sequence = self.connection.query_row(
            "SELECT COALESCE(MAX(sequence), 0) + 1 FROM messages WHERE task_id = ?1",
            [task_id.to_string()],
            |row| row.get::<_, i64>(0),
        )? as u64;
        let message = Message {
            id: Uuid::new_v4(),
            task_id,
            run_id,
            sequence,
            role,
            kind,
            content: content.to_owned(),
            created_at: Utc::now(),
        };
        self.connection.execute(
            "INSERT INTO messages(id, task_id, run_id, sequence, role, kind, content, created_at)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                message.id.to_string(),
                task_id.to_string(),
                run_id.to_string(),
                sequence as i64,
                role_string(role),
                kind_string(kind),
                content,
                message.created_at.to_rfc3339()
            ],
        )?;
        Ok(message)
    }

    pub fn messages(&self, task_id: Uuid) -> Result<Vec<Message>> {
        let mut statement = self.connection.prepare(
            "SELECT id, task_id, run_id, sequence, role, kind, content, created_at
             FROM messages WHERE task_id = ?1 ORDER BY sequence",
        )?;
        let rows = statement.query_map([task_id.to_string()], |row| {
            Ok(Message {
                id: parse_uuid(row.get::<_, String>(0)?)?,
                task_id: parse_uuid(row.get::<_, String>(1)?)?,
                run_id: parse_uuid(row.get::<_, String>(2)?)?,
                sequence: row.get::<_, i64>(3)? as u64,
                role: parse_role(row.get::<_, String>(4)?)?,
                kind: parse_kind(row.get::<_, String>(5)?)?,
                content: row.get(6)?,
                created_at: parse_date(row.get::<_, String>(7)?)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    pub fn setting(&self, key: &str) -> Result<Option<String>> {
        self.connection
            .query_row("SELECT value FROM settings WHERE key = ?1", [key], |row| {
                row.get(0)
            })
            .optional()
            .map_err(Into::into)
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.connection.execute(
            "INSERT INTO settings(key, value) VALUES(?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn provider_profiles(&self) -> Result<Vec<ProviderProfile>> {
        self.setting("provider_profiles")?
            .map(|profiles| serde_json::from_str(&profiles).context("解析 Provider Profile"))
            .transpose()
            .map(Option::unwrap_or_default)
    }

    pub fn set_provider_profiles(&self, profiles: &[ProviderProfile]) -> Result<()> {
        let profiles = serde_json::to_string(profiles).context("序列化 Provider Profile")?;
        self.set_setting("provider_profiles", &profiles)
    }
}

fn table_has_column(connection: &Connection, table: &str, column: &str) -> Result<bool> {
    let mut statement = connection.prepare(&format!("PRAGMA table_info({table})"))?;
    let columns = statement.query_map([], |row| row.get::<_, String>(1))?;
    for name in columns {
        if name? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

fn project_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Project> {
    Ok(Project {
        id: parse_uuid(row.get::<_, String>(0)?)?,
        display_name: row.get(1)?,
        canonical_path: row.get(2)?,
        created_at: parse_date(row.get::<_, String>(3)?)?,
        last_opened_at: parse_date(row.get::<_, String>(4)?)?,
    })
}

fn parse_uuid(value: String) -> rusqlite::Result<Uuid> {
    Uuid::parse_str(&value).map_err(to_sql_error)
}

fn parse_date(value: String) -> rusqlite::Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(&value)
        .map(|date| date.with_timezone(&Utc))
        .map_err(to_sql_error)
}

fn parse_status(value: String) -> rusqlite::Result<RunStatus> {
    RunStatus::from_str(&value).map_err(to_sql_data_error)
}

fn parse_role(value: String) -> rusqlite::Result<MessageRole> {
    match value.as_str() {
        "user" => Ok(MessageRole::User),
        "assistant" => Ok(MessageRole::Assistant),
        "tool" => Ok(MessageRole::Tool),
        "system" => Ok(MessageRole::System),
        _ => Err(to_sql_data_error(format!("unknown role: {value}"))),
    }
}

fn parse_kind(value: String) -> rusqlite::Result<MessageKind> {
    match value.as_str() {
        "text" => Ok(MessageKind::Text),
        "tool_call" => Ok(MessageKind::ToolCall),
        "tool_result" => Ok(MessageKind::ToolResult),
        "status" => Ok(MessageKind::Status),
        "error" => Ok(MessageKind::Error),
        _ => Err(to_sql_data_error(format!("unknown kind: {value}"))),
    }
}

fn role_string(role: MessageRole) -> &'static str {
    match role {
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::Tool => "tool",
        MessageRole::System => "system",
    }
}

fn kind_string(kind: MessageKind) -> &'static str {
    match kind {
        MessageKind::Text => "text",
        MessageKind::ToolCall => "tool_call",
        MessageKind::ToolResult => "tool_result",
        MessageKind::Status => "status",
        MessageKind::Error => "error",
    }
}

fn to_sql_error(error: impl std::error::Error + Send + Sync + 'static) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
}

fn to_sql_data_error(error: impl std::fmt::Display) -> rusqlite::Error {
    to_sql_error(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        error.to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persists_history_and_recovers_active_runs() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("nexus.db");
        let project_dir = directory.path().join("project");
        fs::create_dir(&project_dir).unwrap();

        let mut storage = Storage::open(&database).unwrap();
        let project = storage.open_project(&project_dir).unwrap();
        let (task_id, run_id) = storage
            .create_task_run(NewTaskRun {
                project_id: project.id,
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
        drop(storage);

        let storage = Storage::open(&database).unwrap();
        let tasks = storage.tasks(project.id).unwrap();
        assert_eq!(tasks[0].status, RunStatus::Interrupted);
        let messages = storage.messages(task_id).unwrap();
        assert_eq!(messages[0].content, "hello");
        let config = storage.conversation_config(task_id).unwrap().unwrap();
        assert_eq!(config.harness, HarnessKind::Claude);
        assert_eq!(config.executable, "claude-custom");
        assert_eq!(config.model, "sonnet");
        assert_eq!(config.effort, ThinkingEffort::High);
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
                project_id: project.id,
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
            )
            .unwrap();
        storage
            .update_run_status(run_id, RunStatus::Running)
            .unwrap();
        storage
            .finish_run(run_id, RunStatus::Completed, Some(0))
            .unwrap();
        drop(storage);

        let storage = Storage::open(&database).unwrap();
        let tasks = storage.tasks(project.id).unwrap();
        assert_eq!(tasks[0].status, RunStatus::Completed);
        let messages = storage.messages(task_id).unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].content, "describe this project");
        assert_eq!(messages[1].content, "project summary");
    }

    #[test]
    fn settings_round_trip() {
        let directory = tempfile::tempdir().unwrap();
        let storage = Storage::open(&directory.path().join("nexus.db")).unwrap();
        storage.set_setting("model", "opus").unwrap();
        assert_eq!(storage.setting("model").unwrap().as_deref(), Some("opus"));
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
    fn migrates_existing_runs_to_a_claude_harness() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("nexus.db");
        let connection = Connection::open(&database).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE runs (
                    id TEXT PRIMARY KEY,
                    task_id TEXT NOT NULL,
                    status TEXT NOT NULL,
                    model TEXT NOT NULL,
                    effort TEXT NOT NULL,
                    harness_version TEXT,
                    started_at TEXT NOT NULL,
                    ended_at TEXT,
                    exit_code INTEGER,
                    failure_code TEXT
                );
                INSERT INTO runs(id, task_id, status, model, effort, started_at)
                VALUES('run-1', 'task-1', 'completed', 'sonnet', 'high', '2026-09-03');",
            )
            .unwrap();
        drop(connection);

        let storage = Storage::open(&database).unwrap();
        let harness: String = storage
            .connection
            .query_row(
                "SELECT harness_kind FROM runs WHERE id = 'run-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(harness, "claude");
    }
}
