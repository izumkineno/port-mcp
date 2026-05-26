# 18 TypeScript MCP server 边界说明

本文档承接 [04-运行时架构设计](04-运行时架构设计.md)、[10-开发里程碑](10-开发里程碑.md)、[13-代码施工大纲](13-代码施工大纲.md) 与 [17-初版施工任务拆分清单](17-初版施工任务拆分清单.md)，明确仓库中 `mcp-server/` TypeScript 工程与本项目初版 Rust `port-mcp` 目标之间的边界。

本文档是仓库边界说明，不重新定义 [03-初版工具契约](03-初版工具契约.md)、[05-状态机与资源生命周期](05-状态机与资源生命周期.md)、[06-并发与缓冲区设计](06-并发与缓冲区设计.md)、[07-错误模型与返回格式](07-错误模型与返回格式.md) 或 [08-测试与验收计划](08-测试与验收计划.md) 已经确定的行为。后续施工中如果需要改变 Rust 初版目标或复用 TypeScript 工程能力，应先回写本文和相关上游文档，再继续实现。

## 边界结论

`mcp-server/` 是现有 TypeScript/OMG workflow MCP server，不属于端口 MCP 初版 Rust 工程的交付对象、里程碑产物或验收依据。

| 项目 | 结论 | 说明 |
| --- | --- | --- |
| 初版交付对象 | Rust `port-mcp` | 以 `Cargo.toml`、`src/` 和 04/10/13/17 定义的 Rust 模块与里程碑为准。 |
| TypeScript 目录性质 | 现有 OMG workflow server | `mcp-server/package.json` 的 `name` 为 `omg-mcp-server`，描述为 workflow state、PRD、memory management。 |
| 协议壳选择 | Rust 原生 MCP server | 04 已明确初版不再以 TypeScript 作为协议壳。 |
| 施工覆盖范围 | 不覆盖 `mcp-server/` | 13 已明确 TypeScript MCP server 不作为本施工大纲执行对象。 |
| 验收计分 | 不计入 Rust 初版完成度 | TypeScript build/test 通过不能替代 `cargo check`、`cargo test`、MCP smoke 或 08 的验收矩阵。 |
| 发布产物 | 默认隔离 | Rust port-mcp 发布不应默认包含或依赖 TypeScript `dist/`、`node_modules/` 或 npm 包。 |

## 目录归属

| 路径 | 归属 | 初版施工处理 |
| --- | --- | --- |
| `Cargo.toml` | Rust port-mcp crate | M0 起写入 Rust 依赖基线，是初版施工入口。 |
| `src/` | Rust port-mcp 源码 | 04/13/17 的模块边界和任务清单均落在此目录。 |
| `docs/` | port-mcp 设计、施工和验收文档 | 记录 Rust 初版目标、施工证据、阶段 gate 和仓库边界。 |
| `mcp-server/` | TypeScript OMG workflow MCP server | 不作为 Rust port-mcp 初版施工目录；除非任务明确要求维护 OMG server，否则不修改。 |
| `target/` | Rust 构建输出 | 不作为源码或文档证据提交依据。 |

## 可参考与不可借用

TypeScript 工程可以作为仓库背景事实，但不能作为 Rust 初版行为的替代实现。

| 类别 | 是否允许 | 规则 |
| --- | --- | --- |
| MCP 基本概念 | 可参考 | 可参考工具注册、stdio server、状态类工具的组织经验，但 Rust 具体实现以 `rmcp` 和 14 的 SDK gate 为准。 |
| workflow state、PRD、memory 语义 | 不作为初版功能 | 这些属于 OMG workflow server，不属于端口 MCP 的 Serial/TCP/UDP/Mock 工具契约。 |
| TypeScript SDK API | 不直接迁移 | 不应把 `@modelcontextprotocol/sdk` 的类型、错误外形或运行模型当作 Rust 层 API 设计来源。 |
| 测试组织经验 | 可参考 | 可参考 smoke/integration 的组织方式，但验收命令必须落到 Rust `cargo`、MCP smoke 和 08 矩阵。 |
| 发布脚本 | 不复用为 Rust 发布依据 | npm build/start/test 不能证明 Rust port-mcp 可发布。 |

## 施工隔离规则

| 场景 | 正确处理 | 禁止事项 |
| --- | --- | --- |
| M0 依赖施工 | 修改 `Cargo.toml`、`Cargo.lock`、`src/` 和 15 记录。 | 为了让 M0 通过而改 `mcp-server/package.json`。 |
| M1-M7 功能施工 | 按 17 的任务 ID 在 Rust 模块中推进。 | 把 TypeScript server 的已有工具当作 Rust 工具完成证据。 |
| 文档更新 | 在 00、14-18 或 acceptance 记录中说明实际边界。 | 在文档中暗示 `mcp-server/` 已经实现端口 MCP 初版能力。 |
| 修 OMG workflow server | 只有用户明确要求维护 OMG server 时进入 `mcp-server/`。 | 在 port-mcp 初版任务中顺手重构 TypeScript workflow server。 |
| 发现边界冲突 | 先回写本文、04、10、13 或 17，再调整施工计划。 | 一边保留 Rust 初版目标，一边让 TypeScript server 承担运行时职责。 |

