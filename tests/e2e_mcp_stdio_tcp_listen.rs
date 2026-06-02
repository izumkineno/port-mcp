use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, UdpSocket};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

const RPC_TIMEOUT: Duration = Duration::from_secs(5);
const POLL_TIMEOUT: Duration = Duration::from_secs(5);

#[test]
fn compiled_stdio_mcp_tcp_listen_supports_multiple_clients()
-> Result<(), Box<dyn std::error::Error>> {
    let binary = port_mcp_binary()?;
    let mut mcp = McpProcess::spawn(binary)?;
    let mut handles = Vec::new();

    let result = run_tcp_multiclient_flow(&mut mcp, &mut handles);

    for handle_id in handles.iter().rev() {
        let _ = mcp.call_tool("port_disconnect", json!({ "handle_id": handle_id }));
        let _ = mcp.call_tool(
            "instance_release",
            json!({ "handle_id": handle_id, "force": true }),
        );
    }

    result
}

#[test]
fn compiled_stdio_mcp_tcp_client_roundtrips_with_loopback_echo()
-> Result<(), Box<dyn std::error::Error>> {
    let binary = port_mcp_binary()?;
    let mut mcp = McpProcess::spawn(binary)?;
    let mut handles = Vec::new();

    let result = run_tcp_client_roundtrip_flow(&mut mcp, &mut handles);

    cleanup_handles(&mut mcp, &handles);
    result
}

#[test]
fn compiled_stdio_mcp_udp_instances_roundtrip_datagrams() -> Result<(), Box<dyn std::error::Error>>
{
    let binary = port_mcp_binary()?;
    let mut mcp = McpProcess::spawn(binary)?;
    let mut handles = Vec::new();

    let result = run_udp_roundtrip_flow(&mut mcp, &mut handles);

    cleanup_handles(&mut mcp, &handles);
    result
}

#[test]
fn compiled_stdio_mcp_error_and_lifecycle_smoke() -> Result<(), Box<dyn std::error::Error>> {
    let binary = port_mcp_binary()?;
    let mut mcp = McpProcess::spawn(binary)?;
    let mut handles = Vec::new();

    let result = run_error_lifecycle_flow(&mut mcp, &mut handles);

    cleanup_handles(&mut mcp, &handles);
    result
}

fn run_tcp_multiclient_flow(
    mcp: &mut McpProcess,
    handles: &mut Vec<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    mcp.initialize()?;

    let listen_port = free_loopback_port()?;

    let server = mcp.call_tool("instance_create", json!({ "type": "TCP" }))?;
    let server_handle = response_string(&server, &["handle_id"])?;
    handles.push(server_handle.clone());

    mcp.call_tool(
        "tcp_udp_config",
        json!({
            "handle_id": server_handle,
            "mode": "listen",
            "bind_host": "127.0.0.1",
            "bind_port": listen_port,
            "timeout_ms": 1000,
        }),
    )?;
    mcp.call_tool("port_connect", json!({ "handle_id": server_handle }))?;

    let client_a = create_connected_tcp_client(mcp, listen_port)?;
    handles.push(client_a.clone());
    let client_b = create_connected_tcp_client(mcp, listen_port)?;
    handles.push(client_b.clone());

    let peers = wait_for_peers(mcp, &server_handle, 2)?;
    let peer_a = peers[0]
        .get("peer_id")
        .and_then(Value::as_str)
        .ok_or("first peer has no peer_id")?
        .to_owned();
    let peer_b = peers[1]
        .get("peer_id")
        .and_then(Value::as_str)
        .ok_or("second peer has no peer_id")?
        .to_owned();
    assert!(
        peer_a.starts_with("h_tcp_001:peer-"),
        "unexpected peer id: {peer_a}"
    );
    assert_ne!(peer_a, peer_b);

    let targeted = mcp.call_tool(
        "port_send",
        json!({
            "handle_id": server_handle,
            "peer_id": peer_a,
            "data": "one",
            "encoding": "text"
        }),
    )?;
    assert_eq!(targeted["data"]["target"]["mode"], "peer");
    assert_eq!(targeted["data"]["target"]["successful_peer_ids"][0], peer_a);

    let received_a = wait_for_pull(mcp, &client_a, None, 8)?;
    assert_eq!(received_a["data"]["payload"]["preview"], "one");

    let broadcast = mcp.call_tool(
        "port_send",
        json!({
            "handle_id": server_handle,
            "data": "all",
            "encoding": "text"
        }),
    )?;
    assert_eq!(broadcast["data"]["target"]["mode"], "broadcast");
    assert_eq!(broadcast["data"]["target"]["peer_count"], 2);
    assert_eq!(
        wait_for_pull(mcp, &client_a, None, 8)?["data"]["payload"]["preview"],
        "all"
    );
    assert_eq!(
        wait_for_pull(mcp, &client_b, None, 8)?["data"]["payload"]["preview"],
        "all"
    );

    mcp.call_tool(
        "port_send",
        json!({ "handle_id": client_a, "data": "from-a", "encoding": "text" }),
    )?;
    mcp.call_tool(
        "port_send",
        json!({ "handle_id": client_b, "data": "from-b", "encoding": "text" }),
    )?;

    let pulled_a = wait_for_pull(mcp, &server_handle, Some(&peer_a), 16)?;
    assert_eq!(pulled_a["data"]["payload"]["preview"], "from-a");
    assert_eq!(pulled_a["data"]["source"]["peer_id"], peer_a);

    let pulled_b = wait_for_pull(mcp, &server_handle, Some(&peer_b), 16)?;
    assert_eq!(pulled_b["data"]["payload"]["preview"], "from-b");
    assert_eq!(pulled_b["data"]["source"]["peer_id"], peer_b);

    mcp.call_tool("port_disconnect", json!({ "handle_id": client_a }))?;
    let remaining = wait_for_peers(mcp, &server_handle, 1)?;
    assert_eq!(remaining[0]["peer_id"], peer_b);

    Ok(())
}

