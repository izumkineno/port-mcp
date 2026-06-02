# 24 MCP 工具列表与调用收发详解

本文档承接 [03-初版工具契约](03-初版工具契约.md)、[07-错误模型与返回格式](07-错误模型与返回格式.md) 和 [23-M7-MCP-server接入施工记录](23-M7-MCP-server接入施工记录.md)，面向 MCP 客户端调用者、维护者和测试验收者说明当前 Rust `port-mcp` 暴露的工具列表、调用时的请求/响应外形、实现映射、错误边界和收发日志字段。

本文档不新增工具、不改变工具契约、不修改状态机和错误码；它只是把已经实现的 `src/mcp/tools.rs`、`src/mcp/response.rs` 和模型层返回结构整理成可调用、可排查、可验收的参考手册。

## 当前工具总览

当前 stdio MCP server 暴露初版端口工具、VISA 配置工具和轻量协议 helper。

| 分组 | 工具 | 主要用途 | 实现入口 |
| --- | --- | --- | --- |
| 实例管理 | `instance_create` | 创建 Serial/TCP/UDP 实例。 | `PortMcpServer::instance_create` |
| 实例管理 | `instance_list` | 列出未释放实例摘要。 | `PortMcpServer::instance_list` |
| 实例管理 | `instance_query` | 查询指定实例或当前会话默认实例。 | `PortMcpServer::instance_query` |
| 实例管理 | `instance_use` | 把实例绑定为当前 MCP 会话默认实例。 | `PortMcpServer::instance_use` |
| 实例管理 | `instance_release` | 释放实例，可选强制释放。 | `PortMcpServer::instance_release` |
| 连接配置 | `serial_config` | 配置 Serial 实例。 | `PortMcpServer::serial_config` |
| 连接配置 | `tcp_udp_config` | 配置 TCP 或 UDP 实例。 | `PortMcpServer::tcp_udp_config` |
| 端口行为 | `port_scan` | 扫描允许范围内的 loopback TCP 端口。 | `PortMcpServer::port_scan` |
| 端口行为 | `port_connect` | 连接已配置实例。 | `PortMcpServer::port_connect` |
| 端口行为 | `port_disconnect` | 断开已连接实例。 | `PortMcpServer::port_disconnect` |
| 端口行为 | `port_send` | 发送 text 或 hex payload。 | `PortMcpServer::port_send` |
| 端口行为 | `port_pull` | 从接收缓冲区拉取数据摘要。 | `PortMcpServer::port_pull` |
| 端口行为 | `port_clear` | 清理 tx、rx 或全部缓冲。 | `PortMcpServer::port_clear` |
| 最小流式订阅 | `port_subscribe_stream` | 订阅当前会话的实例接收通知。 | `PortMcpServer::port_subscribe_stream` |
| 最小流式订阅 | `port_unsubscribe_stream` | 取消当前会话的实例接收通知。 | `PortMcpServer::port_unsubscribe_stream` |
| 调试配置 | `debug_log_config` | 设置调试日志中的端口原始收发数据显示范围。 | `PortMcpServer::debug_log_config` |
| 协议 helper | `usage_guide` | 返回面向 MCP agent 的机器可读调用指南。 | `PortMcpServer::usage_guide` |
| 协议 helper | `str_to_hex` | UTF-8 文本转 hex。 | `PortMcpServer::str_to_hex` |
| 协议 helper | `hex_to_str` | hex 转 UTF-8 文本。 | `PortMcpServer::hex_to_str` |
| 协议 helper | `modbus_helper` | Modbus RTU pack/unpack。 | `PortMcpServer::modbus_helper` |
| 协议 helper | `scpi_helper` | SCPI 命令归一化摘要。 | `PortMcpServer::scpi_helper` |
| 协议 helper | `at_helper` | AT 命令分类摘要。 | `PortMcpServer::at_helper` |
| 协议 helper | `slip_helper` | SLIP payload encode/decode。 | `PortMcpServer::slip_helper` |

### 轻量协议 helper 契约

`str_to_hex` 入参为 `input_string`，输出 `hex` 和 `input_bytes`。输入超过硬限制返回 `INVALID_RANGE`。

