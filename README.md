<p align="center">
  <img src="docs/assets/readme-hero.svg" width="1200" alt="Nexus Agent — 统一管理你的本地编码 Agent，支持 Claude Code、Codex CLI 和 Oh My Pi。">
</p>

<p align="center">
  <a href="https://github.com/ji233-Sun/nexus-agent/actions/workflows/ci.yml"><img src="https://github.com/ji233-Sun/nexus-agent/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://github.com/ji233-Sun/nexus-agent/releases"><img src="https://img.shields.io/badge/channel-Alpha%20%2F%20Nightly-477AF5?style=flat-square" alt="Alpha / Nightly"></a>
  <img src="https://img.shields.io/badge/platform-macOS%20%C2%B7%20Linux%20%C2%B7%20Windows-303030?style=flat-square" alt="macOS / Linux / Windows">
</p>

Nexus Agent 是一个面向 Linux、macOS 和 Windows 的本地桌面应用，用统一时间线驱动本机已安装的 Claude Code、Codex CLI 或 Oh My Pi（OMP）。应用内显示当前构建的完整发布标签，与对应的 GitHub Release 和安装包一致。

<p align="center">
  <a href="https://github.com/ji233-Sun/nexus-agent/releases"><strong>下载应用</strong></a> ·
  <a href="#快速开始">快速开始</a> ·
  <a href="docs/usage.md">使用指南</a> ·
  <a href="docs/development.md">参与开发</a>
</p>

---

**Nexus Agent** 是一款基于 Rust 与 GPUI 的原生桌面应用。在一个工作区里选择项目、切换本地编码 Agent、跟进工具执行，并继续已有任务的会话。

支持 **Claude Code、Codex CLI 和 Oh My Pi（OMP）**，提供简体中文与 English 界面。

## 核心能力

| 会话与项目 | 配置与控制 |
| --- | --- |
| **统一时间线**<br>把回答、工具调用、执行状态和错误放在同一处，随时查看任务进展。 | **自由选择 Harness**<br>切换 Claude Code、Codex CLI 或 OMP，查看可用模型与思考层级。 |
| **原生会话续聊**<br>任务绑定原生 Session，重新打开后继续；也可只读浏览 Codex 原有历史。 | **多套服务商配置**<br>管理 API Key、Base URL 和默认模型，密钥保存在系统凭据库。 |
| **排队与 Steer**<br>运行中补充消息，排队等待下一轮，或在工具调用后介入当前任务。 | **明确的执行权限**<br>选择授权模式，在桌面处理审批；内置 Remote Web 可查看进展和停止运行。 |

## 下载

