use std::{net::SocketAddr, time::Duration};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    time::timeout,
};

use super::{
    TransportError, ensure_loopback_host, map_read_error, map_tcp_bind_error,
    map_tcp_connect_error, map_write_error,
};

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
