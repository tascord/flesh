// src/network.rs

use {
    crate::{
        events::EventTarget,
        transport::{
            PacketTransport,
            encoding::{FLESHMessage, MessageType, RoutingMessage},
        },
    },
    ed25519_dalek::{SigningKey, VerifyingKey},
    fl_uid::Fluid,
    futures::{Stream, StreamExt},
    rand_core::OsRng,
    std::{
        collections::HashMap,
        ops::Deref,
        sync::Arc,
        time::{Duration, Instant},
    },
    tokio::{select, spawn, sync::RwLock},
    tracing::{error, info},
};

pub const RESOLUTION_TTL_SECS: u64 = 5000;
pub const ANNOUNCE_DURATION_SECS: u64 = 30;

#[derive(Clone)]
pub struct Network<T: PacketTransport> {
    map: Arc<RwLock<HashMap<Fluid, (Instant, VerifyingKey)>>>,
    target: EventTarget<FLESHMessage>,
    router_target: EventTarget<RoutingMessage>,
    pub(crate) key: SigningKey,
    pub id: Fluid,
    transport: T,
}

impl<T: PacketTransport + Clone + 'static> Network<T> {
    /// Creates a new Network instance that operates over any compatible packet transport.
    pub fn new(transport: T) -> Self {
        let mut rng = OsRng;
        let key = SigningKey::generate(&mut rng);
        let id = Fluid::new();

        let s = Self {
            id,
            key,
            map: Default::default(),
            target: Default::default(),
            router_target: Default::default(),
            transport,
        };

        info!("Resolver #{} started.", s.id);

        // Spawn the main loop that receives all incoming packets from the transport
        spawn(Self::packet_processing_loop(s.target.clone(), s.router_target.clone(), s.transport.clone()));

        // Spawn the handler for internal routing messages (requests/responses for keys)
        spawn(Self::handle_requests(
            s.id,
            s.key.verifying_key(),
            s.router_target.as_stream(),
            s.map.clone(),
            s.transport.clone(),
        ));

        // Spawn the task that periodically broadcasts a discovery message
        spawn(Self::periodic_announcements(s.id, s.transport.clone()));

        s
    }

    /// The main inbound message loop. It continually waits for packets from the
    /// transport, deserializes them, and forwards them to the correct handler.
    async fn packet_processing_loop(
        target: EventTarget<FLESHMessage>,
        router_target: EventTarget<RoutingMessage>,
        mut transport: T,
    ) {
        loop {
            match transport.recv().await {
                Ok(data) => {
                    if let Ok(message) = FLESHMessage::deserialize(&data) {
                        match message.message_type {
                            MessageType::Routing(r) => router_target.emit(r),
                            MessageType::Data { .. } => target.emit(message),
                            _ => {}
                        }
                    }
                }
                Err(e) => {
                    error!("Transport receive error: {}. Retrying in 1s.", e);
                    tokio::time::sleep(Duration::from_secs(1)).await; // Avoid tight error loop
                }
            }
        }
    }

    /// Handles routing logic by listening for `RoutingMessage` events and
    /// sending replies or new requests via the transport.
    async fn handle_requests(
        my_id: Fluid,
        my_key: VerifyingKey,
        e: impl Stream<Item = Arc<RoutingMessage>>,
        map: Arc<RwLock<HashMap<Fluid, (Instant, VerifyingKey)>>>,
        transport: T,
    ) {
        e.for_each(|v| {
            let transport = transport.clone();
            let map = map.clone();
            async move {
                let routing_message_to_send = match &*v {
                    RoutingMessage::Announce(id) => {
                        if !map.read().await.contains_key(id) {
                            Some(RoutingMessage::Request { asking: *id })
                        } else {
                            None
                        }
                    }
                    RoutingMessage::Request { asking } => {
                        let key_bytes = if asking == &my_id {
                            Some(my_key.to_bytes().to_vec())
                        } else if let Some(key) = map.read().await.get(asking)
                            && key.0.elapsed() < Duration::from_secs(RESOLUTION_TTL_SECS)
                        {
                            Some(key.1.to_bytes().to_vec())
                        } else {
                            None
                        };
                        key_bytes.map(|key| RoutingMessage::Response { giving: *asking, key })
                    }
                    RoutingMessage::Response { giving, key } => {
                        if let Ok(key) = VerifyingKey::from_bytes(key.as_slice().try_into().unwrap()) {
                            map.write().await.insert(*giving, (Instant::now(), key));
                        }
                        None
                    }
                };

                // If a response or new request needs to be sent, serialize and send it.
                if let Some(msg) = routing_message_to_send {
                    transport.send(&FLESHMessage::new(MessageType::Routing(msg)).serialize().unwrap()).await.unwrap();
                }
            }
        })
        .await;
    }

    /// Periodically broadcasts a request for its own ID to the network,
    /// serving as a discovery and presence mechanism.
    async fn periodic_announcements(my_id: Fluid, transport: T) {
        loop {
            tokio::time::sleep(Duration::from_secs(ANNOUNCE_DURATION_SECS)).await;
            let announce_msg = RoutingMessage::Request { asking: my_id };
            transport.send(&FLESHMessage::new(MessageType::Routing(announce_msg)).serialize().unwrap()).await.unwrap();
        }
    }

    /// Attempts to resolve a peer's public key, first checking the local cache,
    /// then waiting for a response from the network.
    pub async fn resolve(&self, id: Fluid) -> Option<VerifyingKey> {
        // First, try a quick read lock to check the cache.
        if let Some(k) = self
            .map
            .read()
            .await
            .get(&id)
            .and_then(|v| (v.0.elapsed() < Duration::from_secs(RESOLUTION_TTL_SECS)).then_some(v.1))
        {
            return Some(k);
        }

        // If not in cache, listen on the routing stream for a response.
        let mut resolver = self
            .router_target
            .as_stream()
            .filter_map(|v| async move {
                if let RoutingMessage::Response { giving, key } = &*v
                    && giving == &id
                {
                    return VerifyingKey::from_bytes(key.as_slice().try_into().unwrap()).ok();
                }
                None
            })
            .boxed();

        // Wait for either a response or a timeout.
        let resp = select! {
            v = resolver.next() => v,
            _ = tokio::time::sleep(Duration::from_secs(10)) => None,
        };

        // If we got a key, cache it before returning.
        if let Some(key) = resp {
            self.map.write().await.insert(id, (Instant::now(), key));
            return Some(key);
        }
        None
    }
}

// Allows treating `Network` as an `EventTarget<FLESHMessage>` directly.
impl<T: PacketTransport> Deref for Network<T> {
    type Target = EventTarget<FLESHMessage>;

    fn deref(&self) -> &Self::Target { &self.target }
}
