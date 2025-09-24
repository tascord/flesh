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
    tracing::{debug, info},
};

pub const RESOLUTION_TTL_SECS: u64 = 5000;
pub const ANNOUNCE_DURATION_SECS: u64 = 30;

pub struct Resolver {
    map: Arc<RwLock<HashMap<Fluid, (Instant, VerifyingKey)>>>,
    target: EventTarget<Message>,
    router_target: EventTarget<RoutingMessage>,
    send: Arc<Box<dyn Fn(Vec<u8>) + Send + Sync + 'static>>,
    pub(crate) key: SigningKey,
    pub id: Fluid,
}

impl Resolver {
    pub fn new(stream: EventStream<Vec<u8>>, send: impl Fn(Vec<u8>) + Send + Sync + 'static) -> Self {
        let mut rng = OsRng;
        let secret = SigningKey::generate(&mut rng);

        let s = Self {
            id: Fluid::new(),
            map: Default::default(),
            target: Default::default(),
            send: Arc::new(Box::new(send)),
            router_target: Default::default(),
            key: secret,
        };

        info!("Resolver #{} started.", s.id);

        spawn(Self::process_loop(s.id, s.key.clone(), stream, s.target.clone(), s.router_target.clone()));
        spawn(Self::handle_requests(
            s.id,
            s.key.verifying_key(),
            s.router_target.as_stream(),
            s.map.clone(),
            s.send.clone(),
        ));

        spawn({
            let send = s.send.clone();
            let id = s.id;
            async move {
                loop {
                    send(
                        Message::new()
                            .with_body(
                                bincode::encode_to_vec::<_, Configuration>(
                                    RoutingMessage::Request { asking: id },
                                    Configuration::default(),
                                )
                                .unwrap_or_default(),
                            )
                            .serialize()
                            .as_bytes()
                            .to_vec(),
                    );

                    tokio::time::sleep(Duration::from_secs(ANNOUNCE_DURATION_SECS)).await;
                }
            }
        });

        s
    }

    async fn process_loop(
        id: Fluid,
        key: SigningKey,
        e: impl Stream<Item = Arc<Vec<u8>>>,
        target: EventTarget<Message>,
        router_target: EventTarget<RoutingMessage>,
    ) {
        e.filter_map(|v| {
            let key = key.clone();
            async move {
                if v.is_empty() {
                    return None;
                }

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

    async fn handle_requests(
        my_id: Fluid,
        my_key: VerifyingKey,
        e: impl Stream<Item = Arc<RoutingMessage>>,
        map: Arc<RwLock<HashMap<Fluid, (Instant, VerifyingKey)>>>,
        send: Arc<Box<dyn Fn(Vec<u8>) + Send + Sync + 'static>>,
    ) {
        e.for_each(async |v| match RoutingMessage::clone(&*v) {
            RoutingMessage::Announce(id) => {
                if !(*map.read().await).contains_key(&id) {
                    send(
                        Message::new()
                            .with_body(
                                bincode::encode_to_vec::<_, Configuration>(
                                    RoutingMessage::Request { asking: id },
                                    Configuration::default(),
                                )
                                .unwrap_or_default(),
                            )
                            .serialize()
                            .as_bytes()
                            .to_vec(),
                    )
                }
            }
            RoutingMessage::Request { asking } => {
                if asking == my_id {
                    send(
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
                            .to_vec(),
                    );
                } else if let Some(key) = map.read().await.get(&asking)
                    && key.0.elapsed() < Duration::from_secs(RESOLUTION_TTL_SECS)
                {
                    send(
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
                            .to_vec(),
                    )
                }
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

    fn deref(&self) -> &Self::Target {
        &self.target
    }
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
