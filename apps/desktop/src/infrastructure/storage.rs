use std::{fs, path::Path, str::FromStr as _};

use crate::model::workspace::{Workspace, WorkspaceStatus};
use anyhow::{Context as _, Result, anyhow};
use chrono::{DateTime, Utc};
use nexus_domain::{
    HarnessKind, Message, MessageKind, MessageRole, PermissionMode, Project, ProviderProfile,
    RunStatus, TaskSummary, ThinkingEffort, ToolMetadata,
};
use rusqlite::{Connection, OptionalExtension as _, Transaction, params};
use uuid::Uuid;

pub struct Storage {
    connection: Connection,
}

pub struct ConversationConfig {
    pub harness: HarnessKind,
    pub session_id: Option<String>,
    pub executable: String,
    pub model: String,
    pub effort: ThinkingEffort,
    pub permission_mode: PermissionMode,
}

pub struct NewTaskRun<'a> {
    pub task_id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub project_id: Uuid,
    pub title: &'a str,
    pub prompt: &'a str,
    pub harness: HarnessKind,
    pub executable: &'a str,
    pub model: Option<&'a str>,
    pub effort: ThinkingEffort,
    pub permission_mode: PermissionMode,
    pub harness_version: Option<&'a str>,
}

pub struct PendingTaskRun<'a> {
    pub task_id: Uuid,
    pub run_id: Uuid,
    transaction: Transaction<'a>,
}

