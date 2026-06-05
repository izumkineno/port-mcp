# 14 M0 SDK 与依赖复核记录

本文档承接 [09-库选择与依赖决策](09-库选择与依赖决策.md)、[10-开发里程碑](10-开发里程碑.md)、[11-配置与运行说明](11-配置与运行说明.md) 与 [13-代码施工大纲](13-代码施工大纲.md)，记录 M0 阶段对官方 Rust MCP SDK 与初版关键依赖的复核结论。

本文档是施工记录，不重新定义 [03-初版工具契约](03-初版工具契约.md)、[05-状态机与资源生命周期](05-状态机与资源生命周期.md)、[06-并发与缓冲区设计](06-并发与缓冲区设计.md) 或 [07-错误模型与返回格式](07-错误模型与返回格式.md) 已经确定的行为。后续编码发现依赖能力与本文结论冲突时，应先回写本文和 [09-库选择与依赖决策](09-库选择与依赖决策.md)，再继续调整实现。

## 复核结论

M0 SDK gate 初步判定为通过：初版 MCP 协议层采用官方 Rust SDK `rmcp`，并将 SDK 类型严格封装在 `mcp` 层。

| 项目 | 结论 | 说明 |
| --- | --- | --- |
| SDK crate | 采用 `rmcp` | `rmcp` 是 `modelcontextprotocol/rust-sdk` 仓库提供的官方 Rust Model Context Protocol SDK。 |
| 初步版本 | `1.7.0` | crates.io 当前可见版本为 `rmcp v1.7.0`。实际写入 `Cargo.toml` 前仍应以 `cargo add rmcp --registry crates-io` 或等价命令复核。 |
| 许可证 | Apache-2.0 | GitHub 仓库与 crates.io 页面均显示 Apache-2.0。 |
| stdio server | 支持 | `rmcp::transport::stdio` 与 `ServiceExt::serve(stdio())` 可支撑初版 stdio MCP server。 |
| 工具注册 | 支持 | `#[tool]`、`#[tool_router]`、`#[tool_handler]` 等宏可支撑工具注册。 |
| 工具 schema | 支持 | `schemars` feature 或 re-export 可生成工具参数 JSON Schema。 |
| 请求上下文 | 支持 | handler 示例使用 `RequestContext<RoleServer>`。 |
| resource notification | 支持 | 支持 `notify_resource_updated`、`notify_resource_list_changed` 等资源通知。 |
| resource subscription | 支持 | 支持 `subscribe`、`unsubscribe` 以及资源更新通知。 |
| error response | 支持 | 提供 `ErrorData`、`RmcpError` 等错误类型，可由 `mcp::response` 统一适配。 |
| session/context | 部分待验证 | SDK 提供请求上下文，但是否能稳定区分 AI 会话并满足 `instance_use`、默认句柄解析和订阅隔离，必须在 M0 spike 中实测。 |

## 证据来源

| 来源 | 观察结果 | 对施工的影响 |
| --- | --- | --- |
| `https://github.com/modelcontextprotocol/rust-sdk` | 仓库描述为官方 Rust SDK，包含 `crates/rmcp` 与 `crates/rmcp-macros`。 | 满足 09 对“官方 Rust MCP SDK”的来源要求。 |
| `https://crates.io/crates/rmcp` | crates.io 显示 `rmcp v1.7.0`，包说明为 Rust SDK for Model Context Protocol。 | 可作为 M0 初步版本候选。 |
| `https://docs.rs/rmcp/latest/rmcp/` | feature flags 包含 `server`、`macros`、`schemars`、`transport-io` 等；文档列出 stdio transport、tool macros、resource notification、subscription。 | 支撑 M0 薄 server spike 和 M7 MCP server 接入路径。 |
| 本地 `cargo search rmcp --limit 5` | 当前环境返回 `crates-io is replaced with non-remote-registry source registry rsproxy-sparse`。 | 本机 registry 配置会影响依赖复核命令；M0 施工时应显式使用 `--registry crates-io` 或确认镜像可用。 |

## 推荐 Cargo 入口

