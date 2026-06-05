# 23 M7 MCP server 接入施工记录

本文记录 M7 MCP server 接入阶段的施工结果。M7 只落地 Rust 原生 MCP stdio server 正式入口、初版工具注册、handler 到 app service 的映射、request_id 与日志/session 诊断、以及 MCP 端到端 smoke；不进入 M8 发布前验收或进阶工具。

## 范围

| 项目 | 结果 |
| --- | --- |
| MCP server 入口 | `main.rs` 仍保持薄入口，`mcp::server::run_stdio_server` 启动 `PortMcpServer`。 |
| 工具注册 | 注册 03 中全部初版工具，不再注册 M0 临时 `m0_smoke`。 |
| handler 映射 | handler 只解析参数、提取 request context/session 诊断、调用 app service、包装统一返回。 |
| request_id | 每次工具调用由 MCP 层生成统一 `request_id` 并进入返回 JSON。 |
| 未进入能力 | M8 发布前验收、进阶工具、HTTP server、TypeScript `mcp-server/`。 |
| 仓库边界 | 未修改 `mcp-server/`。 |

## 已完成任务

| 任务 | 状态 | 完成证据 |
| --- | --- | --- |
| M7-01 固化 MCP server 启动入口 | Done | `mcp::server::run_stdio_server` 启动 `PortMcpServer::new()`；`cargo check` 通过。 |
| M7-02 注册全部初版工具 | Done | `m7_tool_list_registers_initial_contract_tools` 验证 15 个初版工具全部注册，且 `m0_smoke` 不再出现。 |
| M7-03 handler 到 app service 映射 | Done | `PortMcpServer` handler 映射 instance/config/port/subscribe 工具到 `InstanceService` 和 `PortService`；`m7_e2e_smoke_covers_instance_config_port_and_release_tools` 通过。 |
| M7-04 request_id 与日志 span | Done | MCP 层生成 `request_id`，返回中可观察；`m7_instance_handler_returns_unified_response_with_request_id` 与 `m7_request_context_is_reflected_in_subscription_response` 通过。 |
| M7-05 MCP 端到端 smoke | Done | rmcp duplex client/server smoke 覆盖 create/config/connect/send/pull/disconnect/release 协议路径。 |

## 代码落点

| 文件 | 内容 |
| --- | --- |
| `src/mcp/server.rs` | stdio server 改为启动正式 `PortMcpServer`。 |
| `src/mcp/tools.rs` | `PortMcpServer`、工具参数结构、15 个初版工具 handler、统一 response 包装和 M7 MCP smoke 测试。 |
| `src/mcp/session.rs` | `SessionMode::RequestContextDebug` 作为当前 request context 诊断标记。 |
| `src/app/mod.rs` | 补齐 MCP handler 需要的 config、mock port 生命周期、send/pull/clear、subscribe/unsubscribe 薄服务方法。 |
| `src/model/mod.rs` | 增加 `HandleId::from_string`，用于 MCP 请求中恢复显式句柄。 |

M7 仍保持 SDK Boundary Gate：`rmcp`、`RequestContext`、`CallToolResult`、`tool_router` 等 SDK 类型只出现在 `src/main.rs` 和 `src/mcp/*`。`app`、`runtime`、`transport`、`model` 不依赖 MCP SDK。

## 验证记录

| 命令或检查 | 结果 |
| --- | --- |
| `cargo test m7_` | 4 passed。 |
| `cargo fmt` | 通过，无输出。 |
| `cargo check` | 通过。 |
| `cargo test` | 40 passed。 |
| SDK 边界搜索 `rmcp|RequestContext|CallToolResult|ServerHandler|tool_router|tool_handler|schemars` | 仅出现在 `src/main.rs` 和 `src/mcp/*`。 |
| `git status --short -- src docs mcp-server` | 修改范围限定在 `src/` 与 `docs/`；`mcp-server/` 无变更。 |

## 已知限制

- M7 的 `port_connect/send/pull/disconnect` MCP smoke 使用已完成的 Mock runtime 生命周期证明协议路径，不把真实 TCP/UDP/Serial runtime task 接入扩大为 M8 发布验收。
- `port_scan` 当前通过 `PortService::scan_loopback` 暴露 loopback-only scan；非 loopback、CIDR、DNS/hostname、通配地址仍按 M5 规则拒绝。
- request context 目前以 `request_context_debug` 诊断标记进入订阅返回；稳定多客户端 session 策略和更细日志样例可在 M8 验收收敛中继续核对。
- `mcp::response` 仍折叠在 `mcp::tools` 的 response helper 中，未拆独立文件；当前边界和测试已覆盖统一返回外形。