`hex_to_str` 入参为 `hex`，输出 `text` 和 `input_bytes`。非法 hex 返回 `INVALID_HEX`；hex 对应字节超过硬限制返回 `INVALID_RANGE`；无法按 UTF-8 解码返回 `TEXT_ENCODING_FAILED`。

`modbus_helper` 当前仅支持 `mode=rtu`：

- `action=pack` 需要 `slave_id`、`function_code`、`address`，并可选 `data_or_hex` 作为地址后的 payload hex。
- `action=unpack` 必须传 `frame_hex` 作为完整 RTU 帧。`data_or_hex` 不作为 unpack 输入；只传 `data_or_hex` 会返回 `MISSING_REQUIRED_FIELD`，`details.field=frame_hex`。
- `crc_check` 默认 `true`。默认模式下坏 CRC 返回 `PROTOCOL_CHECKSUM_FAILED`；显式传 `crc_check=false` 时进入宽松诊断模式，坏 CRC 返回成功响应并置 `checksum_valid=false`。
- 非法 hex 返回 `INVALID_HEX`；帧结构不合法返回 `PROTOCOL_FRAME_INVALID`。

`scpi_helper` 当前支持 `action=normalize`，入参 `command`，可选 `arguments` 和 `expect_response`，返回归一化文本摘要。

`at_helper` 入参 `command`，返回 `basic`、`extended` 或 `custom` 分类。

`slip_helper` 使用 `payload_hex`：`action=encode` 返回带 `C0` 边界的 SLIP frame hex；`action=decode` 要求输入是完整 framed SLIP hex。非法 escape，例如 closing delimiter 前的裸 `DB`，返回 `PROTOCOL_FRAME_INVALID`。

## MCP 调用外形

MCP 客户端调用工具时，传入的是工具名和 JSON object 参数。以 `rmcp` 客户端为例，调用本质等价于：

```json
{
  "name": "instance_create",
  "arguments": {
    "type": "TCP"
  }
}
```

服务端返回 `CallToolResult`，其 `content[0]` 是 text 内容；text 内部是本项目统一序列化后的 JSON。客户端通常需要先取 text，再按 JSON 解析成 `ToolResponse`。

### 成功返回

成功返回统一外形如下。全局工具可以没有 `handle_id` 和 `state`，实例相关工具会带上它们。

```json
{
  "ok": true,
  "tool": "port_send",
  "request_id": "req_20260526_000004",
  "timestamp": "2026-05-26T12:00:00Z",
  "handle_id": "h_tcp_001",
  "state": "Connected",
  "data": {
    "queued": true,
    "sent_bytes": 4
  },
  "warnings": []
}
```

### 失败返回

失败返回统一外形如下。错误由稳定 `category` 和细分 `code` 共同定位；`details` 只放脱敏调试信息。

```json
{
  "ok": false,
  "tool": "port_scan",
  "request_id": "req_20260526_000005",
  "timestamp": "2026-05-26T12:00:01Z",
  "error": {
    "category": "InvalidArgument",
    "code": "INVALID_RANGE",
    "message": "port_scan timeout_ms is outside the allowed range.",
    "recovery_hint": "Use a timeout between 1 and the configured scan_total_timeout_ms.",
    "retryable": false,
    "details": {
      "field": "timeout_ms",
      "min": 1,
      "max": 10000,
      "actual": 10001
    }
  }
}
```

## 收发与日志链路

每个工具 handler 在进入时记录 `Instant::now()`，结束时通过 `mcp::response::call_tool_result_with_duration` 包装为 `CallToolResult`，并写入结构化日志事件。

