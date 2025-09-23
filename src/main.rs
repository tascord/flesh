use tracing::level_filters::LevelFilter;

use {
    flesh::{
        resolution::encoding::Message,
        transport::{Transport, udp::UdpTransport},
    },
    futures::StreamExt,
    std::io,
    tokio::{
        io::{AsyncBufReadExt, BufReader},
        select, spawn,
        sync::mpsc::unbounded_channel,
    },
};

#[tokio::main]
pub async fn main() -> io::Result<()> {
    tracing_subscriber::fmt().pretty().with_thread_names(true).with_max_level(LevelFilter::TRACE).init();

    let bind_addr = "127.0.0.1:1234".parse().unwrap();
    let transport = UdpTransport::new(bind_addr)?;

    let (tx_to_send, mut rx_to_send) = unbounded_channel::<Vec<u8>>();
    spawn(async move {
        let mut reader = BufReader::new(tokio::io::stdin()).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            let _ = tx_to_send.send(line.into_bytes());
        }
    });

    let mut transport_stream = transport.as_stream();

    loop {
        select! {
            res = transport_stream.next() => {
                if let Some(data) = res && let Ok(msg) = Message::deserialize(&String::from_utf8_lossy(&data), &transport) {
                    println!("Received message: {}", String::from_utf8_lossy(msg.body()));
                }
            },

            res = rx_to_send.recv() => {
                if let Some(line_data) = res {
                    let message = Message::new()
                        .with_body(line_data)
                        .signed(&transport);

                    transport.send(message.as_bytes()).await;
                }
            }
        }
    }
}
