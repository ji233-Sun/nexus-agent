# Harness 接入

Nexus 使用本机 CLI 及其原生账户登录，不复制 CLI 凭据。新增适配的启动命令可在设置中覆盖；安装和升级通过官方文档完成。目前使用 CLI 默认模型，或在 Provider Profile 指定完整模型 ID；不将静态模型名称伪装为账户模型目录。

| Harness | 默认命令 | 传输协议 | 权限与运行中输入 |
| --- | --- | --- | --- |
| Pi | `pi` | RPC JSONL | 内置 `tool_call` 扩展执行 Ask / AutoEdit / Yolo；支持 steer |
| Kimi Code | `kimi` | Wire JSON-RPC | 转发原生审批，AutoEdit 自动批准 WriteFile / StrReplaceFile，Yolo 使用原生开关；支持 steer |
| Qoder | `qodercli` | SDK stream-json | initialize 完成后发送输入，原生 stdio 审批；运行中输入按 Nexus 队列处理 |
| CodeBuddy | `codebuddy` | SDK stream-json | 原生 stdio 审批；通过 replay-user-messages 确认 steer 输入 |

Pi 需要 **0.85.1 或更高版本**（官方包 `@earendil-works/pi-coding-agent`）。Nexus 等待 `agent_settled`，而非可能继续自动重试或压缩的 `agent_end`；会话恢复保存完整 sessionFile。权限扩展位于临时目录，随 decoder 生命周期清理，不写入用户项目。未知工具在 Ask / AutoEdit 下必须确认；无 UI、取消或拒绝均阻止调用。

新增 CLI 未提供统一的无凭据身份探测接口，因此设置明确显示“身份状态未探测”。安装可执行文件可用时允许发起任务，认证错误由 CLI 原样报告，`authenticated` 不会因安装成功而变为 true。没有安装 CLI 的环境只能验证协议及 fake-process 集成，不能证明账户登录和真实模型调用成功。

标题生成禁用工具：Pi 关闭工具、扩展与技能；Kimi 使用临时无工具 agent；Qoder 和 CodeBuddy 关闭内置工具并设置空的严格 MCP 配置。Kimi 标题解析 print 模式的 `role/content`，任务解析 Wire 事件。Qoder 标题使用官方 headless text 输入，无需 SDK 握手。

ZCode 尚未接入，路线图 #65 仍保持打开。当前未确定可支持的官方 CLI / Desktop runtime 接口，不将同名非官方 npm 包视为官方实现。

## 协议依据

- [Pi RPC（官方）](https://github.com/badlogic/pi-mono/blob/main/packages/coding-agent/docs/rpc.md)，并核对 0.85.1 npm 发行包 `dist/modes/rpc/rpc-mode.js`。
- [Pi tool_call 扩展示例](https://github.com/badlogic/pi-mono/blob/main/packages/coding-agent/examples/extensions/permission-gate.ts)。
- [Kimi Wire](https://github.com/MoonshotAI/kimi-cli/blob/main/docs/en/customization/wire-mode.md)；[Kimi print](https://github.com/MoonshotAI/kimi-cli/blob/main/docs/en/customization/print-mode.md)。
- [Qoder headless](https://docs.qoder.com/cli/run-in-scripts)、[SDK Reference](https://docs.qoder.com/cli/sdk/references-typescript)，核对官方 `@qoder-ai/qoder-agent-sdk` 1.0.37 的 stdio 参数与控制帧。
- [CodeBuddy CLI Reference](https://www.codebuddy.ai/docs/cli/cli-reference)，并核对本机 2.118.2 CLI `--help` 的 stream-json、权限、模型、effort 与 replay-user-messages 参数，以及官方发行包 `SdkPermissionClient.handleResponse` 的 `allowed/reason` 审批字段（与 Claude / Qoder 不同）。

Desktop / Runner 协议提升为 14，避免旧外置 Runner 接收无法识别的新 Harness 枚举。
