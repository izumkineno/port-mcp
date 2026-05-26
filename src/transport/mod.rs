#![allow(dead_code)]

use std::{collections::VecDeque, io, net::SocketAddr, time::Duration};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream, UdpSocket},
    time::timeout,
};

use crate::model::{DomainError, ErrorCategory, ErrorCode};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportError {
    pub category: ErrorCategory,
    pub code: ErrorCode,
    pub message: String,
    pub fatal: bool,
}

impl TransportError {
    pub fn read_timeout(message: impl Into<String>) -> Self {
        Self {
            category: ErrorCategory::ReadTimeout,
            code: ErrorCode::ReadTimeout,
            message: message.into(),
            fatal: false,
        }
    }

    pub fn transport_closed(message: impl Into<String>) -> Self {
        Self {
            category: ErrorCategory::WriteFailed,
            code: ErrorCode::TransportClosed,
            message: message.into(),
            fatal: true,
        }
    }

    pub fn write_failed(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            category: ErrorCategory::WriteFailed,
            code,
            message: message.into(),
            fatal: true,
        }
    }

    pub fn connect_timeout(message: impl Into<String>) -> Self {
        Self {
            category: ErrorCategory::ConnectTimeout,
            code: ErrorCode::ConnectTimeout,
            message: message.into(),
            fatal: true,
        }
    }

    pub fn tcp_listen_busy(message: impl Into<String>) -> Self {
        Self {
            category: ErrorCategory::ResourceBusy,
            code: ErrorCode::TcpListenAddrBusy,
            message: message.into(),
            fatal: false,
        }
    }

    pub fn udp_bind_busy(message: impl Into<String>) -> Self {
        Self {
            category: ErrorCategory::ResourceBusy,
            code: ErrorCode::UdpBindAddrBusy,
            message: message.into(),
            fatal: false,
        }
    }

    pub fn invalid_address(message: impl Into<String>) -> Self {
        Self {
            category: ErrorCategory::InvalidArgument,
            code: ErrorCode::InvalidAddress,
            message: message.into(),
            fatal: false,
        }
    }
}

#[derive(Debug)]
pub struct TcpClientTransport {
    stream: TcpStream,
}

