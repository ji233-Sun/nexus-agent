# Nexus Agent ADE — MVP 开发计划

> 文档状态：Draft  
> 目标版本：v0.1.0  
> 当前阶段：初版 / 本地 Harness 闭环  
> 目标平台：Linux / macOS / Windows<br>
> 更新时间：2026-09-05

## 1. 文档目的

本文档定义 Nexus Agent ADE 初版的产品范围、技术架构、开发里程碑、验收标准和后续演进边界。

初版只解决一个问题：用户在统一的本地桌面 GUI 中选择本地项目、连接本机已经安装的 Agent Harness、发送 Prompt、查看流式执行过程，并可以取消或结束任务。

本文档是 v0.1.0 的执行基线。任何超出 MVP 范围的功能，在进入开发前都需要先更新本文件或补充 ADR（Architecture Decision Record）。

## 2. 核心决策摘要

| 决策项 | MVP 选择 | 原因 |
| --- | --- | --- |
| 产品形态 | 本地桌面应用 | 最快验证核心 Agent 交互体验 |
| GUI | GPUI，先通过技术 Spike | 面向低延迟、高吞吐和原生体验 |
| 核心语言 | Rust | 便于构建高性能 GUI、进程管理和后续 Headless Runner |
| 进程架构 | Desktop + Local Runner 两个进程 | 隔离 GUI 与 Harness，保留未来远程化边界 |
| 本地通信 | Runner 标准输入/输出上的版本化 JSON Lines | MVP 简单、易调试，不提前引入网络服务 |
| 首个 Harness | Codex | 作为参考适配器稳定最小 Harness Contract |
| 后续 MVP Harness | Claude Code | 在参考适配器和协议稳定后接入 |
| 工作目录 | 用户直接选择的本地目录 | MVP 不创建、不删除 Worktree |
| 持久化 | 本地 SQLite | 保存项目、任务、Run 和最终消息 |
| 并发策略 | 每个 Task 最多一个活动 AgentRun | 避免并发写工作目录和交互歧义 |
| 目标平台 | Linux / macOS / Windows | 共享 MVP 与应用层，平台差异集中于基础设施，CI 三平台验证 |

## 3. 产品目标

### 3.1 MVP 目标

用户能够完成以下闭环：

1. 启动 Nexus Agent 桌面应用。
2. 选择一个本地项目目录。
3. 查看本机可用的 Harness。
4. 创建一个任务并输入 Prompt。
5. 启动 Harness，在所选项目目录中执行。
6. 在统一 GUI 中查看流式文本、状态、工具调用摘要和错误。
7. 根据 Harness 能力响应基本审批请求。
8. 取消正在执行的 Run。
9. 应用重启后查看历史任务和最终消息。

### 3.2 成功标准

- 新用户无需手工编辑配置即可完成第一次本地执行。
- Harness 不存在、未登录、异常退出时，GUI 能给出明确且可执行的错误信息。
- 流式输出期间输入框、滚动和取消操作保持响应。
- GUI 崩溃或关闭时，不遗留失控的 Harness 子进程。
- 领域层、Harness Adapter 和 UI 之间没有直接耦合到某个 Provider 的私有数据结构。
- Codex、Claude Code 至少完成最小能力接入；不支持的能力必须通过 Capability 明确声明。

## 4. MVP 范围

### 4.1 必须实现

- 本地项目目录选择和最近项目列表。
- 本地 Harness 自动探测。
- Codex 参考适配器。
- Claude Code 最小适配器。
- 创建 Task。
- 启动、流式观察、取消 AgentRun。
- 纯文本 Prompt。
- 标准化的 Assistant、Tool、Status、Error、Approval 事件。
- 一个 Task 同时只允许一个活动 Run。
- 本地任务历史和最终消息持久化。
- 基础设置：Harness 可执行文件路径、默认 Harness、日志级别。
- 基础诊断日志和崩溃后子进程清理。
- Linux、macOS 和 Windows 构建与本地安装验证。

### 4.2 明确不做

