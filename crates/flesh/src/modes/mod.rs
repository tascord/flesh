use {
    crate::{events::EventTarget, transport::PacketTransport},
    async_trait::async_trait,
    std::ops::Deref,
};

pub mod lora;

#[async_trait]
pub trait Mode: Deref<Target = EventTarget<Vec<u8>>> + PacketTransport {}
