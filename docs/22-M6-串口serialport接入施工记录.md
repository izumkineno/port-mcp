# 22 M6 串口 serialport 接入施工记录

本文记录 M6 串口 `serialport` 接入阶段的施工结果。M6 只落地串口扫描摘要、`SerialConfig` 到 `serialport` 参数映射、受控阻塞 worker、串口错误映射和 Windows 手工验收记录路径；不进入 M7 MCP 全工具 handler 接入或 M8 发布前验收。

## 范围

| 项目 | 结果 |
| --- | --- |
| 自动化串口能力 | 不依赖真实硬件；默认 `cargo test` 可在无串口设备环境运行。 |
| 手工串口能力 | 已准备 Windows 验收记录；`COM4` 与 `COM5` 双向收发通过。 |
| 未进入能力 | MCP handler 全工具接入、发布前验收、进阶协议 helper。 |
| 仓库边界 | 未修改 `mcp-server/`。 |

## 已完成任务

| 任务 | 状态 | 完成证据 |
| --- | --- | --- |
| M6-01 串口扫描摘要 | Done | `scan_serial_ports` 返回脱敏 `SerialPortSummary`；`cargo test unit_serial_scan`。 |
| M6-02 serial_config 参数映射 | Done | `SerialPortSettings::try_from_config` 映射 baudrate、data bits、stop bits、parity、flow control、timeout；`cargo test unit_serial_config`。 |
| M6-03 串口阻塞 worker | Done | `SerialWorker` 使用专用线程和控制消息，测试用 scripted device 验证读、写、关闭；`cargo test unit_serial_worker`。 |
| M6-04 串口错误映射 | Done | `map_serial_error`/`map_serial_io_error` 将不存在、权限/占用、打开超时、读写失败映射为稳定 category/code；`cargo test unit_serial_errors`。 |
| M6-05 Windows 手工验收记录模板 | Done | [acceptance/2026-05-26-M6-serial-windows验收记录](acceptance/2026-05-26-M6-serial-windows验收记录.md)。 |
| M6-06 Windows 串口手工验收 | Done | 系统枚举到 `COM1`、`COM4`、`COM5`；`COM4 -> COM5` 与 `COM5 -> COM4` 双向发送均收到完整 payload。 |

## 代码落点

| 文件 | 内容 |
| --- | --- |
| `src/transport/mod.rs` | 串口扫描摘要、serialport 参数映射、`SerialWorker`、scripted device 测试辅助、串口错误映射和 M6 单元测试。 |
| `docs/acceptance/2026-05-26-M6-serial-windows验收记录.md` | Windows 手工验收模板与本次未通过/未确认的设备证据。 |

本轮没有拆分 `transport::serial` 独立文件。串口实现目前仍在 `transport` 模块内，因为 M6 只需要底层封装和测试；后续进入真实 runtime connect task 或 MCP handler 前，再按 `transport/serial`、`runtime/tasks`、`app/port_service` 拆分更有收益。

## 验证记录

| 命令或检查 | 结果 |
| --- | --- |
| `cargo test unit_serial_scan` | 通过。 |
| `cargo test unit_serial_config` | 通过。 |
| `cargo test unit_serial_worker` | 通过。 |
| `cargo test unit_serial_errors` | 通过。 |
| `[System.IO.Ports.SerialPort]::GetPortNames()` | 枚举到 `COM1`、`COM4`、`COM5`。 |
| COM4/COM5 回环尝试 | 通过：`COM4 -> COM5` 发送 `m6-com4-to-com5` 并收到同值；`COM5 -> COM4` 发送 `m6-com5-to-com4` 并收到同值。 |
| `cargo fmt` | 通过，无输出。 |
| `cargo check` | 通过。 |
| `cargo test` | 37 passed。 |
| VS Code Problems / `get_errors` | No errors found。 |
| SDK 边界搜索 `rmcp` | 仅出现在 `src/main.rs` 和 `src/mcp/*`。 |

## 已知限制

- M6 的 `SerialWorker` 已具备受控线程和控制消息，但尚未接入 `RuntimeRegistry` 的真实 connect/disconnect 流程；M7 前仍不注册 MCP handler 全工具路径。
- M6-06 已在当前 Windows 环境通过 `COM4`/`COM5` 互联串口对完成基础双向读写验收；M7 后仍需通过 MCP 工具端重新验证完整工具链。
- 串口错误返回默认使用稳定 category/code 和脱敏摘要，不断言外部库原始错误文本。