- UDP Remote、移动端原生客户端、云端 Remote Control Plane。
- 后台常驻 Daemon、局域网监听、云端 Control Plane。
- 部署到服务器或多节点调度。
- Agent 中途切换和跨 Harness Handoff。
- Agent 原生 Session 恢复、Fork 或同步。
- GitHub、GitLab、CNB 的 Issue/PR 管理。
- TAPD、Jira 或其他项目管理集成。
- 自动创建、回收或清理 Git Worktree。
- Git Commit、Push、Merge、Rebase 等写操作。
- 内置完整终端、终端多 Tab。
- 快速启动 VS Code 或其他 IDE。
- 多 Agent 并行协作。
- 图片、音频、文件附件和富媒体 Prompt。
- 插件系统、脚本市场和第三方扩展 SDK。
- 团队账户、组织权限、计费和云同步。

### 4.3 MVP 中的 Git 行为

- Nexus Agent 不主动创建分支或 Worktree。
- Harness 直接在用户选择的目录中运行。
- 启动前展示当前目录路径。
- 如果目录存在未提交修改，展示非阻塞警告。
- Nexus Agent 不自动还原、删除或提交任何文件变更。
- Harness 造成的文件修改由用户自行审查和处理。

## 5. 用户故事

### US-01：打开本地项目

作为开发者，我可以选择一个已有本地目录，以便让 Harness 在该目录中工作。

验收标准：

- 目录路径被规范化并持久化。
- 不存在或无权限的目录不能进入执行状态。
- 最近项目可以再次打开。

### US-02：发现 Harness

作为开发者，我可以看到本机已安装且可运行的 Harness。

验收标准：

- 展示 Harness 名称、检测状态、可执行文件路径和基础版本信息。
- 未安装、无法执行、未登录分别显示不同状态。
- 探测失败不影响其他 Harness。

### US-03：发送 Prompt

作为开发者，我可以创建任务并向选定 Harness 发送纯文本 Prompt。

验收标准：

- 空 Prompt 不可提交。
- 重复点击不会启动两个 Run。
- Run 明确关联 Task、Project、Harness 和工作目录。

### US-04：观察执行过程

作为开发者，我可以在统一时间线中查看 Harness 的执行状态和流式输出。

验收标准：

- 流式文本可以增量显示。
- Tool、Approval、Error、Exit 状态具有不同的视觉语义。
- UI 不直接展示无法理解的 Provider 私有协议帧。
- 原始帧只进入诊断日志，并默认脱敏。

### US-05：取消执行

作为开发者，我可以取消正在执行的 Run。

验收标准：

- 首先发送温和中断。
- 超时后允许终止子进程。
- 最终状态明确区分 Cancelled、Failed 和 Completed。
- 取消操作可以幂等重试。

### US-06：查看历史

作为开发者，我可以在应用重启后查看已有 Task 和最终消息。

验收标准：

- 已落盘的 Task、Run、Message 可以恢复。
- 上次异常退出时处于 Running 的 Run 被标记为 Interrupted。
- MVP 不尝试自动恢复 Provider 原生会话。

## 6. 总体架构

```text
┌──────────────────────────────────────────┐
│ nexus-desktop                            │
│ GPUI View / Presenter / Model + SQLite   │
└───────────────────┬──────────────────────┘
                    │ versioned JSONL over stdio
┌───────────────────▼──────────────────────┐
│ nexus-runner                             │
│ Command Router / Orchestrator            │
│ Harness Registry / Process Supervisor    │
└──────────────┬──────────────┬────────────┘
               │              │
        Codex Adapter   Claude Adapter
               │              │
        Local child processes in project cwd
```

### 6.1 Desktop 职责

- 窗口、导航、列表、输入和交互状态。
- 启动和监督 Local Runner。
- 将用户意图转换成 Runner Command。
- 将 Runner Event 投影为 UI 状态。
- 保存项目、任务、Run 和最终消息。
- 展示错误、审批和退出状态。

Desktop 不负责：

- 直接拼接或执行 Harness 命令。
- 解析 Provider 原始输出。
- 管理 Harness 子进程树。
- 修改工作目录中的文件。

### 6.2 Runner 职责

- 探测本机 Harness。
- 校验命令参数和工作目录。
- 启动、观察、中断和终止 Harness 子进程。
- 将 Provider 原始输出转换为统一 AgentEvent。
- 维护 Task 内活动 Run 的独占约束。
- 在退出时回收自己创建的子进程。
- 通过标准输出发送协议事件，日志写入标准错误或独立日志文件。