fn run_tcp_client_roundtrip_flow(
    mcp: &mut McpProcess,
    handles: &mut Vec<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    mcp.initialize()?;

    let listener = TcpListener::bind("127.0.0.1:0")?;
    let listen_port = listener.local_addr()?.port();
    let (ready_sender, ready_receiver) = mpsc::channel();
    let echo_thread = thread::spawn(move || -> Result<(), String> {
        listener
            .set_nonblocking(false)
            .map_err(|error| error.to_string())?;
        ready_sender.send(()).map_err(|error| error.to_string())?;
        let (mut stream, _) = listener.accept().map_err(|error| error.to_string())?;
        stream
            .set_read_timeout(Some(RPC_TIMEOUT))
            .map_err(|error| error.to_string())?;
        stream
            .set_write_timeout(Some(RPC_TIMEOUT))
            .map_err(|error| error.to_string())?;
        let mut buffer = [0_u8; 4];
        stream
            .read_exact(&mut buffer)
            .map_err(|error| error.to_string())?;
        if buffer != *b"ping" {
            return Err(format!("tcp echo received unexpected bytes: {buffer:?}"));
        }
        stream
            .write_all(b"pong")
            .map_err(|error| error.to_string())?;
        Ok(())
    });
    ready_receiver.recv_timeout(RPC_TIMEOUT)?;

    let client = create_connected_tcp_client(mcp, listen_port)?;
    handles.push(client.clone());

    let sent = mcp.call_tool(
        "port_send",
        json!({ "handle_id": client, "data": "ping", "encoding": "text" }),
    )?;
    assert_eq!(sent["data"]["sent_bytes"], 4);
    let pulled = wait_for_pull(mcp, &client, None, 8)?;
    assert_eq!(pulled["data"]["payload"]["preview"], "pong");

    echo_thread
        .join()
        .map_err(|_| "tcp echo thread panicked")?
        .map_err(|error| format!("tcp echo thread failed: {error}"))?;

    Ok(())
}

fn run_udp_roundtrip_flow(
    mcp: &mut McpProcess,
    handles: &mut Vec<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    mcp.initialize()?;

    let port_a = free_loopback_port()?;
    let port_b = free_loopback_port()?;
    let udp_a = create_connected_udp_peer(mcp, port_a, port_b)?;
    handles.push(udp_a.clone());
    let udp_b = create_connected_udp_peer(mcp, port_b, port_a)?;
    handles.push(udp_b.clone());

    let sent_a = mcp.call_tool(
        "port_send",
        json!({ "handle_id": udp_a, "data": "ping", "encoding": "text" }),
    )?;
    assert_eq!(sent_a["data"]["sent_bytes"], 4);
    assert_eq!(
        wait_for_pull(mcp, &udp_b, None, 8)?["data"]["payload"]["preview"],
        "ping"
    );

    let sent_b = mcp.call_tool(
        "port_send",
        json!({ "handle_id": udp_b, "data": "pong", "encoding": "text" }),
    )?;
    assert_eq!(sent_b["data"]["sent_bytes"], 4);
    assert_eq!(
        wait_for_pull(mcp, &udp_a, None, 8)?["data"]["payload"]["preview"],
        "pong"
    );

    Ok(())
}

