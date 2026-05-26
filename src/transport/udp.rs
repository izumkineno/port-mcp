use std::{net::SocketAddr, time::Duration};

use tokio::{net::UdpSocket, time::timeout};

use super::{
    TransportError, ensure_loopback_host, map_read_error, map_udp_bind_error, map_write_error,
};

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
