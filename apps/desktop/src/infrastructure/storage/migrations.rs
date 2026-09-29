//! Versioned, atomic upgrades of the existing SQLite schema and legacy records.
use super::{Storage, project_from_row};
use anyhow::{Result, ensure};
use rusqlite::Connection;
use uuid::Uuid;

// Versions through 9 used idempotent column checks without consistently advancing
// user_version. Consolidate those upgrades once, then gate future migrations here.
pub(super) const CURRENT_VERSION: u32 = 10;

pub(super) fn upgrade(storage: &Storage) -> Result<()> {
    let connection = &storage.connection;
    let version: u32 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    ensure!(
        version <= CURRENT_VERSION,
        "数据库版本 {version} 高于当前应用支持的版本 {CURRENT_VERSION}"
    );
    connection.execute_batch("PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL;")?;
    if version == CURRENT_VERSION {
        return Ok(());
    }
    // SQLite requires foreign keys to be disabled before starting a table rebuild.
    connection.pragma_update(None, "foreign_keys", false)?;
    let result = (|| -> Result<()> {
        let transaction = connection.unchecked_transaction()?;
        upgrade_legacy_schema(&transaction)?;
        migrate_optional_projects(&transaction)?;
        backfill_workspaces(storage)?;
        ensure!(
            !transaction
                .prepare("PRAGMA foreign_key_check")?
                .exists([])?,
            "迁移任务项目关联后外键校验失败"
        );
        transaction.pragma_update(None, "user_version", CURRENT_VERSION)?;
        transaction.commit()?;
        Ok(())
    })();
    let restored = connection.pragma_update(None, "foreign_keys", true);
    result?;
    restored?;
    Ok(())
}

fn upgrade_legacy_schema(connection: &Connection) -> Result<()> {
    connection.execute_batch(include_str!("schema.sql"))?;
    for (table, column, declaration) in [
        (
            "runs",
            "permission_mode",
            "TEXT NOT NULL DEFAULT 'auto_edit'",
        ),
        ("runs", "harness_kind", "TEXT NOT NULL DEFAULT 'claude'"),
        ("runs", "executable", "TEXT NOT NULL DEFAULT ''"),
        ("messages", "tool", "TEXT"),
        ("messages", "attachments", "TEXT NOT NULL DEFAULT '[]'"),
        ("tasks", "session_id", "TEXT"),
        ("tasks", "archived_at", "TEXT"),
        ("tasks", "workspace_id", "TEXT REFERENCES workspaces(id)"),
        ("runs", "cwd", "TEXT"),
    ] {
        if !table_has_column(connection, table, column)? {
            connection.execute(
                &format!("ALTER TABLE {table} ADD COLUMN {column} {declaration}"),
                [],
            )?;
        }
    }
    Ok(())
}

fn backfill_workspaces(storage: &Storage) -> Result<()> {
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
             WHERE cwd IS NULL;",
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
    Ok(())
}

fn migrate_optional_projects(connection: &Connection) -> Result<()> {
    let required: bool = connection.query_row(
        "SELECT \"notnull\" FROM pragma_table_info('tasks') WHERE name = 'project_id'",
        [],
        |row| row.get(0),
    )?;
    if !required {
        return Ok(());
    }
    // Rebuild inside the outer migration transaction, preserving inbound foreign keys.
    connection.execute_batch(
        "CREATE TABLE tasks_optional_project (
             id TEXT PRIMARY KEY,
             project_id TEXT REFERENCES projects(id),
             title TEXT NOT NULL,
             status TEXT NOT NULL,
             created_at TEXT NOT NULL,
             updated_at TEXT NOT NULL,
             archived_at TEXT,
             session_id TEXT,
             workspace_id TEXT REFERENCES workspaces(id)
         );
         INSERT INTO tasks_optional_project
             SELECT id, project_id, title, status, created_at, updated_at,
                    archived_at, session_id, workspace_id FROM tasks;
         DROP TABLE tasks;
         ALTER TABLE tasks_optional_project RENAME TO tasks;
         CREATE TABLE workspaces_optional_project (
             id TEXT PRIMARY KEY,
             project_id TEXT REFERENCES projects(id),
             record TEXT NOT NULL
         );
         INSERT INTO workspaces_optional_project SELECT id, project_id, record FROM workspaces;
         DROP TABLE workspaces;
         ALTER TABLE workspaces_optional_project RENAME TO workspaces;",
    )?;
    Ok(())
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
