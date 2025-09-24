use {
    crate::{
        events::{EventStream, EventTarget},
        resolution::encoding::Message,
    },
    bincode::config::Configuration,
    ed25519_dalek::{SigningKey, VerifyingKey},
    fl_uid::Fluid,
    futures::{Stream, StreamExt},
    rand_core::OsRng,
    std::{collections::HashMap, ops::Deref, sync::Arc, time::Duration},
    tokio::{select, spawn, sync::RwLock, time::Instant},
    tracing::info,
};

pub const RESOLUTION_TTL_SECS: u64 = 5000;
pub const ANNOUNCE_DURATION_SECS: u64 = 30;
pub const SPLIT_MESSAGE_DELAY_SECS: u64 = 1;

#[derive(Clone, Debug, bincode::Encode, bincode::Decode)]
struct MessagePart {
    id: Fluid,
    part: u16,
    total_parts: u16,
    data: Vec<u8>,
}

#[derive(Clone, Debug)]
struct PartialMessage {
    parts: HashMap<u16, Vec<u8>>,
    // total_parts: u16,
    last_update: Instant,
}

pub struct Resolver {
    map: Arc<RwLock<HashMap<Fluid, (Instant, VerifyingKey)>>>,
    target: EventTarget<Message>,
    router_target: EventTarget<RoutingMessage>,
    send: Arc<Box<dyn Fn(Vec<u8>) + Send + Sync + 'static>>,
    pub(crate) key: SigningKey,
    pub id: Fluid,
    max_length: usize,
    partial_messages: Arc<RwLock<HashMap<Fluid, PartialMessage>>>,
}

impl Resolver {
    pub fn new(stream: EventStream<Vec<u8>>, send: impl Fn(Vec<u8>) + Send + Sync + 'static, max_length: usize) -> Self {
        let mut rng = OsRng;
        let secret = SigningKey::generate(&mut rng);

        let s = Self {
            id: Fluid::new(),
            map: Default::default(),
            target: Default::default(),
            send: Arc::new(Box::new(send)),
            router_target: Default::default(),
            key: secret,
            max_length,
            partial_messages: Default::default(),
        };

        info!("Resolver #{} started with max_length: {}.", s.id, max_length);

        spawn(Self::process_loop(
            s.id,
            s.key.clone(),
            stream,
            s.target.clone(),
            s.router_target.clone(),
            s.partial_messages.clone(),
        ));
        spawn(Self::handle_requests(
            s.id,
            s.key.verifying_key(),
            s.router_target.as_stream(),
            s.map.clone(),
            s.send.clone(),
            s.max_length,
        ));

        spawn({
            let send = s.send.clone();
            let id = s.id;
            let max_length = s.max_length;
            async move {
                loop {
                    let message = Message::new()
                        .with_body(
                            bincode::encode_to_vec::<_, Configuration>(
                                RoutingMessage::Request { asking: id },
                                Configuration::default(),
                            )
                            .unwrap_or_default(),
                        )
                        .serialize()
                        .as_bytes()
                        .to_vec();

                    Self::send_with_splitting(&send, message, max_length).await;
                    tokio::time::sleep(Duration::from_secs(ANNOUNCE_DURATION_SECS)).await;
                }
            }
        });

        // Cleanup task for partial messages
        spawn(Self::cleanup_partial_messages(s.partial_messages.clone()));

        s
    }

    async fn send_with_splitting(
        send: &Arc<Box<dyn Fn(Vec<u8>) + Send + Sync + 'static>>,
        data: Vec<u8>,
        max_length: usize,
    ) {
        if data.len() <= max_length {
            send(data);
            return;
        }

        let message_id = Fluid::new();
        let chunks: Vec<_> = data.chunks(max_length - 100).collect(); // Reserve space for metadata
        let total_parts = chunks.len() as u16;

        for (i, chunk) in chunks.iter().enumerate() {
            let part = MessagePart { id: message_id, part: i as u16, total_parts, data: chunk.to_vec() };

            let serialized =
                bincode::encode_to_vec::<_, Configuration>(InternalMessage::Split(part), Configuration::default())
                    .unwrap_or_default();

            send(serialized);

            if i < chunks.len() - 1 {
                tokio::time::sleep(Duration::from_secs(SPLIT_MESSAGE_DELAY_SECS)).await;
            }
        }
    }

