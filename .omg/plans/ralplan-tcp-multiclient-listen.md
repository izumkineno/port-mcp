# Ralplan: TCP Multi-Client Listen

Generated: 2026-06-02
Source spec: `.omg/specs/deep-interview-tcp-multiclient-listen.md`
Consensus: approved after Planner, Architect, and Critic review with user decisions captured.

## ADR

### Decision

Implement TCP listen multi-client support inside the existing TCP listen instance and existing MCP tools. The listen worker will become a multi-peer owner that tracks connected peers, routes writes by optional `peer_id`, broadcasts when `peer_id` is omitted, and returns source metadata for server-side reads.

### Decision Drivers

- Preserve the existing MCP tool surface while adding peer-aware optional fields.
- Keep one TCP listen instance as the ownership boundary; do not create peer sub-instances.
- Make peer identity, source attribution, broadcast behavior, and end-to-end verification explicit and testable.

### Chosen Approach

- Add peer-aware optional parameters to `PortSendParams` and `PortPullParams`.
- Generate `peer_id` values with the listen handle prefix plus an incrementing suffix, for example `h_tcp_001:peer-1`.
- Expose connected peers from `instance_query` through structured peer summaries containing at least `peer_id` and `remote_addr`.
- For TCP listen only, `port_send` without `peer_id` broadcasts to all active peers.
- Broadcast succeeds if at least one peer receives the payload; the result must include successful peer ids and failure counts/details where feasible.
- For TCP listen only, `port_pull` may accept `peer_id`; with a filter, it only consumes data from that peer. Without a filter, it returns the next available inbound chunk from any peer with `source` metadata.
- Use an inbound queue per listen worker; if full, drop the oldest frame and count the drop.

### Alternatives Considered

- Add a separate peer tool family such as `tcp_peer_send`: rejected because the user chose to extend existing tools.
- Model each peer as an independent instance: rejected because it violates the spec and complicates resource lifecycle.
- Require explicit broadcast flags: rejected because the user confirmed omitted `peer_id` should intentionally broadcast.
- Use `remote_addr` as the id: rejected because reconnects and ephemeral ports make it a weak operation key.

### Consequences

- TCP listen default send behavior intentionally changes in multi-client mode: omitted `peer_id` broadcasts.
- The transport layer must own peer lifecycle and inbound source attribution.
- The app and MCP layers must reject `peer_id` for unsupported instance types rather than silently ignoring it.
- Tests must wait for peer discovery before asserting send/pull behavior to avoid network timing flakes.

## Implementation Plan

1. Model peer metadata and peer-aware results.
   - Add peer/source structs in the model or app result layer.
   - Extend send results with target/broadcast metadata and successful peer ids.
   - Extend pull results with optional source metadata.
   - Extend instance summaries with optional connected peer summaries.

2. Refactor `TcpListenWorker` into a multi-peer owner.
   - Replace the single `peer: Option<TcpClientTransport>` model in `src/transport/tcp.rs`.
   - Track peer writers in a map keyed by `peer_id`.
   - Track each peer's `remote_addr`.
   - Continuously accept clients while processing commands.
   - Spawn or manage per-peer reads that push inbound frames into a worker-owned queue.
   - Remove disconnected peers without stopping the listen instance.

3. Add transport commands.
   - `ListPeers` returns active peer summaries.
   - `Write { peer_id: Some(..) }` sends to one peer.
   - `Write { peer_id: None }` broadcasts to active peers.
   - `Read { peer_id: Some(..) }` reads the next queued chunk from that peer.
   - `Read { peer_id: None }` reads the next queued chunk from any peer.
   - `Close` closes listener and all active peers.

4. Thread peer options through the app layer.
   - Update `PortService::send` and `PortService::pull` to accept peer options.
   - Update `NetworkWorker::tcp_write` and `NetworkWorker::tcp_read` accordingly.
   - Enrich TCP listen `instance_query` / `instance_list` summaries with active peers.
   - Keep Serial, TCP client, UDP, and Visa behavior unchanged when `peer_id` is absent.
   - Return a clear invalid-argument error if `peer_id` is passed to unsupported transport modes.

5. Extend MCP schemas and JSON responses.
   - Add optional `peer_id` to `PortSendParams`.
   - Add optional `peer_id` to `PortPullParams`.
   - Add source metadata to TCP listen `port_pull` responses.
   - Add broadcast/target metadata and successful peer ids to TCP listen `port_send` responses.
   - Update tool descriptions and `usage_guide` notes.

6. Update documentation.
   - Update `docs/24-MCP工具列表与调用收发详解.md` with the new TCP listen peer contract.
   - Update progress or acceptance docs if the implementation introduces new verified behavior.

7. Add verification.
   - Rust tests: transport-level multi-client peer list, targeted send, broadcast, filtered pull, any-peer pull, disconnect cleanup.
   - App/MCP tests: optional parameter parsing and JSON response shape.
   - Compiled-binary end-to-end self-call: spawn the built MCP server, create one TCP listen server and multiple TCP clients, connect all, query peers, targeted send, broadcast, concurrent client-to-server sends with attribution, filtered pull, and client disconnect.

## Test Strategy

- Use loopback `127.0.0.1` and dynamic ports where possible.
- Wait for `instance_query.peers.len()` to reach expected counts before send/pull assertions.
- Assert `peer_id` uniqueness and handle-prefixed format.
- Assert disconnected peer operations return a stable error and do not break remaining peers.
- Assert non-TCP-listen transports reject `peer_id` cleanly.
- Run `cargo test` and a compiled-binary MCP self-call test before completion.

## Critic Concerns Addressed

- Default broadcast risk is intentional and documented by user decision.
- Peer lifecycle uses handle-prefixed ids unique within a listen handle lifecycle.
- Disconnect cleanup is part of acceptance criteria.
- Pull filtering is strict: specifying a peer never consumes another peer's data.
- Compiled-binary self-call is required in addition to in-process tests.

## Open Follow-Ups For Later Slices

- Per-peer clear behavior for `port_clear`.
- Peer-aware stream subscription events.
- Per-peer stats beyond active peer list and source metadata.
- UDP datagram/source metadata.