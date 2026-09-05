# Nexus Agent ADE

Nexus Agent 是一个面向 Linux、macOS 和 Windows 的本地桌面应用，用统一时间线驱动本机已安装的 Claude Code 或 Codex CLI。当前版本是 `0.1.0-alpha.1`。

## 当前能力

- 选择本地项目并记录最近项目。
- 启动时自动探测 Claude Code 与 Codex CLI 的可执行文件、版本和登录状态，仍可手动覆盖路径。
- 通过 Codex 本地 `app-server` 只读浏览 CLI、Desktop 及已归档的原有会话。
- 为 Claude Code 选择 `默认 / Sonnet / Opus / Haiku` 模型；Codex 使用 CLI 当前默认模型。
- 配置 `Low / Medium / High / XHigh / Max` 思考层级。
- 通过 JSON Lines Runner 启动 Harness，显示文本、工具调用、状态和错误。
- 取消和关闭时清理 Harness 进程树：Unix 先中断再超时终止，Windows 使用系统 `taskkill /T /F`。
- SQLite 持久化 Nexus 发起的项目、任务、Run 和最终消息；启动时将遗留运行标为 `Interrupted`。
- 在本机回环地址提供带令牌鉴权的 Remote Control 服务，并内置 React Web Client，可通过 FRP TCP 转发后远程查看会话、发起任务和取消运行。
- 提示项目中的未提交修改，但不创建 Worktree，也不执行 Git 写操作。

Claude Code 以 `--permission-mode acceptEdits` 运行：文件编辑按 Claude Code 的该模式处理，其余受限工具仍遵循 Claude Code 自身权限策略。

