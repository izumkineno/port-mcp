# 26 TCP/UDP 真实运行时契约与测试计划

本文档承接 [21-M5 TCP/UDP loopback 施工记录](21-M5-TCP-UDP-loopback施工记录.md)、[23-M7 MCP server 接入施工记录](23-M7-MCP-server接入施工记录.md)、[24-MCP 工具列表与调用收发详解](24-MCP工具列表与调用收发详解.md) 与 [25-进阶能力施工拆分与验收门槛](25-进阶能力施工拆分与验收门槛.md)，专门收敛 TCP/UDP 从 Mock runtime 进入真实 app/MCP 运行时之前必须固定的契约、架构边界和测试门槛。

本文档不直接修改初版 MCP 工具名称和 JSON 入参，不引入新的网络扫描能力，也不把 TCP/UDP 扩展成泛化网络调试平台。它只解决一个当前已暴露的问题：`port_connect`、`port_send`、`port_pull` 对 TCP/UDP 返回成功时，必须能区分真实 socket I/O 成功与 Mock 队列成功，不能再让 Mock 路径冒充真实网络收发。

## 背景与当前结论

当前仓库已经具备两类能力，但它们尚未在 app/MCP 层闭合：

| 能力 | 当前状态 | 证据 |
| --- | --- | --- |
| TCP/UDP transport | 已有真实 loopback transport 实现与测试 | `TcpClientTransport`、`TcpListenTransport`、`UdpTransport` 以及 M5 transport 测试。 |
| TCP/UDP MCP 工具链 | 已有工具注册、参数解析、状态返回和 smoke | M7 MCP smoke 覆盖 create/config/connect/send/pull/disconnect/release 协议路径。 |
| TCP/UDP app/runtime 真实 I/O | 尚未接入 | 非 Serial 的 `connect/send/pull` 当前仍进入 Mock runtime 路径。 |

因此，后续施工的第一目标不是新增 transport API，而是把现有 TCP/UDP transport 以可取消、可统计、可验收的方式接入实例生命周期、资源锁、接收缓冲区、发送语义和 MCP 返回模型。

## 设计结论

- TCP/UDP 真实 I/O 必须进入既有 `instance_create -> config -> port_connect -> port_send -> port_pull -> port_disconnect/release` 主链路，不允许新增平行收发通道绕开状态机。
- Mock runtime 只能用于 runtime 单元测试、mock transport 测试或明确的测试辅助，不得作为 TCP/UDP 正常配置下的成功收发依据。
- MCP 工具的外层返回结构应保持兼容，但 `queued`、统计字段和错误边界必须真实反映底层 I/O 行为。
- 真实网络 I/O 是 async 行为，施工时不得在全局 `InstanceService` 锁内执行网络 await 或长时间阻塞。
- `RuntimeRegistry` 继续作为实例状态、资源锁、rx buffer、stats、订阅和容量账本的权威来源；真实 socket 生命周期由每实例 worker 或 task 控制面承载。
- 第一轮实现应优先收敛 TCP client；TCP listen 和 UDP 必须先固定语义与测试，再进入实现。

## 适用范围与非目标

| 范围 | 是否覆盖 | 说明 |
| --- | --- | --- |
| TCP client 真实连接、发送、读取、断开 | 覆盖 | 第一优先级，用于证明 `port_send` 可到达真实 loopback peer。 |
| TCP listen 单客户端语义 | 覆盖契约和测试计划 | 实现可排在 TCP client 之后，初版保持单客户端。 |
| UDP bind、remote send、datagram receive | 覆盖契约和测试计划 | 初版先定义 remote 必填发送语义和 datagram 接收边界。 |
| MCP/app 层 e2e 测试 | 覆盖 | 必须用真实 loopback peer 验证，而不是只断言状态返回。 |
| 多客户端 TCP listen | 不在本轮实现 | 属于 25 中 TCP/UDP 传输增强能力，可后续扩展。 |
| UDP 多来源管理和 datagram 元数据公开契约 | 不在本轮实现 | 本轮可保留内部 peer 信息，但不扩展 `port_pull` 外形。 |
| 非 loopback 网络访问和 DNS/hostname 解析 | 不覆盖 | 继续遵守 M5 的 loopback-only 边界。 |
| 泛化端口扫描、CIDR 探测、网络拓扑发现 | 不覆盖 | 不改变 port-mcp 的最小暴露原则。 |

