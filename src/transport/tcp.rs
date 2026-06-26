use std::{
    collections::{HashMap, VecDeque},
    net::SocketAddr,
    sync::mpsc,
    thread::{self, JoinHandle},
    time::Duration,
};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::mpsc as tokio_mpsc,
    time::timeout,
};

use super::{
    TransportError, map_read_error, map_tcp_bind_error, map_tcp_connect_error, map_write_error,
    transport_runtime,
};

const TCP_LISTEN_INBOUND_MAX_FRAMES: usize = 1024;

#[derive(Debug)]
pub struct TcpClientTransport {
    stream: TcpStream,
}

impl TcpClientTransport {
    pub async fn connect(host: &str, port: u16, timeout_ms: u64) -> Result<Self, TransportError> {
        let stream = timeout(
            Duration::from_millis(timeout_ms),
            TcpStream::connect((host, port)),
        )
        .await
        .map_err(|_| TransportError::connect_timeout("tcp connect timed out"))?
        .map_err(map_tcp_connect_error)?;
        Ok(Self { stream })
    }

    pub async fn read_chunk(&mut self, max_bytes: usize) -> Result<Vec<u8>, TransportError> {
        let mut buffer = vec![0; max_bytes];
        let read = self
            .stream
            .read(&mut buffer)
            .await
            .map_err(map_read_error)?;
        if read == 0 {
            return Err(TransportError::transport_closed(
                "tcp peer closed connection",
            ));
        }
        buffer.truncate(read);
        Ok(buffer)
    }

    pub async fn write_all(&mut self, bytes: &[u8]) -> Result<usize, TransportError> {
        self.stream
            .write_all(bytes)
            .await
            .map_err(map_write_error)?;
        Ok(bytes.len())
    }

    pub async fn close(self) -> Result<(), TransportError> {
        drop(self);
        Ok(())
    }
}

#[derive(Debug)]
pub struct TcpClientWorker {
    commands: mpsc::Sender<TcpClientCommand>,
    thread: Option<JoinHandle<()>>,
    timeout_ms: u64,
}

impl Drop for TcpClientWorker {
    fn drop(&mut self) {
        if let Some(handle) = self.thread.take() {
            if let Err(panic) = handle.join() {
                let msg = panic
                    .downcast_ref::<&str>()
                    .map(|s| s.to_string())
                    .or_else(|| panic.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "unknown panic".to_owned());
                tracing::error!(%msg, "tcp client worker thread panicked");
            }
        }
    }
}

impl TcpClientWorker {
    pub fn connect(host: &str, port: u16, timeout_ms: u64) -> Result<Self, TransportError> {
        let host = host.to_owned();
        let (commands, receiver) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::channel();
        let thread = thread::spawn(move || {
            let mut runtime = transport_runtime();
            let result = runtime.block_on(TcpClientTransport::connect(&host, port, timeout_ms));
            match result {
                Ok(transport) => {
                    let _ = ready_tx.send(Ok(()));
                    run_tcp_client_worker(&mut runtime, transport, timeout_ms, receiver);
                }
                Err(error) => {
                    let _ = ready_tx.send(Err(error));
                }
            }
        });
        match ready_rx.recv_timeout(Duration::from_millis(timeout_ms.saturating_add(1_000))) {
            Ok(Ok(())) => Ok(Self {
                commands,
                thread: Some(thread),
                timeout_ms,
            }),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(TransportError::connect_timeout(
                "tcp client initialization timed out",
            )),
        }
    }

    pub fn write(&self, bytes: &[u8]) -> Result<usize, TransportError> {
        self.request(|reply| TcpClientCommand::Write(bytes.to_vec(), reply))
    }

    pub fn read(&self, max_bytes: usize) -> Result<Vec<u8>, TransportError> {
        self.request(|reply| TcpClientCommand::Read(max_bytes, reply))
    }

    pub fn close(&self) -> Result<(), TransportError> {
        self.request(TcpClientCommand::Close)
    }

    fn request<T>(
        &self,
        build: impl FnOnce(mpsc::Sender<Result<T, TransportError>>) -> TcpClientCommand,
    ) -> Result<T, TransportError> {
        let (reply, receiver) = mpsc::channel();
        self.commands
            .send(build(reply))
            .map_err(|_| TransportError::transport_closed("tcp client worker is closed"))?;
        receive_worker_reply(receiver, self.timeout_ms)
    }
}