    async fn process_loop(
        id: Fluid,
        key: SigningKey,
        e: impl Stream<Item = Arc<Vec<u8>>>,
        target: EventTarget<Message>,
        router_target: EventTarget<RoutingMessage>,
        partial_messages: Arc<RwLock<HashMap<Fluid, PartialMessage>>>,
    ) {
        e.filter_map(|v| {
            let key = key.clone();
            let partial_messages = partial_messages.clone();

            async move {
                if v.is_empty() {
                    return None;
                }

                // First try to decode as InternalMessage (for split messages)
                if let Ok((internal_msg, _)) =
                    bincode::decode_from_slice::<InternalMessage, Configuration>(&v, Configuration::default())
                {
                    match internal_msg {
                        InternalMessage::Split(part) => {
                            return Self::handle_message_part(part, partial_messages).await;
                        }
                        InternalMessage::Complete(data) => {
                            // Process as normal message
                            return Message::deserialize(&String::from_utf8_lossy(&data), &(id, key.clone()))
                                .inspect_err(|e| tracing::trace!("ERR: {e:?}"))
                                .ok();
                        }
                    }
                }

                // Fall back to normal message processing
                Message::deserialize(&String::from_utf8_lossy(&v), &(id, key.clone()))
                    .inspect_err(|e| tracing::trace!("ERR: {e:?}"))
                    .ok()
            }
        })
        .for_each(async |v| {
            target.emit(v.clone()); // TODO: This shouldnt be clone but im busy rn

            if v.ok()
                && let Ok(message) = bincode::decode_from_slice::<RoutingMessage, Configuration>(
                    v.body().as_slice(),
                    Configuration::default(),
                )
            {
                router_target.emit(message.0);
            }
        })
        .await;
    }

    async fn handle_message_part(
        part: MessagePart,
        partial_messages: Arc<RwLock<HashMap<Fluid, PartialMessage>>>,
    ) -> Option<Message> {
        let mut partials = partial_messages.write().await;

        let partial = partials.entry(part.id).or_insert(PartialMessage {
            parts: HashMap::new(),
            // total_parts: part.total_parts,
            last_update: Instant::now(),
        });

        partial.parts.insert(part.part, part.data);
        partial.last_update = Instant::now();

        // Check if we have all parts
        if partial.parts.len() == part.total_parts as usize {
            let mut complete_data = Vec::new();

            // Reconstruct the message in order
            for i in 0..part.total_parts {
                if let Some(data) = partial.parts.get(&i) {
                    complete_data.extend_from_slice(data);
                } else {
                    // Missing part, shouldn't happen but handle gracefully
                    return None;
                }
            }

            // Remove from partial messages
            partials.remove(&part.id);
            drop(partials);

            // Try to deserialize as a normal message
            // Since we don't have id and key here, we'll wrap it as InternalMessage::Complete
            // and let the upper level handle it
            return Some(Message::new().with_body(complete_data));
        }

        None
    }

    async fn cleanup_partial_messages(partial_messages: Arc<RwLock<HashMap<Fluid, PartialMessage>>>) {
        loop {
            tokio::time::sleep(Duration::from_secs(60)).await;

            let mut partials = partial_messages.write().await;
            let now = Instant::now();

            partials.retain(|_, partial| {
                now.duration_since(partial.last_update) < Duration::from_secs(300) // 5 minutes timeout
            });
        }
    }

