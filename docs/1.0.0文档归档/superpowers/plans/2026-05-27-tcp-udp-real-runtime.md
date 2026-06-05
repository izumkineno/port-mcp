# TCP/UDP Real Runtime Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make TCP client, TCP server/listen, and UDP use real loopback network I/O in the existing port-mcp runtime instead of mock success paths.

**Architecture:** Keep the MCP tool surface and JSON schema stable. Route `port_connect`, `port_send`, `port_pull`, `port_disconnect`, and release cleanup through `InstanceService` / `RuntimeRegistry`, but store real TCP/UDP transport state on the runtime instance so the app layer can operate on actual sockets. TCP client uses a connected stream, TCP listen binds immediately and accepts the first peer lazily on first I/O, and UDP binds a local socket with an optional remote endpoint for send operations.

**Tech Stack:** Rust 2024, Tokio, rmcp, existing `transport::{tcp, udp}` helpers, current `model` error/result types.

---

### Task 1: Lock the failure mode with regression tests

**Files:**
- Modify: `src/mcp/tools.rs`
- Modify: `src/runtime/mod.rs`

- [ ] **Step 1: Write the failing test**

Add one MCP integration test for TCP client send reaching a real loopback listener and one runtime test for UDP datagram round-trip. The tests must fail before implementation because the current path still behaves like mock success.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test mcp::tools::tests::r1_mcp_tcp_client_send_reaches_real_loopback_listener -- --nocapture`

Expected: FAIL because the real listener does not receive bytes yet.

- [ ] **Step 3: Keep the tests focused**

Keep the new tests narrow:
- TCP client should prove `port_send` reaches a real `TcpListener`
- TCP listen should prove the first peer can connect and exchange bytes
- UDP should prove bind/send/pull works with a real datagram socket

- [ ] **Step 4: Commit**

Do not commit yet; keep the failing tests as the red baseline for the next task.

### Task 2: Implement real TCP/UDP runtime state and routing

**Files:**
- Modify: `src/runtime/mod.rs`
- Modify: `src/app/instance_service.rs`
- Modify: `src/app/port_service.rs`
- Modify: `src/model/config.rs`
- Modify: `src/transport/tcp.rs`
- Modify: `src/transport/udp.rs`

- [ ] **Step 1: Add transport state to runtime instances**

Store real runtime socket state on `RuntimeInstance` so TCP client, TCP listen, and UDP can survive across `connect`, `send`, `pull`, and `disconnect`.

- [ ] **Step 2: Route by instance type**

Make `InstanceService::connect`, `send`, `pull`, and `disconnect` dispatch to:
- serial mock/worker path for Serial
- real transport path for TCP and UDP

- [ ] **Step 3: Implement TCP client behavior**

Use the existing `TcpClientTransport` to connect, write, read, and close the stream. `port_send` must report bytes written to the real socket.

- [ ] **Step 4: Implement TCP listen behavior**

Bind the listener on `port_connect`, then lazily accept the first peer on first send/pull. Keep a single active peer for the initial runtime.

- [ ] **Step 5: Implement UDP behavior**

Bind the UDP socket on `port_connect`. `port_send` must use the configured remote endpoint when present, and `port_pull` must return the received datagram bytes from the real socket.

- [ ] **Step 6: Preserve error semantics**

Map connection timeouts, bind conflicts, closed peers, and invalid UDP remote configuration into the existing domain error model instead of falling back to mock queue success.

### Task 3: Verify the full path and update acceptance evidence

**Files:**
- Modify: `docs/00-索引.md` only if the runtime plan changes need indexing
- Modify: `docs/acceptance/*` if a new acceptance note is needed

- [ ] **Step 1: Run focused tests**

Run:
- `cargo test transport::tests::integration_tcp_loopback_client_round_trips -- --nocapture`
- `cargo test transport::tests::integration_udp_loopback_datagrams_conflict_and_rebind -- --nocapture`
- `cargo test mcp::tools::tests::r1_mcp_tcp_client_send_reaches_real_loopback_listener -- --nocapture`

- [ ] **Step 2: Run the full suite**

Run: `cargo test`

Expected: PASS with the new TCP/UDP runtime behavior covered by unit and MCP tests.

- [ ] **Step 3: Record the implementation evidence**

If the runtime changes affect the acceptance story, add or update an acceptance note under `docs/acceptance/` with the commands run and the concrete TCP/UDP behaviors verified.

- [ ] **Step 4: Commit**

Commit only after the targeted tests and full suite pass.
