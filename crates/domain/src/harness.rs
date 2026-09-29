use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};

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
    QoderCn,
    Codebuddy,
    Opencode,
    Deepseek,
    CommandCode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum HarnessTransport {
    #[default]
    Cli,
    Acp,
}

/// Static integration defaults shared by configuration, installation and execution.
pub struct HarnessInfo {
    pub id: &'static str,
    pub name: &'static str,
    pub executable: &'static str,
    pub api_key_env: &'static str,
    pub base_url_env: &'static str,
    pub npm_package: &'static str,
    pub documentation: &'static str,
    /// The first transport is the default for new sessions.
    pub transports: &'static [HarnessTransport],
}

impl HarnessKind {
    pub const ALL: [Self; 11] = [
        Self::Claude,
        Self::Codex,
        Self::Omp,
        Self::Pi,
        Self::Kimi,
        Self::Qoder,
        Self::QoderCn,
        Self::Codebuddy,
        Self::Opencode,
        Self::Deepseek,
        Self::CommandCode,
    ];

    pub fn info(self) -> HarnessInfo {
        match self {
            Self::Claude => HarnessInfo {
                id: "claude",
                name: "Claude Code",
                executable: "claude",
                api_key_env: "ANTHROPIC_API_KEY",
                base_url_env: "ANTHROPIC_BASE_URL",
                npm_package: "@anthropic-ai/claude-code",
                documentation: "https://code.claude.com/docs/en/setup",
                transports: &[HarnessTransport::Cli],
            },
            Self::Codex => HarnessInfo {
                id: "codex",
                name: "Codex CLI",
                executable: "codex",
                api_key_env: "CODEX_API_KEY",
                base_url_env: "OPENAI_BASE_URL",
                npm_package: "@openai/codex",
                documentation: "https://developers.openai.com/codex/cli/",
                transports: &[HarnessTransport::Cli],
            },
            Self::Omp => HarnessInfo {
                id: "omp",
                name: "Oh My Pi",
                executable: "omp",
                api_key_env: "DEEPSEEK_API_KEY",
                base_url_env: "",
                npm_package: "@oh-my-pi/pi-coding-agent",
                documentation: "https://github.com/can1357/oh-my-pi#install",
                transports: &[HarnessTransport::Cli],
            },
            Self::Pi => HarnessInfo {
                id: "pi",
                name: "Pi",
                executable: "pi",
                api_key_env: "ANTHROPIC_API_KEY",
                base_url_env: "",
                npm_package: "@earendil-works/pi-coding-agent",
                documentation: "https://github.com/badlogic/pi-mono/tree/main/packages/coding-agent",
                transports: &[HarnessTransport::Cli],
            },
            Self::Kimi => HarnessInfo {
                id: "kimi",
                name: "Kimi Code",
                executable: "kimi",
                api_key_env: "KIMI_API_KEY",
                base_url_env: "KIMI_BASE_URL",
                npm_package: "@moonshot-ai/kimi-code",
                documentation: "https://moonshotai.github.io/kimi-code/en/guides/getting-started",
                transports: &[HarnessTransport::Acp],
            },
            Self::Qoder => HarnessInfo {
                id: "qoder",
                name: "Qoder",
                executable: "qoder",
                api_key_env: "QODER_PERSONAL_ACCESS_TOKEN",
                base_url_env: "",
                npm_package: "@qoder-ai/qodercli",
                documentation: "https://docs.qoder.com/cli/quick-start",
                transports: &[HarnessTransport::Cli, HarnessTransport::Acp],
            },
            Self::QoderCn => HarnessInfo {
                id: "qodercn",
                name: "Qoder CN",
                executable: "qodercn",
                api_key_env: "QODERCN_PERSONAL_ACCESS_TOKEN",
                base_url_env: "",
                npm_package: "@qodercn-ai/qoderclicn",
                documentation: "https://docs.qoder.cn/cli/what-is-qoder-cli-cn",
                transports: &[HarnessTransport::Cli, HarnessTransport::Acp],
            },
            Self::Codebuddy => HarnessInfo {
                id: "codebuddy",
                name: "CodeBuddy",
                executable: "codebuddy",
                api_key_env: "CODEBUDDY_API_KEY",
                base_url_env: "CODEBUDDY_BASE_URL",
                npm_package: "@tencent-ai/codebuddy-code",
                documentation: "https://www.codebuddy.ai/docs/cli/overview",
                transports: &[HarnessTransport::Cli, HarnessTransport::Acp],
            },
            Self::Opencode => HarnessInfo {
                id: "opencode",
                name: "OpenCode",
                executable: "opencode",
                api_key_env: "ANTHROPIC_API_KEY",
                base_url_env: "ANTHROPIC_BASE_URL",
                npm_package: "opencode-ai",
                documentation: "https://opencode.ai/docs/",
                transports: &[HarnessTransport::Acp],
            },
            Self::Deepseek => HarnessInfo {
                id: "deepseek",
                name: "DeepSeek Harness",
                executable: "dsh",
                api_key_env: "DEEPSEEK_API_KEY",
                base_url_env: "DEEPSEEK_BASE_URL",
                npm_package: "@deepseek-ai/dsh",
                documentation: "https://deepseek-harness.github.io/deepseek-harness/en/",
                transports: &[HarnessTransport::Acp],
            },
            Self::CommandCode => HarnessInfo {
                id: "commandcode",
                name: "Command Code",
                executable: if cfg!(windows) { "cmdc" } else { "cmd" },
                api_key_env: "COMMAND_CODE_API_KEY",
                base_url_env: "",
                npm_package: "command-code",
                documentation: "https://commandcode.ai/docs/reference/cli",
                transports: &[HarnessTransport::Cli],
            },
        }
    }

    pub fn as_str(self) -> &'static str {
        self.info().id
    }
    pub fn default_executable(self) -> &'static str {
        self.info().executable
    }
    pub fn default_transport(self) -> HarnessTransport {
        self.info().transports[0]
    }
    pub fn has_transport_choice(self) -> bool {
        self.info().transports.len() > 1
    }

    pub fn next(self) -> Self {
        let index = Self::ALL
            .iter()
            .position(|kind| *kind == self)
            .expect("registered harness");
        Self::ALL[(index + 1) % Self::ALL.len()]
    }
}

impl fmt::Display for HarnessKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.info().name)
    }
}

impl FromStr for HarnessKind {
    type Err = String;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|kind| kind.as_str() == value)
            .ok_or_else(|| format!("unknown harness: {value}"))
    }
}