### 6.3 为什么 MVP 就拆成两个进程

- Harness 崩溃、输出异常或进程树问题不会直接破坏 GUI 状态。
- Runner 标准输出可以保留为严格协议通道。
- 后续 Remote 版本可以替换 Transport，而不重写 Orchestrator 和 Harness Adapter。
- 可以独立编写 Runner 集成测试，无需启动 GUI。

MVP 不实现后台常驻 Runner。Desktop 启动时创建 Runner，退出时结束 Runner。

## 7. 当前仓库结构

```text
apps/
  desktop/src/
    main.rs            # 调用启动入口
    bootstrap.rs       # 窗口和依赖装配
    model/             # MVP 状态与数据
    presenter/         # 用户操作、事件处理和持久化协调
    view/              # GPUI 渲染与控件
    infrastructure/    # SQLite、RunnerClient、Codex 历史、Git 状态
  desktop/tests/       # 内置 Runner 协议集成测试
  runner/src/
    main.rs            # 调用库入口
    lib.rs             # Tokio 运行时与传输装配
    transport.rs       # JSONL 传输及版本校验
    application/       # 命令调度、运行独占、取消、事件转换
    infrastructure/    # Harness 选择、进程执行和清理
  runner/tests/        # Fake Harness 协议集成测试
crates/
  domain/              # Task、Run、Message、状态和错误
  protocol/            # Desktop ↔ Runner 协议
  harness-core/        # 启动规格、事件解码接口和可执行文件解析
  harness-codex/       # Codex 适配器
  harness-claude/      # Claude Code 适配器
```

约束：

- 初始阶段不要提前创建空 crate。
- 只有出现第二个真实调用者时才抽取共享 UI 或基础设施模块。
- Provider 私有类型只能存在于对应 Adapter 内。

## 8. 领域模型

### 8.1 Project

```text
Project
  id
  display_name
  canonical_path
  created_at
  last_opened_at
```

### 8.2 Task

```text
Task
  id
  project_id
  title
  status: Draft | Running | WaitingForApproval | Completed | Failed | Cancelled | Interrupted
  created_at
  updated_at
```

### 8.3 AgentRun

```text
AgentRun
  id
  task_id
  harness_kind
  harness_version
  status: Starting | Running | Cancelling | Completed | Failed | Cancelled | Interrupted
  started_at
  ended_at
  exit_code
  failure_code
```

### 8.4 Message

```text
Message
  id
  task_id
  run_id
  sequence
  role: User | Assistant | Tool | System
  kind: Text | ToolCall | ToolResult | Approval | Status | Error
  content
  created_at
```

### 8.5 重要不变量

- 一个 Task 同时最多存在一个 Starting、Running 或 Cancelling 状态的 AgentRun。
- Message 的 sequence 在单个 Task 内单调递增。
- Completed、Failed、Cancelled、Interrupted 是 Run 的终态。
- Provider 原生 Session ID 是可选元数据，不作为 Nexus Task ID。
- 工作目录必须等于 Project 的 canonical_path；MVP 不允许 Harness 临时覆盖 cwd。

## 9. Harness Adapter Contract

### 9.1 最小接口

```rust
trait HarnessAdapter {
    fn kind(&self) -> HarnessKind;
    fn capabilities(&self) -> HarnessCapabilities;
    fn probe(&self) -> ProbeResult;
    fn build_launch_spec(&self, request: StartRunRequest) -> LaunchSpec;
    fn decode_event(&mut self, frame: RawFrame) -> Vec<AgentEvent>;
}
```

进程启动、终止和超时由统一 ProcessSupervisor 完成，Adapter 不自行管理通用进程生命周期。

### 9.2 MVP Capability

```text
stream_text
tool_events
approval_events
graceful_cancel
version_probe
login_probe
```

未支持能力返回 false。UI 根据 Capability 决定是否展示对应交互，不通过 Harness 名称写条件分支。

### 9.3 Adapter 接入顺序

1. Fake Harness：验证协议、取消、错误和高吞吐测试。
2. Codex：参考实现，稳定 Contract。
3. Claude Code：验证第二个 Provider，修正错误抽象。

### 9.4 Adapter 约束