## CI 与测试隔离

初版 Rust port-mcp 的验证以 Rust 工程命令和 08 的验收矩阵为准。TypeScript 工程的 npm 命令只验证 OMG workflow server 本身。

| 验证项 | Rust port-mcp 口径 | TypeScript `mcp-server/` 口径 |
| --- | --- | --- |
| 构建检查 | `cargo check` | `npm run build` 只验证 OMG server。 |
| 单元/集成测试 | `cargo test` 及后续命名测试 | `npm test` 不计入 port-mcp 初版完成度。 |
| MCP 端到端 smoke | M7 后以 Rust stdio server 和 03 工具契约为准。 | 不能替代 Rust MCP smoke。 |
| 无硬件 CI | MockTransport、TCP/UDP loopback、错误模型和并发测试。 | 不证明 Serial/TCP/UDP/Mock port-mcp runtime。 |
| 手工硬件验收 | Windows 串口验收记录或 acceptance 文件。 | 与 TypeScript workflow server 无关。 |

如果未来 CI 同时覆盖 Rust 和 TypeScript，应在 job 名称、触发条件和失败说明中明确区分：Rust job 用于 port-mcp 初版 gate，TypeScript job 用于 OMG workflow server 回归。

Rust port-mcp 发布 gate 必须 fail closed：若 Rust job 缺失、被跳过，或未产出 `cargo check`、`cargo test`、MCP smoke 和必要 acceptance 证据，不能因为 TypeScript job 通过而判定 Rust 初版可发布。

## 发布隔离

Rust port-mcp 发布前检查应只以 Rust 初版产物为主，不默认打包 TypeScript 工程。

| 发布问题 | 规则 |
| --- | --- |
| Rust binary 是否可发布 | 以 `cargo check`、`cargo test`、MCP smoke、08/12/acceptance 证据判断。 |
| TypeScript dist 是否存在 | 不影响 Rust port-mcp 发布判定。 |
| npm package 是否可发布 | 不属于 port-mcp 初版发布范围。 |
| README 或客户端配置 | 必须明确启动的是 Rust port-mcp 还是 OMG workflow server。 |
| 版本号 | Rust crate 与 TypeScript server 不应默认共用版本语义。 |

## 验收口径

Rust port-mcp 的阶段 gate 以 10、15、16、17 和 08 为准。`mcp-server/` 的存在不能让任何阶段自动完成。

| 阶段 | 是否可由 TypeScript server 证明 | 说明 |
| --- | --- | --- |
| M0 工程骨架 | 不可 | 必须由 Rust 依赖、模块、薄入口、SDK spike 和 `cargo check` 证明。 |
| M1 模型与错误基线 | 不可 | 必须由 Rust 强类型模型、错误码、脱敏和单元测试证明。 |
| M2-M4 runtime | 不可 | 必须由 Rust RuntimeRegistry、MockTransport、队列、缓冲区和订阅测试证明。 |
| M5-M6 TCP/UDP/Serial | 不可 | 必须由 Rust transport、loopback 和 Windows 串口手工验收证明。 |
| M7 MCP server 接入 | 不可 | 必须由 Rust `rmcp` stdio server、工具注册和 handler smoke 证明。 |
| M8 发布前整理 | 不可 | 必须由 Rust 全量测试、日志脱敏、文档同步和验收记录证明。 |

## 改动决策规则

当任务描述没有明确提到 OMG workflow server、PRD、memory、checkpoint、ultragoal 或 TypeScript MCP server 时，默认不进入 `mcp-server/`。判断规则如下：

| 用户目标 | 应修改 | 不应修改 |
| --- | --- | --- |
| 实现端口 MCP 初版能力 | `src/`、`Cargo.toml`、Rust 测试、`docs/` | `mcp-server/` |
| 补 Rust 施工记录或验收记录 | `docs/`、必要时 Rust 代码 | `mcp-server/` |
| 修 OMG workflow state/PRD/memory 工具 | `mcp-server/` | Rust `src/`，除非明确涉及跨项目文档。 |
| 调整仓库说明或边界说明 | `docs/` | 两边代码，除非说明已经过期且用户要求同步。 |

## 回写规则

出现以下情况时，应先回写本文和相关上游文档，再继续施工：

- 用户明确要求把 TypeScript server 改造成 Rust port-mcp 的临时协议壳。
- CI、发布脚本或客户端配置把 `mcp-server/` 与 Rust port-mcp 混为同一启动目标。
- 任何施工记录把 TypeScript build/test 当作 Rust M0-M8 gate 证据。
- 需要从 OMG workflow server 迁移工具、状态或 memory 语义进入 port-mcp。
- README、配置说明或验收记录使读者无法判断当前启动的是哪个 MCP server。

结论：18 已固定仓库边界。后续可以继续进入 M0 Rust 工程骨架施工，但不得把 `mcp-server/` 的现有能力计入 port-mcp 初版完成度。