fn run_error_lifecycle_flow(
    mcp: &mut McpProcess,
    handles: &mut Vec<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    mcp.initialize()?;

    let missing = mcp.call_tool("instance_query", json!({ "handle_id": "h_missing" }))?;
    assert_tool_error(&missing, "HANDLE_NOT_FOUND")?;

    let socket = UdpSocket::bind("127.0.0.1:0")?;
    let local_port = socket.local_addr()?.port();
    drop(socket);
    let udp = create_connected_udp_peer(mcp, local_port, local_port)?;
    handles.push(udp.clone());

    let release_without_force = mcp.call_tool(
        "instance_release",
        json!({ "handle_id": udp, "force": false }),
    )?;
    assert_tool_error(&release_without_force, "CONNECTED_RELEASE_REQUIRES_FORCE")?;

    let release_forced = mcp.call_tool(
        "instance_release",
        json!({ "handle_id": udp, "force": true }),
    )?;
    assert_eq!(release_forced["ok"], true);
    handles.retain(|handle_id| handle_id != &udp);

    let query_released = mcp.call_tool("instance_query", json!({ "handle_id": udp }))?;
    assert_tool_error(&query_released, "HANDLE_RELEASED")?;

    Ok(())
}

fn cleanup_handles(mcp: &mut McpProcess, handles: &[String]) {
    for handle_id in handles.iter().rev() {
        let _ = mcp.call_tool("port_disconnect", json!({ "handle_id": handle_id }));
        let _ = mcp.call_tool(
            "instance_release",
            json!({ "handle_id": handle_id, "force": true }),
        );
    }
}

fn create_connected_tcp_client(
    mcp: &mut McpProcess,
    port: u16,
) -> Result<String, Box<dyn std::error::Error>> {
    let created = mcp.call_tool("instance_create", json!({ "type": "TCP" }))?;
    let handle_id = response_string(&created, &["handle_id"])?;
    mcp.call_tool(
        "tcp_udp_config",
        json!({
            "handle_id": handle_id,
            "mode": "client",
            "host": "127.0.0.1",
            "port": port,
            "timeout_ms": 1000,
        }),
    )?;
    mcp.call_tool("port_connect", json!({ "handle_id": handle_id }))?;
    Ok(handle_id)
}

fn create_connected_udp_peer(
    mcp: &mut McpProcess,
    bind_port: u16,
    remote_port: u16,
) -> Result<String, Box<dyn std::error::Error>> {
    let created = mcp.call_tool("instance_create", json!({ "type": "UDP" }))?;
    let handle_id = response_string(&created, &["handle_id"])?;
    mcp.call_tool(
        "tcp_udp_config",
        json!({
            "handle_id": handle_id,
            "bind_host": "127.0.0.1",
            "bind_port": bind_port,
            "remote_host": "127.0.0.1",
            "remote_port": remote_port,
            "timeout_ms": 1000,
        }),
    )?;
    mcp.call_tool("port_connect", json!({ "handle_id": handle_id }))?;
    Ok(handle_id)
}

fn assert_tool_error(
    response: &Value,
    expected_code: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(response["ok"], false, "expected tool failure: {response}");
    assert_eq!(response["error"]["code"], expected_code);
    Ok(())
}

fn wait_for_peers(
    mcp: &mut McpProcess,
    handle_id: &str,
    expected: usize,
) -> Result<Vec<Value>, Box<dyn std::error::Error>> {
    let deadline = Instant::now() + POLL_TIMEOUT;
    loop {
        let query = mcp.call_tool("instance_query", json!({ "handle_id": handle_id }))?;
        let peers = query["data"]["summary"]["peers"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        if peers.len() == expected {
            return Ok(peers);
        }
        if Instant::now() >= deadline {
            return Err(
                format!("timed out waiting for {expected} peers; last peers: {peers:?}").into(),
            );
        }
        thread::sleep(Duration::from_millis(25));
    }
}

fn wait_for_pull(
    mcp: &mut McpProcess,
    handle_id: &str,
    peer_id: Option<&str>,
    max_bytes: usize,
) -> Result<Value, Box<dyn std::error::Error>> {
    let deadline = Instant::now() + POLL_TIMEOUT;
    loop {
        let mut args = json!({ "handle_id": handle_id, "max_bytes": max_bytes });
        if let Some(peer_id) = peer_id {
            args["peer_id"] = json!(peer_id);
        }
        let response = mcp.call_tool("port_pull", args)?;
        if response.get("ok").and_then(Value::as_bool) == Some(true) {
            return Ok(response);
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "timed out waiting for pull from {handle_id}; last response: {response:?}"
            )
            .into());
        }
        thread::sleep(Duration::from_millis(25));
    }
}

