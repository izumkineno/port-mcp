#![allow(dead_code)]

mod common;
mod mock;
mod serial;
mod tcp;
mod udp;

#[allow(unused_imports)]
pub use common::{ScanResult, TransportError, port_scan_loopback};
#[allow(unused_imports)]
pub use mock::MockTransport;
#[allow(unused_imports)]
pub use serial::{SerialPortSettings, SerialPortSummary, SerialWorker, scan_serial_ports};
#[allow(unused_imports)]
pub use tcp::{TcpClientTransport, TcpListenTransport};
#[allow(unused_imports)]
pub use udp::{UdpDatagram, UdpTransport};

pub(crate) use common::{
    ensure_loopback_host, map_read_error, map_tcp_bind_error, map_tcp_connect_error,
    map_udp_bind_error, map_write_error,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        DataBits, ErrorCategory, ErrorCode, FlowControl, Parity, PayloadEncoding, SerialConfig,
        StopBits,
    };
    use std::{io, time::Duration};

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
        let device = serial::ScriptedSerialDevice::new(vec![b"pong".to_vec()]);
        let worker = SerialWorker::start_for_tests(device);

        assert_eq!(worker.write(b"ping", 100).unwrap(), 4);
        assert_eq!(worker.read(8, 100).unwrap(), b"pong".to_vec());
        worker.close(100).unwrap();

        let closed = worker.write(b"after", 100).unwrap_err();
        assert_eq!(closed.code, ErrorCode::TransportClosed);
    }

    #[test]
    fn unit_serial_errors_map_without_raw_os_text() {
        let busy = serial::map_serial_error_for_tests(
            serialport::ErrorKind::Io(io::ErrorKind::PermissionDenied),
            "access denied at C:\\Users\\alice\\secret",
        );
        assert_eq!(busy.category, ErrorCategory::ResourceBusy);
        assert_eq!(busy.code, ErrorCode::SerialPortBusy);
        assert!(!busy.message.contains("alice"));

        let missing =
            serial::map_serial_error_for_tests(serialport::ErrorKind::NoDevice, "COM404 missing");
        assert_eq!(missing.category, ErrorCategory::InvalidArgument);
        assert_eq!(missing.code, ErrorCode::InvalidAddress);

        let timeout = serial::serial_open_timeout_for_tests("COM9");
        assert_eq!(timeout.category, ErrorCategory::ConnectTimeout);
        assert_eq!(timeout.code, ErrorCode::SerialOpenTimeout);
    }
}
