# 27 VISA 集成与工具契约扩展

本文档承接 [00-索引](00-索引.md)、[02-进阶实现大纲](02-进阶实现大纲.md)、[04-运行时架构设计](04-运行时架构设计.md)、[07-错误模型与返回格式](07-错误模型与返回格式.md)、[09-库选择与依赖决策](09-库选择与依赖决策.md)、[24-MCP工具列表与调用收发详解](24-MCP工具列表与调用收发详解.md) 与 [25-进阶能力施工拆分与验收门槛](25-进阶能力施工拆分与验收门槛.md)，把 VISA 能力收敛成一份可实施、可验收、可继续演进的进阶文档，并作为 `docs/00-索引.md` 中的正式编号条目维护。

本文档不是代码实现记录，也不是完整仪器平台设计。它只覆盖与当前 `port-mcp` 通用端口抽象相匹配的 VISA 基础能力、工具契约扩展、错误退化规则和 Rust `feature` 选择性编译方案。

## 设计结论

- VISA 必须作为现有通用端口链路的一类正式实例能力接入，而不是另起一套 `visa_*_send` / `visa_*_pull` 平行工作流。
- 依赖选择固定为 `visa-rs`，并通过 Rust `feature` + optional dependency 隔离；默认基础构建不得强依赖 VISA runtime、驱动或硬件环境。
- 运行期收发工具统一复用 `port_connect`、`port_disconnect`、`port_send`、`port_pull`、`port_clear`；`instance_query` 与 `instance_list` 也必须能表达 VISA 实例摘要。
- `port_scan(type=Visa)` 必须成为正式契约的一部分，用于通过 VISA resource manager 枚举资源地址摘要。
- 配置面遵循“**复用优先，例外最少**”原则：若现有 `serial_config` / `tcp_udp_config` 不能自然承载 VISA 资源地址、超时和基础终止符等参数，则允许新增 `visa_config`，但它只能作为配置面唯一例外，不得演化成独立 VISA 工具族。
- 范围仅限基础能力：资源枚举、配置、连接、基础 SCPI/原始文本或二进制收发、可选识别摘要、错误退化和验收路径；不覆盖截图、波形抓取、文件系统、厂商专有高阶特性或批量仪器编排。

## 目标与非目标

### 目标

1. 在不破坏现有 `mcp -> app -> runtime -> transport -> model` 分层的前提下，为 VISA 补齐文档级架构与契约定义。
2. 固定 `visa-rs` 的依赖边界、Cargo feature 设计和缺依赖退化规则。
3. 让后续实现可以按当前 `instance_create -> configure -> connect -> send/pull -> disconnect -> release` 序列接入 VISA。
4. 把 `instance_create(type=Visa)`、`port_scan(type=Visa)`、配置面、连接、收发、清理、查询相关契约写到接近 [03-初版工具契约](03-初版工具契约.md) 与 [24-MCP工具列表与调用收发详解](24-MCP工具列表与调用收发详解.md) 的粒度。

### 非目标

- 不设计第二套 VISA 专用 session、实例表、资源锁、发送队列或订阅系统。
- 不覆盖示波器截图、波形抓取、文件传输、寄存器缓存、事件回调、批量脚本编排或厂商专有高级 API。
- 不在本轮把 VISA 扩展成“通用仪器资产管理平台”。
- 不承诺首版就兼容所有 VISA 后端、所有厂商和所有资源族；首版只要求基础地址枚举与基础收发链路。

## 与当前架构的适配关系

VISA 仍然必须落入 [04-运行时架构设计](04-运行时架构设计.md) 的四层模型：

```text
MCP Client
-> Rust MCP Server
-> Tool Handler
-> App Service
-> RuntimeRegistry
-> RuntimeInstance
-> TransportTask / Transport Worker
-> VISA Transport
```

### 分层落点

