# Ralplan: Compiled MCP Binary E2E Self-Test

## Decision
Add a Rust integration test that spawns the compiled `port-mcp` MCP server over stdio and verifies TCP listen multi-client core behavior through JSON-RPC/MCP tool calls.

## Decision Drivers
1. The missing coverage is the production binary + stdio MCP boundary, not the in-process business behavior alone.
2. The test must be repeatable from the repository and suitable for `cargo test` workflows.
3. Windows process, pipe, and port handling must be bounded and diagnosable.

## Chosen Approach
Create `tests/e2e_mcp_stdio_tcp_listen.rs` with a small MCP stdio harness.

Binary location strategy:
- Prefer Cargo's `CARGO_BIN_EXE_port-mcp` when available.
- If missing, run `cargo build` from `CARGO_MANIFEST_DIR` and then spawn `target/debug/port-mcp(.exe)`.

This preserves the user's requirement that the test can actively build, while avoiding nested build work when Cargo already built the binary for the integration test.

## Alternatives Considered
- Unconditional internal `cargo build`: closer to the first requirement, but slower and riskier in default `cargo test`, especially on Windows.
- Node SDK script: already proven manually, but rejected because the desired permanent entrypoint is Rust integration test.
- In-process rmcp duplex test only: already exists and does not cover the compiled binary boundary.

## Implementation Plan
1. Add a Rust integration test file under `tests/`.
2. Implement a minimal newline JSON-RPC stdio client harness:
   - spawn child with piped stdin/stdout/stderr;
   - drain stderr in background;
   - send `initialize`, `notifications/initialized`, and `tools/call`;
   - parse `CallToolResult.content[0].text` into the project's tool response JSON.
3. Add bounded polling helpers for peer discovery, MCP pull, and TCP stream reads.
4. Allocate a loopback port by probing `127.0.0.1:0`; retry if needed.
5. Execute the TCP multi-client flow:
   - create TCP listen server;
   - create two TCP clients through MCP tools;
   - wait until server lists two peers;
   - verify targeted send, broadcast, two-client upstream sends, `source.peer_id`, and disconnect peer list update.
6. Best-effort cleanup of handles and child process.
7. Run formatting, targeted E2E test, and full test suite.

## Acceptance Criteria
- `cargo test --test e2e_mcp_stdio_tcp_listen` passes.
- The test spawns the compiled `port-mcp` binary, not an in-process server.
- TCP listen server can list multiple peers, route to a selected peer, broadcast, and identify the source peer for inbound messages.
- The test uses timeouts/polling and cleans up child process and MCP instances.

## Consequences
- Adds a heavier E2E test path than existing in-process tests.
- Provides direct regression coverage for the real binary stdio MCP surface.
- The build fallback may add time only when Cargo does not expose `CARGO_BIN_EXE_port-mcp`.
