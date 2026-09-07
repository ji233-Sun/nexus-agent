# Nexus Agent ADE

Nexus Agent 是一个面向 Linux、macOS 和 Windows 的本地桌面应用，用统一时间线驱动本机已安装的 Claude Code、Codex CLI 或 Oh My Pi（OMP）。当前版本是 `0.1.0-alpha.1`。

## 当前能力

- 选择本地项目并记录最近项目。
- 启动时自动探测 Claude Code、Codex CLI 与 OMP 的可执行文件、版本和登录状态，仍可手动覆盖路径。
- 通过 Codex 本地 `app-server` 只读浏览 CLI、Desktop 及已归档的原有会话。
- 管理多个 Provider Profile，在任务输入区快捷切换 API Key、Base URL 和默认模型。
- 为 Claude Code 选择 `默认 / Sonnet / Opus / Haiku` 模型；Codex 与 OMP 可使用 CLI 默认模型或当前 Profile 的模型。
- 配置 `Low / Medium / High / XHigh / Max` 思考层级。
- 为每轮消息选择并记忆权限模式（请求授权 / 自动编辑 / YOLO），在桌面弹窗中处理运行期间的授权请求。
- 通过 JSON Lines Runner 启动 Harness，显示文本、工具调用、状态和错误。
- 用当前 Harness 异步生成简洁任务标题；生成失败时保留首条 Prompt 的本地回退标题。
- 取消和关闭时清理 Harness 进程树：Unix 先中断再超时终止，Windows 使用系统 `taskkill /T /F`。
- SQLite 持久化 Nexus 发起的项目、任务、Run 和最终消息；启动时将遗留运行标为 `Interrupted`。
- 当前任务中的后续消息追加为新一轮 Run，并复用同一个 Harness Session；重新打开任务后仍可继续对话，点击“新建任务”才开始独立会话。
- 运行中发送的消息默认排队，每轮成功结束后按顺序发送一条；输入区可查看和移除排队消息。停止、运行失败或轮次结束时已切换到其他任务，都会暂停自动发送，返回原任务后可手动继续。队列属于原任务，仅保留在当前应用内，退出应用后不恢复；归档时保留，永久删除对话时清理。
- 点击排队消息上的 **Steer**，在下一次工具调用结束后介入当前轮次；同批并行工具全部结束后才会发送。收到 Harness 回执后，消息才从队列移入当前对话。如果本轮不再调用工具，消息会优先作为下一轮发送；停止、失败或送达结果未确认时保留消息并暂停自动发送。
- 在本机回环地址提供带令牌鉴权的 Remote Control 服务，并内置 React Web Client，可通过 FRP TCP 转发后远程查看会话、发起任务和取消运行。
- 提示项目中的未提交修改，但不创建 Worktree，也不执行 Git 写操作。

输入区的权限选择按 Harness 分别记忆，默认“自动编辑”。每轮发送时保存所选权限；重新打开会话会恢复该会话最后一轮的选择，新建任务使用该 Harness 最近选择的模式。运行中调整权限只影响下一条消息。排队消息各自保留发送时的权限；权限与当前轮次不同的消息需等到下一轮发送，不能通过 Steer 改变当前轮次权限。

| 权限模式 | Claude Code | Codex | OMP |
| --- | --- | --- | --- |
| 请求授权 | `default` | `read-only` + `on-request` | `always-ask` |
| 自动编辑 | `acceptEdits` | `workspace-write` + `on-request` | `write` |
| YOLO | `bypassPermissions` | `danger-full-access` + `never` | `yolo` |

“请求授权”让编辑或受限操作按 CLI 策略请求批准；“自动编辑”允许文件编辑，其余受限操作按需授权。Codex 在工作区沙箱内的命令可以直接执行。YOLO 自动允许操作，Codex 同时关闭自身沙箱；CLI 的强制策略仍然有效。

