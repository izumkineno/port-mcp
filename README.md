# port-mcp

![Rust](https://img.shields.io/badge/Rust-2024%20edition-000000?logo=rust)
![MCP](https://img.shields.io/badge/MCP-stdio%20server-2F80ED)
![Status](https://img.shields.io/badge/status-initial%20scope%20implemented-2D9C5A)

Rust 原生 `Model Context Protocol` 端口调试服务，面向串口、TCP、UDP、VISA 仪器调试场景，提供统一的实例管理、连接配置、资源扫描、受控设备探测、收发、缓冲区读取和最小流式订阅能力。

这个仓库的目标不是做一个泛化设备平台，而是让 MCP 客户端能够以稳定、可审计、可验证的方式操作端口连接，并把调试过程沉淀为后续可复用的记录与文档。

## 当前能做什么

当前 Rust `port-mcp` 已实现初版能力：

- 创建 `Serial`、`TCP`、`UDP`、`Visa` 四类实例
- 配置串口参数、TCP/UDP 连接参数与 VISA resource address
- 扫描串口、TCP/UDP loopback 端口与 VISA 资源
- 使用 `device_probe` 对 Serial/VISA 资源执行受控写入、读取和响应匹配，返回成功配置合集
- 连接、断开、释放实例
- 发送 `text` 或 `hex` payload
- 从接收缓冲区拉取数据摘要
- 清理 `tx`、`rx` 或全部缓冲
- 订阅实例接收流的最小广播通知
- 使用 `str_to_hex`、`hex_to_str`、`modbus_helper`、`scpi_helper`、`at_helper`、`slip_helper` 进行轻量编码与协议辅助
- 通过 `usage_guide` 获取机器可读调用指引，通过 `debug_log_config` 控制原始 I/O 日志预览
- 通过结构化错误返回、资源锁和状态机约束保证行为可诊断

当前 stdio MCP server 暴露 22 个 MCP 工具。当前工具面、实现状态和验证状态见 [01-当前工具与实现状态](docs/01-当前工具与实现状态.md)；1.0.0 历史设计和调用资料见 [1.0.0 文档归档](docs/1.0.0文档归档/00-索引.md)。

## 项目定位

`port-mcp` 更偏向一个可被 AI 和开发者共同调用的“端口调试基础设施”，而不是一个完整的协议平台或设备管理平台。

当前仓库重点是：

- Rust 原生 MCP server
- Serial / TCP / UDP / VISA 的统一运行时与受控 Serial/VISA 探测
- 明确的状态机、资源生命周期和并发语义
- 可自动化验证的无硬件路径
- Windows 串口手工验收兜底

当前仓库暂不包含这些进阶能力：

- VISA 之外的专用 USBTMC / GPIB 高级封装
- 高级流式订阅触发条件
- 自动重连、心跳、异常注入
- 非 loopback 的网络扫描扩展

后续进阶待做项见 [02-进阶实现 TODO](docs/02-进阶实现TODO.md)；1.0.0 设计、进阶规划和施工记录已经归档，见 [1.0.0 文档归档](docs/1.0.0文档归档/00-索引.md)。

## 快速开始

### 1. 环境要求

- Rust stable toolchain
- Windows 优先验证；Linux / macOS 可做补充验证
- 若要做真实串口验证，需要本机可用串口设备、USB-TTL 回环或虚拟串口对
- 若要做真实 VISA 验证，需要本机安装并配置可用的 VISA runtime / 驱动与可访问的仪器资源

### 2. 构建

```powershell
cargo check
```

### 3. 启动 MCP server

```powershell
cargo run
```

服务以 stdio 方式运行，适合由 MCP 客户端直接拉起。

### 4. 最小验证

```powershell
cargo test
```

当前自动化验证优先覆盖无硬件路径，包括：

- 单元测试
- Mock transport
- TCP/UDP loopback
- VISA 类型、配置、资源锁与无硬件错误路径
- `device_probe` 参数校验、matcher 与默认 VISA fallback
- 轻量编码与协议 helper
- 错误模型
- MCP smoke

当前验证摘要见 [01-当前工具与实现状态](docs/01-当前工具与实现状态.md)。历史 M8 验收记录保存在 [1.0.0 文档归档](docs/1.0.0文档归档/00-索引.md)。

## MCP 客户端接入

开发期可以直接让 MCP 客户端通过 stdio 拉起：

```json
{
  "name": "port-mcp",
  "command": "cargo",
  "args": ["run"],
  "cwd": "<repo-root>",
  "transport": "stdio",
  "env": {
    "PORT_MCP_LOG": "info"
  }
}
```

更完整的运行口径、容量限制和日志配置可从 [1.0.0 文档归档](docs/1.0.0文档归档/00-索引.md) 中追溯；当前主要能力摘要见 [01-当前工具与实现状态](docs/01-当前工具与实现状态.md)。

## 当前工具分组

当前 MCP 工具分为 8 组：

- 使用指引：`usage_guide`
- 实例管理：`instance_create`、`instance_list`、`instance_query`、`instance_use`、`instance_release`
- 连接配置：`serial_config`、`tcp_udp_config`、`visa_config`
- 端口行为：`port_scan`、`device_probe`、`port_connect`、`port_disconnect`、`port_send`、`port_pull`、`port_clear`
- 最小流式订阅：`port_subscribe_stream`、`port_unsubscribe_stream`
- 调试日志：`debug_log_config`
- 编码辅助：`str_to_hex`、`hex_to_str`
- 协议辅助：`modbus_helper`、`scpi_helper`、`at_helper`、`slip_helper`

其中 `port_scan` 支持：

- `type=Serial`：扫描本机串口
- `type=Visa`：按 `resource_filter` 和 `max_results` 扫描 VISA 资源，例如 `?*INSTR`
- `type=TCP` / `type=UDP`：按 loopback host 与端口范围扫描开放端口

其中 `device_probe` 支持：

- `targets=["Serial"]`：扫描或指定串口资源，按候选串口配置发送 payload 并匹配响应
- `targets=["Visa"]`：扫描或指定 VISA resource，启用 `visa` feature 后执行同样的写读匹配；默认 feature 下返回结构化 unsupported/family error
- 匹配器支持 `contains`、`hex_contains`、`regex` 和 `any_response`；`regex` 使用 Rust `regex` 引擎，受 pattern 长度和响应字节上限约束，不支持 PCRE lookaround/backreference
- 失败输出通过 `failure_output` 控制：默认 `counts` 只返回 `summary.failure_status_counts` / `summary.failure_error_counts`，需要明细时可设为 `samples`

如果你需要：

- 当前工具分组
- 当前实现状态
- 当前验证状态

直接看 [01-当前工具与实现状态](docs/01-当前工具与实现状态.md)。历史调用详解保存在 [1.0.0 文档归档](docs/1.0.0文档归档/00-索引.md)。

## 验证状态

当前仓库已有自动化验证证据：

- `cargo fmt --check`
- `cargo check`
- `cargo test`
- TCP/UDP loopback e2e
- helper 单元测试

当前默认 feature 下最近一次 `rtk cargo test` 结果为 104 个 Rust 测试通过。验证摘要见 [01-当前工具与实现状态](docs/01-当前工具与实现状态.md)。

## 仓库结构

```text
src/
  app/
  mcp/
  model/
  runtime/
  transport/
  util/

docs/
  00-索引.md
  01-当前工具与实现状态.md
  02-进阶实现TODO.md
  1.0.0文档归档/
```

如果你想从源码理解项目，建议先读：

1. [00-索引](docs/00-索引.md)
2. [01-当前工具与实现状态](docs/01-当前工具与实现状态.md)
3. [02-进阶实现 TODO](docs/02-进阶实现TODO.md)
4. [1.0.0 文档归档](docs/1.0.0文档归档/00-索引.md)

## 文档导航

按用途分，推荐这样看：

- 想知道项目做什么：
  - [00-索引](docs/00-索引.md)
  - [01-当前工具与实现状态](docs/01-当前工具与实现状态.md)

- 想知道当前工具怎么调用：
  - [01-当前工具与实现状态](docs/01-当前工具与实现状态.md)
  - [1.0.0 文档归档](docs/1.0.0文档归档/00-索引.md)

- 想知道怎么运行和验证：
  - [01-当前工具与实现状态](docs/01-当前工具与实现状态.md)
  - [1.0.0 文档归档](docs/1.0.0文档归档/00-索引.md)

- 想知道 GitHub CI、版本更替和发布自动化：
  - [1.0.0 文档归档](docs/1.0.0文档归档/00-索引.md)

- 想知道后续进阶能力怎么落地：
  - [02-进阶实现 TODO](docs/02-进阶实现TODO.md)
  - [1.0.0 文档归档](docs/1.0.0文档归档/00-索引.md)

## 开发与贡献

如果你准备继续开发这个仓库，建议先做这几件事：

1. 跑一遍 `cargo check` 和 `cargo test`
2. 阅读 [01-当前工具与实现状态](docs/01-当前工具与实现状态.md)
3. 若要做进阶能力，先阅读 [02-进阶实现 TODO](docs/02-进阶实现TODO.md)
4. 按需追溯 [1.0.0 文档归档](docs/1.0.0文档归档/00-索引.md)
5. 修改实现前，先确认对应设计文档是否已经覆盖该行为

这个仓库的约束比较明确：如果实现与设计冲突，优先回写文档，再改代码。