M0 初始依赖应保持最小 feature，先满足 stdio server、工具注册、参数 schema、Tokio runtime 和统一返回需要。建议从以下方向开始，实际版本与 feature 以 M0 spike 编译结果为准。

```toml
[dependencies]
rmcp = { version = "1.7.0", features = ["server", "macros", "schemars", "transport-io"] }
tokio = { version = "1", features = ["macros", "rt-multi-thread", "sync", "time", "net", "io-util"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "2"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["fmt", "env-filter"] }
bytes = "1"
serialport = "4"
time = { version = "0.3", features = ["formatting", "parsing", "macros"] }
```

M0 不应引入 HTTP transport、OAuth、VISA、协议 helper、数据库、Web 框架或重型 actor 框架。若 `rmcp` 默认 feature 过宽，应在 `Cargo.toml` 中显式关闭默认 feature 并逐项开启需要的 feature。

## SDK 能力矩阵

| M0/M1 关注能力 | 初步结论 | 施工要求 |
| --- | --- | --- |
| stdio server | 可用 | `mcp::server` 只封装 `rmcp` 的 stdio server 创建、工具注册和运行等待。 |
| tool schema | 可用 | `mcp::tools` 可以使用 SDK macro/schema 能力，但进入 `app` 前必须转换成本项目强类型参数。 |
| typed parameters | 可用 | 参数结构可使用 `serde` 与 `schemars`，但校验错误必须映射为 07 的 `InvalidArgument`。 |
| request context | 可用 | 可传入 `mcp::session` 做会话提取；上下文类型不得进入 `app`、`runtime`、`transport`。 |
| stable session id | 待实测 | M0 spike 必须确认是否存在稳定 session 标识；若没有，M1 实现 `session_mode=unavailable`。 |
| resource updated notification | 可用 | 可用于实现 `port_subscribe_stream` 的最小通知适配；具体队列、限速和订阅隔离仍由 `runtime` 实现。 |
| resource subscribe/unsubscribe | 可用 | 可对接 MCP 层订阅入口，但不得绕过 `StreamService` 与 `runtime::subscription`。 |
| logging notification | 可用 | 可选用于 MCP 客户端日志通知；结构化内部日志仍以 `tracing` 为主。 |
| error data | 可用 | SDK 错误只作为协议外壳；工具业务错误由 `PortMcpError` 转换。 |
| task support | 可用但初版不依赖 | 初版已有 runtime 后台任务模型，不把 SDK task support 作为 M0 必需能力。 |

## Session 模式判定

M0 spike 必须特别验证会话身份，因为它直接影响 [03-初版工具契约](03-初版工具契约.md) 中的默认句柄解析、`instance_use`、`port_subscribe_stream` 和 `port_unsubscribe_stream`。

| 验证结果 | 处理方式 |
| --- | --- |
| SDK 或客户端上下文能提供稳定会话 ID | `session_mode=auto` 可启用多会话绑定和订阅隔离。 |
| 只能获得连接级上下文，不能稳定区分 AI 会话 | 默认使用 `session_mode=unavailable`；依赖会话身份的工具返回 `InvalidState/SESSION_ID_UNAVAILABLE`。 |
| 本地开发显式启用单会话模式 | 仅允许 stdio 本地开发入口启用 `session_mode=single_dev`，工具返回和日志必须标记 `session_mode=single`。 |
| 远程、多客户端或网络入口尝试启用单会话模式 | 必须启动失败或拒绝会话，不允许只记录 warning。 |

在 session 能力未通过本地 spike 前，不得把默认句柄解析和订阅隔离标记为完成。

## M0 Spike 最小验收

M0 代码施工时，应提交一个最小 `rmcp` spike 或等价复核记录，至少证明以下事项：