| 层级 | VISA 职责 | 不负责 |
| --- | --- | --- |
| `mcp` | 暴露 `instance_create(type=Visa)`、`port_scan(type=Visa)`、VISA 配置入口和通用 `port_*` 工具参数解析 | 直接持有 VISA session 句柄 |
| `app` | 校验实例类型、写入 VISA 配置、协调连接和错误映射 | 直接拼接底层 FFI 错误文本 |
| `runtime` | 保存 VISA 实例状态、资源摘要、统计、队列和生命周期 | 理解具体厂商命令语义 |
| `transport::visa` | 调用 `visa-rs` 完成 resource manager 枚举、open、read、write、close | 决定 MCP 返回 JSON |

### 推荐模块触点

| 模块 | 建议扩展 |
| --- | --- |
| `src/model/state.rs` | 新增 `InstanceType::Visa`。 |
| `src/model/config.rs` | 新增 `VisaConfig`、`ConfigSnapshot::Visa`、`ResourceSummary::visa(...)`。 |
| `src/app/config_service.rs` | 增加 VISA 配置写入路径。 |
| `src/app/port_service.rs` | 在 connect/send/pull/disconnect 中分发 VISA worker。 |
| `src/transport/mod.rs` | 导出 `visa` 模块及其 scan/open/read/write/close 能力。 |
| `src/transport/visa.rs` | 封装 `visa-rs` 依赖和 feature-gated 退化实现。 |
| `src/model/error.rs` / `docs/07` | 增补 VISA 相关细分错误码建议。 |

## 依赖与 Rust feature 设计

### 依赖选择

VISA 依赖固定为 `visa-rs`。具体 patch 版本和其所依赖的底层实现（例如系统已安装的 VISA runtime）应在实际编码前复核，但本设计文档先锁定以下原则：

- `visa-rs` 只通过可选依赖进入 `Cargo.toml`
- 默认构建不启用 VISA
- 不允许因为启用 VISA 文档或工具 schema，而把 VISA runtime 变成 Serial/TCP/UDP 基础构建的硬前置

### 推荐 Cargo 结构

```toml
[features]
default = []
visa = ["dep:visa-rs"]

[dependencies]
visa-rs = { version = "<to-be-reviewed>", optional = true }
```

### feature 行为约定

| 场景 | 约定行为 |
| --- | --- |
| 未启用 `visa` feature | 所有 VISA 专属路径返回稳定错误，不影响 Serial/TCP/UDP。 |
| 启用 `visa` feature，但运行环境缺少底层 runtime / resource manager | 扫描、连接或 I/O 返回清晰错误，实例不应污染其他类型资源。 |
| 启用 `visa` feature 且底层 runtime 可用 | `port_scan(type=Visa)`、VISA 配置、连接和基础收发按本文契约工作。 |

### 设计建议

- `InstanceType::Visa` 建议作为稳定模型枚举保留，即使当前构建未启用 `visa` feature，也应能在工具层识别并返回“功能未编译”错误，而不是把 `Visa` 视为非法枚举值。
- `transport::visa` 建议采用 `cfg(feature = "visa")` 与兜底 stub 实现双轨方式：启用 feature 时链接真实 `visa-rs`，未启用时返回稳定错误。

## 模型扩展建议

### 实例类型

`instance_create` 的 `type` 建议从：

- `Serial`
- `TCP`
- `UDP`

扩展为：

- `Serial`
- `TCP`
- `UDP`
- `Visa`

### 资源摘要

VISA 资源摘要建议至少表达：

| 字段 | 说明 |
| --- | --- |
| `kind` | 固定为 `visa`。 |
| `display` | 脱敏后的 VISA 资源地址，例如 `TCPIP0::192.168.1.10::INSTR`、`USB0::0x0957::0x1798::MY1234567::INSTR`。 |
| `resource_class` | 可选，地址解析得到的资源族，如 `tcpip`、`usb`、`gpib`。 |

### VISA 配置模型

推荐新增：

```text
VisaConfig
- resource_address
- open_timeout_ms
- io_timeout_ms
- read_termination
- write_termination
- encoding
- query_idn_on_connect
```

说明：

