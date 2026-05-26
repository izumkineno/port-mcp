# 21 M5 TCP/UDP loopback 施工记录

本文记录 M5 TCP/UDP loopback 阶段的施工结果。M5 只落地本机 loopback TCP/UDP 能力、loopback-only `port_scan` 和网络错误映射，不进入 M6 串口、M7 MCP 全工具接入或 M8 发布前验收。

## 范围

| 项目 | 结果 |
| --- | --- |
| 允许的真实 I/O | 仅限 `127.0.0.1`、`127.0.0.0/8` 字面量和 `::1` loopback TCP/UDP。 |
| 禁止目标 | 非 loopback、`0.0.0.0`、`::`、CIDR、通配地址、hostname/DNS 展开目标。 |
| 未进入能力 | serial I/O、MCP handler 全工具接入、发布前验收。 |
| 仓库边界 | 未修改 `mcp-server/`。 |

## 已完成任务

| 任务 | 完成证据 |
| --- | --- |
| M5-01 TCP client transport | `TcpClientTransport::connect/read_chunk/write_all/close`，`integration_tcp_loopback_client_round_trips`。 |
| M5-02 TCP listen 路径 | `TcpListenTransport::bind/local_addr/accept_one/close`，地址冲突与释放复用测试。 |
| M5-03 UDP transport | `UdpTransport::bind/send_to/recv_datagram/close`，datagram 元数据、地址冲突与重绑测试。 |
| M5-04 loopback-only port_scan | `port_scan_loopback` 与 `PortService::scan_loopback`，拒绝 unsafe target、hostname 和过大范围，发现本机开放端口。 |
| M5-05 网络错误映射 | 地址冲突映射到 `TcpListenAddrBusy` / `UdpBindAddrBusy`，scan 限制映射到 `ScanTargetNotAllowed` / `ScanRangeTooLarge`，超时和关闭映射到稳定 `ErrorCode`。 |

## 代码落点

| 文件 | 内容 |
| --- | --- |
| `src/transport/mod.rs` | TCP client/listen、UDP datagram transport、loopback-only scan、M5 transport 集成测试。 |
| `src/app/mod.rs` | `PortService` 的 loopback scan 薄封装和服务层测试。 |

本轮没有拆分 `transport/tcp`、`transport/udp`、`runtime/tasks` 或 `app/port_service` 子文件。当前实现仍在原模块内，因为 M5 代码量较小，拆分暂时不能带来实际复杂度收益；后续接入 runtime 真实任务或 MCP handler 时再按能力拆分。

## 验证记录

| 命令或检查 | 结果 |
| --- | --- |
| `cargo test integration_tcp_loopback` | 通过。 |
| `cargo test integration_tcp_listen` | 通过。 |
| `cargo test integration_udp_loopback` | 通过。 |
| `cargo test integration_port_scan_loopback` | 通过。 |
| `cargo fmt` | 通过，无输出。 |
| `cargo check` | 通过。 |
| `cargo test` | 33 passed。 |
| VS Code Problems / `get_errors` | No errors found。 |
| SDK 边界搜索 `rmcp` | 仅出现在 `src/main.rs` 和 `src/mcp/*`。 |
| `git status --short -- src docs mcp-server` | 修改范围限定在 `src/` 与 `docs/`；`mcp-server/` 无变更。 |

## 已知限制

- M5 的 TCP/UDP transport 还没有接入 `runtime::tasks` 的真实连接生命周期；当前验证粒度是 transport 层和 `PortService` scan 层。
- `port_scan_loopback` 当前按受限范围顺序探测，未实现复杂并发扫描调度；M5 gate 只要求受限 loopback、端口范围和并发参数拒绝策略可验证。
- `instance_query` 尚不能观察 TCP listen strategy 或 UDP datagram 统计；这属于后续真实连接任务和 MCP 工具接入收敛内容。