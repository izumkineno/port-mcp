# Deep Interview Spec: TCP Multi-Client Listen

Generated: 2026-06-02
Ambiguity score: 18%

## Goal

Implement TCP listen multi-client support in the existing Rust `port-mcp` MCP server. A single TCP listen instance must support multiple simultaneous clients, expose the client list through `instance_query`, send to a specified client, broadcast to all connected clients, and identify which client produced each received payload.

## Confirmed Contract Decisions

- Extend existing MCP tools rather than adding a separate peer tool family.
- `instance_query` for a connected TCP listen instance must list connected clients.
- Each client must have an internal stable `peer_id` plus observable `remote_addr` metadata.
- `port_send` must accept an optional `peer_id` for targeted sends.
- For TCP listen, `port_send` without `peer_id` intentionally broadcasts to all currently connected clients.
- `port_pull` must accept an optional `peer_id` filter.
- `port_pull` without `peer_id` may return the next available inbound payload from any client, but the response must include source metadata such as `peer_id` and `remote_addr`.

## Constraints

- Keep the feature inside the existing TCP listen instance model; do not create independent instances per client.
- Preserve existing TCP client behavior.
- Preserve `port_send` and `port_pull` usability for Serial, TCP client, UDP, and Visa instances.
- Multi-client listen may intentionally change TCP listen default send behavior to broadcast when multiple clients are online.
- Peer/source metadata must be structured and suitable for debugging and automated tests.
- The implementation should follow current app, transport, model, and MCP layering.

## Non-Goals

- Do not implement UDP datagram/source enhancements in this slice.
- Do not implement advanced stream subscription triggers in this slice.
- Do not add a new peer sub-handle instance system.
- Do not implement application-level client self-registration protocols.

## Acceptance Criteria

- A compiled `port-mcp` binary can create one TCP listen server and multiple TCP clients through MCP tool calls.
- Multiple clients can connect and stay online concurrently.
- `instance_query` on the server exposes all online clients with `peer_id` and `remote_addr`.
- `port_send(handle_id=server, peer_id=<id>)` sends only to the selected client.
- `port_send(handle_id=server)` broadcasts to all connected clients.
- Multiple clients can send data to the server; server-side `port_pull` identifies the source client.
- `port_pull(handle_id=server, peer_id=<id>)` can filter by a specific client.
- Disconnecting one client removes or marks only that client without disrupting other clients.
- Rust tests cover core transport/app behavior.
- End-to-end self-call testing uses the compiled MCP binary to exercise server/client creation, connection, targeted send, broadcast send, client-to-server receive attribution, and disconnect behavior.

## Codebase Facts

- Current single-client listen logic lives in `src/transport/tcp.rs` with `TcpListenWorker`, `TcpListenCommand`, `run_tcp_listen_worker`, and a single `peer: Option<TcpClientTransport>`.
- Existing app routing for TCP send/read lives in `src/app/port_service.rs` and `src/app/instance_service.rs`.
- Existing MCP parameter types for `port_send` and `port_pull` live in `src/mcp/tools.rs` as `PortSendParams` and `PortPullParams`.
- Existing response/documentation contract is described in `docs/24-MCP工具列表与调用收发详解.md`.
- Advanced TCP multi-client planning already appears in `docs/02-进阶实现大纲.md` and `docs/25-进阶能力施工拆分与验收门槛.md`.

## Assumptions Resolved

- The desired API surface is an extension of existing tools, not a new tool family.
- Internal `peer_id` is the send/filter key; `remote_addr` is observational metadata.
- Default TCP listen send is broadcast when `peer_id` is omitted.
- Verification must include both Rust tests and compiled-binary MCP self-call tests.

## Interview Transcript Summary

- Round 1: Chose to extend existing MCP tools.
- Round 2: Chose `peer_id` plus `remote_addr` metadata.
- Round 3: Chose `port_pull` with optional `peer_id` filtering.
- Round 4: Chose default broadcast for `port_send` when `peer_id` is omitted.
- Round 5: Confirmed the compatibility tradeoff of default broadcast.
- Round 6: Chose Rust tests plus compiled-binary end-to-end self-call verification.

## Ontology

- Listen instance: the existing TCP server instance created through `instance_create(type=TCP)` and configured with `tcp_udp_config(mode=listen)`.
- Peer/client: one accepted TCP client connection tracked under the listen instance.
- `peer_id`: server-generated identifier used for targeted send and filtered pull.
- `remote_addr`: socket address metadata exposed for observability.
- Broadcast send: `port_send` on a listen server without `peer_id` sends to all connected peers.
- Source attribution: receive metadata that identifies which peer sent a payload.