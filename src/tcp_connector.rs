use crate::traits::TcpConnector;
use anyhow::{Context, Result};
use async_trait::async_trait;
use tokio::net::TcpStream;

#[derive(Debug)]
pub struct RealTcpConnector;

#[async_trait]
impl TcpConnector for RealTcpConnector {
    async fn connect(&self, addr: String) -> Result<TcpStream> {
        TcpStream::connect(&addr)
            .await
            .with_context(|| format!("failed to connect to {}", addr))
    }
}
