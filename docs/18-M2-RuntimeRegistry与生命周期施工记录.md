# 18 M2 RuntimeRegistry 与生命周期施工记录

本文档记录 M2 RuntimeRegistry 与实例生命周期基线施工结果。当前仓库原有 `18-TypeScript-MCP-server-边界说明.md` 保留不动；本文按本轮施工命令创建，用于承接 `17` 中 M2 的阶段证据。

## 施工边界

本轮只实现无 I/O 的 RuntimeRegistry、实例记录、会话绑定、配置保存、状态前置检查、基础 create/configure/release 语义、错误映射和 M2 单元测试。

按本轮 gate 明确不做：真实 Serial/TCP/UDP I/O、资源锁、closing tombstone、队列、缓冲区、订阅、scan、后台 transport 任务、M3/M4 能力和 `mcp-server/` 修改。因此 `17` 中涉及资源锁的 M2-05、依赖资源锁/后台关闭的 M2-06 未在本轮完成。

## M2 结论

| 能力域 | 状态 | 证据 |
| --- | --- | --- |
| RuntimeRegistry 基础表 | 已完成 | `RuntimeRegistry` 支持 create/list/query/release，测试 `unit_registry_creates_lists_queries_and_releases_without_io`。 |
| 实例记录 | 已完成 | `RuntimeInstance` 保存 `handle_id`、`instance_type`、`state`、`config`、`stats`，通过 `InstanceSummary` 输出。 |
| handle_id 分配与查找 | 已完成 | `IdGenerator` 生成 `h_ser_001`、`h_tcp_001`、`h_udp_001`，查询 released handle 返回 `HANDLE_RELEASED`。 |
| 会话默认实例绑定 | 已完成 | `use_instance`、`resolve_handle` 支持稳定 session；无 session 返回 `SESSION_ID_UNAVAILABLE`，无绑定返回 `SESSION_BINDING_MISSING`。 |
| 配置保存与原子覆盖 | 已完成 | serial/tcp/udp 配置保存为 `ConfigSnapshot`；类型错误或校验失败保留旧配置和旧状态。 |
| 状态前置检查 | 已完成 | 配置只允许 `Created`、`Configured`、`Disconnected`；Connected 普通 release 返回 `CONNECTED_RELEASE_REQUIRES_FORCE`。 |
| 基础 create/configure/release 语义 | 已完成 | create 不打开底层资源；configure 不检查真实占用；release 移出实例表并清除会话绑定。 |
| 错误映射 | 已完成 | 覆盖 `HANDLE_NOT_FOUND`、`HANDLE_RELEASED`、`SESSION_BINDING_MISSING`、`SESSION_ID_UNAVAILABLE`、`STATE_NOT_ALLOWED`、`CONNECTED_RELEASE_REQUIRES_FORCE`、`TYPE_MISMATCH`。 |
| app 层实例服务 | 已完成 | `InstanceService` 薄封装 registry，测试 `unit_app_instances_delegates_to_registry_and_maps_errors`。 |
| 资源锁表 | 未执行 | 本轮命令明确禁止资源锁；不实现 M2-05。 |
| force release closing tombstone | 未执行 | 依赖资源锁和后台关闭，本轮命令明确禁止资源锁和后台 transport 任务；不实现 M2-06 的 tombstone 语义。 |

## 任务对应关系

| 任务 | 本轮状态 | 说明 |
| --- | --- | --- |
| M2-01 RuntimeRegistry 基础表 | Done | 无 I/O 实例表已实现并测试。 |
| M2-02 会话默认实例绑定 | Done | 稳定 session 绑定和缺省解析错误已实现并测试。 |
| M2-03 配置写入与原子覆盖 | Done | 类型匹配、地址校验和旧配置保留已测试。 |
| M2-04 状态前置检查 | Done | 配置状态矩阵和普通 release 拒绝已测试。 |
| M2-05 资源锁表 | Blocked | 与本轮“不得实现资源锁”约束冲突，未施工。 |
| M2-06 release 与 force release 语义 | Partial | 无资源锁部分已实现：普通 release、force 移出实例表、清理 session binding；closing tombstone 未施工。 |
| M2-07 app 层实例服务 | Done | 无资源锁路径的 instance create/list/query/use/release/configure 已实现并测试。 |
| M2-08 M2 单元测试 | Done | 本轮新增 5 个 M2 单元测试。 |
| M2-09 M2 验证 | Done | `cargo check`、`cargo test` 均通过。 |
| M2-10 回填 M2 施工记录 | Done | 本文档记录本轮完成项和受约束未执行项。 |

## 测试证据

| 命令 | 结果 |
| --- | --- |
| `cargo test unit_` | 13 passed，覆盖 M1 与 M2 单元测试。 |
| `cargo check` | 通过，无 warning。 |
| `cargo test` | 14 passed，覆盖 M0 smoke、M1 单元测试、M2 单元测试。 |

## 验收备注

- 本轮未修改 `mcp-server/`。
- `rmcp` 仍只应出现在 `main.rs` 和 `src/mcp/*`，RuntimeRegistry 不依赖 MCP SDK。
- `app` 和 `runtime` 使用阶段性 `#![allow(dead_code)]`，原因是 M7 handler 接入前生产入口不会直接调用这些 M2 基线 API；这避免为了消除 warning 提前越界接线。
- 进入下一步前，应先决定是否放宽“不得实现资源锁”的限制；否则不能完成 M2-05，也不能完整完成 M2-06 或进入 M3 gate。