## 统一运行时原则

### 1. 状态机原则

- `port_connect` 只允许从 `Configured` 或 `Disconnected` 进入 `Connected`。
- 真实 transport 打开失败时，实例不得进入 `Connected`，不得留下 held resource lock 或半初始化 worker。
- `port_disconnect` 对已连接实例必须停止 worker、关闭 socket、释放相关资源锁，并把实例收敛到 `Disconnected`。
- `instance_release(force=false)` 对 `Connected` 实例仍应保持当前拒绝语义；`force=true` 必须能停止 worker 并进入可回收清理路径。

### 2. Async 边界原则

- 不允许在 `std::sync::Mutex<InstanceService>` 持有期间执行 TCP/UDP connect/read/write await。
- 推荐用每实例 network worker 或 task control plane 承载 async socket，并通过 channel 或短锁回写 `RuntimeRegistry`。
- 如改造 MCP/app 边界为 async-aware，必须确保只在短锁内读取配置和提交状态，不在锁内等待外部 I/O。

### 3. 发送语义原则

真实 TCP/UDP 发送必须显式选择一种语义，并在测试中固定：

| 语义 | `queued` | 适用场景 | 要求 |
| --- | --- | --- | --- |
| 直接写入 | `false` | TCP client 首轮最小实现 | `port_send` 返回前底层 write 已完成或失败。 |
| worker 队列 | `true` | 后续统一后台发送队列 | 返回只表示进入 worker 队列，必须另有错误回填和 stats 更新机制。 |

第一轮 TCP client 推荐使用直接写入或同步等待 worker ack，避免再次出现“已入队但未真正写出”的假成功。如果采用 worker 队列，必须在文档和返回中明确 `queued=true` 不是 peer 已收到。

### 4. 接收语义原则

- `port_pull` 必须从真实 socket 或由真实 reader task 写入的 rx buffer 读取。
- TCP `read_chunk` 可保持字节流语义，不承诺消息边界。
- UDP `recv_datagram` 内部保留 datagram 边界和 peer，但本轮 `port_pull` 可先返回 payload bytes；peer 元数据后续再扩展专门契约。
- timeout 行为必须统一：要么返回结构化 `READ_TIMEOUT`，要么沿用当前 MCP 友好的空 payload 成功返回；不能 TCP、UDP、Mock 三套行为互相冲突。本轮建议先按 24 的错误边界返回 `READ_TIMEOUT`，如需空 payload 兼容必须在工具文档中明确。

### 5. 资源锁原则

- TCP client 连接远端不应占用 `tcp-listen:host:port` 锁，因为多个 client 连接同一远端是合法场景。
- TCP listen 必须占用 `tcp-listen:bind_host:bind_port` 锁。
- UDP bind 必须占用 `udp-bind:bind_host:bind_port` 锁。
- connect 失败、disconnect、release、force release 后必须验证锁释放或关闭路径可观察。

## TCP client 契约

### 配置

`tcp_udp_config` 对 TCP client 继续使用：

```json
{
  "handle_id": "h_tcp_001",
  "mode": "client",
  "host": "127.0.0.1",
  "port": 8081,
  "timeout_ms": 1000
}
```

约束：

- `host` 必须是允许的 loopback 字面量。
- `port` 必须在 `0..=65535`。
- `timeout_ms` 必须沿用当前范围限制。
- 配置阶段只校验参数，不打开 socket。

### 连接

`port_connect` 必须调用真实 TCP connect。没有 peer 监听时不得返回 `Connected`，应返回 `CONNECT_TIMEOUT` 或 `TRANSPORT_CLOSED`/`WRITE_IO_FAILED` 中已有稳定映射。

成功后：

- 实例状态为 `Connected`。
- worker/transport 与 handle 绑定。
- `instance_query` 可观察 TCP client 配置和 Connected 状态。
- 不使用 Mock tx queue 证明连接成功。

### 发送

`port_send` 必须把 bytes 写入真实 TCP stream。

成功后：

- 对端 loopback server 能收到相同 bytes。
- `sent_bytes` 等于实际写入字节数。
- `queued` 按选定发送语义返回，第一轮推荐 `false`。
- tx stats 更新，且不增加 Mock `tx_queue_items`。

### 拉取

`port_pull` 必须读取真实 TCP stream 或真实 reader task 写入的 rx buffer。

成功后：