impl PendingTaskRun<'_> {
    pub fn commit(self) -> Result<(Uuid, Uuid)> {
        self.transaction.commit()?;
        Ok((self.task_id, self.run_id))
    }
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
                 updated_at TEXT NOT NULL,
                 archived_at TEXT
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
        if !table_has_column(&connection, "runs", "permission_mode")? {
            connection.execute(
                "ALTER TABLE runs ADD COLUMN permission_mode TEXT NOT NULL DEFAULT 'auto_edit'",
                [],
            )?;
        }
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
        if !table_has_column(&connection, "messages", "tool")? {
            connection.execute("ALTER TABLE messages ADD COLUMN tool TEXT", [])?;
        }
        if !table_has_column(&connection, "tasks", "session_id")? {
            connection.execute("ALTER TABLE tasks ADD COLUMN session_id TEXT", [])?;
        }
        if !table_has_column(&connection, "tasks", "archived_at")? {
            connection.execute("ALTER TABLE tasks ADD COLUMN archived_at TEXT", [])?;
        }
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS workspaces (
                id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL REFERENCES projects(id),
                record TEXT NOT NULL
            );",
        )?;
        if !table_has_column(&connection, "tasks", "workspace_id")? {
            connection.execute(
                "ALTER TABLE tasks ADD COLUMN workspace_id TEXT REFERENCES workspaces(id)",
                [],
            )?;
        }
        if !table_has_column(&connection, "runs", "cwd")? {
            connection.execute("ALTER TABLE runs ADD COLUMN cwd TEXT", [])?;
        }
        let storage = Self { connection };
        let projects = {
            let mut statement = storage.connection.prepare(
                "SELECT id, display_name, canonical_path, created_at, last_opened_at FROM projects",
            )?;
            statement
                .query_map([], project_from_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        for project in projects {
            storage.ensure_local_workspace(&project)?;
        }
        storage.connection.execute_batch(
            "UPDATE tasks SET workspace_id = project_id WHERE workspace_id IS NULL;
             UPDATE runs SET cwd = (SELECT projects.canonical_path FROM tasks
                 JOIN projects ON projects.id = tasks.project_id WHERE tasks.id = runs.task_id)
                 WHERE cwd IS NULL;
             PRAGMA user_version = 7;",
        )?;
        let legacy_tasks = {
            let mut statement = storage
                .connection
                .prepare("SELECT id, workspace_id FROM tasks WHERE workspace_id = project_id")?;
            statement
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        for (task_id, workspace_id) in legacy_tasks {
            if let Some(mut workspace) = storage.workspace(Uuid::parse_str(&workspace_id)?)? {
                workspace.id = Uuid::parse_str(&task_id)?;
                workspace.task_id = Some(workspace.id);
                storage.save_workspace(&workspace)?;
                storage.connection.execute(
                    "UPDATE tasks SET workspace_id = ?1 WHERE id = ?1",
                    [&task_id],
                )?;
            }
        }
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
        let records = {
            let mut statement = self.connection.prepare("SELECT record FROM workspaces")?;
            statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        for record in records {
            let mut workspace: Workspace = serde_json::from_str(&record)?;
            if let Some(log) = &mut workspace.initialization
                && log.running
            {
                log.running = false;
                log.success = false;
                log.output
                    .push_str("\n上次初始化期间应用退出，请检查目录后重试。\n");
                self.save_workspace(&workspace)?;
            }
        }
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
        let project = self.project(id)?.ok_or_else(|| anyhow!("项目保存失败"))?;
        self.ensure_local_workspace(&project)?;
        Ok(project)
    }

    pub fn projects(&self) -> Result<Vec<Project>> {
        let mut statement = self.connection.prepare(
            "WITH recent_projects AS (
                 SELECT id FROM projects ORDER BY last_opened_at DESC LIMIT 20
             )
             SELECT id, display_name, canonical_path, created_at, last_opened_at
             FROM projects
             WHERE id IN (SELECT id FROM recent_projects)
                OR EXISTS (
                    SELECT 1 FROM tasks
                    WHERE tasks.project_id = projects.id AND archived_at IS NOT NULL
                )
                OR EXISTS (
                    SELECT 1 FROM workspaces WHERE workspaces.project_id = projects.id
                    AND json_extract(record, '$.managed') = 1
                    AND json_extract(record, '$.status') <> 'removed'
                )
             ORDER BY last_opened_at DESC",
        )?;
        let rows = statement.query_map([], project_from_row)?;
        let mut projects = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        let order: Vec<Uuid> = self
            .setting("project_order")?
            .and_then(|value| serde_json::from_str(&value).ok())
            .unwrap_or_default();
        // Keep recency order for projects that have not been manually positioned.
        projects.sort_by_key(|project| {
            order
                .iter()
                .position(|id| *id == project.id)
                .unwrap_or(order.len())
        });
        Ok(projects)
    }

    pub(crate) fn save_project_order(&self, order: &[Uuid]) -> Result<()> {
        self.set_setting("project_order", &serde_json::to_string(order)?)
    }

    pub(crate) fn project(&self, id: Uuid) -> Result<Option<Project>> {
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
             FROM tasks
             WHERE project_id = ?1 AND archived_at IS NULL
             ORDER BY created_at DESC",
        )?;
        let rows = statement.query_map([project_id.to_string()], task_from_row)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    pub fn update_task_title(&self, task_id: Uuid, title: &str) -> Result<bool> {
        let now = Utc::now().to_rfc3339();
        self.connection
            .execute(
                "UPDATE tasks SET title = ?2, updated_at = ?3 WHERE id = ?1 AND title <> ?2",
                params![task_id.to_string(), title, now],
            )
            .map(|changed| changed > 0)
            .map_err(Into::into)
    }

    pub fn archived_tasks(&self) -> Result<Vec<TaskSummary>> {
        let mut statement = self.connection.prepare(
            "SELECT id, project_id, title, status, created_at
             FROM tasks
             WHERE archived_at IS NOT NULL
             ORDER BY archived_at DESC, created_at DESC",
        )?;
        let rows = statement.query_map([], task_from_row)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    pub fn archive_task(&self, task_id: Uuid) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        let updated = self.connection.execute(
            "UPDATE tasks SET archived_at = ?2, updated_at = ?2
             WHERE id = ?1 AND archived_at IS NULL",
            params![task_id.to_string(), now],
        )?;
        if updated != 1 {
            return Err(anyhow!("对话不存在或已经归档"));
        }
        Ok(())
    }

    pub fn restore_task(&mut self, task_id: Uuid) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        let transaction = self.connection.transaction()?;
        let updated = transaction.execute(
            "UPDATE tasks SET archived_at = NULL, updated_at = ?2
             WHERE id = ?1 AND archived_at IS NOT NULL",
            params![task_id.to_string(), &now],
        )?;
        if updated != 1 {
            return Err(anyhow!("归档对话不存在"));
        }
        let updated = transaction.execute(
            "UPDATE projects SET last_opened_at = ?2
             WHERE id = (SELECT project_id FROM tasks WHERE id = ?1)",
            params![task_id.to_string(), now],
        )?;
        if updated != 1 {
            return Err(anyhow!("归档对话所属项目不存在"));
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn delete_task(&mut self, task_id: Uuid) -> Result<()> {
        let transaction = self.connection.transaction()?;
        transaction.execute(
            "DELETE FROM messages WHERE task_id = ?1",
            [task_id.to_string()],
        )?;
        transaction.execute("DELETE FROM runs WHERE task_id = ?1", [task_id.to_string()])?;
        let deleted =
            transaction.execute("DELETE FROM tasks WHERE id = ?1", [task_id.to_string()])?;
        if deleted != 1 {
            return Err(anyhow!("对话不存在"));
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn delete_archived_tasks(&mut self) -> Result<usize> {
        let transaction = self.connection.transaction()?;
        let count = transaction.query_row(
            "SELECT COUNT(*) FROM tasks WHERE archived_at IS NOT NULL",
            [],
            |row| row.get::<_, i64>(0),
        )?;
        transaction.execute(
            "DELETE FROM messages
             WHERE task_id IN (SELECT id FROM tasks WHERE archived_at IS NOT NULL)",
            [],
        )?;
        transaction.execute(
            "DELETE FROM runs
             WHERE task_id IN (SELECT id FROM tasks WHERE archived_at IS NOT NULL)",
            [],
        )?;
        transaction.execute("DELETE FROM tasks WHERE archived_at IS NOT NULL", [])?;
        transaction.commit()?;
        Ok(count as usize)
    }

    #[cfg(test)]
    pub fn create_task_run(&mut self, request: NewTaskRun<'_>) -> Result<(Uuid, Uuid)> {
        self.prepare_task_run(request)?.commit()
    }

    // Keep creation uncommitted until the runner accepts the start command.
    // Dropping the pending run rolls back its message and task status as well.
    pub fn prepare_task_run(&mut self, request: NewTaskRun<'_>) -> Result<PendingTaskRun<'_>> {
        let NewTaskRun {
            task_id,
            workspace_id,
            project_id,
            title,
            prompt,
            harness,
            executable,
            model,
            effort,
            permission_mode,
            harness_version,
        } = request;
        let mut workspace = if let Some(task_id) = task_id {
            self.task_workspace(task_id)?
                .ok_or_else(|| anyhow!("任务缺少执行目录"))?
        } else {
            self.workspace(workspace_id.unwrap_or(project_id))?
                .ok_or_else(|| anyhow!("项目缺少执行目录"))?
        };
        if workspace.project_id != project_id || workspace.status != WorkspaceStatus::Ready {
            return Err(anyhow!("任务目录不属于项目或尚未就绪"));
        }
        if workspace_id.is_some_and(|id| id != workspace.id) {
            return Err(anyhow!("任务开始后不能更换执行目录"));
        }
        if task_id.is_none() && !workspace.managed && workspace.task_id.is_none() {
            workspace.id = Uuid::new_v4();
            workspace.task_id = Some(workspace.id);
            self.save_workspace(&workspace)?;
        }
        let existing_task = task_id;
        let task_id = task_id.or(workspace.task_id).unwrap_or_else(Uuid::new_v4);
        let run_id = Uuid::new_v4();
        let message_id = Uuid::new_v4();
        let now = Utc::now().to_rfc3339();
        let transaction = self.connection.transaction()?;
        if existing_task.is_some() {
            let updated = transaction.execute(
                "UPDATE tasks SET status = 'starting', updated_at = ?3
                 WHERE id = ?1 AND project_id = ?2",
                params![task_id.to_string(), project_id.to_string(), now],
            )?;
            if updated != 1 {
                return Err(anyhow!("任务不属于当前项目"));
            }
        } else {
            transaction.execute(
                "INSERT INTO tasks(id, project_id, title, status, created_at, updated_at, workspace_id)
                 VALUES(?1, ?2, ?3, 'starting', ?4, ?4, ?5)",
                params![task_id.to_string(), project_id.to_string(), title, now, workspace.id.to_string()],
            )?;
        }
        transaction.execute(
            "INSERT INTO runs(
                 id, task_id, status, harness_kind, executable, model, effort,
                 harness_version, started_at, permission_mode, cwd
             ) VALUES(?1, ?2, 'starting', ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                run_id.to_string(),
                task_id.to_string(),
                harness.as_str(),
                executable,
                model.unwrap_or("default"),
                effort.as_str(),
                harness_version,
                now,
                permission_mode.as_str(),
                workspace.path
            ],
        )?;
        transaction.execute(
            "INSERT INTO messages(id, task_id, run_id, sequence, role, kind, content, created_at)
             VALUES(?1, ?2, ?3,
                 (SELECT COALESCE(MAX(sequence), 0) + 1 FROM messages WHERE task_id = ?2),
                 'user', 'text', ?4, ?5)",
            params![
                message_id.to_string(),
                task_id.to_string(),
                run_id.to_string(),
                prompt,
                now
            ],
        )?;
        Ok(PendingTaskRun {
            task_id,
            run_id,
            transaction,
        })
    }

    pub fn conversation_config(&self, task_id: Uuid) -> Result<Option<ConversationConfig>> {
        self.connection
            .query_row(
                "SELECT harness_kind, executable, model, effort, tasks.session_id, permission_mode
                 FROM runs JOIN tasks ON tasks.id = runs.task_id
                 WHERE task_id = ?1 ORDER BY started_at DESC, runs.rowid DESC LIMIT 1",
                [task_id.to_string()],
                |row| {
                    Ok(ConversationConfig {
                        harness: HarnessKind::from_str(&row.get::<_, String>(0)?)
                            .map_err(to_sql_data_error)?,
                        executable: row.get(1)?,
                        model: row.get(2)?,
                        effort: ThinkingEffort::from_str(&row.get::<_, String>(3)?)
                            .map_err(to_sql_data_error)?,
                        session_id: row.get(4)?,
                        permission_mode: PermissionMode::from_str(&row.get::<_, String>(5)?)
                            .map_err(to_sql_data_error)?,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn save_run_session(&self, run_id: Uuid, session_id: &str) -> Result<()> {
        self.connection.execute(
            "UPDATE tasks SET session_id = ?2
             WHERE id = (SELECT task_id FROM runs WHERE id = ?1)",
            params![run_id.to_string(), session_id],
        )?;
        Ok(())
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
        tool: Option<ToolMetadata>,
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
            tool,
            created_at: Utc::now(),
        };
        self.connection.execute(
            "INSERT INTO messages(id, task_id, run_id, sequence, role, kind, content, created_at, tool)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                message.id.to_string(),
                task_id.to_string(),
                run_id.to_string(),
                sequence as i64,
                role_string(role),
                kind_string(kind),
                content,
                message.created_at.to_rfc3339(),
                message.tool.as_ref().map(serde_json::to_string).transpose()?
            ],
        )?;
        Ok(message)
    }

    pub fn messages(&self, task_id: Uuid) -> Result<Vec<Message>> {
        let mut statement = self.connection.prepare(
            "SELECT id, task_id, run_id, sequence, role, kind, content, created_at, tool
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
                tool: row
                    .get::<_, Option<String>>(8)?
                    .map(|tool| serde_json::from_str(&tool).map_err(to_sql_error))
                    .transpose()?,
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

    fn ensure_local_workspace(&self, project: &Project) -> Result<()> {
        let mut workspace = Workspace::local(project);
        let path = Path::new(&project.canonical_path);
        if let Ok(common) = super::git::repository(path) {
            workspace.external = super::git::git(path, &["rev-parse", "--absolute-git-dir"])
                .ok()
                .and_then(|git_dir| Path::new(git_dir.trim_end()).canonicalize().ok())
                .is_some_and(|git_dir| git_dir != common);
            workspace.repository = Some(common.to_string_lossy().into_owned());
            workspace.branch = super::git::current_branch(path);
        }
        self.save_workspace(&workspace)?;
        Ok(())
    }

    pub(crate) fn save_workspace(&self, workspace: &Workspace) -> Result<()> {
        self.connection.execute(
            "INSERT INTO workspaces(id, project_id, record) VALUES(?1, ?2, ?3)
             ON CONFLICT(id) DO UPDATE SET record = excluded.record",
            params![
                workspace.id.to_string(),
                workspace.project_id.to_string(),
                serde_json::to_string(workspace)?
            ],
        )?;
        Ok(())
    }

    pub(crate) fn workspace(&self, id: Uuid) -> Result<Option<Workspace>> {
        let record: Option<String> = self
            .connection
            .query_row(
                "SELECT record FROM workspaces WHERE id = ?1",
                [id.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        record
            .map(|record| serde_json::from_str(&record).map_err(Into::into))
            .transpose()
    }

    pub(crate) fn task_workspace(&self, task_id: Uuid) -> Result<Option<Workspace>> {
        let record: Option<String> = self.connection.query_row(
            "SELECT record FROM workspaces JOIN tasks ON tasks.workspace_id = workspaces.id WHERE tasks.id = ?1",
            [task_id.to_string()], |row| row.get(0),
        ).optional()?;
        record
            .map(|record| serde_json::from_str(&record).map_err(Into::into))
            .transpose()
    }

    pub(crate) fn workspaces(&self, project_id: Uuid) -> Result<Vec<Workspace>> {
        let mut statement = self
            .connection
            .prepare("SELECT record FROM workspaces WHERE project_id = ?1 ORDER BY rowid")?;
        let records =
            statement.query_map([project_id.to_string()], |row| row.get::<_, String>(0))?;
        records
            .map(|record| Ok(serde_json::from_str(&record?)?))
            .collect()
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

fn task_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TaskSummary> {
    Ok(TaskSummary {
        id: parse_uuid(row.get::<_, String>(0)?)?,
        project_id: parse_uuid(row.get::<_, String>(1)?)?,
        title: row.get(2)?,
        status: parse_status(row.get::<_, String>(3)?)?,
        created_at: parse_date(row.get::<_, String>(4)?)?,
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
                workspace_id: None,
                permission_mode: PermissionMode::Ask,
                task_id: None,
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
                workspace_id: None,
                permission_mode: nexus_domain::PermissionMode::AutoEdit,
                task_id: None,
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
                None,
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
        assert!(messages.iter().all(|message| message.tool.is_none()));

        // Simulate the previous schema with real messages, then migrate in place.
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
                    workspace_id: None,
                    permission_mode: nexus_domain::PermissionMode::AutoEdit,
                    task_id: None,
                    project_id: project.id,
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
