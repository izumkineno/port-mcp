# Deep Interview Spec: E2E Self-Test for Compiled MCP Binary

## Goal
Add a repeatable Rust integration test that performs a compiled-binary E2E self-call against the `port-mcp` MCP server.

The test must build the binary, spawn the compiled MCP executable over stdio, and drive the MCP protocol to validate TCP listen multi-client behavior end to end.

## Constraints
- Use a Rust integration test entrypoint, intended to run under `cargo test`.
- The test should actively run `cargo build` internally before spawning the binary.
- The spawned process must be the compiled `port-mcp` binary, not an in-process service or mock.
- The E2E driver should use JSON-RPC/MCP stdio directly from Rust; avoid adding a Node-based runtime dependency for this test.
- Keep the scope limited to TCP multi-client core behavior.

## Non-Goals
- Do not cover all transport types in this E2E test.
- Do not add stress testing, long-running soak testing, or queue-full backpressure testing in this pass.
- Do not require external TCP services or serial devices.

## Acceptance Criteria
1. Running the E2E test builds the debug binary before execution.
2. The test spawns the compiled `port-mcp` executable using stdio.
3. The test initializes an MCP client session and calls tools through JSON-RPC.
4. The flow creates one TCP listen server and two TCP client instances.
5. Both clients can connect and remain online simultaneously.
6. `instance_query` on the server lists both peers with stable `peer_id` values.
7. Server can send a targeted message to one peer.
8. Server can broadcast a message to all peers.
9. Both clients can send messages to the server.
10. Server-side `port_pull` can identify which `peer_id` sent each message.
11. Disconnecting one client updates the server peer list.
12. The test cleans up spawned MCP process and instances even on failure as much as practical.

## Resolved Assumptions
- "E2E self-test" means a repository test that runs the compiled binary and calls it over MCP stdio.
- Preferred entrypoint is Rust integration test, not Node script or PowerShell script.
- Test should build internally rather than relying on a previous manual `cargo build`.
- Coverage target is complete TCP multi-client core flow, not broader transport or pressure coverage.

## Ambiguity Score
- Goal clarity: 90%
- Constraint clarity: 84%
- Success criteria clarity: 88%
- Context clarity: 82%
- Residual ambiguity: 16%
