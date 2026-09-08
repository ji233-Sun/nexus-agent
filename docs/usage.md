# 使用指南

[返回首页](../README.md) · [开发与维护](development.md)

从打开项目到继续会话，这里介绍 Nexus 的日常使用、执行权限和数据保存方式。

[会话与消息](#会话与消息) · [模型与服务商](#模型与服务商) · [权限与审批](#权限与审批) · [安装与更新](#安装与更新) · [远程访问](#远程访问)

## 会话与消息

选择本地项目后发送第一条消息，即可创建任务。时间线显示回答、工具调用、状态和错误，任务标题会异步生成；标题生成失败时，保留首条消息的本地回退标题。

三个 Harness 均保存原生 Session，Nexus 在自己的数据库中记录 Session ID、各轮运行状态和消息。旧版本关闭了原生 Session 保存，因此旧任务可能只能浏览历史；缺少 Session ID 时会提示无法续聊。继续对话需使用原任务的 Harness，切换 Harness 请新建任务。

运行中继续发送消息会进入该任务的队列，每轮成功结束后按顺序发送一条。停止、运行失败，或轮次结束时已切换到其他任务，都会暂停自动发送，返回原任务后可手动继续。

**Steer** 用于让排队消息在下一次工具调用结束后介入当前轮次。同批并行工具全部结束后才会发送；收到 Harness 回执后，消息才移入时间线。如果本轮不再调用工具，消息会优先作为下一轮发送。停止、失败或送达未确认时保留消息并暂停自动发送。

队列只保留在当前应用内，退出后不恢复；归档时保留，永久删除对话时清理。任务运行时使用所选项目的实际目录，界面会提示未提交修改。

### 浏览 Codex 原有历史

Codex 原有历史通过 CLI 自带的实验性 `codex app-server` 协议读取，不复制到 Nexus 数据库，也不会被 Nexus 修改。若独立 CLI 无法读取 Desktop 创建的新版分页会话，Nexus 会自动尝试 Desktop 内置的 Codex。Nexus 自己完成或失败的任务继续保存在 `nexus.db` 中。

## 模型与服务商

Nexus 从当前 Harness 的模型目录读取可选模型、默认模型和支持的思考层级。可以跟随 CLI 默认，也可以选择目录中可用的模型；列表与能力取决于 CLI 版本、项目配置和账号权限。

模型及思考层级按 Harness 与 Provider Profile 记忆。切换模型后，如果原思考层级不再受支持，会恢复为模型默认。运行期间锁定本轮的模型配置。

在设置中创建多个 **Provider Profile**，为每个 Harness 配置名称、API Key、Base URL、环境变量名和默认模型，再从任务输入区切换。

Provider Profile 的名称、Base URL、环境变量名和默认模型保存在 `nexus.db`；API Key 持久化时只保存在系统凭据库（macOS Keychain、Windows Credential Manager 或 Linux Secret Service），不会写入数据库、命令参数、运行记录或 Debug 输出。选中的配置只在单次 Harness 子进程中注入，不修改全局 shell 环境。系统凭据库不可用时 Nexus 会显示错误，不会退回明文存储。配置 `CODEX_API_KEY` 时，Nexus 会在 Codex App Server 内完成仅存于该进程内存的 API Key 登录，不改写 CLI 的持久登录凭据；DeepSeek 等 Provider 可按目标 Harness 要求填写自己的环境变量名。

## 权限与审批

输入区的权限选择按 Harness 分别记忆，默认“自动编辑”。每轮发送时保存所选权限；重新打开会话会恢复该会话最后一轮的选择，新建任务使用该 Harness 最近选择的模式。运行中调整权限只影响下一条消息。排队消息各自保留发送时的权限；权限与当前轮次不同的消息需等到下一轮发送，不能通过 Steer 改变当前轮次权限。

| 权限模式 | Claude Code | Codex | OMP |
| --- | --- | --- | --- |
| 请求授权 | `default` | `read-only` + `on-request` | `always-ask` |
| 自动编辑 | `acceptEdits` | `workspace-write` + `on-request` | `write` |
| YOLO | `bypassPermissions` | `danger-full-access` + `never` | `yolo` |

“请求授权”让编辑或受限操作按 CLI 策略请求批准；“自动编辑”允许文件编辑，其余受限操作按需授权。Codex 在工作区沙箱内的命令可以直接执行。YOLO 自动允许操作，Codex 同时关闭自身沙箱；CLI 的强制策略仍然有效。

审批弹窗显示所属任务和操作详情，可允许、拒绝或停止任务。回复只发送给对应运行中的请求；停止、CLI 撤销请求、超时或轮次结束后，旧请求失效。后台标题生成不继承 YOLO：Claude Code / OMP 继续禁用工具，Codex 保留只读沙箱。

Harness 发起交互问题时，输入区会显示问题和可用的回答方式；提交后等待 Agent 继续。已结束或失效的请求不能重复回复。

## 安装与更新

### 管理 Harness

Windows 的程序探测支持 `PATHEXT` 中的 `.exe`、`.com`、`.bat` 和 `.cmd`，包括 npm 安装生成的命令入口。

在 **设置 → 执行引擎 → Harness 管理** 中，可以同时查看三个 Harness 的当前版本、最新版本、实际路径和安装来源，点击「重新扫描」刷新。未安装时从「安装」菜单选择本机可用的安装器；已安装且检测到更新版本时，显示「更新」入口，并沿用拥有该可执行文件的安装器。执行前显示具体命令，也可复制更新命令。

| 安装来源 | 安装与更新方式 |
| --- | --- |
| 官方安装器 | Claude Code 使用 `claude update`；Codex 重跑官方安装器并保留安装目录；OMP 使用 `omp update`，首次安装固定使用官方二进制模式 |
| npm / pnpm / Yarn Classic / Bun / Vite+ (`vp`) | 使用对应的全局包管理命令；npm 固定原 prefix，Bun、Yarn、Vite+ 保留各自的安装目录；pnpm 按其查询出的全局目录识别安装 |
| Homebrew | 区分 Formula / Cask，并校验 Homebrew 前缀；支持 OMP 的 `can1357/tap/omp` |
| WinGet / Scoop / Volta | 识别已有安装并通过原管理器更新；WinGet 也可用于首次安装 Claude Code / Codex |
| Nix / mise / asdf / Linux 系统包 / 应用内置 / 自定义来源 | 显示可检测的版本、路径和来源，通过原配置、终端或安装文档完成更新 |

扫描优先遵循 `PATH`，同时查找官方安装目录、Node 版本管理器及全局包管理器目录，包括自定义 `VP_HOME`、`BUN_INSTALL`、`PNPM_HOME`。Windows 优先选择可执行扩展名，避免误运行 npm 的 Unix 同名脚本。包管理器发现的有效入口若不在 `PATH` 中，会保存到已有可执行路径设置；手动指定但不存在的路径需先修正或恢复命令名。解析来源时同时检查入口和真实路径：Vite+ 的代理入口按 `vp` 处理，Homebrew Node 下的 npm 全局包仍按 npm 更新，共用 `bin` 目录不会被当作安装来源的证据。

安装和更新在后台执行，任务运行期间禁用，完成后重新检测版本及运行环境。版本输出无效、命令失败或超时会显示诊断，可取消或重试；取消、超时和关闭应用时清理安装进程树。不会在启动或扫描时自动安装、更新，也不会为来源不明的文件猜测更新命令。重新扫描会同时读取本地版本与 npm registry 的最新版本；仅在最新版本高于当前版本时提供更新操作。远端查询失败会显示原因，保留本地安装信息。

来源判定参考 [t3code 的维护逻辑](https://github.com/pingdotgg/t3code/blob/main/apps/server/src/provider/providerMaintenance.ts)，安装命令依据 [Claude Code](https://code.claude.com/docs/en/setup)、[Codex CLI](https://developers.openai.com/codex/cli/)、[OMP](https://github.com/can1357/oh-my-pi#install) 和 [Vite+ 全局包管理](https://viteplus.dev/guide/install)说明。

<details>
<summary>在终端检查 CLI 是否可用</summary>

只需准备你要使用的 Harness。以下命令分别检查版本、登录状态或模型目录：

```sh
claude --version
claude auth status --json

codex --version
codex login status

omp --version
omp models --json
```

</details>

### 更新 Nexus

在 **设置 → 通用 → 软件更新** 中可选择 **Release** 或 **Nightly** 频道，并手动检查更新。默认频道跟随当前安装包：Release 比较 `v<版本>` 的语义版本（包括 Alpha 等版本号预发布），Nightly 比较 `nightly-…` 标签中的提交时间，两者互不混入。主动切换频道后，检查更新会下载该频道的最新版本。发布工作流通过 `NEXUS_RELEASE_TAG` 将完整发布标签编译进应用；本地构建未指定时使用 `v<Cargo 版本>`。

应用默认启动时检查所选频道，发现更新后在后台自动下载当前操作系统及架构对应的压缩包，可关闭「启动时检查更新」。更新包保存在应用数据目录的 `updates/<频道>/` 下，通过文件大小和 GitHub 提供的 SHA-256 校验后才显示「更新已下载」；重复检查会复用已校验的下载，失败可点击「检查更新」重试。下载期间仍可使用应用。点击「打开下载位置」后解压更新包，退出应用再替换安装；此功能不自动替换程序或重启，也不修改会话数据。网络、GitHub 限流或平台包缺失会在设置中显示错误。

## 界面与快捷键

点击侧栏底部或任务顶部的「设置」，管理执行环境、远程访问和交互偏好。「返回工作区」会恢复原任务、输入草稿和滚动位置。

桌面端默认简体中文，可在 **设置 → 通用 → 界面语言** 切换为 **English**，立即生效并在重启后保留。用户内容、Agent 输出、原始诊断和 Remote Web 内容保持原样。

| 操作 | macOS | Linux / Windows |
| --- | --- | --- |
| 搜索会话 | `⌘ K` | `Ctrl K` |
| 新建任务 | `⌘ N` | `Ctrl N` |
| 打开或关闭设置 | `⌘ ,` | `Ctrl ,` |
| 发送消息 | `⌘ Enter` | `Ctrl Enter` |

## 数据目录

应用数据保存在：

| 系统 | 数据库路径 |
| --- | --- |
| macOS | `~/Library/Application Support/Nexus Agent/nexus.db` |
| Linux | `$XDG_DATA_HOME/nexus-agent/nexus.db`，默认 `~/.local/share/nexus-agent/nexus.db` |
| Windows | `%LOCALAPPDATA%/Nexus Agent/nexus.db`，缺省时使用 `%USERPROFILE%/AppData/Local/Nexus Agent/nexus.db` |

SQLite 保存项目、任务、Run、最终消息和 Session ID。启动时会把上次遗留的活动运行标记为 `Interrupted`，历史仍可阅读。API Key 使用系统凭据库，详见[模型与服务商](#模型与服务商)。

## 远程访问

Desktop 启动后默认在 `127.0.0.1:3210` 提供 HTTP/WebSocket 服务。设置页面的 `REMOTE CONTROL` 区域会显示服务地址，并提供“复制链接”和“复制令牌”按钮。访问令牌保存在现有 SQLite `settings` 表中；API 请求必须使用 Bearer Token，WebSocket 使用页面生成的临时连接参数。

直接在本机打开复制的链接即可进入内置 React 页面。通过 FRP 时，创建一个 TCP 代理，将公网端口转发到本机 `127.0.0.1:3210`，再把链接中的主机和端口替换成 FRP 公网地址。令牌放在 URL Fragment（`#token=...`）中，不会随 HTTP 请求发送；页面读取后会立即清除地址栏 Fragment，并只在当前标签页的 `sessionStorage` 中保留令牌。

远程页面可以：

- 浏览 Nexus 自己保存的项目、会话和消息；Codex CLI/Desktop 的只读导入历史不通过 Remote API 暴露。
- 使用 Desktop 当前选择的 Harness、模型和思考层级发起任务。
- 发起任务时使用 Desktop 当前选择的权限模式；需要授权时在 Desktop 弹窗中处理，Remote Web 暂不提供审批入口。
- 通过 WebSocket 接收状态变化和流式输出，并取消当前运行。

默认只监听回环地址，不直接暴露给局域网。端口冲突时可以在启动 Desktop 前设置 `NEXUS_REMOTE_ADDR`，例如 `127.0.0.1:4310`。本版本不内置 TLS 或 FRP 配置；公网暴露时应优先使用支持 HTTPS/WSS 的入口，并妥善保管访问令牌。

## 已知限制

- 当前 Alpha 在一个项目目录中执行任务；独立 Worktree、Git 成果接收及并发任务的进度见 [#69](https://github.com/ji233-Sun/nexus-agent/issues/69)。
- Codex 原有历史只读，不能从 Nexus 续聊这些导入会话。
- Remote Web 的授权请求仍需回到 Desktop 处理；当前不提供多设备账户、云端 Control Plane 或内置 TLS。
- Codex 的 MCP elicitation 尚未接入。模型能力、权限策略和文本输出时机取决于对应 Harness。
- macOS 发布包使用本地临时签名，尚无开发者签名或 Apple 公证。