#[derive(Debug)]
pub struct TcpListenWorker {
    commands: mpsc::Sender<TcpListenCommand>,
    thread: Option<JoinHandle<()>>,
    timeout_ms: u64,
}

impl Drop for TcpListenWorker {
    fn drop(&mut self) {
        if let Some(handle) = self.thread.take() {
            if let Err(panic) = handle.join() {
                let msg = panic
                    .downcast_ref::<&str>()
                    .map(|s| s.to_string())
                    .or_else(|| panic.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "unknown panic".to_owned());
                tracing::error!(%msg, "tcp listen worker thread panicked");
            }
        }
    }
}

impl TcpListenWorker {
    pub fn bind(
        handle_id: &str,
        host: &str,
        port: u16,
        timeout_ms: u64,
    ) -> Result<Self, TransportError> {
        let peer_id_prefix = handle_id.to_owned();
        let host = host.to_owned();
        let (commands, receiver) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::channel();
        let thread = thread::spawn(move || {
            let mut runtime = transport_runtime();
            let result = runtime.block_on(TcpListenTransport::bind(&host, port));
            match result {
                Ok(listener) => {
                    let _ = ready_tx.send(Ok(()));
                    run_tcp_listen_worker(
                        &mut runtime,
                        listener,
                        timeout_ms,
                        peer_id_prefix,
                        receiver,
                    );
                }
                Err(error) => {
                    let _ = ready_tx.send(Err(error));
                }
            }
        });
        match ready_rx.recv_timeout(Duration::from_millis(timeout_ms.saturating_add(1_000))) {
            Ok(Ok(())) => Ok(Self {
                commands,
                thread: Some(thread),
                timeout_ms,
            }),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(TransportError::connect_timeout(
                "tcp listen initialization timed out",
            )),
        }
    }

    pub fn write(
        &self,
        peer_id: Option<&str>,
        bytes: &[u8],
    ) -> Result<TcpListenWriteResult, TransportError> {
        self.request(|reply| TcpListenCommand::Write {
            peer_id: peer_id.map(str::to_owned),
            bytes: bytes.to_vec(),
            reply,
        })
    }

    pub fn read(
        &self,
        peer_id: Option<&str>,
        max_bytes: usize,
    ) -> Result<TcpListenReadResult, TransportError> {
        self.request(|reply| TcpListenCommand::Read {
            peer_id: peer_id.map(str::to_owned),
            max_bytes,
            reply,
        })
    }

    pub fn list_peers(&self) -> Result<Vec<TcpPeerSummary>, TransportError> {
        self.request(TcpListenCommand::ListPeers)
    }

    pub fn close(&self) -> Result<(), TransportError> {
        self.request(TcpListenCommand::Close)
    }

    fn request<T>(
        &self,
        build: impl FnOnce(mpsc::Sender<Result<T, TransportError>>) -> TcpListenCommand,
    ) -> Result<T, TransportError> {
        let (reply, receiver) = mpsc::channel();
        self.commands
            .send(build(reply))
            .map_err(|_| TransportError::transport_closed("tcp listen worker is closed"))?;
        receive_worker_reply(receiver, self.timeout_ms)
    }
}

enum TcpClientCommand {
    Write(Vec<u8>, mpsc::Sender<Result<usize, TransportError>>),
    Read(usize, mpsc::Sender<Result<Vec<u8>, TransportError>>),
    Close(mpsc::Sender<Result<(), TransportError>>),
}