- `resource_address`：VISA 资源地址，配置必填。
- `open_timeout_ms`：打开会话超时。
- `io_timeout_ms`：默认读写超时。
- `read_termination` / `write_termination`：基础文本协议终止符；用于 SCPI 一问一答场景，不做复杂协议解释。
- `encoding`：沿用现有 `text` / `hex` 方向，文本默认 `UTF-8`。
- query_idn_on_connect：基础能力阶段仅作为预留字段，不要求连接后识别或 warnings。

## 配置面决策

### 结论

本轮推荐 **新增 `visa_config`**，并把它明确写成“配置面唯一例外”，理由如下：

1. `serial_config` 语义已经固定为本机串口参数，不适合塞入 VISA 资源地址。
2. `tcp_udp_config` 语义已经固定为网络实例配置，若强行承载 VISA，会混淆 `host` / `port` 与标准 VISA 资源地址。
3. VISA 资源地址、终止符和 `query_idn_on_connect` 与现有配置字段差异较大，硬塞进现有配置面会降低文档可读性并增加实现歧义。

### 边界

- 允许新增 `visa_config`
- 不允许新增 `visa_connect`、`visa_send`、`visa_pull`、`visa_disconnect` 等平行运行期工具
- 文档中必须显式说明：`visa_config` 是配置入口例外，不是新工具族起点

## 工具契约扩展

本节延续 [03-初版工具契约](03-初版工具契约.md) 与 [24-MCP工具列表与调用收发详解](24-MCP工具列表与调用收发详解.md) 的写法。

### 通用约定补充

- VISA 句柄前缀建议为 `h_visa_001`。
- `instance_list`、`instance_query` 中的 `data.summary.type` 允许出现 `Visa`。
- 当 `visa` feature 未启用时，VISA 相关工具失败返回仍应沿用统一错误外形，不得退化成“工具不存在”。

### 轻量协议 helper 最终契约

这批 helper 已在代码中实现，属于 capability 2 的协议辅助工具，不是新的 VISA-only 工具族。它们仍然复用现有 `port_*` 抽象；VISA 本身继续只通过 `port_scan(type=Visa)`、`visa_config` 以及通用 `port_connect` / `port_send` / `port_pull` / `port_clear` 链路接入。

#### `str_to_hex`

用途：把 UTF-8 文本转换为十六进制字符串，供协议分帧、审计安全传输或二进制型 payload 使用。

入参：

| 字段 | 必填 | 说明 |
| --- | --- | --- |
| `input_string` | 是 | 待转换的文本。当前实现不提供单独编码选择器，默认按 UTF-8 处理。 |

出参：

| 字段 | 说明 |
| --- | --- |
| `hex` | 小写十六进制文本。 |
| `input_bytes` | 输入文本占用的字节数。 |

错误边界：

- 输入超限：`INVALID_RANGE`

#### `hex_to_str`

用途：把十六进制字符串还原为 UTF-8 文本。

入参：

| 字段 | 必填 | 说明 |
| --- | --- | --- |
| `hex` | 是 | 十六进制字符串。 |

出参：

| 字段 | 说明 |
| --- | --- |
| `text` | 解码后的 UTF-8 文本。 |
| `input_bytes` | 输入十六进制对应的字节数。 |

错误边界：

- 非法十六进制：`INVALID_HEX`
- 输入超限：`INVALID_RANGE`
- 字节序列无法解码为 UTF-8：`TEXT_ENCODING_FAILED`

#### `modbus_helper`

用途：提供 Modbus RTU 的帧打包/拆包基础能力。当前只覆盖 `rtu`；`ascii` 仅保留在枚举中，不作为可用实现路径。

入参：

| 字段 | 必填 | 说明 |
| --- | --- | --- |
| `action` | 是 | `pack` 或 `unpack`。 |
| `mode` | 是 | 当前仅支持 `rtu`。 |
| `slave_id` | pack 时是 | 从站地址。 |
| `function_code` | pack 时是 | 功能码。 |
| `address` | pack 时是 | 寄存器/线圈地址。 |
| `data_or_hex` | pack 时否 | pack 时追加到地址后的可选 hex 数据；unpack 不接受该字段作为帧输入。 |
| `frame_hex` | unpack 时是 | unpack 的完整 Modbus RTU 帧 hex 输入。 |
| `crc_check` | 否 | 是否做严格 CRC 校验；默认 `true`，坏 CRC 返回 `PROTOCOL_CHECKSUM_FAILED`；显式传 `false` 时返回 `checksum_valid=false` 作为宽松诊断。 |