Codex 按[官方非交互模式](https://learn.chatgpt.com/docs/non-interactive-mode)通过 `codex exec --skip-git-repo-check --json --sandbox workspace-write --ephemeral -` 运行。用户已在 Nexus 中明确选择工作目录，因此也允许运行非 Git 项目。Prompt 由 stdin 传入，Codex 可以修改所选工作目录，但不能通过 Nexus 请求交互式提权。当前版本没有交互式审批面板。

Codex 原有历史通过 CLI 自带的实验性 `codex app-server` 协议读取，不复制到 Nexus 数据库，也不会被 Nexus 修改。若独立 CLI 无法读取 Desktop 创建的新版分页会话，Nexus 会自动尝试 Desktop 内置的 Codex。Nexus 自己完成或失败的任务继续保存在 `nexus.db` 中。

## 环境要求

- Linux（X11 或 Wayland）、macOS 或 Windows（MSVC 工具链）
- Rust 1.98 或更高版本（GPUI Kit 0.6 使用新版 GPUI）
- 已安装并登录至少一种 Harness

```bash
claude --version
claude auth status --json

codex --version
codex login status
```

macOS 构建需要 Xcode 与命令行工具。Windows 构建需要 Visual Studio 的 C++ 桌面开发组件、Windows SDK 和 CMake。Linux 构建依赖可参照 [GPUI/Zed Linux 构建说明](https://zed.dev/docs/development/linux)；Ubuntu/Debian 安装命令为：

```bash
sudo apt-get install -y clang cmake pkg-config \
  libfontconfig1-dev libfreetype6-dev libwayland-dev libx11-xcb-dev \
  libxkbcommon-x11-dev libssl-dev libvulkan1 libglib2.0-dev
```

Linux 运行界面需要可用的 Vulkan 驱动和桌面会话，目录选择需要 XDG Desktop Portal 及对应桌面后端。单元测试与内置 Runner 测试不需要显示服务或真实 Harness 登录。

## 启动指南

使用 rustup 管理 Rust。仓库的 `rust-toolchain.toml` 固定使用 Rust 1.98.1；在项目目录执行 Cargo 命令时会自动选择该版本，首次运行需要联网下载工具链。此配置不修改其他项目使用的全局默认版本。

```bash
cargo run -p nexus-desktop
```

默认开发构建已开启编译优化，保留调试信息和运行时检查，避免未优化的布局与文本渲染拖慢滚动。首次构建依赖会更久，后续仍可增量编译。评估最终发布性能请使用 `cargo run -p nexus-desktop --release --locked`。

可用现有长消息场景对比滚动的 CPU 处理耗时：

```bash
cargo test -p nexus-desktop --locked scroll_frame_cost -- --ignored --nocapture
```

该测试模拟触控板输入，报告侧栏和消息区的耗时中位数与 P95；测试平台不执行 GPU 呈现，结果不代表屏幕实际帧率。

Desktop 默认以独立子进程运行内置 Runner，确保两者始终使用相同协议版本。若需要改用外部 Runner，可通过 `NEXUS_RUNNER_PATH` 指定其完整路径。应用数据保存在：

| 系统 | 数据库路径 |
| --- | --- |
| macOS | `~/Library/Application Support/Nexus Agent/nexus.db` |
| Linux | `$XDG_DATA_HOME/nexus-agent/nexus.db`，默认 `~/.local/share/nexus-agent/nexus.db` |
| Windows | `%LOCALAPPDATA%/Nexus Agent/nexus.db`，缺省时使用 `%USERPROFILE%/AppData/Local/Nexus Agent/nexus.db` |

Windows 的程序探测支持 `PATHEXT` 中的 `.exe`、`.com`、`.bat` 和 `.cmd`，包括 npm 安装生成的命令入口。

应用内可切换 Harness 并修改各自的可执行文件路径。Claude 模型与通用思考层级会持久化：Claude 分别转换为 `--model` 与 `--effort` 参数，Codex 使用 CLI 默认模型并通过 `model_reasoning_effort` 配置覆盖思考层级。两种 Harness 的 Prompt 都通过子进程 stdin 传递，不会出现在进程参数中。

## Remote Control

Desktop 启动后默认在 `127.0.0.1:3210` 提供 HTTP/WebSocket 服务。右侧 `REMOTE CONTROL` 面板会显示服务地址，并提供“复制链接”和“复制令牌”按钮。访问令牌保存在现有 SQLite `settings` 表中；API 请求必须使用 Bearer Token，WebSocket 使用页面生成的临时连接参数。

直接在本机打开复制的链接即可进入内置 React 页面。通过 FRP 时，创建一个 TCP 代理，将公网端口转发到本机 `127.0.0.1:3210`，再把链接中的主机和端口替换成 FRP 公网地址。令牌放在 URL Fragment（`#token=...`）中，不会随 HTTP 请求发送；页面读取后会立即清除地址栏 Fragment，并只在当前标签页的 `sessionStorage` 中保留令牌。

远程页面可以：

- 浏览 Nexus 自己保存的项目、会话和消息；Codex CLI/Desktop 的只读导入历史不通过 Remote API 暴露。
- 使用 Desktop 当前选择的 Harness、模型和思考层级发起任务。
- 通过 WebSocket 接收状态变化和流式输出，并取消当前运行。

默认只监听回环地址，不直接暴露给局域网。端口冲突时可以在启动 Desktop 前设置 `NEXUS_REMOTE_ADDR`，例如 `127.0.0.1:4310`。本版本不内置 TLS 或 FRP 配置；公网暴露时应优先使用支持 HTTPS/WSS 的入口，并妥善保管访问令牌。

修改远程页面后需要重新生成 Desktop 内嵌的静态资源：

```bash
cd apps/remote-web
npm ci
npm run build
cd ../..
cargo build --workspace
```

## 架构

桌面 UI 基于 [GPUI Kit 0.6](https://github.com/longbridge/gpui-kit)，使用其 Sidebar 导航、图标资源、Button、Input / Textarea、下拉菜单、Switch 和 Markdown 组件，统一石墨灰主题与控件交互。环境面板覆盖在主内容右侧，打开时不会压缩任务输入区。界面保留 `⌘/Ctrl K` 搜索、`⌘/Ctrl N` 新任务、`⌘/Ctrl ,` 环境和 `⌘/Ctrl Enter` 发送快捷键。

桌面 UI 使用 MVP（Model–View–Presenter），Runner 使用分层架构。两个进程的入口只负责启动装配，业务逻辑放在独立模块中。

```text
React Remote Web ── authenticated HTTP/WebSocket ──┐
                                                   ▼
                                      nexus-desktop
                                        View (GPUI) ──▶ Presenter ──▶ Model
                                             └────────读取 Model────────┘
                                                        │
                                      SQLite / RunnerClient / Codex 历史
                                                        │ versioned JSONL over stdio
                                                        ▼
                                      nexus-runner
                                        Transport ──▶ Application ──▶ Infrastructure
                                        JSONL         调度、独占、取消   Harness / 进程组
                                                        │
                                                        ├── stream-json ──▶ Claude Code
                                                        └── exec --json ──▶ Codex CLI
```

- `crates/domain`：领域状态、模型和思考层级。
- `crates/protocol`：Desktop 与 Runner 的版本化 JSONL 协议。
- `crates/harness-core`：Harness 共用的启动规格、事件和可执行文件解析。
- `crates/harness-claude`：Claude Code 探测、启动参数和事件解码。
- `crates/harness-codex`：Codex CLI 探测、非交互启动参数和 JSONL 事件解码。
- `apps/runner/src/transport.rs`：JSONL 命令读取、协议版本校验和事件写出。
- `apps/runner/src/application`：命令调度、运行独占、取消和统一事件转换。
- `apps/runner/src/infrastructure`：Harness 适配器选择、子进程执行和平台相关的进程树清理。
- `apps/desktop/src/bootstrap.rs`：窗口、主题、存储和 Runner 的启动装配。
- `apps/desktop/src/model`：界面状态、历史消息数据和提交可用性，不依赖 GPUI。
- `apps/desktop/src/presenter`：项目选择、配置、提交、远程命令、事件处理和持久化协调，不依赖 GPUI；通过 `RunnerPort` 注入真实或测试 Runner。
- `apps/desktop/src/view`：GPUI 渲染、控件状态和事件转交，按侧栏、时间线、设置、组件和主题拆分。
- `apps/desktop/src/infrastructure`：平台数据目录、SQLite、Runner 进程通信、Codex 历史和 Git 状态读取。
- `apps/desktop/src/remote_control.rs`：带令牌鉴权的 HTTP/WebSocket 服务及静态资源托管。
- `apps/remote-web`：React + Vite 静态 Remote Client，生产构建产物嵌入 Desktop。

View 只能通过 Presenter 的只读 `model()` 获取业务状态，通过 Presenter 方法发起操作。SQLite 格式和 Desktop–Runner JSONL 协议保持兼容；已有领域与 Harness crate 继续复用，不额外引入框架或空 crate。

## 验证

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo build --workspace --release --locked
cd apps/remote-web && npm ci && npm run typecheck && npm run build
```

测试中的 Fake Claude / Fake Codex 只验证进程和协议闭环，不发起真实模型请求。
Presenter 单元测试使用内存 SQLite 与 Fake Runner，不打开 GPUI 窗口；Runner 单元测试覆盖任务独占、取消、事件转换和协议传输。

[GitHub Actions CI](.github/workflows/ci.yml) 在 push、pull request 和手动触发时，分别使用 Ubuntu、macOS、Windows runner 执行以上检查。工具链固定为 Rust 1.98.1，依赖使用 `Cargo.lock`；缓存按平台和工具链区分。原生 Rust Fake Harness 的启动、流式输出、取消和关闭测试在三个系统上运行；Codex 历史的 shell fixture 测试目前在 Unix 系统上运行。

CI 验证构建和自动化行为；窗口显示、输入法、目录选择、真实 CLI 登录以及发布包仍需在各系统上人工验收。生成发布构建可运行 `cargo build --workspace --release --locked`；Windows Release 的 GPUI shader 编译还需要 Windows SDK 的 `fxc.exe`（可通过 `GPUI_FXC_PATH` 指定）。

## 致谢

[Vercel Design MD](https://github.com/educlopez/design-bites/blob/main/design-mds/vercel.com/DESIGN.md)

[Synara](https://github.com/Emanuele-web04/synara)

## 当前边界

这个 Alpha 的 Remote Control 仅包含单机 TCP、令牌鉴权和静态 Web Client，不包含 UDP、内置 TLS、FRP 自动配置、云端 Control Plane 或多设备账户。它同样不包含 Worktree 管理、Git 提交、附件、多 Agent、交互式审批、从 Nexus 续聊 Codex 原有会话、签名或公证。Codex 历史浏览依赖当前 CLI 的实验性 `app-server` 协议。Codex CLI 的 `--json` 模式会实时提供生命周期和工具事件，但 Assistant 文本按完成消息输出，不提供 token 级文本增量。模型与思考层级是否可用取决于本机 CLI 版本和账户权限。
