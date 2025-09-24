use {
    flesh::{
        resolution::encoding::Message,
        transport::{Transport, udp::UdpTransport},
    },
    futures::StreamExt,
    serde::{Deserialize, Serialize},
    std::io,
    tokio::{
        io::{AsyncBufReadExt, BufReader},
        select, spawn,
        sync::mpsc::unbounded_channel,
    },
    tracing::level_filters::LevelFilter,
};

#[tokio::main]
pub async fn main() -> io::Result<()> {
    tracing_subscriber::fmt().pretty().with_thread_names(true).with_max_level(LevelFilter::TRACE).init();

    let bind_addr = "127.0.0.1:1234".parse().unwrap();
    let transport = UdpTransport::new(bind_addr)?;

    let (tx_to_send, mut rx_to_send) = unbounded_channel::<String>();
    spawn(async move {
        let mut reader = BufReader::new(tokio::io::stdin()).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            let _ = tx_to_send.send(line);
        }
    });

    let mut transport_stream = transport.resolver().as_stream();

    loop {
        select! {
            res = transport_stream.next() => {
                if let Some(data) = res && let Ok(msg) = serde_json::from_slice::<TextMessage>(data.body()) {
                    println!("Received message: {}", msg.text);
                }
            },

            res = rx_to_send.recv() => {
                if let Some(line) = res {
                    let message = Message::new()
                        .with_body(serde_json::to_string(&TextMessage{ text: line }).unwrap())
                        .signed(&transport);

                    transport.send(message.as_bytes()).await;
                }
            }
        }
    }
}

#[derive(Serialize, Deserialize)]
pub struct TextMessage {
    text: String,
}
