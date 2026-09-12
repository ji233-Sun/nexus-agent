<p align="center">
  <img src="docs/assets/readme-hero.svg" width="1200" alt="Nexus Agent — 统一管理你的本地编码 Agent，支持 Claude Code、Codex CLI 和 Oh My Pi。">
</p>

<p align="center">
  <a href="https://github.com/ji233-Sun/nexus-agent/actions/workflows/ci.yml"><img src="https://github.com/ji233-Sun/nexus-agent/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://github.com/ji233-Sun/nexus-agent/releases"><img src="https://img.shields.io/badge/channel-Alpha%20%2F%20Nightly-477AF5?style=flat-square" alt="Alpha / Nightly"></a>
  <img src="https://img.shields.io/badge/platform-macOS%20%C2%B7%20Linux%20%C2%B7%20Windows-303030?style=flat-square" alt="macOS / Linux / Windows">
</p>

<p align="center">
  <a href="#下载"><strong>下载应用</strong></a> ·
  <a href="#快速开始">快速开始</a> ·
  <a href="#核心能力">核心能力</a> ·
  <a href="docs/usage.md">使用指南</a> ·
  <a href="docs/development.md">参与开发</a>
</p>

---

**Nexus Agent** 是一个原生桌面工作区，将 **Claude Code、Codex CLI、Oh My Pi（OMP）、Pi、Kimi Code、Qoder（国际/国内版）和 CodeBuddy** 汇集在同一界面。从选择项目、发起任务，到跟进执行、审查代码变更和继续会话，都可以在这里完成。

基于 Rust 与 GPUI 构建，支持 macOS、Linux 和 Windows，提供简体中文与 English 界面。

> [!NOTE]
> 项目处于 Alpha 阶段。本页按当前 `main` 分支维护，下载包支持的功能以对应版本的发布说明为准。

## 核心能力