**[前往 GitHub Releases →](https://github.com/ji233-Sun/nexus-agent/releases)**

在对应发布版本的 **Assets** 中选择与你的系统和架构匹配的压缩包：

| 平台 | 文件名中的目标架构 | 安装 |
| --- | --- | --- |
| macOS · Apple Silicon | `aarch64-apple-darwin` | 解压 ZIP，将 `Nexus Agent.app` 移入「应用程序」 |
| macOS · Intel | `x86_64-apple-darwin` | 解压 ZIP，将 `Nexus Agent.app` 移入「应用程序」 |
| Linux · x86_64 | `x86_64-unknown-linux-gnu` | 解压 TAR.GZ，运行 `nexus-desktop`；查看[运行依赖](docs/development.md#环境要求) |
| Windows · x64 | `x86_64-pc-windows-msvc` | 解压 ZIP，运行 `nexus-desktop.exe` |

安装包已内置 Runner。发布页同时提供 `SHA256SUMS.txt` 校验文件。

> [!NOTE]
> 项目处于 Alpha 阶段，当前基础版本为 `0.1.0-alpha.1`。Nightly 提供通过检查的新构建；本页说明以当前 main 为准，下载包能力请结合对应版本查看。macOS 包采用本地临时签名，尚未获得 Apple 公证。

## 快速开始

Nightly 每 4 小时定时检查默认分支 `main` 的最新提交（cron：`0 */4 * * *`，UTC 每天 00:00、04:00、08:00、12:00、16:00、20:00，北京时间也为这六个时刻）。若该提交尚未发布 Nightly，则执行同一套检查和四平台打包流程，全部成功后自动发布一个 Nightly 预发布版；已经发布过的提交会跳过构建和发布。向 `main` 推送新提交（包括合并 PR）仅触发 CI，Nightly 等待下一次定时运行。标签及压缩包文件名使用 `nightly-YYYY-MM-DD-<Unix秒时间戳>-<12位提交SHA>`，例如 `nightly-2026-09-06-1788673923-6bbbdd981018`。日期与时间戳取自该提交的提交者时间，日期按 UTC 计算；同一提交重跑工作流时保持相同标签。Nightly 标签指向本次构建的完整提交 SHA，始终标记为 Pre-release，不占用 Latest；不同提交的发布独立执行。Cargo 包版本和 macOS 的数字基础版本继续沿用 `Cargo.toml`，应用内显示完整 Nightly 标签，无需为每次 Nightly 修改版本文件。

1. **准备一个 Harness。** 打开应用，在「设置 → 执行引擎 → Harness 管理」检查或安装要使用的 CLI。完成 CLI 登录，或配置对应的 [Provider Profile](docs/usage.md#模型与服务商)。
2. **打开本地项目。** 选择工作目录，在输入区选好 Harness、模型和权限模式。
3. **发送你的任务。** 从时间线查看执行过程，按需回复审批或补充消息；之后打开同一任务即可继续会话。

小提示：`⌘ / Ctrl K` 搜索会话，`⌘ / Ctrl N` 新建任务，`⌘ / Ctrl Enter` 发送消息。

<details>
<summary><strong>从源码运行</strong></summary>

在 **设置 → 通用 → 软件更新** 中可选择 **Release** 或 **Nightly** 频道，并手动检查更新。默认频道跟随当前安装包：Release 比较 `v<版本>` 的语义版本（包括 Alpha 等版本号预发布），Nightly 比较 `nightly-…` 标签中的提交时间，两者互不混入。主动切换频道后，检查更新会展示该频道的最新版本。发布工作流通过 `NEXUS_RELEASE_TAG` 将完整发布标签编译进应用；设置页、更新检查和 `nexus-desktop --version` 共用此标签。本地构建优先使用显式指定的标签，否则使用当前提交上的版本标签或按提交生成的 Nightly 标签；没有 Git 元数据的源码包才回退到 `v<Cargo 版本>`。macOS 的系统版本字段保留数字格式，构建编号使用发布工作流运行编号，完整标签另存于 bundle 元数据。

应用默认启动时检查所选频道，可关闭「启动时检查更新」。发现新版本后在侧栏提示，并在设置中展示完整版本标签、GitHub Release 的 Markdown 更新日志和完整发布说明链接。检查只读取发布信息；点击「更新并重启」后才开始下载对应操作系统及架构的压缩包。更新包保存在应用数据目录的 `updates/<频道>/` 下，校验文件大小和 GitHub 提供的 SHA-256；再次下载时复用已校验的缓存。

下载期间仍可使用应用。校验成功后，等待当前任务和 Harness 安装操作结束，再自动解压、退出、安装并重启；准备安装期间暂停启动新任务。macOS 替换整个 `.app`，Windows/Linux 替换 Desktop 和 Runner 程序文件，保留其他文件及会话数据。更新通过独立进程等待原程序退出，替换或重启失败时回滚到旧程序并展示错误。网络、校验和安装错误可在设置中查看，安装进程日志保存在 `updates/installation.log`；恢复失败时保留安装现场供排查。

自动安装要求应用所在目录可写；macOS 需要从已解压的 `.app` 中运行。开发目录中的裸 macOS 可执行文件和只读安装位置会显示安装错误。

准备 rustup 和对应平台的[构建依赖](docs/development.md#环境要求)，然后运行：

```sh
git clone https://github.com/ji233-Sun/nexus-agent.git
cd nexus-agent
cargo run -p nexus-desktop --locked
```

仓库会自动选择 Rust 1.98.1。更多构建、测试和架构说明见[开发指南](docs/development.md)。

</details>

## 继续了解

| 你想了解 | 文档 |
| --- | --- |
| 续聊、排队消息与 Steer | [会话与消息](docs/usage.md#会话与消息) |
| 模型、服务商和密钥保存 | [模型与服务商](docs/usage.md#模型与服务商) |
| 三种权限模式和桌面审批 | [权限与审批](docs/usage.md#权限与审批) |
| Harness 安装、版本检测和应用更新 | [安装与更新](docs/usage.md#安装与更新) |
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