| 字段 | 来源 | 说明 |
| --- | --- | --- |
| `event` | `mcp::response` | 当前固定为 `tool_call`。 |
| `tool` | `ToolResponse.tool` | 工具名。 |
| `request_id` | `IdGenerator` | 单次工具调用 ID。 |
| `handle_id` | `ToolResponse.handle_id` | 实例相关工具的句柄。 |
| `session` | 当前实现预留 | 当前日志包装未写入具体 session，订阅返回里会暴露 `request_context_debug`。 |
| `state_before` / `state_after` | 当前 response state | 当前实现用返回 state 作为状态摘要。 |
| `error_code` | `error.code` | 失败时的细分错误码。 |
| `duration_ms` | handler 起点到响应包装 | 工具处理耗时，包含 app/runtime 调用和响应包装前的主要路径。 |
| `sensitive` | 固定 false | 工具调用基础日志不携带敏感 payload；端口原始收发数据只在显式打开显示范围后写入 `port_io`。 |
| `port_io.direction` | `port_send` / `port_pull` | `tx` 表示发送，`rx` 表示接收。仅当 `debug_log_config.port_io_log_bytes > 0` 时出现。 |
| `port_io.bytes` | 原始收发字节 | 本次发送或拉取的总字节数。 |
| `port_io.preview_encoding` | 调用编码 | `text` 或 `hex`；hex 发送按十六进制字符串显示。 |
| `port_io.preview` | 原始收发数据预览 | 按 `port_io_log_bytes` 截断后的 text 或 hex 字符串。 |
| `port_io.hex` | 原始收发数据十六进制 | 同一段预览字节的十六进制字符串，便于二进制排查。 |
| `port_io.omitted_bytes` | 截断计算 | 超过显示范围而未写入日志的字节数。 |

真实串口、TCP、UDP 的底层收发由 `transport` 层封装；MCP 工具层只接收 JSON 参数、调用 `app` 服务、返回摘要。`port_send` 返回发送字节数和是否入队；`port_pull` 返回 payload 摘要，不直接返回无限制原始数据。调试日志默认不展示原始收发数据；需要排查时先调用 `debug_log_config` 设置显示字节数，设置为 `0` 可关闭。

## 通用参数类型

| 类型 | 字段或取值 | 说明 |
| --- | --- | --- |
| `InstanceTypeParam` | `Serial`、`TCP`、`UDP`，并接受大小写别名 | `instance_create.type`。 |
| `HandleParams` | `handle_id: string` | 多数实例操作使用。 |
| `EncodingParam` | `text`、`hex` | `port_send.encoding`；默认 `text`。 |
| `ClearTargetParam` | `tx`、`rx`、`all` | `port_clear.target`；默认 `all`。 |
| `TcpModeParam` | `client`、`listen` | `tcp_udp_config.mode`；默认 `client`。 |
| `DataBitsParam` | `Seven`、`Eight` | `serial_config.data_bits`；默认 `Eight`。 |
| `StopBitsParam` | `One`、`Two` | `serial_config.stop_bits`；默认 `One`。 |
| `ParityParam` | `None`、`Odd`、`Even` | `serial_config.parity`；默认 `None`。 |
| `FlowControlParam` | `None`、`Software`、`Hardware` | `serial_config.flow_control`；默认 `None`。 |

默认数值：`baudrate=115200`、常规 `timeout_ms=1000`、`port_scan.max_concurrency=16`、`port_subscribe_stream.max_payload_bytes=16384`。

## 工具调用详解

### `instance_create`

用途：创建 Serial、TCP 或 UDP 实例，不打开底层资源。

入参：

```json
{
  "type": "TCP"
}
```

成功返回重点：

```json
{
  "ok": true,
  "tool": "instance_create",
  "handle_id": "h_tcp_001",
  "state": "Created",
  "data": {
    "type": "TCP",
    "summary": {
      "handle_id": "h_tcp_001",
      "type": "TCP",
      "state": "Created"
    }
  },
  "warnings": []
}
```

错误边界：`type` 非法时返回 `InvalidArgument`；不支持 VISA 或其他进阶类型。

### `instance_list`

用途：列出所有未释放实例。

入参：无参数，传 `{}`。

```json
{}
```

成功返回重点：

```json
{
  "ok": true,
  "tool": "instance_list",
  "data": {
    "instances": [
      {
        "handle_id": "h_tcp_001",
        "type": "TCP",
        "state": "Configured",
        "stats": {
          "tx_bytes": 0,
          "rx_bytes": 0,
          "rx_buffer_bytes": 0,
          "subscriber_count": 0
        }
      }
    ]
  },
  "warnings": []
}
```

错误边界：通常没有业务错误；内部异常由 MCP/统一响应路径包装。

### `instance_query`

用途：查询显式 `handle_id`，或在未传 `handle_id` 时查询当前 MCP session 默认实例。