出参：

- `pack`：`action`、`mode`、`frame_hex`、`frame_bytes`、`crc_hex`
- `unpack`：`action`、`mode`、`slave_id`、`function_code`、`address`、`data_hex`、`crc_hex`、`checksum_valid`

错误边界：

- 缺少必填字段：`MISSING_REQUIRED_FIELD`
- 非法十六进制：`INVALID_HEX`
- 帧结构不合法：`PROTOCOL_FRAME_INVALID`
- CRC 不匹配：`PROTOCOL_CHECKSUM_FAILED`

#### `scpi_helper`

用途：对 SCPI 命令做归一化和摘要，不做完整协议解析。

入参：

| 字段 | 必填 | 说明 |
| --- | --- | --- |
| `action` | 是 | 当前仅支持 `normalize`。 |
| `command` | 是 | SCPI 命令本体。 |
| `arguments` | 否 | 命令参数文本。 |
| `expect_response` | 否 | 对响应类型的预期提示。 |

出参：

| 字段 | 说明 |
| --- | --- |
| `kind` | 固定为 `scpi`。 |
| `normalized` | 归一化后的文本。 |
| `response_class` | 响应类别摘要。 |

#### `at_helper`

用途：对 AT 指令做基础分类和摘要，不做厂商专有语义扩展。

入参：

| 字段 | 必填 | 说明 |
| --- | --- | --- |
| `command` | 是 | AT 命令本体。 |

出参：

| 字段 | 说明 |
| --- | --- |
| `kind` | 固定为 `at`。 |
| `normalized` | 归一化后的文本。 |
| `response_class` | `basic` / `extended` / `custom`。 |

分类规则：

- `AT` → `basic`
- `AT+...` → `extended`
- 其他 → `custom`

#### `slip_helper`

用途：对 SLIP 帧做 encode/decode，输入和输出都以 hex payload 表示。

入参：

| 字段 | 必填 | 说明 |
| --- | --- | --- |
| `action` | 是 | `encode` 或 `decode`。 |
| `payload_hex` | 是 | 十六进制 payload。 |

出参：

- `encode`：`kind`、`normalized`、`payload_hex`
- `decode`：`kind`、`normalized`、`payload_hex`、`response_class`

错误边界：

- 非法十六进制：`INVALID_HEX`
- SLIP 帧或转义结构不合法：`PROTOCOL_FRAME_INVALID`

#### 与现有工具抽象的关系

- 这些 helper 只补充协议侧的“最小公共能力”，不替代 `port_send` / `port_pull`。
- `port_send` 继续负责实际写入，`port_pull` 继续负责实际读回；helper 只负责输入整形、摘要和帧处理。
- VISA 不需要专属 helper 工具族；VISA 的新增面仍然是 `port_scan(type=Visa)` 与 `visa_config`，再加上通用 `port_*` 链路在 VISA 实例上的实现。

### `instance_create`

用途：创建 Serial、TCP、UDP 或 Visa 实例，不打开底层资源。

VISA 入参增量：

| 字段 | 必填 | 说明 |
| --- | --- | --- |
| `type` | 是 | 允许 `Visa`。 |

VISA 成功返回重点：

| 字段 | 说明 |
| --- | --- |
| `handle_id` | 建议为 `h_visa_001` 形态。 |
| `state` | `Created`。 |
| `data.type` | `Visa`。 |

错误边界：

- 未启用 `visa` feature：建议返回 `InvalidState/FEATURE_NOT_COMPILED`
- `type` 非法：仍返回 `InvalidArgument/INVALID_ENUM_VALUE`

边界条件：

- 创建实例不触发底层 VISA runtime 探测
- 仅在后续 `port_scan(type=Visa)`、`visa_config` 或 `port_connect` 中接触底层依赖

### `visa_config`

用途：为 `Visa` 实例写入或覆盖 VISA 基础配置。

