use crate::traits::TcpConnector;
use anyhow::{Context, Result};
use async_trait::async_trait;
use std::time::Duration;
use tokio::net::TcpStream;

#[derive(Debug)]
pub struct RealTcpConnector;

#[async_trait]
impl TcpConnector for RealTcpConnector {
    async fn connect(&self, addr: String) -> Result<TcpStream> {
        tokio::time::timeout(Duration::from_secs(5), TcpStream::connect(&addr))
            .await
            .with_context(|| format!("connection timeout to {}", addr))?
            .with_context(|| format!("failed to connect to {}", addr))
    }
}