入参：

```json
{
  "handle_id": "h_tcp_001"
}
```

成功返回重点：`data.summary` 包含 `handle_id`、`type`、`state`、`resource`、`config`、`stats`、`last_error`。

错误边界：句柄不存在返回 `HANDLE_NOT_FOUND`；未传句柄且无默认绑定返回 `SESSION_BINDING_MISSING`；会话身份不可用时返回 `SESSION_ID_UNAVAILABLE`。

### `instance_use`

用途：把实例绑定为当前 MCP session 的默认实例，方便后续支持缺省句柄解析的工具使用。

入参：

```json
{
  "handle_id": "h_tcp_001"
}
```

成功返回重点：

```json
{
  "ok": true,
  "tool": "instance_use",
  "handle_id": "h_tcp_001",
  "data": {
    "bound": true,
    "previous_handle_id": null
  }
}
```

错误边界：句柄不存在或 session 不可用。当前 session 来源是 `RequestContext<RoleServer>` 的诊断字符串。

### `instance_release`

用途：释放实例。`Connected` 状态普通释放会被拒绝，除非 `force=true`。

入参：

```json
{
  "handle_id": "h_tcp_001",
  "force": false
}
```

成功返回重点：返回释放后的 `data.summary` 和 `state`。释放后实例不应再出现在 `instance_list`。

错误边界：句柄不存在、状态不允许、连接中普通释放需要 force。

### `serial_config`

用途：配置 Serial 实例。

入参：

```json
{
  "handle_id": "h_ser_001",
  "port": "COM5",
  "baudrate": 115200,
  "data_bits": "Eight",
  "stop_bits": "One",
  "parity": "None",
  "flow_control": "None",
  "timeout_ms": 1000,
  "encoding": "text"
}
```

成功返回重点：实例进入 `Configured`，`data.summary.config.kind` 为 `serial`。

错误边界：类型不匹配返回 `TYPE_MISMATCH`；状态不允许返回 `STATE_NOT_ALLOWED`；真实打开串口不发生在配置阶段。

### `tcp_udp_config`

用途：配置 TCP 或 UDP 实例。服务端会先查询实例类型，再写入对应配置；不能用它配置 Serial 实例。

TCP client 示例：

```json
{
  "handle_id": "h_tcp_001",
  "mode": "client",
  "host": "127.0.0.1",
  "port": 9000,
  "timeout_ms": 1000
}
```

TCP listen 示例：

```json
{
  "handle_id": "h_tcp_001",
  "mode": "listen",
  "bind_host": "127.0.0.1",
  "bind_port": 9000,
  "timeout_ms": 1000
}
```

UDP 示例：

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

成功返回重点：实例进入 `Configured`，`data.summary.config.kind` 为 `tcp` 或 `udp`。

错误边界：Serial 实例返回 `TYPE_MISMATCH`；空或非法地址、端口越界、状态不允许会返回对应错误。

### `port_scan`

用途：按 `type` 选择串口枚举或允许范围内的 loopback TCP/UDP 端口扫描。初版网络扫描只允许 loopback 单主机，不支持非 loopback、CIDR、通配地址或 DNS 扫描。

入参：

```json
{
  "type": "Serial",
  "config": {}
}
```

```json
{
  "type": "TCP",
  "config": {
    "host": "127.0.0.1",
    "start_port": 9000,
    "end_port": 9010,
    "max_concurrency": 16,
    "timeout_ms": 1000
  }
}
```

成功返回重点：

```json
{
  "ok": true,
  "tool": "port_scan",
  "data": {
    "resources": [
      {
        "name": "COM3",
        "display": "usb vid=1a86 pid=7523",
        "port_type": "usb vid=1a86 pid=7523"
      }
    ]
  },
  "warnings": []
}
```

```json
{
  "ok": true,
  "tool": "port_scan",
  "data": {
    "open_ports": [9000]
  },
  "warnings": []
}
```

错误边界：

- `timeout_ms` 必须在 `1..=10000`，越界返回 `INVALID_RANGE`。
- 非 loopback 目标返回 `SCAN_TARGET_NOT_ALLOWED`。
- 扫描范围超过 256 个端口或并发超过 64 返回 `SCAN_RANGE_TOO_LARGE`。