- 优先使用 Harness 官方机器可读模式。
- 禁止通过脆弱的终端颜色和自然语言文本判断核心状态。
- 所有命令使用 executable + argv 结构，禁止拼接 Shell 字符串。
- 原始输出设置单帧和总缓冲上限。
- 无法识别的帧转换为受限 Diagnostic 事件，不得导致整个 Run 崩溃。

## 10. Desktop ↔ Runner 协议

### 10.1 Envelope

```json
{
  "protocol_version": 1,
  "id": "request-or-event-id",
  "kind": "run.start",
  "sequence": 42,
  "payload": {}
}
```

### 10.2 MVP Commands

- `runner.hello`
- `harness.list`
- `harness.probe`
- `run.start`
- `run.cancel`
- `approval.respond`
- `runner.shutdown`

### 10.3 MVP Events

- `runner.ready`
- `harness.detected`
- `run.started`
- `run.output.delta`
- `run.message.completed`
- `run.tool.started`
- `run.tool.completed`
- `run.approval.requested`
- `run.status.changed`
- `run.failed`
- `run.exited`

### 10.4 协议规则

- Runner 标准输出只允许出现协议帧。
- Runner 日志不得写入标准输出。
- 每个 Command 必须有唯一 ID。
- Runner 对同一个取消请求保持幂等。
- 每个 Run 的 Event sequence 单调递增。
- 未知事件类型被忽略并记录诊断，不导致客户端崩溃。
- MVP 只保证同版本 Desktop 和 Runner 兼容。
- 协议仍必须携带 version，为后续升级预留显式拒绝机制。

## 11. UI 信息架构

### 11.1 主窗口

```text
┌──────────────┬──────────────────────────┬──────────────┐
│ Projects     │ Task Timeline            │ Run Status   │
│ Tasks        │                          │ Harness      │
│              │ User / Assistant / Tool  │ Version      │
│              │ Error / Approval         │ CWD          │
│              ├──────────────────────────┤              │
│              │ Prompt Composer          │ Cancel       │
└──────────────┴──────────────────────────┴──────────────┘
```

### 11.2 MVP 页面

- 首次启动页：选择项目。
- 主任务页：Task 列表、时间线、Prompt 输入区和 Run 状态。
- Harness 设置页：探测状态、路径、版本和默认项。
- 诊断页：应用版本、Runner 状态和脱敏日志导出。

### 11.3 UI 状态原则

- Durable State 来自 SQLite。
- Live State 来自当前 Runner Event Stream。
- View State 只保存选择、滚动、展开和输入草稿。
- 流式 delta 在内存中按帧合并；消息完成后再写入最终正文。
- UI 不持久化每个 token delta。

## 12. 持久化策略

### 12.1 SQLite 内容

- Projects。
- Tasks。
- AgentRuns。
- 最终 Messages。
- Harness 配置。
- Schema version。

### 12.2 不持久化

- 每个 token 的原始 delta。
- 未脱敏的环境变量。
- Harness 登录 Token。
- 完整进程环境。
- 工作目录文件内容。
- Provider 不透明内部状态。

### 12.3 崩溃恢复

应用启动时：

1. 打开数据库并执行向前兼容迁移。
2. 将遗留的 Starting、Running、Cancelling Run 标记为 Interrupted。
3. 不自动重启 Harness。
4. 在 Task 时间线显示异常中断提示。

## 13. 进程生命周期

### 13.1 启动

1. Desktop 启动 Runner 子进程。
2. 发送 `runner.hello`。
3. 校验协议版本。
4. Runner 返回 `runner.ready`。
5. Desktop 请求 Harness 探测。

### 13.2 Run 启动

1. Desktop 持久化 Task 和 Starting Run。
2. 发送 `run.start`。
3. Runner 校验目录和 Harness。
4. Runner 启动子进程并返回 `run.started`。
5. Desktop 将状态更新为 Running。

### 13.3 取消

1. Desktop 将 Run 标记为 Cancelling。
2. Runner 向 Harness 发送平台对应的温和中断。
3. 等待配置化短超时。
4. 未退出则终止 Runner 创建的进程树。
5. 发出唯一终态事件。

### 13.4 Desktop 退出

