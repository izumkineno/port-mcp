# M8 初版验收收敛与发布前整理记录

- 日期：2026-05-26
- 平台：Windows
- 阶段：M8 验收收敛与发布前整理
- 结论：初版 M0-M8 验收通过；可作为发布前整理基线。
- 范围：仅执行 M8-01 到 M8-05，不新增进阶工具，不修改 `mcp-server/`。

## M8 任务验收

| 任务 | 状态 | 证据 |
| --- | --- | --- |
| M8-01 运行自动化测试矩阵 | 通过 | `cargo test`：41 passed，覆盖 model、runtime、app、transport、MCP smoke、TCP/UDP loopback、Mock lifecycle、串口无硬件单元测试和 M8 日志字段样例。 |
| M8-02 汇总 Windows 必测结果 | 通过 | Windows 自动化测试已运行；M6 记录确认 `COM4`/`COM5` 互联串口对双向收发通过。 |
| M8-03 检查日志字段与脱敏 | 通过 | `unit_redaction_removes_sensitive_paths_users_env_payload_and_os_text` 覆盖脱敏；`m8_tool_log_event_contains_correlation_state_duration_and_sensitivity_fields` 覆盖 `request_id`、`tool`、`handle_id`、`session`、`state_before/state_after`、`error_code`、`duration_ms`、`sensitive=false`。 |
| M8-04 同步施工记录和已知限制 | 通过 | 00、13、17 与 M6/M7/M8 acceptance 记录已同步；已知限制见下文。 |
| M8-05 发布前构建检查 | 通过 | `cargo fmt --check` 无输出；`cargo check` 通过；`cargo test` 41 passed；MCP M7 smoke 4 passed。 |

## 验证命令

| 命令或检查 | 结果 |
| --- | --- |
| `cargo fmt --check` | 通过，无输出。 |
| `cargo check` | 通过。 |
| `cargo test` | 41 passed。 |
| `cargo test m7_` | 4 passed。 |
| `cargo test m8_tool_log_event` | 1 passed。 |
| `cargo test unit_redaction` | 通过，包含在全量测试矩阵中。 |
| SDK 边界搜索 `rmcp|RequestContext|CallToolResult|ServerHandler|tool_router|tool_handler|schemars` | 仅出现在 `src/main.rs` 和 `src/mcp/*`。 |
| 串口硬件依赖复核 | 默认自动化不依赖真实串口；`serialport` 调用只在 `transport` 封装和测试映射中。 |
| `git status --short -- src docs mcp-server` | 修改范围限定在 Rust `src/` 与 `docs/`；`mcp-server/` 无变更。 |
| VS Code Problems / `get_errors` | No errors found。 |

## Windows 必测汇总

| 项目 | 结果 |
| --- | --- |
| Windows 自动化矩阵 | `cargo test` 41 passed。 |
| 串口枚举与手工验收 | 见 [2026-05-26-M6-serial-windows验收记录](2026-05-26-M6-serial-windows验收记录.md)。 |
| COM4 -> COM5 | 发送 `m6-com4-to-com5`，接收 `m6-com4-to-com5`，15 bytes，passed=true。 |
| COM5 -> COM4 | 发送 `m6-com5-to-com4`，接收 `m6-com5-to-com4`，15 bytes，passed=true。 |
| No-Hardware CI Gate | 默认自动化不需要真实串口硬件，串口真实设备证据只作为 Windows 手工验收记录。 |

## 边界复核

| Gate | 结果 |
| --- | --- |
| SDK Boundary Gate | 通过；MCP SDK 类型只在 `src/main.rs` 和 `src/mcp/*`。 |
| No-Hardware CI Gate | 通过；`cargo test` 不依赖真实串口硬件。 |
| TypeScript MCP server 边界 | 通过；未修改 `mcp-server/`。 |
| 进阶能力边界 | 通过；未新增 VISA、协议 helper、HTTP server、非 loopback scan allowlist 或高级流式订阅。 |

## 已知限制

- MCP `port_connect/send/pull/disconnect` smoke 使用 Mock runtime 生命周期验证协议路径；真实 TCP/UDP/Serial runtime task 的全链路连接与 I/O 仍是后续增强方向。
- `port_scan` 仍只允许 loopback 字面量目标；非 loopback、CIDR、DNS/hostname 和通配地址不进入初版。
- M6 串口 `SerialWorker` 已完成底层封装和 COM4/COM5 手工验收，但 MCP 工具端真实串口 connect/send/pull 仍未作为发布前硬门槛。
- `mcp::response` 当前折叠在 `mcp::tools` 的 response helper 中，未拆独立文件；统一返回外形已有测试覆盖。
- `request_context_debug` 是当前 session 诊断标记；更严格的多客户端 session 策略可在后续版本收敛。