fn response_string(response: &Value, path: &[&str]) -> Result<String, Box<dyn std::error::Error>> {
    let mut value = response;
    for segment in path {
        value = value
            .get(*segment)
            .ok_or_else(|| format!("missing response path segment {segment}: {response}"))?;
    }
    value
        .as_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| format!("response path is not a string {path:?}: {response}").into())
}

fn free_loopback_port() -> Result<u16, Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    drop(listener);
    Ok(port)
}

fn port_mcp_binary() -> Result<PathBuf, Box<dyn std::error::Error>> {
    if let Ok(path) = std::env::var("CARGO_BIN_EXE_port-mcp") {
        return Ok(PathBuf::from(path));
    }

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let status = Command::new("cargo")
        .arg("build")
        .current_dir(&manifest_dir)
        .status()?;
    if !status.success() {
        return Err(format!("cargo build failed with status {status}").into());
    }

    let binary_name = if cfg!(windows) {
        "port-mcp.exe"
    } else {
        "port-mcp"
    };
    Ok(manifest_dir.join("target").join("debug").join(binary_name))
}

struct McpProcess {
    child: Child,
    stdin: ChildStdin,
    responses: Receiver<Value>,
    next_id: u64,
    pending: VecDeque<Value>,
}

impl McpProcess {
    fn spawn(binary: PathBuf) -> Result<Self, Box<dyn std::error::Error>> {
        let mut child = Command::new(binary)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let stdin = child.stdin.take().ok_or("child stdin was not piped")?;
        let stdout = child.stdout.take().ok_or("child stdout was not piped")?;
        let stderr = child.stderr.take().ok_or("child stderr was not piped")?;
        let (sender, responses) = mpsc::channel();

        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if line.trim().is_empty() {
                    continue;
                }
                match serde_json::from_str::<Value>(&line) {
                    Ok(value) => {
                        let _ = sender.send(value);
                    }
                    Err(error) => {
                        let _ = sender.send(json!({
                            "jsonrpc": "2.0",
                            "error": { "message": format!("invalid json from stdout: {error}; line={line}") }
                        }));
                    }
                }
            }
        });

        thread::spawn(move || {
            for line in BufReader::new(stderr).lines() {
                if line.is_err() {
                    break;
                }
            }
        });

        Ok(Self {
            child,
            stdin,
            responses,
            next_id: 1,
            pending: VecDeque::new(),
        })
    }

    fn initialize(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.request(
            "initialize",
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {
                    "name": "port-mcp-e2e",
                    "version": "0.1.0"
                }
            }),
        )?;
        self.notify("notifications/initialized", json!({}))?;
        Ok(())
    }

    fn call_tool(
        &mut self,
        name: &str,
        arguments: Value,
    ) -> Result<Value, Box<dyn std::error::Error>> {
        let response = self.request(
            "tools/call",
            json!({
                "name": name,
                "arguments": arguments
            }),
        )?;
        let text = response["result"]["content"]
            .as_array()
            .and_then(|content| content.first())
            .and_then(|content| content.get("text"))
            .and_then(Value::as_str)
            .ok_or_else(|| {
                format!("tool {name} response did not contain text content: {response}")
            })?;
        let tool_response: Value = serde_json::from_str(text)?;
        Ok(tool_response)
    }

    fn request(
        &mut self,
        method: &str,
        params: Value,
    ) -> Result<Value, Box<dyn std::error::Error>> {
        let id = self.next_id;
        self.next_id += 1;
        let request = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        writeln!(self.stdin, "{request}")?;
        self.stdin.flush()?;
        self.wait_for_response(id)
    }

    fn notify(&mut self, method: &str, params: Value) -> Result<(), Box<dyn std::error::Error>> {
        let notification = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        writeln!(self.stdin, "{notification}")?;
        self.stdin.flush()?;
        Ok(())
    }

    fn wait_for_response(&mut self, id: u64) -> Result<Value, Box<dyn std::error::Error>> {
        let deadline = Instant::now() + RPC_TIMEOUT;
        loop {
            if let Some(position) = self
                .pending
                .iter()
                .position(|value| value.get("id").and_then(Value::as_u64) == Some(id))
            {
                return Ok(self
                    .pending
                    .remove(position)
                    .expect("pending position exists"));
            }

            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(format!("timed out waiting for JSON-RPC response id {id}").into());
            }
            let value = self.responses.recv_timeout(remaining)?;
            if value.get("id").and_then(Value::as_u64) == Some(id) {
                if value.get("error").is_some() {
                    return Err(format!("JSON-RPC request {id} failed: {value}").into());
                }
                return Ok(value);
            }
            self.pending.push_back(value);
        }
    }
}

impl Drop for McpProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
