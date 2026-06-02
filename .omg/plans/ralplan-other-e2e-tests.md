# Ralplan: Additional Compiled-Binary E2E Tests

## Decision
Extend `tests/e2e_mcp_stdio_tcp_listen.rs` with three independent compiled-binary MCP stdio E2E tests:

1. TCP client roundtrip against a local loopback echo server.
2. UDP datagram roundtrip using two MCP UDP instances with explicit loopback ports.
3. Error/lifecycle smoke for unified failure responses and force release semantics.

## Drivers
- Fill high-value gaps left by the TCP listen multi-client E2E test.
- Reuse the existing stdio harness and binary-location strategy.
- Keep tests independent and bounded with loopback-only networking.

## Key Design Choices
- TCP client test starts a local `TcpListener` echo thread, so it covers the TCP client transport path rather than the TCP listen path again.
- UDP test creates two MCP UDP instances with preallocated explicit ports and reciprocal remotes, avoiding reliance on querying OS-assigned `bind_port=0` from the MCP summary.
- Error/lifecycle test asserts tool-level JSON responses with `ok:false`, not JSON-RPC errors.
- Every test best-effort disconnects/releases handles and lets `McpProcess` kill/wait the child process on drop.

## Test Commands
- `rtk cargo test --test e2e_mcp_stdio_tcp_listen -- --nocapture`
- `rtk cargo test`

## Non-Goals
- No Serial or VISA hardware E2E.
- No stress/performance tests.
- No exhaustive parameter validation E2E; unit tests keep that role.