1. 对活动 Run 发起取消。
2. 等待受限时间。
3. 请求 Runner 关闭。
4. 必要时终止 Runner 及其子进程树。
5. 禁止无限等待阻塞应用退出。

## 14. 错误模型

错误必须包含稳定代码和面向用户的说明：

```text
HarnessNotFound
HarnessNotExecutable
HarnessNotAuthenticated
ProjectNotFound
ProjectPermissionDenied
ProtocolVersionMismatch
RunnerUnavailable
RunAlreadyActive
LaunchFailed
MalformedHarnessOutput
ApprovalUnsupported
CancellationTimeout
UnexpectedExit
PersistenceFailed
```

错误处理要求：

- 技术细节进入诊断日志。
- UI 提供用户下一步动作，例如“重新探测”“打开设置”“复制诊断信息”。
- 不展示 Token、完整环境变量或可能含敏感信息的命令参数。
- 单个 Adapter 失败不得让 Runner 退出。

## 15. 安全边界

- Runner 只接受 Desktop 启动时建立的私有 stdio 通道。
- MVP 不开启任何网络端口。
- 工作目录在 Run 启动前做 canonicalize 和存在性检查。
- 进程启动禁止通过 Shell 拼接用户输入。
- Harness 凭证继续由 Harness 自己管理，Nexus Agent 不接管登录 Token。
- Prompt 不写入普通日志；诊断导出默认移除 Prompt 正文和路径中的用户目录信息。
- Approval 必须显示 Harness、工作目录、操作摘要和风险信息。
- Desktop 不具备静默执行外部命令的通用接口。

## 16. 性能目标

性能基准必须记录测试机器、构建类型和数据规模，禁止只报告主观体验。

### 16.1 MVP 指标

- Release 构建冷启动进入可交互状态：目标小于 1 秒。
- 流式输出期间 Prompt 输入延迟：p95 小于 50 ms。
- 时间线正常滚动：目标 60 FPS，p95 frame time 小于 16.7 ms。
- 单 Task 10,000 条消息仍使用虚拟化渲染，不一次性创建全部元素。
- Runner 可以持续处理每秒 200 个标准化事件，GUI 不冻结。
- 日志或输出消费者变慢时，内存保持有界。
- 取消操作发起后 100 ms 内更新 UI 状态。

### 16.2 性能实现原则

- UI 线程不解析 Provider 原始协议。
- Runner 批量发送可合并的文本 delta。
- Desktop 每帧最多提交一次同一消息的文本更新。
- 有界 Channel 必须定义丢弃、合并或反压策略。
- SQLite 写入不得发生在 GPUI 绘制路径中。
- 只持久化完成消息和必要状态转换。
- Release 性能数据才用于决策。

## 17. GPUI 技术 Spike

在搭建完整 UI 前完成独立 Spike。

### 17.1 Spike 内容

- 10,000 条虚拟化消息。
- 单条持续增长的流式 Markdown 文本。
- 代码块、复制和文本选择。
- 中文输入法、emoji 和组合字符。
- 高速事件注入和取消按钮响应。
- 窗口缩放、HiDPI 和深浅主题。
- 基础无障碍树验证。
- Release 构建和应用打包。

### 17.2 Go 条件

- 无 P0 输入法、文本选择或崩溃问题。
- 达到第 16 节核心交互指标，或有明确且低风险的优化路径。
- 团队可以维护所需的基础组件，不需要复制大规模 GPL UI 代码。
- Linux、macOS 和 Windows 构建流程可重复，发布包分别验收。

### 17.3 No-Go 处理

- 暂停正式 UI 开发。
- 记录失败原因和测量数据。
- 单独评估其他桌面框架。
- 不在 MVP 中同时维护 GPUI 和第二套生产 UI。
- Runner、Domain 和 Protocol 继续保留，不受 GUI 框架变更影响。

## 18. 测试策略

### 18.1 单元测试

- Task 和 Run 状态机。
- 单活动 Run 不变量。
- Protocol 编解码和版本拒绝。
- Adapter 原始帧到 AgentEvent 的映射。
- 错误脱敏。
- 取消操作幂等性。

### 18.2 Fake Harness 集成测试

Fake Harness 必须支持：

