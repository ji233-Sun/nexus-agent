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
    Omp,
    Pi,
    Kimi,
    Qoder,
    CodeBuddy,
}

impl HarnessKind {
    pub const ALL: [Self; 7] = [
        Self::Claude,
        Self::Codex,
        Self::Omp,
        Self::Pi,
        Self::Kimi,
        Self::Qoder,
        Self::CodeBuddy,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Omp => "omp",
            Self::Pi => "pi",
            Self::Kimi => "kimi",
            Self::Qoder => "qoder",
            Self::CodeBuddy => "codebuddy",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Claude => Self::Codex,
            Self::Codex => Self::Omp,
            Self::Omp => Self::Pi,
            Self::Pi => Self::Kimi,
            Self::Kimi => Self::Qoder,
            Self::Qoder => Self::CodeBuddy,
            Self::CodeBuddy => Self::Claude,
        }
    }

    pub fn default_executable(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Omp => "omp",
            Self::Pi => "pi",
            Self::Kimi => "kimi",
            Self::Qoder => "qodercli",
            Self::CodeBuddy => "codebuddy",
        }
    }
}

impl fmt::Display for HarnessKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Claude => "Claude Code",
            Self::Codex => "Codex CLI",
            Self::Omp => "Oh My Pi",
            Self::Pi => "Pi",
            Self::Kimi => "Kimi Code",
            Self::Qoder => "Qoder",
            Self::CodeBuddy => "CodeBuddy",
        })
    }
}

impl FromStr for HarnessKind {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "claude" => Ok(Self::Claude),
            "codex" => Ok(Self::Codex),
            "omp" => Ok(Self::Omp),
            "pi" => Ok(Self::Pi),
            "kimi" => Ok(Self::Kimi),
            "qoder" => Ok(Self::Qoder),
            "codebuddy" => Ok(Self::CodeBuddy),
            _ => Err(format!("unknown harness: {value}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PermissionMode {
    Ask,
    #[default]
    AutoEdit,
    Yolo,
}

impl PermissionMode {
    pub const ALL: [Self; 3] = [Self::Ask, Self::AutoEdit, Self::Yolo];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ask => "ask",
            Self::AutoEdit => "auto_edit",
            Self::Yolo => "yolo",
        }
    }
}

impl FromStr for PermissionMode {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "ask" => Ok(Self::Ask),
            "auto_edit" => Ok(Self::AutoEdit),
            "yolo" => Ok(Self::Yolo),
            _ => Err(format!("unknown permission mode: {value}")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderProfile {
    pub id: Uuid,
    pub name: String,
    pub harness: HarnessKind,
    pub api_key_env: String,
    pub base_url_env: Option<String>,
    pub base_url: Option<String>,
    pub model: Option<String>,
    #[serde(skip)]
    pub credential_configured: bool,
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
    Default,
    None,
    Off,
    Minimal,
    Low,
    #[default]
    Medium,
    High,
    XHigh,
    Max,
    Ultra,
    Auto,
}

impl ThinkingEffort {
    pub const ALL: [Self; 5] = [Self::Low, Self::Medium, Self::High, Self::XHigh, Self::Max];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::None => "none",
            Self::Off => "off",
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::XHigh => "xhigh",
            Self::Max => "max",
            Self::Ultra => "ultra",
            Self::Auto => "auto",
        }
    }

    pub fn next(&self) -> Self {
        match self {
            Self::Low => Self::Medium,
            Self::Medium => Self::High,
            Self::High => Self::XHigh,
            Self::XHigh => Self::Max,
            _ => Self::Low,
        }
    }

    pub fn is_default(&self) -> bool {
        matches!(self, Self::Default)
    }
}

impl fmt::Display for ThinkingEffort {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Default => "模型默认",
            Self::None => "None",
            Self::Off => "Off",
            Self::Minimal => "Minimal",
            Self::Low => "Low",
            Self::Medium => "Medium",
            Self::High => "High",
            Self::XHigh => "XHigh",
            Self::Max => "Max",
            Self::Ultra => "Ultra",
            Self::Auto => "Auto",
        })
    }
}

