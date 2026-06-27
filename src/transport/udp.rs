use std::{
    net::SocketAddr,
    net::ToSocketAddrs,
    sync::mpsc,
    thread::{self, JoinHandle},
    time::Duration,
};

use tokio::{net::UdpSocket, time::timeout};

use super::{
    TransportError, map_read_error, map_udp_bind_error, map_write_error, transport_runtime,
};

#[derive(Debug)]
pub struct UdpTransport {
    socket: UdpSocket,
}

impl UdpTransport {
    pub async fn bind(host: &str, port: u16) -> Result<Self, TransportError> {
        let address = format!("{host}:{port}");
        let socket = UdpSocket::bind(&address)
            .await
            .map_err(map_udp_bind_error)?;
        Ok(Self { socket })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.socket
            .local_addr()
            .expect("udp socket should have a local address")
    }

    pub async fn send_to(&self, bytes: &[u8], peer: SocketAddr) -> Result<usize, TransportError> {
        self.socket
            .send_to(bytes, peer)
            .await
            .map_err(map_write_error)
    }

    pub async fn recv_datagram(
        &mut self,
        max_bytes: usize,
        timeout_ms: u64,
    ) -> Result<UdpDatagram, TransportError> {
        let mut buffer = vec![0; max_bytes];
        let (read, peer) = timeout(
            Duration::from_millis(timeout_ms),
            self.socket.recv_from(&mut buffer),
        )
        .await
        .map_err(|_| TransportError::read_timeout("udp receive timed out"))?
        .map_err(map_read_error)?;
        buffer.truncate(read);
        Ok(UdpDatagram {
            bytes: buffer,
            peer,
            datagram: true,
        })
    }

    pub async fn close(self) -> Result<(), TransportError> {
        drop(self);
        Ok(())
    }
}

#[derive(Debug)]
pub struct UdpWorker {
    commands: Option<mpsc::Sender<UdpCommand>>,
    thread: Option<JoinHandle<()>>,
    timeout_ms: u64,
    local_addr: SocketAddr,
}

impl Drop for UdpWorker {
    fn drop(&mut self) {
        let _ = self.commands.take();
        if let Some(handle) = self.thread.take() {
            if let Err(panic) = handle.join() {
                let msg = panic
                    .downcast_ref::<&str>()
                    .map(|s| s.to_string())
                    .or_else(|| panic.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "unknown panic".to_owned());
                tracing::error!(%msg, "udp worker thread panicked");
            }
        }
    }
}

impl UdpWorker {
    pub fn bind(host: &str, port: u16, timeout_ms: u64) -> Result<Self, TransportError> {
        let host = host.to_owned();
        let (commands, receiver) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::channel();
        let thread = thread::spawn(move || {
            let runtime = transport_runtime();
            let result = runtime.block_on(UdpTransport::bind(&host, port));
            match result {
                Ok(transport) => {
                    let local_addr = transport.local_addr();
                    let _ = ready_tx.send(Ok(local_addr));
                    run_udp_worker(runtime, transport, timeout_ms, receiver);
                }
                Err(error) => {
                    let _ = ready_tx.send(Err(error));
                }
            }
        });
        match ready_rx.recv_timeout(Duration::from_millis(timeout_ms.saturating_add(1_000))) {
            Ok(Ok(local_addr)) => Ok(Self {
                commands: Some(commands),
                thread: Some(thread),
                timeout_ms,
                local_addr,
            }),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(TransportError::connect_timeout(
                "udp worker initialization timed out",
            )),
        }
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    pub fn send_to(&self, bytes: &[u8], peer: SocketAddr) -> Result<usize, TransportError> {
        self.request(|reply| UdpCommand::Send(bytes.to_vec(), peer, reply))
    }

    pub fn recv(&self, max_bytes: usize) -> Result<Vec<u8>, TransportError> {
        self.request(|reply| UdpCommand::Recv(max_bytes, reply))
    }

    pub fn close(&self) -> Result<(), TransportError> {
        self.request(UdpCommand::Close)
    }

    fn request<T>(
        &self,
        build: impl FnOnce(mpsc::Sender<Result<T, TransportError>>) -> UdpCommand,
    ) -> Result<T, TransportError> {
        let (reply, receiver) = mpsc::channel();
        self.commands
            .as_ref()
            .expect("udp worker command sender should exist")
            .send(build(reply))
            .map_err(|_| TransportError::transport_closed("udp worker is closed"))?;
        receive_worker_reply(receiver, self.timeout_ms)
    }
}

enum UdpCommand {
    Send(
        Vec<u8>,
        SocketAddr,
        mpsc::Sender<Result<usize, TransportError>>,
    ),
    Recv(usize, mpsc::Sender<Result<Vec<u8>, TransportError>>),
    Close(mpsc::Sender<Result<(), TransportError>>),
}

fn run_udp_worker(
    runtime: tokio::runtime::Runtime,
    mut transport: UdpTransport,
    timeout_ms: u64,
    receiver: mpsc::Receiver<UdpCommand>,
) {
    for command in receiver {
        match command {
            UdpCommand::Send(bytes, peer, reply) => {
                let result = runtime.block_on(async {
                    timeout(
                        Duration::from_millis(timeout_ms),
                        transport.send_to(&bytes, peer),
                    )
                    .await
                    .map_err(|_| {
                        TransportError::write_failed(
                            crate::model::ErrorCode::WriteIoFailed,
                            "udp send timed out",
                        )
                    })?
                });
                let _ = reply.send(result);
            }
            UdpCommand::Recv(max_bytes, reply) => {
                let result = runtime.block_on(async {
                    timeout(
                        Duration::from_millis(timeout_ms),
                        transport.recv_datagram(max_bytes, timeout_ms),
                    )
                    .await
                    .map_err(|_| TransportError::read_timeout("udp receive timed out"))?
                    .map(|datagram| datagram.bytes)
                });
                let _ = reply.send(result);
            }
            UdpCommand::Close(reply) => {
                let _ = reply.send(runtime.block_on(transport.close()));
                break;
            }
        }
    }
}

fn receive_worker_reply<T>(
    receiver: mpsc::Receiver<Result<T, TransportError>>,
    timeout_ms: u64,
) -> Result<T, TransportError> {
    receiver
        .recv_timeout(Duration::from_millis(timeout_ms))
        .map_err(|error| match error {
            mpsc::RecvTimeoutError::Timeout => {
                TransportError::read_timeout("udp worker response timed out")
            }
            mpsc::RecvTimeoutError::Disconnected => {
                TransportError::transport_closed("udp worker thread exited unexpectedly")
            }
        })?
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UdpDatagram {
    pub bytes: Vec<u8>,
    pub peer: SocketAddr,
    pub datagram: bool,
}

pub(crate) fn resolve_udp_peer(host: &str, port: u16) -> Result<SocketAddr, TransportError> {
    (host, port)
        .to_socket_addrs()
        .map_err(|_| TransportError::invalid_address("udp remote endpoint is invalid"))?
        .next()
        .ok_or_else(|| TransportError::invalid_address("udp remote endpoint is invalid"))
}
