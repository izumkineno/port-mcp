# 19 M3 MockTransport 与任务骨架施工记录

本文档记录 M3 MockTransport 与任务骨架施工结果。M3 的目标是用无真实 I/O 的 MockTransport 和可测试任务状态模型验证 connect/disconnect、任务失败收敛和 force release 后台关闭路径，为 M4 队列/缓冲区和后续真实 TCP/UDP/Serial 接入做准备。

## 施工边界

本轮只执行 M3-01 到 M3-06：transport 统一读写错误边界、MockTransport、任务组与取消信号、Mock connect/disconnect、last_error 与任务失败收敛、force release 后台关闭路径验证。

本轮明确不做：真实 Serial/TCP/UDP I/O、M4 队列、M4 缓冲区、订阅、scan、MCP handler 全工具接入、`mcp-server/` 修改。后台关闭路径仅基于 MockTransport 和无真实 I/O 的任务状态模型验证。

## M3 结论

| 能力域 | 状态 | 证据 |
| --- | --- | --- |
| transport 统一读写边界 | 已完成 | `TransportError` 表达 category/code/message/fatal；测试 `unit_transport_common_maps_mock_errors_without_deciding_response_shape`。 |
| MockTransport | 已完成 | `MockTransport` 支持注入读、观察写、模拟写失败、关闭后返回 `TRANSPORT_CLOSED`；测试 `integration_mock_transport_injects_reads_observes_writes_and_failures`。 |
| 任务组与取消信号 | 已完成 | `TaskGroup`、`TaskGroupState`、`TaskExit` 表达 Running/Cancelling/Finished 和 clean/failed exit；测试 `unit_tasks_create_cancel_and_report_mock_task_state`。 |
| Mock connect/disconnect | 已完成 | `RuntimeRegistry::connect_mock` 和 `disconnect_mock` 完成 Configured -> Connected -> Disconnected，无真实 I/O；测试 `integration_mock_lifecycle_connects_disconnects_and_releases_without_real_io`。 |
| last_error 与任务失败收敛 | 已完成 | mock task failure 进入 `Error`，`instance_query` 可见 `last_error`；测试 `integration_mock_task_error_records_last_error_and_enters_error_state`。 |
| force release 后台关闭路径 | 已完成 | force release 后句柄失效，资源锁保持 `Closing`，mock close 完成后释放 tombstone；测试 `integration_force_release_keeps_closing_lock_until_mock_close_completes`。 |

## 任务对应关系

| 任务 | 本轮状态 | 说明 |
| --- | --- | --- |
| M3-01 transport 统一读写边界 | Done | `TransportError` 已覆盖 timeout/closed/fatal 语义，不决定 MCP 返回外形。 |
| M3-02 MockTransport | Done | 支持注入读、观察写、模拟写失败和 close。 |
| M3-03 任务组与取消信号 | Done | 无真实后台任务，仅用可测试状态模型表达取消和退出。 |
| M3-04 Mock connect/disconnect | Done | Mock 路径完成 create -> config -> connect -> disconnect -> release。 |
| M3-05 last_error 与任务失败收敛 | Done | mock task failure 收敛到 `Error` 并写入 `LastErrorSummary`。 |
| M3-06 force release 后台关闭路径 | Done | closing tombstone 阻止复用，mock close 完成释放 tombstone。 |

## 测试证据

| 命令 | 结果 |
| --- | --- |
| `cargo test unit_transport_common` | 1 passed。 |
| `cargo test integration_mock_transport` | 1 passed。 |
| `cargo test unit_tasks` | 1 passed。 |
| `cargo test integration_mock` | 3 passed。 |
| `cargo test integration_force_release` | 1 passed。 |
| `cargo check` | 通过，无 warning。 |
| `cargo test` | 22 passed，覆盖 M0 smoke、M1、M2、M3。 |

## 验收备注

- 本轮未修改 `mcp-server/`。
- `rmcp` 仍只应出现在 `main.rs` 和 `src/mcp/*`；M3 的 `runtime` 和 `transport` 不依赖 MCP SDK。
- `transport` 使用阶段性 `#![allow(dead_code)]`，原因是 M7 handler 和 M4/M5 真实调用路径接入前，MockTransport 只由测试驱动。
- 进入 M4 前，应继续保持真实 TCP/UDP/Serial I/O 禁入，先在 Mock 路径上实现队列、rx buffer、订阅、预算和限速。