impl TcpClientTransport {
    pub async fn connect(host: &str, port: u16, timeout_ms: u64) -> Result<Self, TransportError> {
        ensure_loopback_host(host)?;
        let address = format!("{host}:{port}");
        let stream = timeout(
            Duration::from_millis(timeout_ms),
            TcpStream::connect(&address),
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
pub struct TcpListenTransport {
    listener: TcpListener,
}

impl TcpListenTransport {
    pub async fn bind(host: &str, port: u16) -> Result<Self, TransportError> {
        ensure_loopback_host(host)?;
        let address = format!("{host}:{port}");
        let listener = TcpListener::bind(&address)
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

#[derive(Debug)]
pub struct UdpTransport {
    socket: UdpSocket,
}

impl UdpTransport {
    pub async fn bind(host: &str, port: u16) -> Result<Self, TransportError> {
        ensure_loopback_host(host)?;
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UdpDatagram {
    pub bytes: Vec<u8>,
    pub peer: SocketAddr,
    pub datagram: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanResult {
    pub open_ports: Vec<u16>,
}

pub async fn port_scan_loopback(
    host: &str,
    start_port: u16,
    end_port: u16,
    max_concurrency: usize,
    timeout_ms: u64,
) -> Result<ScanResult, DomainError> {
    if !is_loopback_single_host(host) {
        return Err(DomainError::invalid_argument(
            ErrorCode::ScanTargetNotAllowed,
            "port_scan target must be a loopback single host.",
            "Use 127.0.0.1 or ::1 for initial loopback scanning.",
        )
        .with_detail("field", serde_json::json!("host")));
    }
    if end_port < start_port {
        return Err(DomainError::invalid_argument(
            ErrorCode::InvalidRange,
            "port_scan end_port must be greater than or equal to start_port.",
            "Use a valid inclusive port range.",
        )
        .with_detail("field", serde_json::json!("end_port")));
    }
    let port_count = usize::from(end_port - start_port) + 1;
    if port_count > 256 || max_concurrency == 0 || max_concurrency > 64 {
        return Err(DomainError::new(
            ErrorCategory::BufferLimitExceeded,
            ErrorCode::ScanRangeTooLarge,
            "port_scan range or concurrency exceeds the M5 loopback limit.",
            "Reduce port range to at most 256 ports and concurrency to 1..64.",
            false,
        )
        .with_detail("limit", serde_json::json!(256))
        .with_detail("requested", serde_json::json!(port_count)));
    }

    let mut open_ports = Vec::new();
    for port in start_port..=end_port {
        let address = format!("{host}:{port}");
        if let Ok(Ok(stream)) = timeout(
            Duration::from_millis(timeout_ms),
            TcpStream::connect(address),
        )
        .await
        {
            drop(stream);
            open_ports.push(port);
        }
    }
    Ok(ScanResult { open_ports })
}

fn ensure_loopback_host(host: &str) -> Result<(), TransportError> {
    if is_loopback_single_host(host) {
        Ok(())
    } else {
        Err(TransportError::invalid_address(
            "network transport is restricted to loopback during M5",
        ))
    }
}

fn is_loopback_single_host(host: &str) -> bool {
    let trimmed = host.trim();
    trimmed
        .parse::<std::net::IpAddr>()
        .map(|address| address.is_loopback() && !matches!(trimmed, "0.0.0.0" | "::"))
        .unwrap_or(false)
}

fn map_tcp_connect_error(error: io::Error) -> TransportError {
    if matches!(error.kind(), io::ErrorKind::TimedOut) {
        TransportError::connect_timeout("tcp connect timed out")
    } else {
        TransportError::transport_closed("tcp connect failed")
    }
}

fn map_tcp_bind_error(error: io::Error) -> TransportError {
    if matches!(error.kind(), io::ErrorKind::AddrInUse) {
        TransportError::tcp_listen_busy("tcp listen address is already in use")
    } else {
        TransportError::write_failed(ErrorCode::WriteIoFailed, "tcp listen bind failed")
    }
}

fn map_udp_bind_error(error: io::Error) -> TransportError {
    if matches!(error.kind(), io::ErrorKind::AddrInUse) {
        TransportError::udp_bind_busy("udp bind address is already in use")
    } else {
        TransportError::write_failed(ErrorCode::WriteIoFailed, "udp bind failed")
    }
}

fn map_read_error(error: io::Error) -> TransportError {
    if matches!(
        error.kind(),
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
    ) {
        TransportError::read_timeout("transport read timed out")
    } else {
        TransportError::write_failed(ErrorCode::ReadIoFailed, "transport read failed")
    }
}

fn map_write_error(error: io::Error) -> TransportError {
    if matches!(
        error.kind(),
        io::ErrorKind::BrokenPipe | io::ErrorKind::ConnectionReset
    ) {
        TransportError::transport_closed("transport peer closed")
    } else {
        TransportError::write_failed(ErrorCode::WriteIoFailed, "transport write failed")
    }
}

#[derive(Debug, Default)]
pub struct MockTransport {
    reads: VecDeque<Vec<u8>>,
    writes: Vec<Vec<u8>>,
    next_write_error: Option<TransportError>,
    closed: bool,
}

impl MockTransport {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn inject_read(&mut self, bytes: &[u8]) {
        self.reads.push_back(bytes.to_vec());
    }

    pub fn read_chunk(&mut self, _max_bytes: usize) -> Result<Vec<u8>, TransportError> {
        if self.closed {
            return Err(TransportError::transport_closed("mock transport is closed"));
        }
        self.reads
            .pop_front()
            .ok_or_else(|| TransportError::read_timeout("mock transport has no injected reads"))
    }

    pub fn write_all(&mut self, bytes: &[u8]) -> Result<usize, TransportError> {
        if self.closed {
            return Err(TransportError::transport_closed("mock transport is closed"));
        }
        if let Some(error) = self.next_write_error.take() {
            return Err(error);
        }
        self.writes.push(bytes.to_vec());
        Ok(bytes.len())
    }

    pub fn writes(&self) -> &[Vec<u8>] {
        &self.writes
    }

    pub fn fail_next_write(&mut self, code: ErrorCode) {
        self.next_write_error = Some(TransportError::write_failed(code, "mock write failed"));
    }

    pub fn close(&mut self) -> Result<(), TransportError> {
        self.closed = true;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ErrorCode;

    #[test]
    fn unit_transport_common_maps_mock_errors_without_deciding_response_shape() {
        let timeout = TransportError::read_timeout("mock read timeout");
        assert_eq!(timeout.code, ErrorCode::ReadTimeout);
        assert!(!timeout.fatal);

        let closed = TransportError::transport_closed("mock closed");
        assert_eq!(closed.code, ErrorCode::TransportClosed);
        assert!(closed.fatal);
    }

    #[test]
    fn integration_mock_transport_injects_reads_observes_writes_and_failures() {
        let mut transport = MockTransport::new();
        transport.inject_read(b"pong");

        let read = transport.read_chunk(8).unwrap();
        assert_eq!(read, b"pong".to_vec());

        let written = transport.write_all(b"ping").unwrap();
        assert_eq!(written, 4);
        assert_eq!(transport.writes(), &[b"ping".to_vec()]);

        transport.fail_next_write(ErrorCode::WriteIoFailed);
        let failed = transport.write_all(b"boom").unwrap_err();
        assert_eq!(failed.code, ErrorCode::WriteIoFailed);

        transport.close().unwrap();
        let closed = transport.read_chunk(1).unwrap_err();
        assert_eq!(closed.code, ErrorCode::TransportClosed);
    }

    #[tokio::test]
    async fn integration_tcp_loopback_client_round_trips() {
        let listener = TcpListenTransport::bind("127.0.0.1", 0).await.unwrap();
        let address = listener.local_addr();
        let server = tokio::spawn(async move {
            let mut peer = listener.accept_one().await.unwrap();
            let bytes = peer.read_chunk(4).await.unwrap();
            assert_eq!(bytes, b"ping".to_vec());
            peer.write_all(b"pong").await.unwrap();
        });

        let mut client = TcpClientTransport::connect("127.0.0.1", address.port(), 1_000)
            .await
            .unwrap();
        client.write_all(b"ping").await.unwrap();
        assert_eq!(client.read_chunk(4).await.unwrap(), b"pong".to_vec());
        client.close().await.unwrap();
        server.await.unwrap();
    }

    #[tokio::test]
    async fn integration_tcp_listen_rejects_address_conflict_and_allows_reuse_after_close() {
        let listener = TcpListenTransport::bind("127.0.0.1", 0).await.unwrap();
        let address = listener.local_addr();

        let busy = TcpListenTransport::bind("127.0.0.1", address.port())
            .await
            .unwrap_err();
        assert_eq!(busy.category, ErrorCategory::ResourceBusy);
        assert_eq!(busy.code, ErrorCode::TcpListenAddrBusy);

        listener.close().await.unwrap();
        let rebound = TcpListenTransport::bind("127.0.0.1", address.port())
            .await
            .unwrap();
        rebound.close().await.unwrap();
    }

    #[tokio::test]
    async fn integration_udp_loopback_datagrams_conflict_and_rebind() {
        let mut server = UdpTransport::bind("127.0.0.1", 0).await.unwrap();
        let server_addr = server.local_addr();
        let server_task = tokio::spawn(async move {
            let datagram = server.recv_datagram(16, 1_000).await.unwrap();
            assert_eq!(datagram.bytes, b"ping".to_vec());
            server.send_to(b"pong", datagram.peer).await.unwrap();
            server.close().await.unwrap();
        });

        let mut client = UdpTransport::bind("127.0.0.1", 0).await.unwrap();
        let client_addr = client.local_addr();
        let busy = UdpTransport::bind("127.0.0.1", client_addr.port())
            .await
            .unwrap_err();
        assert_eq!(busy.category, ErrorCategory::ResourceBusy);
        assert_eq!(busy.code, ErrorCode::UdpBindAddrBusy);

        client.send_to(b"ping", server_addr).await.unwrap();
        let response = client.recv_datagram(16, 1_000).await.unwrap();
        assert_eq!(response.bytes, b"pong".to_vec());
        assert!(response.datagram);
        client.close().await.unwrap();
        server_task.await.unwrap();

        let rebound = UdpTransport::bind("127.0.0.1", client_addr.port())
            .await
            .unwrap();
        rebound.close().await.unwrap();
    }

    #[tokio::test]
    async fn integration_port_scan_loopback_rejects_unsafe_targets_and_finds_open_port() {
        let listener = TcpListenTransport::bind("127.0.0.1", 0).await.unwrap();
        let open_port = listener.local_addr().port();

        let unsafe_target = port_scan_loopback("0.0.0.0", open_port, open_port, 8, 100)
            .await
            .unwrap_err();
        assert_eq!(unsafe_target.category, ErrorCategory::InvalidArgument);
        assert_eq!(unsafe_target.code, ErrorCode::ScanTargetNotAllowed);

        let dns_target = port_scan_loopback("localhost", open_port, open_port, 8, 100)
            .await
            .unwrap_err();
        assert_eq!(dns_target.code, ErrorCode::ScanTargetNotAllowed);

        let too_large = port_scan_loopback("127.0.0.1", 1, 300, 8, 100)
            .await
            .unwrap_err();
        assert_eq!(too_large.category, ErrorCategory::BufferLimitExceeded);
        assert_eq!(too_large.code, ErrorCode::ScanRangeTooLarge);

        let result = port_scan_loopback("127.0.0.1", open_port, open_port, 8, 1_000)
            .await
            .unwrap();
        assert_eq!(result.open_ports, vec![open_port]);
        listener.close().await.unwrap();
    }
}