- `cargo check` 通过，并生成可复现 `Cargo.lock`。
- `mcp::server` 能创建 stdio server 并阻塞等待客户端关闭。
- 至少注册一个临时 smoke tool，用于验证 tool 注册、参数解析和返回包装；该临时工具不得进入最终初版工具列表。
- 能从 handler 获得 `RequestContext<RoleServer>` 或等价上下文，并记录 session 能力结论。
- 能构造或发送资源更新通知，验证 `port_subscribe_stream` 后续可映射到资源通知机制。
- 能将 SDK 层错误转换为本项目统一失败返回外形的外壳，不让 `rmcp` 错误类型穿透到 `app` 或 `runtime`。
- `mcp` 模块之外不得引用 `rmcp` 类型。

## 初版依赖复核清单

| 依赖 | 初步结论 | M0/M1 处理 |
| --- | --- | --- |
| `rmcp` | 采用 | M0 加入，feature 最小化；SDK 类型只留在 `mcp` 层。 |
| `tokio` | 采用 | M0 加入；M1/M2 后用于 runtime、任务、队列和 TCP/UDP。 |
| `serde` / `serde_json` | 采用 | M1 用于模型、配置、返回、错误 details。 |
| `thiserror` | 采用 | M1 定义 `PortMcpError` 与 transport/domain error。 |
| `tracing` / `tracing-subscriber` | 采用 | M0 初始化日志；M1 后统一 request_id、handle_id、state 和 error code 字段。 |
| `bytes` | 采用 | M3/M4 用于 payload、rx buffer 和发送帧。 |
| `serialport` | 采用 | M6 接入；M0 只锁定依赖，不开始真实串口 I/O。 |
| `time` | 初步采用 | M1 用于 RFC3339 timestamp；若实现复杂度高，可回写 09 后换 `chrono`。 |
| `schemars` | 通过 `rmcp` feature 使用 | 仅用于工具参数 schema；不让 schema 类型进入 runtime。 |
| `anyhow` | 可选 | 只用于 `main`、启动 glue 和测试辅助，不作为领域错误返回。 |

## Gate 判定

| Gate | 状态 | 判定依据 |
| --- | --- | --- |
| 官方 Rust MCP SDK 是否存在 | 通过 | `modelcontextprotocol/rust-sdk` 与 `rmcp` crate 可用。 |
| stdio server 能力 | 通过 | `rmcp::transport::stdio` 与 server 示例可用。 |
| 工具注册能力 | 通过 | `#[tool]`、`#[tool_router]`、`#[tool_handler]` 可用。 |
| 稳定错误响应能力 | 通过 | SDK 提供 `ErrorData`，本项目可在 `mcp::response` 做统一转换。 |
| resource notification 能力 | 通过 | resource updated/list changed notification 可用。 |
| session/context 能力 | 条件通过 | 请求上下文可用；稳定会话 ID 仍需本地 spike 判定。 |
| 进入 M2 | 未通过 | M0 工程骨架和 M1 模型/错误基线尚未完成。 |

结论：可以进入 M0 工程骨架施工；不应跳过 M1 直接进入 RuntimeRegistry、资源锁或真实 I/O。

## 回写规则

出现以下情况时，应先回写本文和相关上游文档，再继续施工：

- `rmcp` 当前稳定版本无法在 Windows 上通过最小 stdio server spike。
- `rmcp` feature 组合无法满足 stdio server、工具注册或 resource notification。
- session/context 无法支撑多会话语义，且 `session_mode=unavailable` 与工具契约存在冲突。
- SDK 错误模型无法被稳定映射到 07 的统一失败返回。
- 需要引入 HTTP transport、OAuth、任务系统或其他非初版能力才能满足基础工具注册。
- `serialport`、`tokio` 或 `tracing` 的实际 Windows 行为与 09、11 的结论冲突。

## 后续动作

- 在 [15-M0-工程骨架施工记录](15-M0-工程骨架施工记录.md) 中记录实际 `Cargo.toml` 依赖、feature、模块骨架和 `cargo check` 结果。
- 在 [16-M1-模型与错误基线施工记录](16-M1-模型与错误基线施工记录.md) 中记录 `session_mode`、统一返回、错误码、ID 和脱敏模型的实现证据。
- 在 [17-初版施工任务拆分清单](17-初版施工任务拆分清单.md) 中把 M0/M1 的 spike、依赖、模块和测试拆成可执行任务。