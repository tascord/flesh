use {async_trait::async_trait, std::io};

pub mod encoding;
pub mod network;
pub mod status;

#[async_trait]
pub trait PacketTransport: Send + Sync {
    /// Sends a single data packet.
    async fn send(&self, data: &[u8]) -> io::Result<()>;

    /// Receives a single data packet.
    async fn recv(&mut self) -> io::Result<Vec<u8>>;
}