| 能力 | 你可以做什么 |
| --- | --- |
| **多引擎工作区** | 为新任务选择本地编码引擎，在统一时间线中查看回答、工具调用和执行状态。 |
| **持续会话** | 继续任务的原生会话；运行中排队消息，或通过 Steer 在工具调用后补充指令。 |
| **任务 Worktree** | 从所选分支创建独立工作目录，最多同时运行两个独立目录中的任务，分别管理消息、队列与审批。 |
| **Git 变更审查** | 查看完整差异，选择文件、生成提交说明并提交；Worktree 成果可预览后合入本地分支。 |
| **模型与服务商** | 选择可用模型和思考层级，切换多套 Provider Profile；API Key 保存在系统凭据库。 |
| **权限与交互** | 选择执行权限模式，在桌面处理工具审批和 Agent 提问。 |
| **CNB Issues** | 接入本机 CNB CLI，识别项目仓库，浏览 Issue 列表、标签与 Markdown 正文。[配置指南](docs/usage.md#cnb-issues) |
| **语音输入** | 使用 MiMo ASR 或 macOS 系统 Speech，将录音转为可编辑、可撤销的消息草稿。 |
| **PDF 阅读与标注** | 内置预览、文字搜索、页面编辑与标注，整页或框选截图可直接加入聊天，交给 Codex／Claude 看图分析。[使用说明](docs/usage.md#pdf-阅读与标注) |
| **远程访问** | 通过内置 Remote Web 查看会话、发起任务、接收实时输出和停止运行；审批在桌面处理。 |

## 下载

**[前往 GitHub Releases →](https://github.com/ji233-Sun/nexus-agent/releases)**

在发布版本的 **Assets** 中，按系统和芯片架构选择压缩包：

| 平台 | 文件名中的目标架构 | 安装 |
| --- | --- | --- |
| macOS · Apple Silicon | `aarch64-apple-darwin` | 解压 ZIP，将 `Nexus Agent.app` 移入「应用程序」 |
| macOS · Intel | `x86_64-apple-darwin` | 解压 ZIP，将 `Nexus Agent.app` 移入「应用程序」 |
| Linux · x86_64 | `x86_64-unknown-linux-gnu` | 解压 TAR.GZ，运行 `nexus-desktop`；查看[运行依赖](docs/development.md#环境要求) |
| Windows · x64 | `x86_64-pc-windows-msvc` | 解压 ZIP，运行 `nexus-desktop.exe` |

安装包已内置 Runner，发布页提供 `SHA256SUMS.txt` 校验文件。执行任务还需准备至少一个 Agent CLI，见下方[快速开始](#快速开始)。

Release（含 Alpha）和 Nightly 均在发布页提供；Nightly 按需触发，完成检查后发布。在 **设置 → 通用 → 软件更新** 中可选择更新频道并检查新版本，详见[安装与更新](docs/usage.md#更新-nexus)。

macOS 安装包采用本地临时签名，尚未获得 Apple 公证。

## 快速开始

1. **配置执行引擎。** 打开 **设置 → 执行引擎 → Harness 管理**，检查或安装要使用的 Agent CLI。Nexus 将这些执行引擎称为 Harness。完成 CLI 登录，或在 **设置 → 凭据配置** 中添加对应的 [Provider Profile](docs/usage.md#模型与服务商)。
2. **打开本地项目。** 选择工作目录，再选好执行引擎、模型和权限模式。Git 项目可选择「本地」或「Worktree」模式；使用 Worktree 时，选择作为起点的本地分支。
3. **发送任务并跟进。** 描述要完成的工作，从时间线查看进展，按需回复审批或补充消息。任务完成后可查看变更；以后从侧栏打开同一任务即可继续会话。

续聊沿用原任务的执行引擎；需要更换引擎时，新建任务即可。

小提示：`⌘ / Ctrl K` 搜索会话，`⌘ / Ctrl N` 新建任务，`⌘ / Ctrl Enter` 发送消息。

<details>
<summary><strong>从命令行打开项目</strong></summary>

在 **设置 → 通用 → 命令行** 点击「安装 CLI」，重新打开终端后运行：

```sh
nexus-desktop .                 # 打开当前目录
nexus-desktop "/path/to/project" # 打开指定目录（支持相对路径）
```

这是安装包自带的原生命令，无需 Node.js 或 npx。CLI 打开桌面应用后立即返回终端，关闭终端不影响应用运行。无参数时打开应用，`--help` 显示用法，`--version` 显示版本；每次调用打开一个独立的应用进程。

macOS 也可直接运行 `"/Applications/Nexus Agent.app/Contents/MacOS/nexus-desktop" .`；Windows 使用 `nexus-desktop.exe`。

「安装 CLI」会将可执行文件所在目录加入当前用户的 `PATH`，无需管理员权限。macOS / Linux 支持 zsh、bash 和 sh；Windows 更新用户环境变量。现有 Shell 配置与 PATH 条目会保留，移动应用后可再次安装以更新路径。

</details>

<details>
<summary><strong>配置语音输入</strong></summary>

打开 **设置 → 语音输入**，选择并配置 Provider：

| Provider | 支持平台 | 配置与音频处理 |
| --- | --- | --- |
| MiMo ASR | macOS / Linux / Windows | 保存自己的 API Key，并允许麦克风访问；停止录音后，音频发送至 MiMo 识别。 |
| 系统 Speech | macOS | 无需 API Key；首次录音申请麦克风与 Speech 权限。识别可能联网并将音频发送给 Apple，不保证离线。 |

每段最多录制 **60 秒**，最终文本只回填消息草稿，可编辑、可撤销，由你确认后发送。

</details>

<details>
<summary><strong>从源码运行</strong></summary>

准备 rustup 和对应平台的[构建依赖](docs/development.md#环境要求)，然后运行：

```sh
git clone https://github.com/ji233-Sun/nexus-agent.git
cd "nexus-agent"
cargo run -p nexus-desktop --locked
```

rustup 会使用仓库 `rust-toolchain.toml` 指定的工具链，首次构建时按需下载。更多构建、测试和架构说明见[开发指南](docs/development.md)。

</details>

## 文档导航

| 你想了解 | 文档 |
| --- | --- |
| 独立目录、并发、变更审查与提交 | [任务 Worktree](docs/usage.md#任务-worktree) |
| 续聊、排队消息与 Steer | [会话与消息](docs/usage.md#会话与消息) |
| CNB 集成、Issue 列表与详情 | [CNB Issues](docs/usage.md#cnb-issues) |
| 模型、服务商和密钥保存 | [模型与服务商](docs/usage.md#模型与服务商) |
| 三种权限模式、桌面审批与 Agent 提问 | [权限与审批](docs/usage.md#权限与审批) |
| Harness 安装、版本检测和应用更新 | [安装与更新](docs/usage.md#安装与更新) |
| 界面语言和完整快捷键 | [界面与快捷键](docs/usage.md#界面与快捷键) |
| 浏览器远程访问与 FRP | [远程访问](docs/usage.md#远程访问) |
| 数据保存位置和当前限制 | [数据目录](docs/usage.md#数据目录) · [已知限制](docs/usage.md#已知限制) |
| 架构、测试、发布和维护 | [开发与维护](docs/development.md) |

## 致谢

Nexus 的原生界面基于 [GPUI Kit](https://github.com/longbridge/gpui-kit)。感谢 [Vercel Design MD](https://github.com/educlopez/design-bites/blob/main/design-mds/vercel.com/DESIGN.md)、[Synara](https://github.com/Emanuele-web04/synara) 与 [t3code](https://github.com/pingdotgg/t3code) 提供的设计和实现参考。

---

<p align="center">
  <a href="https://github.com/ji233-Sun/nexus-agent/issues/new">报告问题或提出建议</a> ·
  <a href="https://github.com/ji233-Sun/nexus-agent/pulls">查看贡献</a>
</p>