- 正常流式输出。
- 突发高吞吐输出。
- Tool Call 和 Tool Result。
- Approval Request。
- 非零退出码。
- 畸形输出。
- 忽略温和中断。
- 启动后立即崩溃。

### 18.3 Runner 集成测试

- 使用临时目录启动 Run。
- 验证 cwd。
- 验证环境变量白名单/继承策略。
- 取消后不残留子进程。
- Desktop 通道关闭后自动清理。
- 单个 Adapter 异常不终止 Runner。

### 18.4 UI 测试

- 创建 Task 并发送 Prompt。
- Running 状态禁止重复启动。
- 流式消息合并。
- Cancel 状态转换。
- 应用重启后历史恢复。
- Harness 未安装时的错误引导。

### 18.5 手工验收矩阵

| 场景 | Codex | Claude Code |
| --- | --- | --- |
| 探测 | 必须 | 必须 |
| 基础 Prompt | 必须 | 必须 |
| 流式文本 | 必须 | 必须 |
| 取消 | 必须 | 必须 |
| Tool 事件 | 按能力 | 按能力 |
| Approval | 按能力 | 按能力 |
| 异常退出 | 必须 | 必须 |

## 19. 开发里程碑

### M0：技术基线与 GPUI Spike

交付物：

- Rust workspace。
- 基础 CI：format、lint、unit test、release build。
- GPUI Spike 和性能报告。
- ADR-001：桌面 GUI 框架决策。
- ADR-002：Codex 参考协议模式。

退出条件：

- GPUI 完成 Go/No-Go 决策。
- Fake 数据下 UI 没有 P0 阻塞。

### M1：Domain、Protocol 与 Fake Harness

交付物：

- Task、AgentRun、Message 模型。
- Run 状态机。
- Desktop ↔ Runner JSONL 协议。
- Fake Harness。
- Runner 进程监督和取消。
- 协议及生命周期集成测试。

退出条件：

- 可以通过命令行向 Runner 启动 Fake Harness。
- 正常、失败、取消和畸形输出路径全部通过测试。

### M2：Codex 本地闭环

交付物：

- Codex 探测和版本检查。
- Codex Start、Stream、Cancel。
- 最小 Tool/Approval 映射。
- 首版 Task Timeline 和 Prompt Composer。
- SQLite 持久化。

退出条件：

- 用户可以从 GUI 在真实项目中完成一次 Codex 执行。
- 应用重启后可以查看最终历史。

### M3：Claude Code

交付物：

- Claude Code Adapter。
- Capability 驱动的 UI。
- 两个 Harness 的错误与未登录引导。

退出条件：

- Codex 和 Claude Code 通过第 18.5 节验收矩阵。
- UI 和 Orchestrator 中没有 Harness 名称分支。

### M4：稳定性、性能和 Alpha 打包

交付物：

- 性能基准和回归数据。
- 崩溃恢复。
- 日志脱敏与诊断导出。
- Linux、macOS 和 Windows Release 打包。
- 安装、首次运行和故障排查文档。

退出条件：

- 达到性能目标或记录已接受的偏差。
- 不残留 Harness 子进程。
- 完成端到端 Alpha 验收。

## 20. 优先级 Backlog

### P0

- GPUI Spike。
- Rust workspace 和 CI。
- Protocol v1。
- Fake Harness。
- ProcessSupervisor。
- Run 状态机。
- Codex Adapter。
- Project Picker。
- Task Timeline。
- Prompt Composer。
- Cancel。
- SQLite 持久化。
- 崩溃后 Interrupted 修复。
- 三平台构建和打包。

### P1

- Claude Code Adapter。
- Approval UI。
- Harness 设置。
- 诊断导出。
- 虚拟化和流式更新性能优化。

### P2

- 更完整的 Tool Call 展示。
- 快捷键。
- 搜索历史任务。
- 多窗口。

P2 不阻塞 v0.1.0 Alpha，除非实际用户测试证明其属于核心闭环。

## 21. 风险与应对

