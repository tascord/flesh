use {
    flesh::transport::{Transport, udp::UdpTransport},
    futures::StreamExt,
};

#[tokio::main]
pub async fn main() {
    let node = UdpTransport::new("127.0.0.1:1234".parse().unwrap()).unwrap();
    node.as_stream().next().await;
}
