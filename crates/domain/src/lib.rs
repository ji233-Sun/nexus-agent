use std::{fmt, str::FromStr};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum HarnessKind {
    #[default]
    Claude,
    Codex,
}

impl HarnessKind {
    pub const ALL: [Self; 2] = [Self::Claude, Self::Codex];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Claude => Self::Codex,
            Self::Codex => Self::Claude,
        }
    }

    pub fn default_executable(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }
}

impl fmt::Display for HarnessKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Claude => "Claude Code",
            Self::Codex => "Codex CLI",
        })
    }
}

impl FromStr for HarnessKind {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "claude" => Ok(Self::Claude),
            "codex" => Ok(Self::Codex),
            _ => Err(format!("unknown harness: {value}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Starting,
    Running,
    Cancelling,
    Completed,
    Failed,
    Cancelled,
    Interrupted,
}

impl RunStatus {
    pub fn is_active(self) -> bool {
        matches!(self, Self::Starting | Self::Running | Self::Cancelling)
    }

    pub fn can_transition_to(self, next: Self) -> bool {
        use RunStatus::*;
        matches!(
            (self, next),
            (Starting, Running | Failed | Cancelled | Interrupted)
                | (
                    Running,
                    Cancelling | Completed | Failed | Cancelled | Interrupted
                )
                | (Cancelling, Cancelled | Failed | Interrupted)
        )
    }
}

impl fmt::Display for RunStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Cancelling => "cancelling",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Interrupted => "interrupted",
        };
        f.write_str(value)
    }
}

impl FromStr for RunStatus {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "starting" => Ok(Self::Starting),
            "running" => Ok(Self::Running),
            "cancelling" => Ok(Self::Cancelling),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            "interrupted" => Ok(Self::Interrupted),
            _ => Err(format!("unknown run status: {value}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ClaudeModel {
    #[default]
    Default,
    Sonnet,
    Opus,
    Haiku,
}

impl ClaudeModel {
    pub const ALL: [Self; 4] = [Self::Default, Self::Sonnet, Self::Opus, Self::Haiku];

    pub fn cli_value(self) -> Option<&'static str> {
        match self {
            Self::Default => None,
            Self::Sonnet => Some("sonnet"),
            Self::Opus => Some("opus"),
            Self::Haiku => Some("haiku"),
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Default => Self::Sonnet,
            Self::Sonnet => Self::Opus,
            Self::Opus => Self::Haiku,
            Self::Haiku => Self::Default,
        }
    }
}

impl fmt::Display for ClaudeModel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Default => "默认模型",
            Self::Sonnet => "Sonnet",
            Self::Opus => "Opus",
            Self::Haiku => "Haiku",
        })
    }
}

impl FromStr for ClaudeModel {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "default" => Ok(Self::Default),
            "sonnet" => Ok(Self::Sonnet),
            "opus" => Ok(Self::Opus),
            "haiku" => Ok(Self::Haiku),
            _ => Err(format!("unknown Claude model: {value}")),
        }
    }
}

impl ClaudeModel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Sonnet => "sonnet",
            Self::Opus => "opus",
            Self::Haiku => "haiku",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ThinkingEffort {
    Low,
    #[default]
    Medium,
    High,
    XHigh,
    Max,
}

impl ThinkingEffort {
    pub const ALL: [Self; 5] = [Self::Low, Self::Medium, Self::High, Self::XHigh, Self::Max];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::XHigh => "xhigh",
            Self::Max => "max",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Low => Self::Medium,
            Self::Medium => Self::High,
            Self::High => Self::XHigh,
            Self::XHigh => Self::Max,
            Self::Max => Self::Low,
        }
    }
}

impl fmt::Display for ThinkingEffort {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Low => "Low",
            Self::Medium => "Medium",
            Self::High => "High",
            Self::XHigh => "XHigh",
            Self::Max => "Max",
        })
    }
}

impl FromStr for ThinkingEffort {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|effort| effort.as_str() == value)
            .ok_or_else(|| format!("unknown thinking effort: {value}"))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: Uuid,
    pub display_name: String,
    pub canonical_path: String,
    pub created_at: DateTime<Utc>,
    pub last_opened_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskSummary {
    pub id: Uuid,
    pub project_id: Uuid,
    pub title: String,
    pub status: RunStatus,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    User,
    Assistant,
    Tool,
    System,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageKind {
    Text,
    ToolCall,
    ToolResult,
    Status,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: Uuid,
    pub task_id: Uuid,
    pub run_id: Uuid,
    pub sequence: u64,
    pub role: MessageRole,
    pub kind: MessageKind,
    pub content: String,
    pub created_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_state_machine_rejects_terminal_transitions() {
        assert!(RunStatus::Starting.can_transition_to(RunStatus::Running));
        assert!(RunStatus::Running.can_transition_to(RunStatus::Cancelling));
        assert!(RunStatus::Cancelling.can_transition_to(RunStatus::Cancelled));
        assert!(!RunStatus::Completed.can_transition_to(RunStatus::Running));
        assert!(!RunStatus::Starting.can_transition_to(RunStatus::Completed));
    }

    #[test]
    fn model_and_effort_cycle_through_supported_values() {
        assert_eq!(HarnessKind::Claude.next(), HarnessKind::Codex);
        assert_eq!(HarnessKind::Codex.default_executable(), "codex");
        assert_eq!(ClaudeModel::Haiku.next(), ClaudeModel::Default);
        assert_eq!(ThinkingEffort::Max.next(), ThinkingEffort::Low);
        assert_eq!(ThinkingEffort::XHigh.as_str(), "xhigh");
    }
}
