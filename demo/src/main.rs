use {
    flesh::{
        modes::lora::{Lora, LoraSettings},
        transport::{PacketTransport, encoding::FLESHMessage, network::Network},
    },
    futures::{SinkExt, StreamExt},
    serde::{Deserialize, Serialize},
    std::{env, path::Path, time::Duration},
    tokio::{
        net::{TcpListener, TcpStream},
        select, spawn,
        sync::mpsc::{UnboundedSender, unbounded_channel},
    },
    tokio_tungstenite::{accept_async, tungstenite::protocol::Message},
    tracing::{info, warn},
    tracing_subscriber::filter::LevelFilter,
    uuid::Uuid,
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ChatMessage {
    Text { author: String, content: String, channel: String },
    Join(String),
    Channels(Vec<String>),
    CurrentServer(String),
}

const CHANNELS: &[&str] = &["general", "flesh", "silly"];
const PING_INTERVAL: Duration = Duration::from_secs(5);

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_thread_names(true)
        .with_level(true)
        .with_max_level(LevelFilter::INFO)
        .pretty()
        .with_ansi(true)
        .init();

    let lora = Lora::new(
        Path::new(&env::var("LORA").expect("No LoRa env")).to_path_buf(),
        9600,
        LoraSettings { spread_factor: 9, frequency_hz: 915_000_000, bandwidth_khz: 10 },
        false,
    )
    .await
    .expect("Failed to setup LoRa");

    let network = Network::new(lora.clone());
    let node_id = network.id.clone();

    let (to_lora, mut lora_handler) = unbounded_channel::<ChatMessage>();
    let (to_ws, ws_handler) = tokio::sync::broadcast::channel::<ChatMessage>(10);

    let addr = "127.0.0.1:8080";
    let listener = TcpListener::bind(addr).await?;
    info!("Bound web server to 127.0.0.1:8080");

    // Network -> WS
    spawn({
        let to_ws = to_ws.clone();
        async move {
            let to_ws = to_ws.clone();
            network
                .as_stream()
                .filter_map(|m| async move { serde_json::from_slice(&m.body).ok() })
                .for_each({
                    let to_ws = to_ws.clone();
                    move |m| {
                        let to_ws = to_ws.clone();
                        async move {
                            let _ = to_ws.send(m);
                        }
                    }
                })
                .await
        }
    });

    // WS -> Network
    spawn(async move {
        while let Some(msg) = lora_handler.recv().await {
            let encoded = FLESHMessage::new(flesh::transport::status::Status::Acknowledge)
                .with_body(serde_json::to_vec(&msg).unwrap());

            // Also feedback messages into the ws'.
            let _ = to_ws.send(msg);
            lora.send(&encoded.serialize().unwrap()).await.unwrap();
        }
    });

    while let Ok((stream, _)) = listener.accept().await {
        tokio::spawn(handle_connection(stream, node_id.clone(), ws_handler.resubscribe(), to_lora.clone()));
    }

    Ok(())
}

async fn handle_connection(
    stream: TcpStream,
    id: Uuid,
    mut ws_handler: tokio::sync::broadcast::Receiver<ChatMessage>,
    to_lora: UnboundedSender<ChatMessage>,
) {
    let peer_addr = match stream.peer_addr() {
        Ok(addr) => addr.to_string(),
        Err(_) => "unknown address".to_string(),
    };

    info!("New TCP connection from: {}", peer_addr);
    let ws_stream = match accept_async(stream).await {
        Ok(s) => s,
        Err(e) => {
            warn!("Error during WebSocket handshake from {}: {}", peer_addr, e);
            return;
        }
    };

    info!("WebSocket connection established with: {}", peer_addr);
    let (mut sender, mut reader) = ws_stream.split();

    // Announce ID
    let _ =
        sender.send(Message::Text(serde_json::to_string(&ChatMessage::CurrentServer(id.to_string())).unwrap().into())).await;

    // Announce Channels
    let _ = sender
        .send(Message::Text(
            serde_json::to_string(&ChatMessage::Channels(CHANNELS.iter().map(|v| v.to_string()).collect::<Vec<_>>()))
                .unwrap()
                .into(),
        ))
        .await;

    info!("Sent pleasentries: {}", peer_addr);
    let mut ping_timer = tokio::time::interval(PING_INTERVAL);

    loop {
        select! {
            res = reader.next() => {
                 match res {
                    Some(Ok(msg)) => {
                        if msg.is_close() {
                            info!("WebSocket connection closed gracefully by peer: {}", peer_addr);
                            return;
                        }
                         if let Ok(ref m) = serde_json::de::from_slice::<ChatMessage>(&msg.into_data()) {
                            match m {
                                ChatMessage::Text { .. } => {
                                    let _ = to_lora.send(m.clone());
                                }
                                ChatMessage::Join(..) => {
                                    let _ = to_lora.send(m.clone());
                                }
                                ChatMessage::Channels(_) | ChatMessage::CurrentServer(_) => {
                                    warn!("Client {} sent server-only message type: {:?}", peer_addr, m);
                                }
                            }
                        }
                    }
                    Some(Err(e)) => {
                        warn!("Error receiving message from {}: {}", peer_addr, e);
                    }
                    None => {
                        info!("WebSocket stream ended for: {}", peer_addr);
                        return; // <-- EXIT on stream end (disconnect)
                    }
                }
            },
            msg = ws_handler.recv() => {
                // Handle broadcast Lagged error by skipping, as it's a broadcast
                if let Ok(msg) = msg {
                    if let Ok(text) = serde_json::to_string(&msg) {
                        if let Err(e) = sender.send(Message::Text(text.into())).await {
                            warn!("Failed to send broadcast to {}: {}", peer_addr, e);
                            return; // <-- EXIT on send error (broken pipe)
                        }
                    }
                }
            },
            _ = ping_timer.tick() => {
                if let Err(e) = sender.send(Message::Ping(vec![].into())).await {
                    warn!("Failed to send PING to {}: {}", peer_addr, e);
                    return; // <-- EXIT on failed ping
                }
            }
        }
    }
}
