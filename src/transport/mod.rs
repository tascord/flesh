use {
    crate::{events::EventTarget, resolution::resolver::Resolver},
    async_trait::async_trait,
    futures::StreamExt,
    std::{io, ops::Deref, sync::Arc},
};

pub mod lora;
pub mod udp;

#[async_trait]
pub trait Transport: Deref<Target = EventTarget<Vec<u8>>> {
    async fn receive(&self) -> io::Result<Arc<Vec<u8>>> {
        self.as_stream().next().await.ok_or(std::io::Error::new(io::ErrorKind::Interrupted, "Stream closed"))
    }

    fn resolver(&self) -> &Resolver;
    async fn send(&self, data: &[u8]);
}
