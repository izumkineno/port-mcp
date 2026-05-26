# port-mcp

![Rust](https://img.shields.io/badge/Rust-2024%20edition-000000?logo=rust)
![MCP](https://img.shields.io/badge/MCP-stdio%20server-2F80ED)
![Status](https://img.shields.io/badge/status-initial%20scope%20implemented-2D9C5A)

Rust 原生 `Model Context Protocol` 端口调试服务，面向串口、TCP、UDP 调试场景，提供统一的实例管理、连接配置、收发、缓冲区读取和最小流式订阅能力。

这个仓库的目标不是做一个泛化设备平台，而是让 MCP 客户端能够以稳定、可审计、可验证的方式操作端口连接，并把调试过程沉淀为后续可复用的记录与文档。

## 当前能做什么

当前 Rust `port-mcp` 已实现初版能力：

- 创建 `Serial`、`TCP`、`UDP` 三类实例
- 配置串口参数与 TCP/UDP 连接参数
- 连接、断开、释放实例
- 发送 `text` 或 `hex` payload
- 从接收缓冲区拉取数据摘要
- 清理 `tx`、`rx` 或全部缓冲
- 订阅实例接收流的最小广播通知
- 通过结构化错误返回、资源锁和状态机约束保证行为可诊断

当前 stdio MCP server 暴露 15 个初版工具，详细列表见 [24-MCP工具列表与调用收发详解](docs/24-MCP工具列表与调用收发详解.md)。

## 项目定位

`port-mcp` 更偏向一个可被 AI 和开发者共同调用的“端口调试基础设施”，而不是一个完整的协议平台或设备管理平台。

当前仓库重点是：

- Rust 原生 MCP server
- Serial / TCP / UDP 的统一运行时
- 明确的状态机、资源生命周期和并发语义
- 可自动化验证的无硬件路径
- Windows 串口手工验收兜底

当前仓库不包含这些已实现能力：

- VISA / USBTMC / GPIB
- Modbus / SCPI / AT / SLIP helper
- 高级流式订阅触发条件
- 自动重连、心跳、异常注入
- 非 loopback 的网络扫描扩展

这些内容已进入进阶规划，见 [02-进阶实现大纲](docs/02-进阶实现大纲.md) 和 [25-进阶能力施工拆分与验收门槛](docs/25-进阶能力施工拆分与验收门槛.md)。

## 快速开始

### 1. 环境要求

- Rust stable toolchain
- Windows 优先验证；Linux / macOS 可做补充验证
- 若要做真实串口验证，需要本机可用串口设备、USB-TTL 回环或虚拟串口对

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
- 错误模型
- MCP smoke

M8 验收记录见 [2026-05-26-M8-final-acceptance.md](docs/acceptance/2026-05-26-M8-final-acceptance.md)。

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

更完整的运行口径、容量限制和日志配置见 [11-配置与运行说明](docs/11-配置与运行说明.md)。

## 当前工具分组

初版工具分为 4 组：

- 实例管理：`instance_create`、`instance_list`、`instance_query`、`instance_use`、`instance_release`
- 连接配置：`serial_config`、`tcp_udp_config`
- 端口行为：`port_scan`、`port_connect`、`port_disconnect`、`port_send`、`port_pull`、`port_clear`
- 最小流式订阅：`port_subscribe_stream`、`port_unsubscribe_stream`

如果你需要：

- 每个工具的入参和返回外形
- 示例调用片段
- 错误边界与日志字段

直接看 [24-MCP工具列表与调用收发详解](docs/24-MCP工具列表与调用收发详解.md)。

## 验证状态

当前仓库已有初版验收证据：

- `cargo fmt --check`
- `cargo check`
- `cargo test`
- Windows 自动化验证
- Windows 串口手工验收记录

对应记录见 [2026-05-26-M8-final-acceptance.md](docs/acceptance/2026-05-26-M8-final-acceptance.md)。

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
  01-25 设计 / 施工 / 验收文档
  acceptance/
```

如果你想从源码理解项目，建议先读：

1. [00-索引](docs/00-索引.md)
2. [01-初版工程功能设计大纲](docs/01-初版工程功能设计大纲.md)
3. [03-初版工具契约](docs/03-初版工具契约.md)
4. [11-配置与运行说明](docs/11-配置与运行说明.md)
5. [24-MCP工具列表与调用收发详解](docs/24-MCP工具列表与调用收发详解.md)

## 文档导航

按用途分，推荐这样看：

- 想知道项目做什么：
  - [01-初版工程功能设计大纲](docs/01-初版工程功能设计大纲.md)
  - [02-进阶实现大纲](docs/02-进阶实现大纲.md)

- 想知道当前工具怎么调用：
  - [24-MCP工具列表与调用收发详解](docs/24-MCP工具列表与调用收发详解.md)

- 想知道怎么运行和验证：
  - [11-配置与运行说明](docs/11-配置与运行说明.md)
  - [08-测试与验收计划](docs/08-测试与验收计划.md)
  - [2026-05-26-M8-final-acceptance](docs/acceptance/2026-05-26-M8-final-acceptance.md)

- 想知道后续进阶能力怎么落地：
  - [02-进阶实现大纲](docs/02-进阶实现大纲.md)
  - [25-进阶能力施工拆分与验收门槛](docs/25-进阶能力施工拆分与验收门槛.md)

## 开发与贡献

如果你准备继续开发这个仓库，建议先做这几件事：

1. 跑一遍 `cargo check` 和 `cargo test`
2. 阅读 [13-代码施工大纲](docs/13-代码施工大纲.md)
3. 阅读 [17-初版施工任务拆分清单](docs/17-初版施工任务拆分清单.md)
4. 修改实现前，先确认对应设计文档是否已经覆盖该行为

这个仓库的约束比较明确：如果实现与设计冲突，优先回写文档，再改代码。
