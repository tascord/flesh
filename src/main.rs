use {
    flesh::{
        resolution::encoding::Message,
        transport::{Transport, lora::LoraTransport},
    },
    futures::StreamExt,
    serde::{Deserialize, Serialize},
    std::{io, path::Path},
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

    let transport = LoraTransport::new(
        Path::new("/dev/serial/by-id/usb-Silicon_Labs_CP2102_USB_to_UART_Bridge_Controller_0001-if00-port0").to_path_buf(),
        9600,
    )?;

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
