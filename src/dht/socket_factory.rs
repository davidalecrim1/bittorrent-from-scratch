use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;
use tokio::net::UdpSocket;

#[async_trait]
pub trait UdpSocketFactory: Send + Sync + std::fmt::Debug {
    /// Binds a UDP socket to the specified address and returns it wrapped in an Arc.
    async fn bind(&self, addr: &str) -> Result<Arc<UdpSocket>>;
}

#[derive(Debug, Clone)]
pub struct DefaultUdpSocketFactory;

#[async_trait]
impl UdpSocketFactory for DefaultUdpSocketFactory {
    /// Binds a UDP socket to the specified address and returns it wrapped in an Arc.
    async fn bind(&self, addr: &str) -> Result<Arc<UdpSocket>> {
        let socket = UdpSocket::bind(addr).await?;
        Ok(Arc::new(socket))
    }
}
