#![allow(dead_code)]

use std::{
    collections::VecDeque,
    io,
    net::SocketAddr,
    sync::mpsc,
    thread::{self, JoinHandle},
    time::Duration,
};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream, UdpSocket},
    time::timeout,
};

use crate::model::{
    DataBits, DomainError, ErrorCategory, ErrorCode, FlowControl, Parity, SerialConfig, StopBits,
};

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SerialPortSummary {
    pub name: String,
    pub display: String,
    pub port_type: String,
}

pub fn scan_serial_ports() -> Result<Vec<SerialPortSummary>, TransportError> {
    let ports = serialport::available_ports().map_err(map_serial_error)?;
    Ok(ports
        .into_iter()
        .map(|port| SerialPortSummary {
            display: summarize_serial_port(&port),
            name: port.port_name,
            port_type: summarize_serial_port_type(&port.port_type),
        })
        .collect())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SerialPortSettings {
    pub port_name: String,
    pub baudrate: u32,
    pub data_bits: serialport::DataBits,
    pub stop_bits: serialport::StopBits,
    pub parity: serialport::Parity,
    pub flow_control: serialport::FlowControl,
    pub timeout: Duration,
}

impl SerialPortSettings {
    pub fn try_from_config(config: &SerialConfig) -> Result<Self, TransportError> {
        let port_name = config.port.trim();
        if port_name.is_empty() {
            return Err(TransportError::invalid_address(
                "serial port name is required",
            ));
        }
        if config.baudrate == 0 {
            return Err(TransportError {
                category: ErrorCategory::InvalidArgument,
                code: ErrorCode::InvalidRange,
                message: "serial baudrate must be greater than zero".to_owned(),
                fatal: false,
            });
        }

        Ok(Self {
            port_name: port_name.to_owned(),
            baudrate: config.baudrate,
            data_bits: map_serial_data_bits(config.data_bits),
            stop_bits: map_serial_stop_bits(config.stop_bits),
            parity: map_serial_parity(config.parity),
            flow_control: map_serial_flow_control(config.flow_control),
            timeout: Duration::from_millis(config.timeout_ms),
        })
    }
}

pub struct SerialWorker {
    commands: mpsc::Sender<SerialCommand>,
    _thread: JoinHandle<()>,
}

impl SerialWorker {
    pub fn open(config: &SerialConfig) -> Result<Self, TransportError> {
        let settings = SerialPortSettings::try_from_config(config)?;
        let port = serialport::new(settings.port_name, settings.baudrate)
            .data_bits(settings.data_bits)
            .stop_bits(settings.stop_bits)
            .parity(settings.parity)
            .flow_control(settings.flow_control)
            .timeout(settings.timeout)
            .open()
            .map_err(map_serial_error)?;
        Ok(Self::start(Box::new(SerialPortDevice { port })))
    }

    fn start(device: Box<dyn SerialDevice>) -> Self {
        let (commands, receiver) = mpsc::channel();
        let worker_thread = thread::spawn(move || run_serial_worker(device, receiver));
        Self {
            commands,
            _thread: worker_thread,
        }
    }

    pub fn write(&self, bytes: &[u8], timeout_ms: u64) -> Result<usize, TransportError> {
        let (reply, receiver) = mpsc::channel();
        self.commands
            .send(SerialCommand::Write(bytes.to_vec(), reply))
            .map_err(|_| TransportError::transport_closed("serial worker is closed"))?;
        receive_worker_reply(receiver, timeout_ms)
    }

    pub fn read(&self, max_bytes: usize, timeout_ms: u64) -> Result<Vec<u8>, TransportError> {
        let (reply, receiver) = mpsc::channel();
        self.commands
            .send(SerialCommand::Read(max_bytes, reply))
            .map_err(|_| TransportError::transport_closed("serial worker is closed"))?;
        receive_worker_reply(receiver, timeout_ms)
    }

    pub fn close(&self, timeout_ms: u64) -> Result<(), TransportError> {
        let (reply, receiver) = mpsc::channel();
        self.commands
            .send(SerialCommand::Close(reply))
            .map_err(|_| TransportError::transport_closed("serial worker is closed"))?;
        receive_worker_reply(receiver, timeout_ms)
    }
}

enum SerialCommand {
    Write(Vec<u8>, mpsc::Sender<Result<usize, TransportError>>),
    Read(usize, mpsc::Sender<Result<Vec<u8>, TransportError>>),
    Close(mpsc::Sender<Result<(), TransportError>>),
}

trait SerialDevice: Send + 'static {
    fn read_chunk(&mut self, max_bytes: usize) -> Result<Vec<u8>, TransportError>;
    fn write_all(&mut self, bytes: &[u8]) -> Result<usize, TransportError>;
    fn close(&mut self) -> Result<(), TransportError>;
}

struct SerialPortDevice {
    port: Box<dyn serialport::SerialPort>,
}

impl SerialDevice for SerialPortDevice {
    fn read_chunk(&mut self, max_bytes: usize) -> Result<Vec<u8>, TransportError> {
        let mut buffer = vec![0; max_bytes];
        let read = self.port.read(&mut buffer).map_err(map_serial_io_error)?;
        if read == 0 {
            return Err(TransportError::read_timeout("serial read returned no data"));
        }
        buffer.truncate(read);
        Ok(buffer)
    }

