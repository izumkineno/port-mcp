use std::{
    net::SocketAddr,
    sync::mpsc,
    thread::{self, JoinHandle},
    time::Duration,
};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    runtime::Builder,
    time::timeout,
};

use super::{
    TransportError, map_read_error, map_tcp_bind_error, map_tcp_connect_error, map_write_error,
};

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
    _thread: JoinHandle<()>,
    timeout_ms: u64,
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
                _thread: thread,
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
    _thread: JoinHandle<()>,
    timeout_ms: u64,
}

impl TcpListenWorker {
    pub fn bind(host: &str, port: u16, timeout_ms: u64) -> Result<Self, TransportError> {
        let host = host.to_owned();
        let (commands, receiver) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::channel();
        let thread = thread::spawn(move || {
            let mut runtime = transport_runtime();
            let result = runtime.block_on(TcpListenTransport::bind(&host, port));
            match result {
                Ok(listener) => {
                    let _ = ready_tx.send(Ok(()));
                    run_tcp_listen_worker(&mut runtime, listener, timeout_ms, receiver);
                }
                Err(error) => {
                    let _ = ready_tx.send(Err(error));
                }
            }
        });
        match ready_rx.recv_timeout(Duration::from_millis(timeout_ms.saturating_add(1_000))) {
            Ok(Ok(())) => Ok(Self {
                commands,
                _thread: thread,
                timeout_ms,
            }),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(TransportError::connect_timeout(
                "tcp listen initialization timed out",
            )),
        }
    }

    pub fn write(&self, bytes: &[u8]) -> Result<usize, TransportError> {
        self.request(|reply| TcpListenCommand::Write(bytes.to_vec(), reply))
    }

    pub fn read(&self, max_bytes: usize) -> Result<Vec<u8>, TransportError> {
        self.request(|reply| TcpListenCommand::Read(max_bytes, reply))
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
    Write(Vec<u8>, mpsc::Sender<Result<usize, TransportError>>),
    Read(usize, mpsc::Sender<Result<Vec<u8>, TransportError>>),
    Close(mpsc::Sender<Result<(), TransportError>>),
}

fn transport_runtime() -> tokio::runtime::Runtime {
    Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("transport runtime should be constructible")
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
    receiver: mpsc::Receiver<TcpListenCommand>,
) {
    let mut peer: Option<TcpClientTransport> = None;
    for command in receiver {
        match command {
            TcpListenCommand::Write(bytes, reply) => {
                let result = ensure_tcp_listen_peer(&runtime, &listener, &mut peer, timeout_ms)
                    .and_then(|_| {
                        let peer_transport = peer.as_mut().expect("peer should be present");
                        runtime.block_on(async {
                            timeout(
                                Duration::from_millis(timeout_ms),
                                peer_transport.write_all(&bytes),
                            )
                            .await
                            .map_err(|_| {
                                TransportError::write_failed(
                                    crate::model::ErrorCode::WriteIoFailed,
                                    "tcp write timed out",
                                )
                            })?
                        })
                    });
                if matches!(&result, Err(error) if error.code == crate::model::ErrorCode::TransportClosed)
                {
                    peer = None;
                }
                let _ = reply.send(result);
            }
            TcpListenCommand::Read(max_bytes, reply) => {
                let result = ensure_tcp_listen_peer(&runtime, &listener, &mut peer, timeout_ms)
                    .and_then(|_| {
                        let peer_transport = peer.as_mut().expect("peer should be present");
                        runtime.block_on(async {
                            timeout(
                                Duration::from_millis(timeout_ms),
                                peer_transport.read_chunk(max_bytes),
                            )
                            .await
                            .map_err(|_| TransportError::read_timeout("tcp read timed out"))?
                        })
                    });
                if matches!(&result, Err(error) if error.code == crate::model::ErrorCode::TransportClosed)
                {
                    peer = None;
                }
                let _ = reply.send(result);
            }
            TcpListenCommand::Close(reply) => {
                let peer_result = if let Some(peer_transport) = peer.take() {
                    runtime.block_on(peer_transport.close())
                } else {
                    Ok(())
                };
                let listener_result = runtime.block_on(listener.close());
                let result = peer_result.and(listener_result);
                let _ = reply.send(result);
                break;
            }
        }
    }
}

fn ensure_tcp_listen_peer(
    runtime: &tokio::runtime::Runtime,
    listener: &TcpListenTransport,
    peer: &mut Option<TcpClientTransport>,
    timeout_ms: u64,
) -> Result<(), TransportError> {
    if peer.is_some() {
        return Ok(());
    }
    let accepted = runtime.block_on(async {
        timeout(Duration::from_millis(timeout_ms), listener.accept_one())
            .await
            .map_err(|_| TransportError::connect_timeout("tcp listen accept timed out"))?
    })?;
    *peer = Some(accepted);
    Ok(())
}

fn receive_worker_reply<T>(
    receiver: mpsc::Receiver<Result<T, TransportError>>,
    timeout_ms: u64,
) -> Result<T, TransportError> {
    receiver
        .recv_timeout(Duration::from_millis(timeout_ms))
        .map_err(|_| TransportError::read_timeout("tcp worker response timed out"))?
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

    pub async fn close(self) -> Result<(), TransportError> {
        drop(self);
        Ok(())
    }
}
