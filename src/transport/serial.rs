#[cfg(test)]
use std::collections::VecDeque;
use std::{
    io,
    sync::mpsc,
    thread::{self, JoinHandle},
    time::Duration,
};

use crate::model::{
    DataBits, ErrorCategory, ErrorCode, FlowControl, Parity, SerialConfig, StopBits,
};

use super::TransportError;

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
pub(crate) struct ScriptedSerialDevice {
    reads: VecDeque<Vec<u8>>,
    writes: Vec<Vec<u8>>,
    closed: bool,
}

#[cfg(test)]
impl ScriptedSerialDevice {
    pub(crate) fn new(reads: Vec<Vec<u8>>) -> Self {
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
    pub(crate) fn start_for_tests(device: ScriptedSerialDevice) -> Self {
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
pub(crate) fn map_serial_error_for_tests(
    kind: serialport::ErrorKind,
    message: &str,
) -> TransportError {
    map_serial_error_kind(kind, message)
}

#[cfg(test)]
pub(crate) fn serial_open_timeout_for_tests(_port: &str) -> TransportError {
    map_serial_error_kind(
        serialport::ErrorKind::Io(io::ErrorKind::TimedOut),
        "timed out",
    )
}