- 可从 echo server 拉取到预期响应。
- rx stats 更新。
- peer 关闭时返回 `TRANSPORT_CLOSED` 或既有等价错误。

## TCP listen 单客户端契约

TCP listen 初版仍保持单客户端策略，后续多客户端由 25 中增强能力继续扩展。

### 配置

`tcp_udp_config` 对 TCP listen 使用：

```json
{
  "handle_id": "h_tcp_001",
  "mode": "listen",
  "bind_host": "127.0.0.1",
  "bind_port": 9000,
  "timeout_ms": 1000
}
```

工具层可继续折叠到内部 `TcpConfig { mode, host, port }`，但文档和 resource summary 必须能区分 client remote 与 listen bind 的含义。

### 连接

推荐语义：`port_connect(mode=listen)` 只完成 bind 并立即返回 `Connected`，不等待 client accept。

理由：

- 避免 `port_connect` 在无客户端时长时间阻塞。
- 与服务端监听资源生命周期更一致。
- 首个 peer 的 accept 可以由后台 task 或首次 read/write 触发。

### 收发

- 初版只允许一个 active peer。
- 无 active peer 时，`port_send` 应返回稳定错误，例如 `STATE_NOT_ALLOWED` 的细化错误或新增 `PEER_NOT_CONNECTED`，不应静默入队。
- `port_pull` 可等待首个 peer 数据直到 timeout；timeout 行为必须与 TCP client pull 一致。
- 第二个 peer 连接时，初版应拒绝或立即关闭，并在日志或 stats 中可诊断。

## UDP 契约

### 配置

UDP 使用：

```json
{
  "handle_id": "h_udp_001",
  "bind_host": "127.0.0.1",
  "bind_port": 9001,
  "remote_host": "127.0.0.1",
  "remote_port": 9002,
  "timeout_ms": 1000
}
```

约束：

- `bind_host` 必须是允许的 loopback 字面量。
- `remote_host` 如提供，也必须是允许的 loopback 字面量。
- 本轮推荐 `port_send` 要求 remote 必填；缺少 remote 时返回配置错误，不使用“最近 peer”隐式发送。

### 连接

`port_connect` 必须 bind 真实 UDP socket。

成功后：

- 实例状态为 `Connected`。
- 占用 `udp-bind:bind_host:bind_port` 锁。
- 地址冲突返回 `UDP_BIND_ADDR_BUSY`。

### 发送

`port_send` 向配置的 remote endpoint 发送完整 datagram。

成功后：

- 对端 UDP socket 能收到相同 bytes。
- `sent_bytes` 等于 datagram 字节数。
- `queued` 按选定语义返回，第一轮推荐 `false` 或同步 ack。

### 拉取

`port_pull` 从 UDP socket 接收一个 datagram，并按当前 payload summary 返回 bytes。

本轮保留但不公开的内部信息：

- peer address
- datagram length
- receive timestamp

这些信息后续可在 UDP datagram 元数据增强中扩展，不在本轮破坏 `port_pull` 外形。

## 测试计划

### 必须先失败的测试

在实现真实 runtime 前，以下测试应能暴露当前 Mock 假阳性：

| 测试 | 期望 |
| --- | --- |
| TCP client 连接无监听端口 | `port_connect` 不得返回 `Connected`。 |
| TCP client 发送到真实 listener | listener 必须收到 payload；当前 Mock 路径会失败。 |
| MCP smoke 无 echo server 仍断言 TCP 成功 | 应改写，不能继续作为真实 TCP 成功证据。 |
| UDP bind 地址冲突 | 第二个实例 bind 同一地址必须返回 `UDP_BIND_ADDR_BUSY`。 |
| disconnect/release 后重绑 | 原端口必须可再次 bind 或连接。 |

### TCP client e2e

| 步骤 | 验收 |
| --- | --- |
| 启动 loopback TCP echo server | server 监听随机或固定测试端口。 |
| MCP 调用 `instance_create(TCP)` | 返回 TCP handle。 |
| MCP 调用 `tcp_udp_config(client)` | 状态进入 `Configured`。 |
| MCP 调用 `port_connect` | 状态进入 `Connected`，echo server 观察到连接。 |
| MCP 调用 `port_send("ping")` | echo server 收到 `ping`。 |
| MCP 调用 `port_pull` | 返回 `ping` 或 echo 响应。 |
| MCP 调用 `port_disconnect` | socket 关闭，状态进入 `Disconnected`。 |

