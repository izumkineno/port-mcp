# M9 架构文件拆分与测试收敛验收记录

## 范围

M9 只执行结构性拆分和测试迁移/收敛，不新增进阶工具，不修改工具契约、状态机、错误码、并发语义或依赖策略，不修改 `mcp-server/`。

## 拆分结果

- `src/mcp/`：新增 `response.rs`，MCP response wrapping 与工具日志事件 helper 从 `tools.rs` 拆出。
- `src/app/`：新增 `instance_service.rs`、`config_service.rs`、`port_service.rs`、`stream_service.rs`，`mod.rs` 保留模块入口和 re-export。
- `src/model/`：新增 `config.rs`、`state.rs`、`error.rs`、`data.rs`、`ids.rs`、`limits.rs`、`redaction.rs`、`response.rs`，`mod.rs` 保留 re-export 和模型回归测试。
- `src/runtime/`：新增 `locks.rs`、`queues.rs`、`buffers.rs`、`subscriptions.rs`、`tasks.rs`，`mod.rs` 保留 `RuntimeRegistry` 行为入口和回归测试。
- `src/transport/`：新增 `common.rs`、`mock.rs`、`tcp.rs`、`udp.rs`、`serial.rs`，`mod.rs` 保留 re-export 和无硬件/loopback 回归测试。
- `src/util/`：未迁移内容；当前没有真正无状态且跨模块复用到需要独立 util 的 helper，避免制造空壳依赖。

## 子任务证据

- M9-01：`cargo fmt --check`、`cargo check`、`cargo test m7_`、`cargo test`、SDK boundary grep 通过。
- M9-02：`cargo fmt --check`、`cargo check`、`cargo test unit_app_instances`、`cargo test unit_port_service`、`cargo test` 通过。
- M9-03：`cargo fmt --check`、`cargo check`、`cargo test unit_response_shape`、`cargo test unit_redaction`、`cargo test` 通过。
- M9-04：`cargo fmt --check`、`cargo check`、`cargo test unit_registry`、`cargo test unit_queues`、`cargo test unit_buffers`、`cargo test` 通过。
- M9-05：`cargo fmt --check`、`cargo check`、`cargo test integration_mock_transport`、`cargo test integration_tcp`、`cargo test integration_udp`、`cargo test unit_serial`、`cargo test` 通过。
- M9-06：`cargo fmt --check`、`cargo check`、`cargo test`、SDK boundary grep、model/app/runtime transport-boundary grep 通过。

## 总验收命令

- `cargo fmt --check`：通过，无输出。
- `cargo check`：通过，`Finished dev profile`。
- `cargo test`：通过，41 passed，0 failed。
- SDK Boundary Gate grep：`rmcp`、`RequestContext`、`CallToolResult`、`ServerHandler`、`tool_router`、`tool_handler`、`schemars` 仅出现在 `src/main.rs` 与 `src/mcp/*`。
- model/app/runtime transport-boundary grep：对 `src/{model,app,runtime}/**` 搜索 `serialport::|TcpStream|TcpListener|UdpSocket|tokio::net|rmcp` 无匹配。
- `git status --short -- src docs mcp-server .omg`：列出 `src/` 和 `docs/` 改动；无 `mcp-server/` 改动项。

## Gate 结论

- Refactor Verification plan：通过；每个子任务均在完成声明前取得命令证据并更新任务板。
- SDK Boundary Gate：通过；SDK 类型未扩散到 `app`、`runtime`、`transport` 或 `model`。
- No-Hardware CI Gate：通过；自动化仅使用 mock、loopback 和脚本化串口 worker/配置/错误映射，不依赖真实串口硬件。
- TypeScript MCP server boundary：通过；未修改 `mcp-server/`。

## 剩余限制

- M9 未改变任何工具可见行为，也未新增进阶工具。
- `src/runtime/mod.rs` 仍保留 `RuntimeRegistry` 主实现和回归测试入口；后续若继续拆细 registry 方法，应作为新的结构性阶段并单独验证。
- `src/util/mod.rs` 仍为空，因为当前没有足够稳定的跨模块 helper 值得迁移。