    fn write_all(&mut self, bytes: &[u8]) -> Result<usize, TransportError> {
        self.port.write_all(bytes).map_err(map_serial_io_error)?;
        Ok(bytes.len())
    }

    fn close(&mut self) -> Result<(), TransportError> {
        Ok(())
    }
}

#[cfg(test)]
struct ScriptedSerialDevice {
    reads: VecDeque<Vec<u8>>,
    writes: Vec<Vec<u8>>,
    closed: bool,
}

#[cfg(test)]
impl ScriptedSerialDevice {
    fn new(reads: Vec<Vec<u8>>) -> Self {
        Self {
            reads: VecDeque::from(reads),
            writes: Vec::new(),
            closed: false,
        }
    }
}

#[cfg(test)]
impl SerialDevice for ScriptedSerialDevice {
    fn read_chunk(&mut self, _max_bytes: usize) -> Result<Vec<u8>, TransportError> {
        if self.closed {
            return Err(TransportError::transport_closed(
                "scripted serial is closed",
            ));
        }
        self.reads
            .pop_front()
            .ok_or_else(|| TransportError::read_timeout("scripted serial has no data"))
    }

    fn write_all(&mut self, bytes: &[u8]) -> Result<usize, TransportError> {
        if self.closed {
            return Err(TransportError::transport_closed(
                "scripted serial is closed",
            ));
        }
        self.writes.push(bytes.to_vec());
        Ok(bytes.len())
    }

    fn close(&mut self) -> Result<(), TransportError> {
        self.closed = true;
        Ok(())
    }
}

#[cfg(test)]
impl SerialWorker {
    fn start_for_tests(device: ScriptedSerialDevice) -> Self {
        Self::start(Box::new(device))
    }
}