impl FromStr for ThinkingEffort {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim() {
            "" => Err("thinking effort cannot be empty".into()),
            "default" => Ok(Self::Default),
            "none" => Ok(Self::None),
            "off" => Ok(Self::Off),
            "minimal" => Ok(Self::Minimal),
            "low" => Ok(Self::Low),
            "medium" => Ok(Self::Medium),
            "high" => Ok(Self::High),
            "xhigh" => Ok(Self::XHigh),
            "max" => Ok(Self::Max),
            "ultra" => Ok(Self::Ultra),
            "auto" => Ok(Self::Auto),
            _ => Err(format!("unknown thinking effort: {value}")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelReasoningEffort {
    pub effort: ThinkingEffort,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelSource {
    ClaudeAliases,
    CodexAppServer,
    OmpCli,
}

impl ModelSource {
    pub fn harness(&self) -> HarnessKind {
        match self {
            Self::ClaudeAliases => HarnessKind::Claude,
            Self::CodexAppServer => HarnessKind::Codex,
            Self::OmpCli => HarnessKind::Omp,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ModelAvailability {
    // Reported by the catalog, not a guarantee of account quota or successful execution.
    Available,
    Unknown,
    Unavailable { reason: String },
}

impl ModelAvailability {
    pub fn is_selectable(&self) -> bool {
        !matches!(self, Self::Unavailable { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelDescriptor {
    pub id: String,
    pub display_name: String,
    pub source: ModelSource,
    pub availability: ModelAvailability,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    pub is_default: bool,
    pub supported_reasoning_efforts: Vec<ModelReasoningEffort>,
    pub default_reasoning_effort: Option<ThinkingEffort>,
}

impl ModelDescriptor {
    pub fn supports_effort(&self, effort: &ThinkingEffort) -> bool {
        self.supported_reasoning_efforts
            .iter()
            .any(|option| &option.effort == effort)
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

pub const MAX_TASK_TITLE_CHARS: usize = 40;

pub fn compact_task_title(value: &str) -> Option<String> {
    let collapsed = value.split_whitespace().collect::<Vec<_>>().join(" ");
    let without_markup = collapsed.replace("**", "").replace('`', "");
    let title = without_markup
        .trim_matches(|character: char| matches!(character, '#' | '\'' | '"' | '-' | ' '))
        .chars()
        .take(MAX_TASK_TITLE_CHARS)
        .collect::<String>();
    let title = title
        .trim_end_matches(|character: char| {
            matches!(
                character,
                '.' | '。' | '!' | '！' | '?' | '？' | ':' | '：' | ';' | '；' | ',' | '，'
            )
        })
        .trim()
        .to_owned();
    (!title.is_empty()).then_some(title)
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<ToolMetadata>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolMetadata {
    pub id: String,
    pub is_error: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserAskQuestion {
    pub id: String,
    pub prompt: String,
    pub answer_mode: UserAskAnswerMode,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<UserAskOption>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UserAskAnswerMode {
    Text,
    Choice { multiple: bool, allow_custom: bool },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserAskOption {
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserAskAnswer {
    pub question_id: String,
    pub value: UserAskAnswerValue,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum UserAskAnswerValue {
    Text(String),
    Selected(Vec<String>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UserAskStatus {
    Answered,
    Cancelled,
    Expired,
    Failed,
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
        assert_eq!(HarnessKind::Codex.next(), HarnessKind::Omp);
        assert_eq!(HarnessKind::Omp.next(), HarnessKind::Pi);
        assert_eq!(HarnessKind::CodeBuddy.next(), HarnessKind::Claude);
        assert_eq!(HarnessKind::Codex.default_executable(), "codex");
        assert_eq!(ClaudeModel::Haiku.next(), ClaudeModel::Default);
        assert_eq!(ThinkingEffort::Max.next(), ThinkingEffort::Low);
        assert_eq!(ThinkingEffort::XHigh.as_str(), "xhigh");
    }

    #[test]
    fn thinking_effort_parses_all_codex_catalog_values() {
        assert_eq!(
            ThinkingEffort::from_str("ultra").unwrap(),
            ThinkingEffort::Ultra
        );
        assert_eq!(
            ThinkingEffort::from_str("off").unwrap(),
            ThinkingEffort::Off
        );
        assert_eq!(
            ThinkingEffort::from_str("auto").unwrap(),
            ThinkingEffort::Auto
        );
        assert!(ThinkingEffort::from_str("provider-specific").is_err());
        assert!(ThinkingEffort::from_str("").is_err());
    }

    #[test]
    fn task_titles_are_single_line_bounded_and_unicode_safe() {
        assert_eq!(
            compact_task_title("  **修复登录流程。**\n并补充测试  ").as_deref(),
            Some("修复登录流程。 并补充测试")
        );
        assert_eq!(compact_task_title("```  "), None);
        assert_eq!(
            compact_task_title("Fix `__init__`").as_deref(),
            Some("Fix __init__")
        );
        assert_eq!(
            compact_task_title(&"界".repeat(MAX_TASK_TITLE_CHARS + 5))
                .unwrap()
                .chars()
                .count(),
            MAX_TASK_TITLE_CHARS
        );
    }

    #[test]
    fn user_ask_types_preserve_choice_capabilities_and_answer_mapping() {
        let question = UserAskQuestion {
            id: "targets".into(),
            prompt: "Select targets".into(),
            answer_mode: UserAskAnswerMode::Choice {
                multiple: true,
                allow_custom: false,
            },
            options: vec![UserAskOption {
                id: "tests".into(),
                label: "Tests".into(),
                description: Some("Run the focused suite".into()),
            }],
        };
        assert!(matches!(
            question.answer_mode,
            UserAskAnswerMode::Choice {
                multiple: true,
                allow_custom: false
            }
        ));
        assert_eq!(question.options[0].id, "tests");

        let answer = UserAskAnswer {
            question_id: "targets".into(),
            value: UserAskAnswerValue::Selected(vec!["tests".into()]),
        };
        assert_eq!(answer.question_id, "targets");
        assert_eq!(
            answer.value,
            UserAskAnswerValue::Selected(vec!["tests".into()])
        );
    }
}
