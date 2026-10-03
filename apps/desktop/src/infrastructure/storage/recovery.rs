use super::Storage;
use crate::model::workspace::Workspace;
use anyhow::Result;
use chrono::Utc;

pub(super) fn recover_interrupted(storage: &Storage) -> Result<()> {
    let now = Utc::now().to_rfc3339();
    storage.connection.execute(
        "UPDATE runs SET status = 'interrupted', ended_at = ?1
         WHERE status IN ('starting', 'running', 'cancelling')",
        [&now],
    )?;
    storage.connection.execute(
        "UPDATE tasks SET status = 'interrupted', updated_at = ?1
         WHERE status IN ('starting', 'running', 'cancelling')",
        [&now],
    )?;
    let records = {
        let mut statement = storage
            .connection
            .prepare("SELECT record FROM workspaces")?;
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
            storage.save_workspace(&workspace)?;
        }
    }
    Ok(())
}