    async fn handle_requests(
        my_id: Fluid,
        my_key: VerifyingKey,
        e: impl Stream<Item = Arc<RoutingMessage>>,
        map: Arc<RwLock<HashMap<Fluid, (Instant, VerifyingKey)>>>,
        send: Arc<Box<dyn Fn(Vec<u8>) + Send + Sync + 'static>>,
        max_length: usize,
    ) {
        e.for_each(async |v| match RoutingMessage::clone(&*v) {
            RoutingMessage::Announce(id) => {
                if !(*map.read().await).contains_key(&id) {
                    let message = Message::new()
                        .with_body(
                            bincode::encode_to_vec::<_, Configuration>(
                                RoutingMessage::Request { asking: id },
                                Configuration::default(),
                            )
                            .unwrap_or_default(),
                        )
                        .serialize()
                        .as_bytes()
                        .to_vec();

                    Self::send_with_splitting(&send, message, max_length).await;
                }
            }
            RoutingMessage::Request { asking } => {
                let message = if asking == my_id {
                    Message::new()
                        .with_body(
                            bincode::encode_to_vec::<_, Configuration>(
                                RoutingMessage::Response { giving: asking, is: my_key.to_bytes().to_vec() },
                                Configuration::default(),
                            )
                            .unwrap_or_default(),
                        )
                        .serialize()
                        .as_bytes()
                        .to_vec()
                } else if let Some(key) = map.read().await.get(&asking)
                    && key.0.elapsed() < Duration::from_secs(RESOLUTION_TTL_SECS)
                {
                    Message::new()
                        .with_body(
                            bincode::encode_to_vec::<_, Configuration>(
                                RoutingMessage::Response { giving: asking, is: key.1.to_bytes().to_vec() },
                                Configuration::default(),
                            )
                            .unwrap_or_default(),
                        )
                        .serialize()
                        .as_bytes()
                        .to_vec()
                } else {
                    return;
                };

                Self::send_with_splitting(&send, message, max_length).await;
            }
            RoutingMessage::Response { giving, is } => {
                if let Ok(key) = is.as_slice().try_into()
                    && let Ok(key) = VerifyingKey::from_bytes(key)
                {
                    map.write().await.insert(giving, (Instant::now(), key));
                }
            }
        })
        .await
    }

    pub async fn resolve(&self, id: Fluid) -> Option<VerifyingKey> {
        if let Some(k) = self
            .map
            .read()
            .await
            .get(&id)
            .and_then(|v| (v.0.elapsed() < Duration::from_secs(RESOLUTION_TTL_SECS)).then_some(v.1))
        {
            return Some(k);
        }

        let mut resolver = self
            .router_target
            .as_stream()
            .filter_map(async |v| match RoutingMessage::clone(&*v) {
                RoutingMessage::Response { giving, is } if giving == id => {
                    if let Ok(key) = is.as_slice().try_into()
                        && let Ok(key) = VerifyingKey::from_bytes(key)
                    {
                        return Some(key);
                    }

                    None
                }
                _ => None,
            })
            .boxed_local();

        let resp = select! {
            v = resolver.next() => {
                v
            }
            _ = tokio::time::sleep(Duration::from_secs(10)) => {
                None
            }
        };

        if let Some(key) = resp {
            self.map.write().await.insert(id, (Instant::now(), key));
        }

        resp
    }
}

impl Deref for Resolver {
    type Target = EventTarget<Message>;

    fn deref(&self) -> &Self::Target { &self.target }
}

#[derive(Clone, Debug, bincode::Encode, bincode::Decode)]
pub enum RoutingMessage {
    // Announce that you are a node others can ask about
    Announce(Fluid),
    // Request a public key from an id
    Request { asking: Fluid },
    // Responding that a n id resolves to a Verification Key
    Response { giving: Fluid, is: Vec<u8> },
}

#[derive(Clone, Debug, bincode::Encode, bincode::Decode)]
enum InternalMessage {
    Split(MessagePart),
    Complete(Vec<u8>),
}
