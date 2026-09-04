# Nexus Agent ADE

Nexus Agent 是一个 macOS 本地桌面应用，用统一时间线驱动本机已安装的 Claude Code 或 Codex CLI。当前版本是 `0.1.0-alpha.1`。

## 当前能力

- 选择本地项目并记录最近项目。
- 启动时自动探测 Claude Code 与 Codex CLI 的可执行文件、版本和登录状态，仍可手动覆盖路径。
- 通过 Codex 本地 `app-server` 只读浏览 CLI、Desktop 及已归档的原有会话。
- 为 Claude Code 选择 `默认 / Sonnet / Opus / Haiku` 模型；Codex 使用 CLI 当前默认模型。
- 配置 `Low / Medium / High / XHigh / Max` 思考层级。
- 通过 JSON Lines Runner 启动 Harness，显示文本、工具调用、状态和错误。
- 温和中断，超时后终止 Harness 进程组。
- SQLite 持久化 Nexus 发起的项目、任务、Run 和最终消息；启动时将遗留运行标为 `Interrupted`。
- 提示项目中的未提交修改，但不创建 Worktree，也不执行 Git 写操作。

Claude Code 以 `--permission-mode acceptEdits` 运行：文件编辑按 Claude Code 的该模式处理，其余受限工具仍遵循 Claude Code 自身权限策略。

Codex 按[官方非交互模式](https://learn.chatgpt.com/docs/non-interactive-mode)通过 `codex exec --skip-git-repo-check --json --sandbox workspace-write --ephemeral -` 运行。用户已在 Nexus 中明确选择工作目录，因此也允许运行非 Git 项目。Prompt 由 stdin 传入，Codex 可以修改所选工作目录，但不能通过 Nexus 请求交互式提权。当前版本没有交互式审批面板。

Codex 原有历史通过 CLI 自带的实验性 `codex app-server` 协议读取，不复制到 Nexus 数据库，也不会被 Nexus 修改。若独立 CLI 无法读取 Desktop 创建的新版分页会话，Nexus 会自动尝试 Desktop 内置的 Codex。Nexus 自己完成或失败的任务继续保存在 `nexus.db` 中。

## 环境要求

- macOS（Apple Silicon 为当前验证架构）
- Rust 1.88 或更高版本
- 已安装并登录至少一种 Harness

```bash
claude --version
claude auth status --json

codex --version
codex login status
```

## 启动指南

```bash
cargo run -p nexus-desktop
```

Desktop 默认以独立子进程运行内置 Runner，确保两者始终使用相同协议版本。若需要改用外部 Runner，可通过 `NEXUS_RUNNER_PATH` 指定其完整路径。应用数据保存在：

```text
~/Library/Application Support/Nexus Agent/nexus.db
```

应用内可切换 Harness 并修改各自的可执行文件路径。Claude 模型与通用思考层级会持久化：Claude 分别转换为 `--model` 与 `--effort` 参数，Codex 使用 CLI 默认模型并通过 `model_reasoning_effort` 配置覆盖思考层级。两种 Harness 的 Prompt 都通过子进程 stdin 传递，不会出现在进程参数中。

## 架构

```text
nexus-desktop (GPUI + SQLite)
       │ versioned JSONL over stdio
       ▼
nexus-runner (lifecycle + process group)
       │
       ├── stream-json ──▶ Claude Code
       └── exec --json ──▶ Codex CLI
```

- `crates/domain`：领域状态、模型和思考层级。
- `crates/protocol`：Desktop 与 Runner 的版本化 JSONL 协议。
- `crates/harness-core`：Harness 共用的启动规格、事件和可执行文件解析。
- `crates/harness-claude`：Claude Code 探测、启动参数和事件解码。
- `crates/harness-codex`：Codex CLI 探测、非交互启动参数和 JSONL 事件解码。
- `apps/runner`：运行独占、流式转发、取消和进程清理。
- `apps/desktop`：GPUI 界面与 SQLite 历史。

## 验证

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --workspace --release
```

测试中的 Fake Claude / Fake Codex 只验证进程和协议闭环，不发起真实模型请求。

## 致谢

界面视觉语言参考了 [Vercel Design MD](https://github.com/educlopez/design-bites/blob/main/design-mds/vercel.com/DESIGN.md)。感谢 design-bites 项目对 Vercel 设计体系的整理与分享。

## 当前边界

这个 Alpha 不包含远程控制、Worktree 管理、Git 提交、附件、多 Agent、交互式审批、从 Nexus 续聊 Codex 原有会话、签名或公证。Codex 历史浏览依赖当前 CLI 的实验性 `app-server` 协议。Codex CLI 的 `--json` 模式会实时提供生命周期和工具事件，但 Assistant 文本按完成消息输出，不提供 token 级文本增量。模型与思考层级是否可用取决于本机 CLI 版本和账户权限。