### TCP listen e2e

| 步骤 | 验收 |
| --- | --- |
| MCP 配置 listen 随机端口 | `port_connect` 完成 bind。 |
| 外部 TCP client 连接 | listen worker 建立 active peer。 |
| 外部 client 发送 `ping` | `port_pull` 返回 `ping`。 |
| MCP `port_send("pong")` | 外部 client 收到 `pong`。 |
| 第二个 client 连接 | 初版按单客户端策略拒绝或关闭，并可诊断。 |

### UDP e2e

| 步骤 | 验收 |
| --- | --- |
| 启动外部 UDP socket A | 作为 remote peer。 |
| MCP 创建 UDP 实例并 bind socket B | 状态进入 `Connected`。 |
| MCP `port_send("ping")` | socket A 收到 datagram `ping`。 |
| socket A 发送 `pong` 到 socket B | MCP `port_pull` 返回 `pong`。 |
| 释放实例后重绑 bind port | 可成功重绑。 |

### 回归测试

| 范围 | 要求 |
| --- | --- |
| Serial | 现有 serial worker 真实 I/O 和 Windows 手工验收路径不回归。 |
| Mock runtime | Mock 队列、buffer、订阅、force release 测试保留，但不再作为 TCP/UDP 真实收发证据。 |
| MCP 统一返回 | `ok/tool/request_id/handle_id/state/data/warnings/error` 外形不破坏。 |
| SDK 边界 | `rmcp` 仍限制在 `src/main.rs` 和 `src/mcp/*`。 |
| 资源锁 | connect 失败、disconnect、release、force release 都不泄露锁。 |
| 容量限制 | tx frame、pull max bytes、rx buffer budget 仍生效。 |

## 推荐施工切片

| 切片 | 目标结果 | 主要触点 | 验收命令或证据 |
| --- | --- | --- | --- |
| R1 契约测试先行 | 新增会暴露 Mock 假阳性的 app/MCP 测试 | `src/mcp/tools.rs`、`src/app/port_service.rs` tests | TCP 无监听端口 connect 不再被 smoke 当成功。 |
| R2 Async-safe worker 骨架 | 每实例 network worker/task control plane 可创建、关闭、查询 | `runtime::tasks`、`app::instance_service`、`app::port_service` | 不在 `InstanceService` 全局锁内 await。 |
| R3 TCP client 真实 connect/send/pull | TCP client 通过 MCP 与 echo server 往返 | `transport::tcp`、`app`、`runtime`、`mcp` | MCP e2e ping/pong 通过。 |
| R4 UDP bind/send/pull | UDP 通过 MCP 完成 datagram 往返 | `transport::udp`、`app`、`runtime`、`mcp` | UDP e2e ping/pong 通过。 |
| R5 TCP listen 单客户端 | listen 实例可和一个外部 client 收发 | `transport::tcp`、`runtime`、`mcp` | 单客户端 e2e 通过，第二 client 行为可诊断。 |
| R6 文档与验收收口 | 更新 24、README 和 acceptance 记录 | `docs`、`README.md` | `cargo test`、`cargo b --release` 和实际 8081 smoke 证据。 |

## 不做事项

- 不在本轮开放非 loopback TCP/UDP 目标。
- 不做 TCP 多客户端 listen、peer id、广播发送或 UDP 多来源过滤。
- 不把 hostname/DNS、CIDR、通配地址或端口段扫描加入真实 I/O 路径。
- 不新增绕过 `port_send` / `port_pull` 的专用 TCP/UDP send/recv 工具。
- 不让 MCP smoke 仅凭 Mock 状态返回继续证明真实网络收发。

## 最小通过门槛

进入实现完成判定前，至少满足：

1. TCP client 通过 MCP 工具向真实 loopback echo server 发送 `ping`，server 侧能观察到 bytes。
2. `port_send` 返回语义与真实写入语义一致，不再把 Mock 入队长度当成真实发送成功。
3. 无监听端口、地址冲突、timeout、peer 关闭和非法目标都有稳定错误返回。
4. `disconnect`、`instance_release(force=true)` 后 socket 和资源锁可回收。
5. `cargo test`、`cargo b --release` 通过，并有一份 acceptance 记录说明真实 TCP/UDP app/MCP 路径已覆盖到哪一层。
