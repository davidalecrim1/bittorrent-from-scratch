use crate::traits::TcpConnector;
use anyhow::Result;
use async_trait::async_trait;
use tokio::net::TcpStream;

#[derive(Debug)]
pub struct RealTcpConnector;

#[async_trait]
impl TcpConnector for RealTcpConnector {
    async fn connect(&self, addr: String) -> Result<TcpStream> {
        let stream = TcpStream::connect(&addr).await?;
        Ok(stream)
    }
}