fn run_serial_worker(mut device: Box<dyn SerialDevice>, receiver: mpsc::Receiver<SerialCommand>) {
    let mut closed = false;
    for command in receiver {
        match command {
            SerialCommand::Write(bytes, reply) => {
                let result = if closed {
                    Err(TransportError::transport_closed("serial worker is closed"))
                } else {
                    device.write_all(&bytes)
                };
                let _ = reply.send(result);
            }
            SerialCommand::Read(max_bytes, reply) => {
                let result = if closed {
                    Err(TransportError::transport_closed("serial worker is closed"))
                } else {
                    device.read_chunk(max_bytes)
                };
                let _ = reply.send(result);
            }
            SerialCommand::Close(reply) => {
                closed = true;
                let _ = reply.send(device.close());
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
        .map_err(|_| TransportError::read_timeout("serial worker response timed out"))?
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

fn map_serial_data_bits(data_bits: DataBits) -> serialport::DataBits {
    match data_bits {
        DataBits::Seven => serialport::DataBits::Seven,
        DataBits::Eight => serialport::DataBits::Eight,
    }
}

fn map_serial_stop_bits(stop_bits: StopBits) -> serialport::StopBits {
    match stop_bits {
        StopBits::One => serialport::StopBits::One,
        StopBits::Two => serialport::StopBits::Two,
    }
}

fn map_serial_parity(parity: Parity) -> serialport::Parity {
    match parity {
        Parity::None => serialport::Parity::None,
        Parity::Odd => serialport::Parity::Odd,
        Parity::Even => serialport::Parity::Even,
    }
}

fn map_serial_flow_control(flow_control: FlowControl) -> serialport::FlowControl {
    match flow_control {
        FlowControl::None => serialport::FlowControl::None,
        FlowControl::Software => serialport::FlowControl::Software,
        FlowControl::Hardware => serialport::FlowControl::Hardware,
    }
}

fn summarize_serial_port(port: &serialport::SerialPortInfo) -> String {
    format!(
        "{} ({})",
        port.port_name,
        summarize_serial_port_type(&port.port_type)
    )
}

fn summarize_serial_port_type(port_type: &serialport::SerialPortType) -> String {
    match port_type {
        serialport::SerialPortType::UsbPort(info) => {
            format!("usb vid={:04x} pid={:04x}", info.vid, info.pid)
        }
        serialport::SerialPortType::PciPort => "pci".to_owned(),
        serialport::SerialPortType::BluetoothPort => "bluetooth".to_owned(),
        serialport::SerialPortType::Unknown => "unknown".to_owned(),
    }
}

fn map_serial_error(error: serialport::Error) -> TransportError {
    map_serial_error_kind(error.kind(), &error.to_string())
}

fn map_serial_io_error(error: io::Error) -> TransportError {
    match error.kind() {
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => {
            TransportError::read_timeout("serial operation timed out")
        }
        io::ErrorKind::PermissionDenied => TransportError {
            category: ErrorCategory::ResourceBusy,
            code: ErrorCode::SerialPortBusy,
            message: "serial port is unavailable or permission was denied".to_owned(),
            fatal: false,
        },
        io::ErrorKind::NotFound => TransportError::invalid_address("serial port was not found"),
        _ => TransportError::write_failed(ErrorCode::WriteIoFailed, "serial I/O failed"),
    }
}

fn map_serial_error_kind(kind: serialport::ErrorKind, _message: &str) -> TransportError {
    match kind {
        serialport::ErrorKind::NoDevice => {
            TransportError::invalid_address("serial port was not found")
        }
        serialport::ErrorKind::Io(io_kind) if matches!(io_kind, io::ErrorKind::TimedOut) => {
            TransportError {
                category: ErrorCategory::ConnectTimeout,
                code: ErrorCode::SerialOpenTimeout,
                message: "serial port open timed out".to_owned(),
                fatal: true,
            }
        }
        serialport::ErrorKind::Io(io_kind)
            if matches!(
                io_kind,
                io::ErrorKind::PermissionDenied | io::ErrorKind::AddrInUse
            ) =>
        {
            TransportError {
                category: ErrorCategory::ResourceBusy,
                code: ErrorCode::SerialPortBusy,
                message: "serial port is busy or permission was denied".to_owned(),
                fatal: false,
            }
        }
        serialport::ErrorKind::InvalidInput => TransportError {
            category: ErrorCategory::InvalidArgument,
            code: ErrorCode::InvalidRange,
            message: "serial configuration is invalid".to_owned(),
            fatal: false,
        },
        _ => TransportError::write_failed(ErrorCode::WriteIoFailed, "serial operation failed"),
    }
}

#[cfg(test)]
fn map_serial_error_for_tests(kind: serialport::ErrorKind, message: &str) -> TransportError {
    map_serial_error_kind(kind, message)
}

#[cfg(test)]
fn serial_open_timeout_for_tests(_port: &str) -> TransportError {
    map_serial_error_kind(
        serialport::ErrorKind::Io(io::ErrorKind::TimedOut),
        "timed out",
    )
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
    use crate::model::{
        DataBits, ErrorCode, FlowControl, Parity, PayloadEncoding, SerialConfig, StopBits,
    };

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

    #[test]
    fn unit_serial_scan_summarizes_ports_without_sensitive_details() {
        let ports = scan_serial_ports().unwrap();
        for port in ports {
            assert!(!port.name.is_empty());
            assert!(!port.display.contains("Users\\"));
            assert!(!port.display.contains("/home/"));
        }
    }

    #[test]
    fn unit_serial_config_maps_to_serialport_settings() {
        let config = SerialConfig {
            port: "COM9".to_owned(),
            baudrate: 57_600,
            data_bits: DataBits::Seven,
            stop_bits: StopBits::Two,
            parity: Parity::Even,
            flow_control: FlowControl::Hardware,
            timeout_ms: 250,
            encoding: PayloadEncoding::Hex,
        };

        let settings = SerialPortSettings::try_from_config(&config).unwrap();
        assert_eq!(settings.port_name, "COM9");
        assert_eq!(settings.baudrate, 57_600);
        assert_eq!(settings.data_bits, serialport::DataBits::Seven);
        assert_eq!(settings.stop_bits, serialport::StopBits::Two);
        assert_eq!(settings.parity, serialport::Parity::Even);
        assert_eq!(settings.flow_control, serialport::FlowControl::Hardware);
        assert_eq!(settings.timeout, Duration::from_millis(250));

        let invalid = SerialConfig {
            port: "  ".to_owned(),
            ..config
        };
        let error = SerialPortSettings::try_from_config(&invalid).unwrap_err();
        assert_eq!(error.category, ErrorCategory::InvalidArgument);
        assert_eq!(error.code, ErrorCode::InvalidAddress);
    }

    #[test]
    fn unit_serial_worker_reads_writes_and_closes_with_control_messages() {
        let device = ScriptedSerialDevice::new(vec![b"pong".to_vec()]);
        let worker = SerialWorker::start_for_tests(device);

        assert_eq!(worker.write(b"ping", 100).unwrap(), 4);
        assert_eq!(worker.read(8, 100).unwrap(), b"pong".to_vec());
        worker.close(100).unwrap();

        let closed = worker.write(b"after", 100).unwrap_err();
        assert_eq!(closed.code, ErrorCode::TransportClosed);
    }

    #[test]
    fn unit_serial_errors_map_without_raw_os_text() {
        let busy = map_serial_error_for_tests(
            serialport::ErrorKind::Io(io::ErrorKind::PermissionDenied),
            "access denied at C:\\Users\\alice\\secret",
        );
        assert_eq!(busy.category, ErrorCategory::ResourceBusy);
        assert_eq!(busy.code, ErrorCode::SerialPortBusy);
        assert!(!busy.message.contains("alice"));

        let missing = map_serial_error_for_tests(serialport::ErrorKind::NoDevice, "COM404 missing");
        assert_eq!(missing.category, ErrorCategory::InvalidArgument);
        assert_eq!(missing.code, ErrorCode::InvalidAddress);

        let timeout = serial_open_timeout_for_tests("COM9");
        assert_eq!(timeout.category, ErrorCategory::ConnectTimeout);
        assert_eq!(timeout.code, ErrorCode::SerialOpenTimeout);
    }
}