### `port_connect`

用途：连接已配置实例。

入参：

```json
{
  "handle_id": "h_tcp_001"
}
```

成功返回重点：实例进入 `Connected`。

错误边界：未配置返回 `CONFIG_REQUIRED`；串口被占用返回 `SERIAL_PORT_BUSY`；TCP listen 地址冲突返回 `TCP_LISTEN_ADDR_BUSY`；UDP bind 地址冲突返回 `UDP_BIND_ADDR_BUSY`；连接超时返回 `CONNECT_TIMEOUT` 或 `SERIAL_OPEN_TIMEOUT`。

### `port_disconnect`

用途：断开已连接实例，保留配置。

入参：

```json
{
  "handle_id": "h_tcp_001"
}
```

成功返回重点：实例通常进入 `Disconnected`；强制释放或后台关闭路径可能按生命周期规则进入临时清理状态。

错误边界：句柄不存在、状态不允许、断开失败。

### `port_send`

用途：向已连接实例发送 payload。

Text 示例：

```json
{
  "handle_id": "h_tcp_001",
  "data": "ping",
  "encoding": "text",
  "append_line_break": false
}
```

Hex 示例：

```json
{
  "handle_id": "h_tcp_001",
  "data": "70696e67",
  "encoding": "hex",
  "append_line_break": true
}
```

成功返回重点：

```json
{
  "ok": true,
  "tool": "port_send",
  "handle_id": "h_tcp_001",
  "state": "Connected",
  "data": {
    "queued": true,
    "sent_bytes": 4
  },
  "warnings": []
}
```

错误边界：hex 非法返回 `INVALID_HEX`；未连接返回 `STATE_NOT_ALLOWED`；发送队列满返回 `TX_QUEUE_FULL`；单帧过大返回 `TX_FRAME_TOO_LARGE`；底层写失败返回 `WRITE_IO_FAILED` 或 `TRANSPORT_CLOSED`。

### `port_pull`

用途：从接收缓冲区拉取最多 `max_bytes` 字节，并返回 payload 摘要。

入参：

```json
{
  "handle_id": "h_tcp_001",
  "max_bytes": 64
}
```

成功返回重点：

```json
{
  "ok": true,
  "tool": "port_pull",
  "handle_id": "h_tcp_001",
  "state": "Connected",
  "data": {
    "payload": {
      "preview": "pong",
      "preview_encoding": "text",
      "payload_bytes": 4,
      "omitted_bytes": 0,
      "truncated": false,
      "datagram": false
    },
    "truncated": false,
    "remaining_rx_buffer_bytes": 0
  },
  "warnings": []
}
```

错误边界：`max_bytes` 超过 `pull_max_bytes` 返回 `PULL_MAX_BYTES_EXCEEDED`；无数据可返回 `READ_TIMEOUT` 或 `NO_DATA_AVAILABLE`；未连接返回状态错误。

### `port_clear`

用途：清理发送队列、接收缓冲区或两者。

入参：

```json
{
  "handle_id": "h_tcp_001",
  "target": "all"
}
```

成功返回重点：

```json
{
  "ok": true,
  "tool": "port_clear",
  "handle_id": "h_tcp_001",
  "state": "Connected",
  "data": {
    "dropped_tx_items": 0,
    "dropped_tx_bytes": 0,
    "dropped_rx_bytes": 0
  },
  "warnings": []
}
```

错误边界：句柄不存在、状态不允许或 target 枚举非法。

### `port_subscribe_stream`

用途：为当前 MCP session 订阅实例接收通知。初版返回订阅状态，不在本文档中定义进阶流式协议。

入参：

```json
{
  "handle_id": "h_tcp_001",
  "max_payload_bytes": 16384
}
```

成功返回重点：

```json
{
  "ok": true,
  "tool": "port_subscribe_stream",
  "handle_id": "h_tcp_001",
  "state": "Connected",
  "data": {
    "was_subscribed": false,
    "session_mode": "request_context_debug"
  },
  "warnings": []
}
```

错误边界：订阅 payload 上限过大返回 `SUBSCRIBER_PAYLOAD_TOO_LARGE`；订阅者过多返回 `SUBSCRIBER_LIMIT_EXCEEDED`；session 不可用返回 `SESSION_ID_UNAVAILABLE`。