Claude Code 通过 `--print --input-format stream-json --output-format stream-json --replay-user-messages --permission-prompt-tool stdio` 收发消息，并将工具审批请求交给桌面弹窗。允许时保留原始工具输入，拒绝时将拒绝结果返回 CLI；参见[官方权限文档](https://code.claude.com/docs/en/agent-sdk/permissions)。

Codex 通过[官方 App Server 协议](https://learn.chatgpt.com/docs/app-server)运行，使用 `thread/start` / `thread/resume` 和 `turn/start`，Steer 通过带当前轮次 ID 的 `turn/steer` 注入。支持命令执行、文件变更和额外文件系统/网络权限审批；额外权限的批准仅作用于当前轮次。支持非 Git 项目，Prompt 由 stdin 传入。

OMP 通过 `omp --mode rpc --approval-mode <模式>` 运行，Prompt 与 Steer 同样由 stdin 传入，授权使用 RPC 的选择/确认弹窗。Claude Code 与 OMP 的后续轮次均通过 `--resume <SESSION_ID>` 继续原会话。

审批弹窗显示所属任务和操作详情，可允许、拒绝或停止任务。回复只发送给对应运行中的请求；停止、CLI 撤销请求、超时或轮次结束后，旧请求失效。后台标题生成不继承 YOLO：Claude Code / OMP 继续禁用工具，Codex 保留只读沙箱。

三个 Harness 均保存原生 Session，Nexus 在自己的数据库中记录 Session ID、各轮运行状态和消息。旧版本关闭了原生 Session 保存，因此旧任务可能只能浏览历史；缺少 Session ID 时会提示无法续聊。继续对话需使用原任务的 Harness，切换 Harness 请新建任务。

Provider Profile 的名称、Base URL、环境变量名和默认模型保存在 `nexus.db`；API Key 持久化时只保存在系统凭据库（macOS Keychain、Windows Credential Manager 或 Linux Secret Service），不会写入数据库、命令参数、运行记录或 Debug 输出。选中的配置只在单次 Harness 子进程中注入，不修改全局 shell 环境。系统凭据库不可用时 Nexus 会显示错误，不会退回明文存储。配置 `CODEX_API_KEY` 时，Nexus 会在 Codex App Server 内完成仅存于该进程内存的 API Key 登录，不改写 CLI 的持久登录凭据；DeepSeek 等 Provider 可按目标 Harness 要求填写自己的环境变量名。

Codex 原有历史通过 CLI 自带的实验性 `codex app-server` 协议读取，不复制到 Nexus 数据库，也不会被 Nexus 修改。若独立 CLI 无法读取 Desktop 创建的新版分页会话，Nexus 会自动尝试 Desktop 内置的 Codex。Nexus 自己完成或失败的任务继续保存在 `nexus.db` 中。

## 环境要求

- Linux（X11 或 Wayland）、macOS 或 Windows（MSVC 工具链）
- Rust 1.98 或更高版本（GPUI Kit 0.6 使用新版 GPUI）
- 已安装至少一种 Harness，并已完成 CLI 登录或在 Nexus 中配置对应的 Provider Profile

```bash
claude --version
claude auth status --json

codex --version
codex login status

omp --version
omp models --json
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

应用内可切换 Harness 并修改各自的可执行文件路径。Claude 模型与通用思考层级会持久化：Claude 分别转换为 `--model` 与 `--effort` 参数，Codex 通过 `model_reasoning_effort` 配置覆盖思考层级，OMP 使用 `--thinking`；OMP 不支持 `Max`，因此会映射为其最高层级 `xhigh`。三个 Harness 的 Prompt 都通过子进程 stdin 传递，不会出现在进程参数中。

## 下载与发布

[GitHub Releases](https://github.com/ji233-Sun/nexus-agent/releases) 提供以下压缩包，每个文件名都包含版本和目标架构：

| 平台 | 目标架构 | 安装方式 |
| --- | --- | --- |
| macOS Apple Silicon | `aarch64-apple-darwin` | 解压 ZIP，将 `Nexus Agent.app` 移入“应用程序” |
| macOS Intel | `x86_64-apple-darwin` | 解压 ZIP，将 `Nexus Agent.app` 移入“应用程序” |
| Linux | `x86_64-unknown-linux-gnu` | 解压 TAR.GZ，运行 `nexus-desktop`；需要上述 GUI 运行依赖 |
| Windows | `x86_64-pc-windows-msvc` | 解压 ZIP，运行 `nexus-desktop.exe` |

Desktop 已内置 Runner，无需单独配置。Release 同时附带 `SHA256SUMS.txt`。macOS 应用只有本地临时签名，没有开发者签名或 Apple 公证；macOS 与 Windows 首次启动时可能需要按系统提示允许运行。

维护者发布时，先更新根目录 `Cargo.toml` 的 `workspace.package.version` 和 `Cargo.lock`，完成检查后再推送对应的 `v<版本>` 标签（例如 `v0.1.0-alpha.2`）。[Release 工作流](.github/workflows/release.yml) 会校验标签与应用版本一致，重新构建内嵌 Web Client，在四个目标上检查、测试、构建和打包；全部成功后才创建 GitHub Release，上传压缩包、校验和及自动生成的发布说明。含预发布标识的版本会标记为 Pre-release。

Nightly 每 4 小时定时检查默认分支 `main` 的最新提交（cron：`0 */4 * * *`，UTC 每天 00:00、04:00、08:00、12:00、16:00、20:00，北京时间也为这六个时刻）。若该提交尚未发布 Nightly，则执行同一套检查和四平台打包流程，全部成功后自动发布一个 Nightly 预发布版；已经发布过的提交会跳过构建和发布。向 `main` 推送新提交（包括合并 PR）仅触发 CI，Nightly 等待下一次定时运行。标签及压缩包文件名使用 `nightly-YYYY-MM-DD-<Unix秒时间戳>-<12位提交SHA>`，例如 `nightly-2026-09-06-1788673923-6bbbdd981018`。日期与时间戳取自该提交的提交者时间，日期按 UTC 计算；同一提交重跑工作流时保持相同标签。Nightly 标签指向本次构建的完整提交 SHA，始终标记为 Pre-release，不占用 Latest；不同提交的发布独立执行。应用与 macOS bundle 的基础版本继续沿用 `Cargo.toml`，无需为每次 Nightly 修改版本文件。

如需立即构建 Nightly，在 GitHub 仓库的 **Actions → Release → Run workflow** 中选择分支（通常为 `main`）并运行，即可手动触发该分支最新提交的构建。手动运行同样会跳过已经发布过 Nightly 的提交。

## Remote Control

Desktop 启动后默认在 `127.0.0.1:3210` 提供 HTTP/WebSocket 服务。设置页面的 `REMOTE CONTROL` 区域会显示服务地址，并提供“复制链接”和“复制令牌”按钮。访问令牌保存在现有 SQLite `settings` 表中；API 请求必须使用 Bearer Token，WebSocket 使用页面生成的临时连接参数。

直接在本机打开复制的链接即可进入内置 React 页面。通过 FRP 时，创建一个 TCP 代理，将公网端口转发到本机 `127.0.0.1:3210`，再把链接中的主机和端口替换成 FRP 公网地址。令牌放在 URL Fragment（`#token=...`）中，不会随 HTTP 请求发送；页面读取后会立即清除地址栏 Fragment，并只在当前标签页的 `sessionStorage` 中保留令牌。

远程页面可以：

- 浏览 Nexus 自己保存的项目、会话和消息；Codex CLI/Desktop 的只读导入历史不通过 Remote API 暴露。
- 使用 Desktop 当前选择的 Harness、模型和思考层级发起任务。
- 发起任务时使用 Desktop 当前选择的权限模式；需要授权时在 Desktop 弹窗中处理，Remote Web 暂不提供审批入口。
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

桌面 UI 基于 [GPUI Kit 0.6](https://github.com/longbridge/gpui-kit)，使用其 Sidebar 导航、图标资源、Button、Input / Textarea、下拉菜单、Switch 和 Markdown 组件，统一石墨灰主题与控件交互。点击侧栏底部或任务顶部的“设置”进入独立设置页面，管理执行环境、远程访问和交互偏好；点击“返回工作区”恢复原任务、输入草稿和滚动位置。界面保留 `⌘/Ctrl K` 搜索、`⌘/Ctrl N` 新任务、`⌘/Ctrl ,` 切换设置和 `⌘/Ctrl Enter` 发送快捷键。

桌面端默认使用简体中文，可在 **设置 → 通用 → 界面语言** 切换为 **English**，立即生效并在重启后保留。切换会更新界面、菜单、占位文字和应用状态提示，保留当前任务与输入草稿；用户内容、Agent 输出、原始诊断和 Remote Web 内容保持原样。

桌面 UI 使用 MVP（Model–View–Presenter），Runner 使用分层架构。两个进程的入口只负责启动装配，业务逻辑放在独立模块中。

```text
React Remote Web ── authenticated HTTP/WebSocket ──┐
                                                   ▼
                                      nexus-desktop
                                        View (GPUI) ──▶ Presenter ──▶ Model
                                             └────────读取 Model────────┘
                                                        │
                         SQLite / 系统凭据库 / RunnerClient / Codex 历史
                                                        │ versioned JSONL over stdio
                                                        ▼
                                      nexus-runner
                                        Transport ──▶ Application ──▶ Infrastructure
                                        JSONL         调度、独占、取消   Harness / 进程组
                                                        │
                                                        ├── stream-json ──▶ Claude Code
                                                        ├── app-server ──▶ Codex CLI
                                                        └── --mode rpc ──▶ Oh My Pi
```

- `crates/domain`：领域状态、模型和思考层级。
- `crates/protocol`：Desktop 与 Runner 的版本化 JSONL 协议。
- `crates/harness-core`：Harness 共用的启动规格、事件和可执行文件解析。
- `crates/harness-claude`：Claude Code 探测、启动参数和事件解码。
- `crates/harness-codex`：Codex CLI 探测、非交互启动参数和 JSONL 事件解码。
- `crates/harness-omp`：Oh My Pi 探测、受控写入模式和 JSON 事件解码。
- `apps/runner/src/transport.rs`：JSONL 命令读取、协议版本校验和事件写出。
- `apps/runner/src/application`：命令调度、运行独占、取消和统一事件转换。
- `apps/runner/src/infrastructure`：Harness 适配器选择、子进程执行和平台相关的进程树清理。
- `apps/desktop/src/bootstrap.rs`：窗口、主题、存储和 Runner 的启动装配。
- `apps/desktop/src/model`：界面状态、历史消息数据和提交可用性，不依赖 GPUI。
- `apps/desktop/src/presenter`：项目选择、配置、提交、远程命令、事件处理和持久化协调，不依赖 GPUI；通过 `RunnerPort` 注入真实或测试 Runner。
- `apps/desktop/src/view`：GPUI 渲染、控件状态和事件转交，按侧栏、时间线、设置、组件和主题拆分。
- `apps/desktop/src/infrastructure`：平台数据目录、SQLite、系统凭据库、Runner 进程通信、Codex 历史和 Git 状态读取。
- `apps/desktop/src/remote_control.rs`：带令牌鉴权的 HTTP/WebSocket 服务及静态资源托管。
- `apps/remote-web`：React + Vite 静态 Remote Client，生产构建产物嵌入 Desktop。

View 只能通过 Presenter 的只读 `model()` 获取业务状态，通过 Presenter 方法发起操作。SQLite 格式保持向后兼容；Desktop 与 Runner 使用配对的版本化 JSONL 协议，并拒绝不匹配的外置 Runner。已有领域与 Harness crate 继续复用，不额外引入框架或空 crate。

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

## 按指令解决 PR 冲突

[Resolve PR conflicts 工作流](.github/workflows/resolve-conflicts.yml) 在收到 PR 评论 `/resolve-conflicts` 后，使用 DeepSeek 尝试解决合并冲突。评论中只填写这一条命令。PR 创建或追加提交不会自动调用模型，也不会启动自动审查。

启用时，将工作流合并到默认分支，并在仓库 **Settings → Secrets and variables → Actions** 中添加 `DEEPSEEK_API_KEY`。模型固定使用 `deepseek-v4-flash`，费用由对应的 DeepSeek API 账户承担；不需要 Qodo 或 OpenAI 凭据。执行前会检查评论者当前拥有 `write`、`maintain` 或 `admin` 权限。

处理过程如下：

1. 固定 PR 与默认分支的提交 SHA，在临时 runner 中准备合并。
2. 将存在冲突的文件上下文发给 DeepSeek，模型只返回各冲突区的替换内容。脚本保留冲突区外的合并结果，检查替换结构、Git 索引和 `git diff --check`。
3. 在独立任务中重新检查权限和两端 SHA，将带有两个父提交的合并提交写回原 PR 分支。采用普通推送，不强制覆盖分支；期间分支发生变化时停止写回。
4. 显式触发现有 CI，检查格式、Clippy、测试和构建，并在 PR 下报告结果及工作流链接。CI 在写回后运行，请等待通过后再合并 PR。工作流不会自动合并 PR。

模型任务只有仓库读权限，写回任务持有必要的 GitHub 写权限。两个任务均执行默认分支上的控制脚本，不执行 PR 中的程序或安装 PR 的依赖；只有后续 CI 执行项目检查。

当前支持同仓库、目标为默认分支的开放非草稿 PR，以及最多 10 个、每个不超过 128 KiB 的 UTF-8 普通文本冲突。Fork PR、修改了 GitHub 工作流或本地 Action 的 PR，以及二进制、删除、重命名、权限、符号链接和锁文件冲突需要人工处理。模型无法给出完整有效结果时停止，不推送部分修改。模型解决文本冲突不代表业务语义必然正确，仍需查看 diff 和 CI 结果。

处理脚本仅使用 Node.js 24 的内置模块；回归测试使用临时 Git 仓库和模拟 API，不产生模型费用：

```bash
node --test ".github/scripts/resolve-conflicts.test.mjs"
```

## 致谢

[Vercel Design MD](https://github.com/educlopez/design-bites/blob/main/design-mds/vercel.com/DESIGN.md)

[Synara](https://github.com/Emanuele-web04/synara)

[t3code](https://github.com/pingdotgg/t3code)

## 当前边界

这个 Alpha 的 Remote Control 仅包含单机 TCP、令牌鉴权和静态 Web Client，不包含 UDP、内置 TLS、FRP 自动配置、云端 Control Plane 或多设备账户。它同样不包含 Worktree 管理、Git 提交、附件、多 Agent、从 Nexus 续聊 Codex 原有会话、签名或公证。桌面审批支持上述工具/权限请求，尚不支持 Codex 的 MCP elicitation 表单或自由文本问答。Codex 历史浏览依赖当前 CLI 的实验性 `app-server` 协议。Codex CLI 的 `--json` 模式会实时提供生命周期和工具事件，但 Assistant 文本按完成消息输出，不提供 token 级文本增量。模型与思考层级是否可用取决于本机 CLI 版本和账户权限。