入参：

| 字段 | 必填 | 说明 |
| --- | --- | --- |
| `handle_id` | 是 | `Visa` 实例句柄。 |
| `resource_address` | 是 | VISA 资源地址。 |
| `open_timeout_ms` | 否 | open 超时；默认建议 `1000`。 |
| `io_timeout_ms` | 否 | 默认读写超时；默认建议 `1000`。 |
| `read_termination` | 否 | 文本读取终止符，例如 `\\n`。 |
| `write_termination` | 否 | 文本发送追加终止符，例如 `\\n`。 |
| `encoding` | 否 | `text` 或 `hex`，默认 `text`。 |
| query_idn_on_connect | 否 | 预留给后续非阻塞识别逻辑；基础能力阶段可忽略。 |

成功返回重点：

| 字段 | 说明 |
| --- | --- |
| `state` | `Configured`。 |
| `data.summary.config.kind` | `visa`。 |
| `data.summary.resource.kind` | `visa`。 |

状态要求：`Created`、`Configured`、`Disconnected`。

错误边界：

- 句柄不存在：`HandleNotFound/HANDLE_NOT_FOUND`
- 类型不匹配：`InvalidArgument/TYPE_MISMATCH`
- 状态不允许：`InvalidState/STATE_NOT_ALLOWED`
- 资源地址为空或格式明显非法：`InvalidArgument/INVALID_ADDRESS`
- 未启用 `visa` feature：`InvalidState/FEATURE_NOT_COMPILED`

边界条件：

- 配置阶段不强制打开真实 VISA session
- 若 `resource_address` 来自 `port_scan(type=Visa)` 结果，允许直接原样保存
- query_idn_on_connect 在基础能力阶段仅作为预留字段，不改变配置成功条件。

### `port_scan`

用途：按 `type` 选择串口、网络或 VISA 资源发现路径。

VISA 入参：

```json
{
  "type": "Visa",
  "config": {
    "resource_filter": "?*INSTR",
    "max_results": 128
  }
}
```

字段说明：

| 字段 | 必填 | 说明 |
| --- | --- | --- |
| `type` | 是 | `Visa`。 |
| `config.resource_filter` | 否 | 传递给 resource manager 的过滤模式；默认建议 `?*INSTR`。 |
| `config.max_results` | 否 | 返回上限，默认受全局结果上限控制。 |

成功返回重点：

```json
{
  "ok": true,
  "tool": "port_scan",
  "data": {
    "resources": [
      {
        "kind": "visa",
        "display": "TCPIP0::192.168.1.10::INSTR",
        "resource_class": "tcpip"
      }
    ]
  },
  "warnings": []
}
```

错误边界：

- 未启用 `visa` feature：`InvalidState/FEATURE_NOT_COMPILED`
- resource manager 不可用：`InvalidState/VISA_RUNTIME_UNAVAILABLE`
- 枚举失败：`WriteFailed/VISA_ENUM_FAILED`
- 过滤模式非法：`InvalidArgument/INVALID_ADDRESS`
- 返回结果超限：`BufferLimitExceeded/RESULT_TOO_LARGE`

边界条件：

- VISA 扫描不受 loopback-only 网络扫描规则约束，因为它依赖本机 VISA runtime 枚举，而非直接做 IP 范围探测
- 返回值仍应复用 `data.resources`，而不是为 VISA 新开独立顶层字段
- 返回的地址展示必须遵守脱敏规则，不暴露额外本地路径或驱动内部细节

### `port_connect`

用途：打开已配置实例；对于 VISA，即创建底层 resource manager 会话并 open 指定资源。

VISA 成功返回重点：

- `state` 进入 `Connected`
- 基础实现不要求返回识别摘要。
- 基础实现不要求把识别失败转换成 warnings。

错误边界：