| 风险 | 影响 | 应对 |
| --- | --- | --- |
| GPUI API 快速变化 | UI 返工 | M0 先 Spike；封装最小边界；固定版本 |
| 不同 Harness 协议差异大 | Adapter 抽象失真 | 先 Fake + Codex，再用第二个 Harness 验证抽象 |
| Harness 输出吞吐过高 | UI 卡顿或内存增长 | 有界缓冲、批量 delta、每帧合并 |
| 取消后残留子进程 | 资源和安全问题 | 统一 ProcessSupervisor、进程组/Job 管理、集成测试 |
| Harness 登录状态难检测 | 首次体验差 | Adapter 提供明确 ProbeResult 和修复建议 |
| 直接操作用户目录 | 误改风险 | 明确 cwd、dirty 警告、不自动删除/还原/提交 |
| JSONL 协议演进 | Desktop/Runner 不兼容 | version 字段、Hello 握手、未知事件容错 |
| 原始日志泄露信息 | 安全风险 | 默认脱敏、限制日志、显式诊断导出 |

## 22. 可观测性

MVP 只做本地可观测性：

- 结构化本地日志。
- 每个 Task、Run、Command 带关联 ID。
- 记录启动耗时、首个事件耗时、事件吞吐、取消耗时和异常退出码。
- 默认不采集远程遥测。
- 日志轮转并设置大小上限。
- 用户可以查看并导出脱敏诊断包。

## 23. 发布策略

- 版本：`0.1.0-alpha.N`。
- 目标覆盖 Linux、macOS 和 Windows；各平台发布包须通过本机验收后发布。
- 不承诺数据库和协议跨 Alpha 永久兼容，但迁移必须显式。
- 每个发布包必须包含 Desktop 和完全匹配的 Runner。
- 发布前运行：format、lint、unit、integration、release build、手工 Harness 矩阵。
- 不自动更新；MVP 通过手工下载安装新版本。

## 24. 后续演进边界

以下只保留架构接缝，不在 MVP 实现：

### Phase 2：本地开发工作流

- 完整 Worktree Manager。
- Git 状态、Diff、Commit 和 PR 前检查。
- 内置终端。
- 快速启动 VS Code。
- Agent Handoff 和跨 Harness 接力。

### Phase 3：平台集成

- GitHub、GitLab、CNB Connector。
- Issue、PR、Review、Merge。
- TAPD/Jira Project Connector。
- 从外部任务生成 Prompt 和本地 Task。

### Phase 4：Remote 与服务器部署

- Runner 后台服务化。
- Web Remote Client（首个本机 TCP + Token 版本已实现）。
- Control Plane、设备配对和权限 Scope。
- Runner 主动出站连接。
- 多环境、断线重连和事件游标重放。
- Headless 服务器部署和任务调度。

进入任一后续阶段前，必须重新定义范围、威胁模型、数据保留和验收标准。

## 25. Definition of Done

v0.1.0 Alpha 只有在以下条件全部满足时才算完成：

- [ ] GPUI 已完成正式 Go 决策。
- [ ] 用户可以选择本地项目。
- [ ] 可以探测 Codex 和 Claude Code。
- [ ] 两个 Harness 都可以完成最小 Prompt → Stream → Exit 闭环。
- [ ] 支持取消，且不残留子进程。
- [ ] 一个 Task 不会启动两个并发写 Run。
- [ ] 历史 Task、Run 和最终 Message 可以在重启后恢复。
- [ ] 异常退出的 Run 会变为 Interrupted。
- [ ] 未安装、未登录和异常退出具有明确错误提示。
- [ ] 日志默认脱敏且大小有上限。
- [ ] 关键状态机、协议和进程生命周期测试通过。
- [ ] Release 性能基准已记录。
- [ ] Linux、macOS 和 Windows Release 包可以在各自干净环境安装和运行。
- [ ] 未实现 UDP Remote、Control Plane、Connector、自动 Worktree 等非 MVP 功能。
- [ ] 仓库中没有调试代码、备用实现、无用依赖或未说明的生成文件。

## 26. 开工前待确认事项

以下问题必须在对应里程碑开始前确定，但不阻塞本文档作为 MVP 基线：

1. Codex 参考适配器采用的官方非交互协议模式。
2. Harness 环境变量继承采用白名单还是当前进程环境过滤策略。
3. Linux、macOS 和 Windows 的最低支持版本和 CPU 架构。
4. SQLite 已采用各平台原生数据目录，见 README；日志目录和保留策略仍需确定。
5. Alpha 是否需要签名、公证和自动崩溃报告。
