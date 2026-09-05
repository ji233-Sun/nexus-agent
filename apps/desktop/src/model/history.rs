use nexus_domain::{MessageKind, MessageRole};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadSummary {
    pub id: String,
    pub title: String,
    pub cwd: String,
    pub source: String,
    pub updated_at: i64,
    pub archived: bool,
}

impl ThreadSummary {
    pub fn detail(&self) -> String {
        let source = match self.source.as_str() {
            "vscode" => "Desktop",
            "exec" | "cli" => "CLI",
            _ => "Codex",
        };
        let project = Path::new(&self.cwd)
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .unwrap_or("未知目录");
        if self.archived {
            format!("{source} · {project} · 已归档")
        } else {
            format!("{source} · {project}")
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryMessage {
    pub role: MessageRole,
    pub kind: MessageKind,
    pub content: String,
}