- 未配置：`InvalidState/CONFIG_REQUIRED`
- 未启用 `visa` feature：`InvalidState/FEATURE_NOT_COMPILED`
- runtime / resource manager 缺失：`InvalidState/VISA_RUNTIME_UNAVAILABLE`
- 资源地址不存在或不可达：`InvalidArgument/VISA_RESOURCE_NOT_FOUND`
- 资源已被占用：`ResourceBusy/VISA_RESOURCE_BUSY`
- open 超时：`ConnectTimeout/VISA_OPEN_TIMEOUT`
- open 失败：`WriteFailed/VISA_OPEN_FAILED`

边界条件：

- 连接失败时必须按 [05-状态机与资源生命周期](05-状态机与资源生命周期.md) 收敛到一致的错误态或保留已定义状态
- 不允许绕过现有 registry / 生命周期流程直接持有裸会话

### `port_disconnect`

用途：断开已连接 VISA 实例，关闭底层会话，保留配置。

成功返回重点：

- 成功后通常进入 `Disconnected`

错误边界：

- 句柄不存在：`HandleNotFound/HANDLE_NOT_FOUND`
- 状态不允许：`InvalidState/STATE_NOT_ALLOWED`
- close 失败：`WriteFailed/DISCONNECT_FAILED`

边界条件：

- 行为应与 Serial/TCP/UDP 保持一致：断开只释放连接，不清除已保存配置

### `port_send`

用途：向已连接 VISA 实例发送一帧命令或原始 payload。

VISA 适配规则：

- `encoding=text`：适用于 SCPI 或文本命令
- `append_line_break=true`：若未单独指定 `write_termination`，可复用当前通用行为；若已配置 `write_termination`，应以配置优先
- `encoding=hex`：允许发送原始二进制 payload，不要求解释协议语义

成功返回重点：

- `state` 保持 `Connected`
- `data.queued`、`data.sent_bytes` 沿用现有定义

错误边界：

- 未连接：`InvalidState/STATE_NOT_ALLOWED`
- hex 非法：`InvalidArgument/INVALID_HEX`
- VISA 写超时：`ConnectTimeout/CONNECT_TIMEOUT` 或新增 `WriteFailed/VISA_WRITE_TIMEOUT`
- 底层写失败：`WriteFailed/VISA_WRITE_FAILED`
- 会话已关闭：`WriteFailed/TRANSPORT_CLOSED`

边界条件：

- 不增加 `visa_query` 之类同步问答工具；问答仍由 `port_send` + `port_pull` 组合完成
- 不替代用户的真实仪器手册，不对 SCPI 之外语义做解释

### `port_pull`

用途：从 VISA 实例接收缓冲区拉取数据，适用于 `*IDN?` 一类基础问答结果或原始返回 payload。

成功返回重点：

- `data.payload`、`data.encoding`、`data.received_bytes`、`data.remaining_rx_buffer_bytes`、`data.truncated` 沿用现有定义

错误边界：

- 未连接：`InvalidState/STATE_NOT_ALLOWED`
- 读超时：`ReadTimeout/READ_TIMEOUT`
- 无数据：`ReadTimeout/NO_DATA_AVAILABLE`
- pull 超限：`BufferLimitExceeded/PULL_MAX_BYTES_EXCEEDED`
- 底层读失败：`WriteFailed/READ_IO_FAILED` 或新增 `WriteFailed/VISA_READ_FAILED`

边界条件：

- `port_pull` 仍是消耗式读取
- 终止符仅作为基础文本读取协助，不引入复杂解析器

### `port_clear`

用途：清理 VISA 实例的发送队列、接收缓冲区或两者。

成功返回重点：

- `data.cleared` 语义不变

边界条件：

- 行为与现有实例类型保持一致
- `target=rx` 时只清理本地缓冲，不隐式向仪器发送清除指令

### `instance_query` / `instance_list`

用途：查询 VISA 实例摘要和已保存配置。

VISA 可见字段建议至少包括：

| 字段 | 说明 |
| --- | --- |
| `summary.type` | `Visa` |
| `summary.resource.kind` | `visa` |
| `summary.resource.display` | VISA 资源地址摘要 |
| `summary.config.kind` | `visa` |
| `summary.config.config.resource_address` | 已保存资源地址 |
| `summary.last_error` | 最近 VISA 错误摘要 |

基础实现不要求在 summary 下增加识别摘要。