### `port_unsubscribe_stream`

用途：取消当前 MCP session 对实例的接收通知订阅。

入参：

```json
{
  "handle_id": "h_tcp_001"
}
```

成功返回重点：

```json
{
  "ok": true,
  "tool": "port_unsubscribe_stream",
  "handle_id": "h_tcp_001",
  "data": {
    "was_subscribed": true,
    "session_mode": "request_context_debug"
  },
  "warnings": []
}
```

错误边界：句柄不存在或 session 不可用。重复取消订阅应按运行时订阅语义返回 `was_subscribed=false`，而不是业务失败。

### `debug_log_config`

用途：设置调试日志中 `port_send` 和 `port_pull` 的原始收发数据显示范围。该配置为当前 MCP server 进程内存态，默认 `port_io_log_bytes=0`，即不在日志中写原始收发数据。

入参：

```json
{
  "port_io_log_bytes": 64
}
```

成功返回重点：

```json
{
  "ok": true,
  "tool": "debug_log_config",
  "data": {
    "port_io_log_bytes": 64
  },
  "warnings": []
}
```

错误边界：`port_io_log_bytes` 最大为 `65536`；超过上限返回 `INVALID_RANGE`。设置为 `0` 会关闭原始收发数据日志。

## 推荐调用序列

### TCP client mock/loopback 基础序列

```text
instance_create(type=TCP)
tcp_udp_config(handle_id, mode=client, host=127.0.0.1, port=9000)
port_connect(handle_id)
port_send(handle_id, data=ping, encoding=text)
port_pull(handle_id, max_bytes=64)
port_disconnect(handle_id)
instance_release(handle_id)
```

### Serial 基础序列

```text
instance_create(type=Serial)
serial_config(handle_id, port=COM5, baudrate=115200)
port_connect(handle_id)
port_send(handle_id, data=70696e67, encoding=hex)
port_pull(handle_id, max_bytes=64)
port_disconnect(handle_id)
instance_release(handle_id)
```

如果 `port_connect` 返回 `SERIAL_PORT_BUSY`，应先确认外部串口调试工具是否占用该端口，或调用 `instance_list` 检查是否已有实例持有同一资源。

### 订阅序列

```text
instance_create(type=TCP)
tcp_udp_config(handle_id, mode=client, host=127.0.0.1, port=9000)
port_connect(handle_id)
port_subscribe_stream(handle_id, max_payload_bytes=16384)
port_unsubscribe_stream(handle_id)
port_disconnect(handle_id)
instance_release(handle_id)
```

## 验收与排查命令

| 目标 | 命令或检查 |
| --- | --- |
| 工具注册列表 | `cargo test m7_tool_list_registers_initial_contract_tools` |
| MCP 端到端 smoke | `cargo test m7_e2e_smoke_covers_instance_config_port_and_release_tools` |
| 会话订阅返回 | `cargo test m7_request_context_is_reflected_in_subscription_response` |
| 日志字段 | `cargo test m8_tool_log_event_contains_correlation_state_duration_and_sensitivity_fields` |
| `port_scan.timeout_ms` 上限 | `cargo test m9_port_scan_rejects_timeout_above_runtime_limit` |
| 全量回归 | `cargo test` |
| SDK 边界 | 搜索 `rmcp|RequestContext|CallToolResult|ServerHandler|tool_router|tool_handler|schemars`，只应命中 `src/main.rs` 与 `src/mcp/*`。 |

## 已知边界

- 当前实现已暴露 VISA 配置和轻量协议 helper；仍不暴露非 loopback scan allowlist、HTTP transport、OAuth 或高级订阅工具。
- `mcp-server/` 是独立 TypeScript/OMG workflow server，不属于本文档描述的 Rust `port-mcp` 工具列表。
- 默认自动化测试不依赖真实串口硬件；真实 COM 口验收属于 Windows 手工验收记录。
- `port_scan` 只允许 loopback 单主机，并限制端口数量、并发和单项超时；不要把它当作通用网络扫描器。
- 工具返回只携带 payload 摘要和脱敏错误详情；完整底层 I/O 细节应通过结构化日志排查。