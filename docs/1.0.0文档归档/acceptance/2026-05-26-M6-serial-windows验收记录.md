# M6 Windows 串口手工验收记录

- 日期：2026-05-26
- 平台：Windows
- 阶段：M6 串口 serialport 接入
- 自动化测试：通过，`cargo test` 为 37 passed
- 串口手工验收：通过
- 结论：`COM4` 与 `COM5` 已验证为互联串口对，双向收发均收到完整 payload；M6-06 可标记为通过。

## 设备准备

| 字段 | 记录 |
| --- | --- |
| 方案 | 虚拟串口对或互联串口对。 |
| 串口名 | `COM4`、`COM5`；同时枚举到 `COM1`。 |
| 驱动或工具 | PowerShell `[System.IO.Ports.SerialPort]::GetPortNames()`；.NET `SerialPort` 双向读写。 |
| 备注 | `COM4 -> COM5` 与 `COM5 -> COM4` 均收到完整 payload；默认自动化测试仍不得依赖真实串口硬件。 |

## 自动化覆盖

| 项目 | 结果 |
| --- | --- |
| 串口扫描摘要 | 由 `cargo test unit_serial_scan` 覆盖；无硬件环境允许返回空列表。 |
| serial_config 参数映射 | 由 `cargo test unit_serial_config` 覆盖。 |
| 串口阻塞 worker 控制消息 | 由 `cargo test unit_serial_worker` 使用 scripted device 覆盖。 |
| 串口错误映射 | 由 `cargo test unit_serial_errors` 覆盖。 |

## 手工验收步骤

| 步骤 | 目标 | 状态 | 证据 |
| --- | --- | --- | --- |
| 枚举串口 | 能看到目标串口名，例如 `COM3`。 | 已执行 | 枚举到 `COM1`、`COM4`、`COM5`。 |
| 连接串口 | `serial_config` 后连接，状态进入 Connected。 | 部分执行 | 通过 .NET `SerialPort` 成功打开 `COM4` 和 `COM5`；M7 MCP 工具接入前暂不执行工具端状态验证。 |
| 回环收发 | text 和 hex 发送后能读取预期字节。 | 已执行 | `COM4 -> COM5` 发送 `m6-com4-to-com5`，收到 `m6-com4-to-com5`，15 bytes，passed=true；`COM5 -> COM4` 发送 `m6-com5-to-com4`，收到 `m6-com5-to-com4`，15 bytes，passed=true。 |
| 独占冲突 | 同一串口第二个连接返回 `SERIAL_PORT_BUSY`。 | 未执行 | M6 自动化覆盖错误映射；工具端独占冲突留到 M7 handler 接入后验证。 |
| 错误路径 | 不存在、权限不足或占用返回结构化错误。 | 部分执行 | 自动化覆盖错误映射；未额外制造权限不足场景。 |
| 断开释放 | 释放后同串口可重新连接。 | 部分执行 | 每次读写后关闭并释放 .NET `SerialPort`；M7 工具端 release 语义留待 handler 接入后验证。 |

## 阻塞与已知限制

- M6-06 已通过互联串口对的基础读写验收：`COM4 -> COM5` 和 `COM5 -> COM4` 均成功。
- 本记录只证明 M6 手工验收路径已准备，不替代发布前真实硬件验收。
- M6 尚不进入 MCP handler 全工具接入，工具端连接/收发手工调用留到 M7 后结合真实设备复测。
