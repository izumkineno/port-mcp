# Deep Interview Spec: Additional Compiled-Binary E2E Tests

## Goal
Extend the existing compiled-binary MCP stdio E2E coverage beyond TCP listen multi-client behavior.

## Scope
Add the minimum high-value E2E set as three independent tests in the existing `tests/e2e_mcp_stdio_tcp_listen.rs` file, reusing its `McpProcess` harness and helpers.

## Acceptance Criteria
1. Add a TCP client roundtrip E2E test:
   - start a local loopback TCP echo/listener in the test;
   - spawn the compiled `port-mcp` binary through the existing stdio harness;
   - create/config/connect a TCP client instance through MCP tools;
   - send data and verify the local listener receives it;
   - send a reply from the listener and verify `port_pull` receives it;
   - cleanup disconnect/release and child process.
2. Add a UDP datagram E2E test:
   - bind a local UDP receiver on loopback;
   - spawn the compiled `port-mcp` binary;
   - create/config/connect a UDP instance through MCP tools;
   - send a datagram and verify the receiver gets it;
   - reply from the receiver and verify `port_pull` receives it;
   - cleanup disconnect/release and child process.
3. Add an error/lifecycle E2E smoke test:
   - verify a missing handle returns a unified failure response;
   - verify release of a connected instance without `force` fails;
   - verify `force=true` releases the connected instance;
   - verify the released instance is no longer queryable.

## Constraints
- Continue extending `tests/e2e_mcp_stdio_tcp_listen.rs` rather than creating a new harness file.
- Keep the tests independent: each test spawns its own compiled MCP process.
- Do not include Serial/VISA/hardware-dependent E2E tests.
- Do not add stress/performance tests.
- Keep bounded polling/timeouts for network reads and MCP pulls.

## Ambiguity Score
- Goal clarity: 90%
- Constraint clarity: 86%
- Success criteria clarity: 88%
- Context clarity: 82%
- Residual ambiguity: 18%
