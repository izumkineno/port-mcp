# 代码审查修复方案

> 审查日期: 2026-06-26 | 审查范围: `src/` 全部 Rust 源码
> 修复状态: ✅ 全部完成 (6/6)

---

## 🔴 Blocking

### #1 内存 DoS：`Payload` 构造时过量分配

- **文件:** `src/model/data.rs`, `src/mcp/tools.rs`
- **位置:** `Payload::from_text` / `Payload::from_hex`（data.rs:13-34）被 `port_send`（tools.rs:1473-1474）和 `debug_exchange`（tools.rs:1236-1237）调用
- **问题:** 构造 Payload 时使用 `ABS_MAX_TOTAL_BUFFER_BYTES`（512 MiB）作为上限，后续 `validate_tx_frame_len` 才校验 64 KiB。攻击者可发送 ~1 GiB hex 字符串，强制服务器先分配 512 MiB 再被拒绝。
- **方案:** 给 `Payload` 增加 `_with_limit` 版本，在 `port_send` / `debug_exchange` 中传入 `tx_frame_max_bytes`

```rust
// src/model/data.rs
impl Payload {
    pub fn from_text_with_limit(input: &str, append_line_break: bool, max_bytes: usize) -> Result<Self, DomainError> {
        let mut bytes = text_to_bytes_with_limit(input, "input_string", max_bytes)?;
        if append_line_break { bytes.push(b'\n'); }
        Ok(Self { bytes, encoding: PayloadEncoding::Text })
    }

    pub fn from_hex_with_limit(input: &str, append_line_break: bool, max_bytes: usize) -> Result<Self, DomainError> {
        let mut bytes = hex_to_bytes_with_limit(input, "hex", max_bytes)?;
        if append_line_break { bytes.push(b'\n'); }
        Ok(Self { bytes, encoding: PayloadEncoding::Hex })
    }
}
```

### #2 跨会话 debug profile 互相覆盖

- **文件:** `src/mcp/tools.rs:122-124`
- **问题:** `debug_profile_key` 忽略 `context` 参数，始终返回 `"request_context_debug"`，所有并发 MCP 会话共享同一个 debug profile
- **方案:** 一行修复，复用已有的 `session_id` 方法

```rust
fn debug_profile_key(&self, context: &RequestContext<RoleServer>) -> String {
    self.session_id(context)
}
```

---

## 🟡 Should-fix

### #3 生产环境 `new()` 使用硬编码日期

- **文件:** `src/mcp/tools.rs:40`, `src/model/ids.rs`, `src/runtime/mod.rs`
- **问题:** `PortMcpServer::new()` 调用 `new_for_tests("20260526")`，所有 request ID 日期固定
- **方案:** `IdGenerator::new()` 使用 `OffsetDateTime::now_utc()` 生成当日日期字符串，`PortMcpServer::new()` 调用非测试构造器

### #4 Worker 线程 panic 被伪装成超时

- **文件:** `src/transport/tcp.rs:646-653`, `src/transport/udp.rs`
- **问题:** `receive_worker_reply` 将所有 `recv_timeout` 错误统一映射为超时，掩盖线程崩溃
- **方案:** 区分 `RecvTimeoutError::Timeout` 和 `RecvTimeoutError::Disconnected`

```rust
fn receive_worker_reply<T>(...) -> Result<T, TransportError> {
    receiver.recv_timeout(Duration::from_millis(timeout_ms))
        .map_err(|e| match e {
            RecvTimeoutError::Timeout => TransportError::read_timeout("..."),
            RecvTimeoutError::Disconnected => TransportError::transport_closed("worker thread exited"),
        })?
}
```

### #5 JoinHandle 从未 join

- **文件:** `src/transport/tcp.rs:72,133`, `src/transport/udp.rs`
- **问题:** `_thread: JoinHandle<()>` drop 时线程被 detach，panic 静默吞掉
- **方案:** 将 `_thread` 改为 `Option<JoinHandle<()>>`，在 `Drop` 中 take 并 join，panic 时记录 `tracing::error!`

### #6 重复的 `transport_runtime()`

- **文件:** `src/transport/tcp.rs:297`, `src/transport/udp.rs:146`
- **问题:** 完全相同的函数在两处各定义一份
- **方案:** 提取到 `src/transport/common.rs`，两处改为 `use` 引用

---

## 实施顺序

1. **#2**（1 行，零风险） → 先修跨会话 bug
2. **#1**（DoS，涉及 data.rs + tools.rs） → 安全优先级
3. **#6**（纯重构，无行为变更） → 低风险穿插
4. **#4**（小改动，提升可观测性） → 快速收益
5. **#3**（新增构造器，保持测试兼容） → 需仔细处理
6. **#5**（Drop 语义，需测试） → 复杂度最高，最后修