enum TcpListenCommand {
    Write {
        peer_id: Option<String>,
        bytes: Vec<u8>,
        reply: mpsc::Sender<Result<TcpListenWriteResult, TransportError>>,
    },
    Read {
        peer_id: Option<String>,
        max_bytes: usize,
        reply: mpsc::Sender<Result<TcpListenReadResult, TransportError>>,
    },
    ListPeers(mpsc::Sender<Result<Vec<TcpPeerSummary>, TransportError>>),
    Close(mpsc::Sender<Result<(), TransportError>>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TcpPeerSummary {
    pub peer_id: String,
    pub remote_addr: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TcpListenReadResult {
    pub peer_id: String,
    pub remote_addr: String,
    pub bytes: Vec<u8>,
    pub remaining_frames: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TcpListenWriteResult {
    pub sent_bytes: usize,
    pub mode: TcpListenWriteMode,
    pub peer_id: Option<String>,
    pub successful_peer_ids: Vec<String>,
    pub failed_peer_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TcpListenWriteMode {
    Peer,
    Broadcast,
}

struct TcpListenPeer {
    writer: tokio::net::tcp::OwnedWriteHalf,
    remote_addr: SocketAddr,
}

struct TcpInboundFrame {
    peer_id: String,
    remote_addr: SocketAddr,
    bytes: Vec<u8>,
}

enum TcpListenEvent {
    Accepted {
        peer_id: String,
        remote_addr: SocketAddr,
        writer: tokio::net::tcp::OwnedWriteHalf,
    },
    Inbound(TcpInboundFrame),
    Disconnected(String),
}

fn run_tcp_client_worker(
    runtime: &mut tokio::runtime::Runtime,
    mut transport: TcpClientTransport,
    timeout_ms: u64,
    receiver: mpsc::Receiver<TcpClientCommand>,
) {
    for command in receiver {
        match command {
            TcpClientCommand::Write(bytes, reply) => {
                let result = runtime.block_on(async {
                    timeout(
                        Duration::from_millis(timeout_ms),
                        transport.write_all(&bytes),
                    )
                    .await
                    .map_err(|_| {
                        TransportError::write_failed(
                            crate::model::ErrorCode::WriteIoFailed,
                            "tcp write timed out",
                        )
                    })?
                });
                let _ = reply.send(result);
            }
            TcpClientCommand::Read(max_bytes, reply) => {
                let result = runtime.block_on(async {
                    timeout(
                        Duration::from_millis(timeout_ms),
                        transport.read_chunk(max_bytes),
                    )
                    .await
                    .map_err(|_| TransportError::read_timeout("tcp read timed out"))?
                });
                let _ = reply.send(result);
            }
            TcpClientCommand::Close(reply) => {
                let _ = reply.send(runtime.block_on(transport.close()));
                break;
            }
        }
    }
}

fn run_tcp_listen_worker(
    runtime: &mut tokio::runtime::Runtime,
    listener: TcpListenTransport,
    timeout_ms: u64,
    peer_id_prefix: String,
    receiver: mpsc::Receiver<TcpListenCommand>,
) {
    let (command_tx, command_rx) = tokio_mpsc::unbounded_channel();
    thread::spawn(move || {
        for command in receiver {
            if command_tx.send(command).is_err() {
                break;
            }
        }
    });
    runtime.block_on(run_tcp_listen_hub(
        listener,
        timeout_ms,
        peer_id_prefix,
        command_rx,
    ));
}

async fn run_tcp_listen_hub(
    listener: TcpListenTransport,
    timeout_ms: u64,
    peer_id_prefix: String,
    mut command_rx: tokio_mpsc::UnboundedReceiver<TcpListenCommand>,
) {
    let (event_tx, mut event_rx) = tokio_mpsc::unbounded_channel::<TcpListenEvent>();
    let accept_tx = event_tx.clone();
    tokio::spawn(async move {
        let mut next_peer = 1_u64;
        loop {
            match listener.accept_stream().await {
                Ok((stream, remote_addr)) => {
                    let peer_id = format!("{peer_id_prefix}:peer-{next_peer}");
                    next_peer += 1;
                    let (reader, writer) = stream.into_split();
                    if accept_tx
                        .send(TcpListenEvent::Accepted {
                            peer_id: peer_id.clone(),
                            remote_addr,
                            writer,
                        })
                        .is_err()
                    {
                        break;
                    }
                    spawn_tcp_peer_reader(reader, peer_id, remote_addr, accept_tx.clone());
                }
                Err(_) => break,
            }
        }
    });

    let mut peers = HashMap::<String, TcpListenPeer>::new();
    let mut inbound = VecDeque::<TcpInboundFrame>::new();

    loop {
        tokio::select! {
            Some(event) = event_rx.recv() => {
                handle_tcp_listen_event(event, &mut peers, &mut inbound);
            }
            command = command_rx.recv() => {
                let Some(command) = command else { break; };
                if handle_tcp_listen_command(command, &mut peers, &mut inbound, timeout_ms).await {
                    break;
                }
            }
        }
    }
}

fn spawn_tcp_peer_reader(
    mut reader: tokio::net::tcp::OwnedReadHalf,
    peer_id: String,
    remote_addr: SocketAddr,
    event_tx: tokio_mpsc::UnboundedSender<TcpListenEvent>,
) {
    tokio::spawn(async move {
        let mut buffer = vec![0; 4096];
        loop {
            match reader.read(&mut buffer).await {
                Ok(0) => {
                    let _ = event_tx.send(TcpListenEvent::Disconnected(peer_id.clone()));
                    break;
                }
                Ok(read) => {
                    let _ = event_tx.send(TcpListenEvent::Inbound(TcpInboundFrame {
                        peer_id: peer_id.clone(),
                        remote_addr,
                        bytes: buffer[..read].to_vec(),
                    }));
                }
                Err(_) => {
                    let _ = event_tx.send(TcpListenEvent::Disconnected(peer_id.clone()));
                    break;
                }
            }
        }
    });
}

fn handle_tcp_listen_event(
    event: TcpListenEvent,
    peers: &mut HashMap<String, TcpListenPeer>,
    inbound: &mut VecDeque<TcpInboundFrame>,
) {
    match event {
        TcpListenEvent::Accepted {
            peer_id,
            remote_addr,
            writer,
        } => {
            peers.insert(
                peer_id,
                TcpListenPeer {
                    writer,
                    remote_addr,
                },
            );
        }
        TcpListenEvent::Inbound(frame) => {
            if inbound.len() >= TCP_LISTEN_INBOUND_MAX_FRAMES {
                inbound.pop_front();
            }
            inbound.push_back(frame);
        }
        TcpListenEvent::Disconnected(peer_id) => {
            peers.remove(&peer_id);
            inbound.retain(|frame| frame.peer_id != peer_id);
        }
    }
}

async fn handle_tcp_listen_command(
    command: TcpListenCommand,
    peers: &mut HashMap<String, TcpListenPeer>,
    inbound: &mut VecDeque<TcpInboundFrame>,
    timeout_ms: u64,
) -> bool {
    match command {
        TcpListenCommand::Write {
            peer_id,
            bytes,
            reply,
        } => {
            let result =
                write_tcp_listen_peers(peers, peer_id.as_deref(), &bytes, timeout_ms).await;
            let _ = reply.send(result);
            false
        }
        TcpListenCommand::Read {
            peer_id,
            max_bytes,
            reply,
        } => {
            let result = read_tcp_listen_frame(inbound, peer_id.as_deref(), max_bytes)
                .ok_or_else(|| TransportError::read_timeout("tcp read timed out"));
            let _ = reply.send(result);
            false
        }
        TcpListenCommand::ListPeers(reply) => {
            let mut summaries = peers
                .iter()
                .map(|(peer_id, peer)| TcpPeerSummary {
                    peer_id: peer_id.clone(),
                    remote_addr: peer.remote_addr.to_string(),
                })
                .collect::<Vec<_>>();
            summaries.sort_by(|left, right| left.peer_id.cmp(&right.peer_id));
            let _ = reply.send(Ok(summaries));
            false
        }
        TcpListenCommand::Close(reply) => {
            peers.clear();
            inbound.clear();
            let _ = reply.send(Ok(()));
            true
        }
    }
}

async fn write_tcp_listen_peers(
    peers: &mut HashMap<String, TcpListenPeer>,
    peer_id: Option<&str>,
    bytes: &[u8],
    timeout_ms: u64,
) -> Result<TcpListenWriteResult, TransportError> {
    match peer_id {
        Some(peer_id) => {
            let Some(peer) = peers.get_mut(peer_id) else {
                return Err(TransportError::transport_closed("tcp peer not found"));
            };
            timeout(
                Duration::from_millis(timeout_ms),
                peer.writer.write_all(bytes),
            )
            .await
            .map_err(|_| {
                TransportError::write_failed(
                    crate::model::ErrorCode::WriteIoFailed,
                    "tcp write timed out",
                )
            })?
            .map_err(map_write_error)?;
            Ok(TcpListenWriteResult {
                sent_bytes: bytes.len(),
                mode: TcpListenWriteMode::Peer,
                peer_id: Some(peer_id.to_owned()),
                successful_peer_ids: vec![peer_id.to_owned()],
                failed_peer_count: 0,
            })
        }
        None => {
            if peers.is_empty() {
                return Err(TransportError::transport_closed(
                    "tcp listen has no connected peers",
                ));
            }
            let peer_ids = peers.keys().cloned().collect::<Vec<_>>();
            let mut successful_peer_ids = Vec::new();
            let mut failed_peer_ids = Vec::new();
            for peer_id in peer_ids {
                let Some(peer) = peers.get_mut(&peer_id) else {
                    continue;
                };
                let result = timeout(
                    Duration::from_millis(timeout_ms),
                    peer.writer.write_all(bytes),
                )
                .await
                .map_err(|_| {
                    TransportError::write_failed(
                        crate::model::ErrorCode::WriteIoFailed,
                        "tcp write timed out",
                    )
                })
                .and_then(|result| result.map_err(map_write_error));
                match result {
                    Ok(()) => successful_peer_ids.push(peer_id),
                    Err(_) => failed_peer_ids.push(peer_id),
                }
            }
            for peer_id in &failed_peer_ids {
                peers.remove(peer_id);
            }
            if successful_peer_ids.is_empty() {
                return Err(TransportError::transport_closed(
                    "tcp broadcast failed for all peers",
                ));
            }
            Ok(TcpListenWriteResult {
                sent_bytes: bytes.len() * successful_peer_ids.len(),
                mode: TcpListenWriteMode::Broadcast,
                peer_id: None,
                successful_peer_ids,
                failed_peer_count: failed_peer_ids.len(),
            })
        }
    }
}

fn read_tcp_listen_frame(
    inbound: &mut VecDeque<TcpInboundFrame>,
    peer_id: Option<&str>,
    max_bytes: usize,
) -> Option<TcpListenReadResult> {
    let index = match peer_id {
        Some(peer_id) => inbound.iter().position(|frame| frame.peer_id == peer_id)?,
        None => {
            if inbound.is_empty() {
                return None;
            }
            0
        }
    };
    let mut frame = inbound.remove(index)?;
    if frame.bytes.len() > max_bytes {
        let remaining = frame.bytes.split_off(max_bytes);
        let result_bytes = std::mem::replace(&mut frame.bytes, remaining);
        let result = TcpListenReadResult {
            peer_id: frame.peer_id.clone(),
            remote_addr: frame.remote_addr.to_string(),
            bytes: result_bytes,
            remaining_frames: inbound.len() + 1,
        };
        inbound.push_front(frame);
        return Some(result);
    }
    Some(TcpListenReadResult {
        peer_id: frame.peer_id,
        remote_addr: frame.remote_addr.to_string(),
        bytes: frame.bytes,
        remaining_frames: inbound.len(),
    })
}

fn receive_worker_reply<T>(
    receiver: mpsc::Receiver<Result<T, TransportError>>,
    timeout_ms: u64,
) -> Result<T, TransportError> {
    receiver
        .recv_timeout(Duration::from_millis(timeout_ms))
        .map_err(|error| match error {
            mpsc::RecvTimeoutError::Timeout => {
                TransportError::read_timeout("tcp worker response timed out")
            }
            mpsc::RecvTimeoutError::Disconnected => {
                TransportError::transport_closed("tcp worker thread exited unexpectedly")
            }
        })?
}

#[derive(Debug)]
pub struct TcpListenTransport {
    listener: TcpListener,
}

impl TcpListenTransport {
    pub async fn bind(host: &str, port: u16) -> Result<Self, TransportError> {
        let listener = TcpListener::bind((host, port))
            .await
            .map_err(map_tcp_bind_error)?;
        Ok(Self { listener })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.listener
            .local_addr()
            .expect("tcp listener should have a local address")
    }

    pub async fn accept_one(&self) -> Result<TcpClientTransport, TransportError> {
        let (stream, _) = self.listener.accept().await.map_err(map_read_error)?;
        Ok(TcpClientTransport { stream })
    }

    async fn accept_stream(&self) -> Result<(TcpStream, SocketAddr), TransportError> {
        self.listener.accept().await.map_err(map_read_error)
    }

    pub async fn close(self) -> Result<(), TransportError> {
        drop(self);
        Ok(())
    }
}