## 错误模型扩展建议

本文建议在 [07-错误模型与返回格式](07-错误模型与返回格式.md) 与 `src/model/error.rs` 中为 VISA 预留以下细分错误码；分类沿用现有类别：

| category | code | 场景 |
| --- | --- | --- |
| `InvalidState` | `FEATURE_NOT_COMPILED` | 未启用 `visa` feature 却调用 VISA 路径。 |
| `InvalidState` | `VISA_RUNTIME_UNAVAILABLE` | 已编译但底层 resource manager / runtime 不可用。 |
| `InvalidArgument` | `VISA_RESOURCE_NOT_FOUND` | 资源地址不存在或不可解析。 |
| `ResourceBusy` | `VISA_RESOURCE_BUSY` | 资源已被占用。 |
| `ConnectTimeout` | `VISA_OPEN_TIMEOUT` | open 会话超时。 |
| `WriteFailed` | `VISA_ENUM_FAILED` | 资源枚举失败。 |
| `WriteFailed` | `VISA_OPEN_FAILED` | 打开资源失败。 |
| `WriteFailed` | `VISA_WRITE_FAILED` | 写入失败。 |
| `WriteFailed` | `VISA_READ_FAILED` | 读取失败。 |
| WriteFailed | VISA_QUERY_IDN_FAILED | 预留给后续非阻塞识别路径。 |

### 错误表达原则

- 基础实现不要求在 port_connect 中执行 *IDN?。
- `feature` 未启用与 runtime 缺失必须区分；前者是构建能力缺失，后者是运行环境缺失。
- VISA 错误 details 仍应遵守脱敏规则，不直接回传底层长错误链或未裁剪驱动文本。

## 推荐调用序列

### VISA 基础序列

```text
instance_create(type=Visa)
port_scan(type=Visa)
visa_config(handle_id, resource_address=TCPIP0::192.168.1.10::INSTR)
port_connect(handle_id)
port_send(handle_id, data=*IDN?, encoding=text, append_line_break=true)
port_pull(handle_id, max_bytes=256)
port_disconnect(handle_id)
instance_release(handle_id)
```

### 已知资源直连序列

```text
instance_create(type=Visa)
visa_config(handle_id, resource_address=USB0::0x0957::0x1798::MY1234567::INSTR)
port_connect(handle_id)
port_send(handle_id, data=MEAS:VOLT:DC?, encoding=text, append_line_break=true)
port_pull(handle_id, max_bytes=256)
```

## 实施切片建议

承接 [25-进阶能力施工拆分与验收门槛](25-进阶能力施工拆分与验收门槛.md) 的 V1-V5，建议文档后的实现顺序为：

1. **V1 资源枚举**：先打通 `port_scan(type=Visa)` 与 `data.resources` 返回。
2. **V2 配置落地**：新增 `VisaConfig` 与 `visa_config` 契约。
3. **V3 连接**：在 port_connect 中接入 open 与基础连接流程。
4. **V4 基础收发**：复用 `port_send` / `port_pull`。
5. **V5 错误退化**：补齐 feature 未启用、runtime 缺失、资源占用、超时等错误路径。

## 验收门槛

满足以下条件时，27 号文档可视为设计完成：

- 已明确 `visa-rs` 与 Rust `feature` 方案
- 已明确 VISA 在当前分层与模块中的触点
- 已明确 `instance_create(type=Visa)`、`port_scan(type=Visa)`、`visa_config`、`port_connect`、`port_send`、`port_pull`、`port_clear`、`instance_query/list` 的契约
- 已明确 feature 未启用、runtime 缺失、枚举失败、连接失败、读写失败和超时的退化规则
- 已明确仅覆盖基础能力，不扩展高级仪器特性

## 与其他文档的边界

- 若后续实现开始落地，具体施工证据应另起施工记录文档，不回写到本文。
- 若错误码最终进入代码，应同步更新 [07-错误模型与返回格式](07-错误模型与返回格式.md)。
- 若实际实现证明 `visa_config` 仍可被更自然地吸收到统一配置面，必须先更新本文，再调整实现方向。
