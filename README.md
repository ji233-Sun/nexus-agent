# Nexus Agent ADE

Nexus Agent 是一个 macOS 本地桌面应用，用统一时间线驱动本机已安装的 Claude Code。当前版本是 `0.1.0-alpha.1`，只支持 Claude Code。

## 当前能力

- 选择本地项目并记录最近项目。
- 自动探测 Claude Code 可执行文件、版本和登录状态。
- 选择 `默认 / Sonnet / Opus / Haiku` 模型。
- 配置 `Low / Medium / High / XHigh / Max` 思考层级。
- 通过 JSON Lines Runner 启动 Claude Code，实时显示文本、工具调用、状态和错误。
- 温和中断，超时后终止 Claude Code 进程组。
- SQLite 持久化项目、任务、Run 和最终消息；启动时将遗留运行标为 `Interrupted`。
- 提示项目中的未提交修改，但不创建 Worktree，也不执行 Git 写操作。

Claude Code 以 `--permission-mode acceptEdits` 运行：文件编辑按 Claude Code 的该模式处理，其余受限工具仍遵循 Claude Code 自身权限策略。当前版本没有交互式审批面板。

## 环境要求

- macOS（Apple Silicon 为当前验证架构）
- Rust 1.88 或更高版本
- 已安装并登录 Claude Code

```bash
claude --version
claude auth status --json
```

## 启动指南

先构建整个 workspace，确保 Desktop 的同级目录存在 `nexus-runner`：

```bash
cargo build --workspace
cargo run -p nexus-desktop
```

如果单独移动 Desktop，可通过 `NEXUS_RUNNER_PATH` 指定 Runner 的完整路径。应用数据保存在：

```text
~/Library/Application Support/Nexus Agent/nexus.db
```

应用内可修改 Claude Code 可执行文件路径。模型和思考层级会持久化，并分别转换为 Claude Code 的 `--model` 与 `--effort` 参数。Prompt 通过子进程 stdin 传递，不会出现在进程参数中。

## 架构

```text
nexus-desktop (GPUI + SQLite)
       │ versioned JSONL over stdio
       ▼
nexus-runner (lifecycle + process group)
       │ stream-json
       ▼
Claude Code
```

- `crates/domain`：领域状态、模型和思考层级。
- `crates/protocol`：Desktop 与 Runner 的版本化 JSONL 协议。
- `crates/harness-claude`：Claude Code 探测、启动参数和事件解码。
- `apps/runner`：运行独占、流式转发、取消和进程清理。
- `apps/desktop`：GPUI 界面与 SQLite 历史。

## 验证

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --workspace --release
```

测试中的 Fake Claude 只验证进程和协议闭环，不发起真实模型请求。

## 当前边界

这个 Alpha 不包含 Codex、远程控制、Worktree 管理、Git 提交、附件、多 Agent、交互式审批、签名或公证。模型别名是否可用取决于本机 Claude Code 版本和账户权限。
