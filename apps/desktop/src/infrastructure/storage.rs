mod migrations;
mod recovery;
#[cfg(test)]
mod tests;

use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    str::FromStr as _,
};

use crate::model::workspace::{Workspace, WorkspaceKind, WorkspaceStatus};
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
    session_root: PathBuf,
    _temporary_directory: Option<tempfile::TempDir>,
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
    pub project_id: Option<Uuid>,
    pub title: &'a str,
    pub prompt: &'a str,
    pub attachments: &'a [nexus_domain::Attachment],
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
    pub(crate) fn attachment_directory(&self) -> PathBuf {
        self.session_root
            .parent()
            .unwrap_or(&self.session_root)
            .join("attachments")
    }

    pub fn open_default() -> Result<Self> {
        let base = super::paths::data_directory()?;
        fs::create_dir_all(&base).context("创建应用数据目录")?;
        Self::open(&base.join("nexus.db"))
    }

    pub fn open(path: &Path) -> Result<Self> {
        let connection = Connection::open(path).context("打开 SQLite 数据库")?;
        // In-memory stores own temporary session files; persisted stores keep them
        // beside the database so a restart resolves exactly the same task directory.
        let temporary_directory = (path == Path::new(":memory:"))
            .then(tempfile::tempdir)
            .transpose()?;
        let session_root = temporary_directory.as_ref().map_or_else(
            || {
                path.parent()
                    .unwrap_or_else(|| Path::new("."))
                    .join("sessions")
            },
            |directory| directory.path().join("sessions"),
        );
        let storage = Self {
            connection,
            session_root,
            _temporary_directory: temporary_directory,
        };
        migrations::upgrade(&storage)?;
        recovery::recover_interrupted(&storage)?;
        Ok(storage)
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

    pub(crate) fn delete_project(&mut self, project_id: Uuid) -> Result<()> {
        let transaction = self.connection.transaction()?;
        let id = project_id.to_string();
        transaction.execute(
            "DELETE FROM messages WHERE task_id IN (SELECT id FROM tasks WHERE project_id = ?1)",
            [&id],
        )?;
        transaction.execute(
            "DELETE FROM runs WHERE task_id IN (SELECT id FROM tasks WHERE project_id = ?1)",
            [&id],
        )?;
        transaction.execute("DELETE FROM tasks WHERE project_id = ?1", [&id])?;
        // Forget workspace ownership without touching directories or Git branches.
        transaction.execute("DELETE FROM workspaces WHERE project_id = ?1", [&id])?;
        transaction.execute(
            "DELETE FROM settings WHERE key = ?1",
            [format!("workspace_mode:{project_id}")],
        )?;
        if transaction.execute("DELETE FROM projects WHERE id = ?1", [&id])? != 1 {
            return Err(anyhow!("项目不存在"));
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn tasks(&self, project_id: impl Into<Option<Uuid>>) -> Result<Vec<TaskSummary>> {
        let mut statement = self.connection.prepare(
            "SELECT id, project_id, title, status, created_at
             FROM tasks
             WHERE project_id IS ?1 AND archived_at IS NULL
             ORDER BY created_at DESC",
        )?;
        let rows =
            statement.query_map([project_id.into().map(|id| id.to_string())], task_from_row)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    pub(crate) fn task(&self, task_id: Uuid) -> Result<Option<TaskSummary>> {
        self.connection
            .query_row(
                "SELECT id, project_id, title, status, created_at FROM tasks WHERE id = ?1",
                [task_id.to_string()],
                task_from_row,
            )
            .optional()
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
        transaction.execute(
            "UPDATE projects SET last_opened_at = ?2
             WHERE id = (SELECT project_id FROM tasks WHERE id = ?1)",
            params![task_id.to_string(), now],
        )?;
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
            attachments,
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
            self.workspace(
                workspace_id
                    .or(project_id)
                    .ok_or_else(|| anyhow!("任务缺少执行目录"))?,
            )?
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
                 WHERE id = ?1 AND project_id IS ?2",
                params![
                    task_id.to_string(),
                    project_id.map(|id| id.to_string()),
                    now
                ],
            )?;
            if updated != 1 {
                return Err(anyhow!("任务不属于当前项目"));
            }
        } else {
            transaction.execute(
                "INSERT INTO tasks(id, project_id, title, status, created_at, updated_at, workspace_id)
                 VALUES(?1, ?2, ?3, 'starting', ?4, ?4, ?5)",
                params![task_id.to_string(), project_id.map(|id| id.to_string()), title, now, workspace.id.to_string()],
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
            "INSERT INTO messages(id, task_id, run_id, sequence, role, kind, content, created_at, attachments)
             VALUES(?1, ?2, ?3,
                 (SELECT COALESCE(MAX(sequence), 0) + 1 FROM messages WHERE task_id = ?2),
                 'user', 'text', ?4, ?5, ?6)",
            params![
                message_id.to_string(),
                task_id.to_string(),
                run_id.to_string(),
                prompt,
                now,
                serde_json::to_string(attachments)?
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
            attachments: Vec::new(),
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

    pub fn completed_runs(&self, task_id: Uuid) -> Result<HashSet<Uuid>> {
        let mut statement = self.connection.prepare(
            "SELECT id FROM runs WHERE task_id = ?1
             AND status = 'completed' AND ended_at IS NOT NULL",
        )?;
        let rows = statement.query_map([task_id.to_string()], |row| {
            parse_uuid(row.get::<_, String>(0)?)
        })?;
        rows.collect::<rusqlite::Result<HashSet<_>>>()
            .map_err(Into::into)
    }

    pub fn messages(&self, task_id: Uuid) -> Result<Vec<Message>> {
        let mut statement = self.connection.prepare(
            "SELECT id, task_id, run_id, sequence, role, kind, content, created_at, tool, attachments
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
                attachments: serde_json::from_str(&row.get::<_, String>(9)?)
                    .map_err(to_sql_error)?,
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

    pub(crate) fn prepare_projectless_workspace(&self, task_id: Uuid) -> Result<Workspace> {
        let path = self.session_root.join(task_id.to_string());
        fs::create_dir_all(&path).context("创建独立会话目录")?;
        let workspace = Workspace {
            id: task_id,
            project_id: None,
            task_id: Some(task_id),
            path: path.canonicalize()?.to_string_lossy().into_owned(),
            repository: None,
            kind: WorkspaceKind::Local,
            managed: true,
            external: false,
            base_sha: None,
            branch: None,
            merge_target: None,
            status: WorkspaceStatus::Ready,
            merge: None,
            initialization: None,
        };
        self.save_workspace(&workspace)?;
        Ok(workspace)
    }

    pub(crate) fn save_workspace(&self, workspace: &Workspace) -> Result<()> {
        self.connection.execute(
            "INSERT INTO workspaces(id, project_id, record) VALUES(?1, ?2, ?3)
             ON CONFLICT(id) DO UPDATE SET record = excluded.record",
            params![
                workspace.id.to_string(),
                workspace.project_id.map(|id| id.to_string()),
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
        project_id: row
            .get::<_, Option<String>>(1)?
            .map(parse_uuid)
            .transpose()?,
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
