use {
    crate::{
        events::EventTarget,
        transport::{
            PacketTransport,
            encoding::{FLESHMessage, Identity},
            status::Status,
        },
    },
    anyhow::anyhow,
    ed25519_dalek::{SigningKey, VerifyingKey},
    futures::{Stream, StreamExt},
    rand_core::OsRng,
    std::{
        collections::HashMap,
        ops::{Deref, Not},
        sync::Arc,
        time::{Duration, Instant},
    },
    tokio::{spawn, sync::RwLock},
    tracing::{error, info, trace, warn},
    uuid::Uuid,
};

pub const RESOLUTION_TTL_SECS: u64 = 5000;
pub const ANNOUNCE_DURATION_SECS: u64 = 30;

#[derive(Clone)]
pub struct Network<T: PacketTransport> {
    nodes: Arc<RwLock<NodeRelationshipMap>>,
    target: EventTarget<FLESHMessage>,
    router_target: EventTarget<RoutingMessage>,
    pub(crate) key: SigningKey,
    pub id: Uuid,
    transport: T,
}

impl<T: PacketTransport + Clone + 'static> Network<T> {
    /// Creates a new Network instance that operates over any compatible packet transport.
    pub fn new(transport: T) -> Self {
        let mut rng = OsRng;
        let key = SigningKey::generate(&mut rng);
        let id = Uuid::new_v4();

        let s = Self {
            id,
            key,
            nodes: Default::default(),
            target: Default::default(),
            router_target: Default::default(),
            transport,
        };

        info!("Resolver #{} started.", s.id);
        let identity = (s.id, s.key.clone());

        // Spawn the main loop that receives all incoming packets from the transport
        spawn(Self::packet_processing_loop(
            s.target.clone(),
            s.router_target.clone(),
            identity.clone(),
            s.transport.clone(),
        ));

        // Spawn the handler for internal routing messages (requests/responses for keys)
        spawn(Self::handle_requests(identity, s.router_target.as_stream(), s.nodes.clone(), s.transport.clone(), {
            let t = s.target.clone();
            move |m: FLESHMessage| {
                t.emit(m);
            }
        }));

        // Spawn the task that periodically broadcasts a discovery message
        spawn(Self::periodic_announcements(s.id, s.transport.clone()));

        s
    }

    /// The main inbound message loop. It continually waits for packets from the
    /// transport, deserializes them, and forwards them to the correct handler.
    async fn packet_processing_loop(
        target: EventTarget<FLESHMessage>,
        router_target: EventTarget<RoutingMessage>,
        id: impl Identity + Clone,
        mut transport: T,
    ) {
        loop {
            match transport.recv().await {
                Ok(data) => {
                    if let Ok(message) = FLESHMessage::deserialize(&data) {
                        match RoutingMessage::from_message(&message) {
                            Ok(Some(rm)) if message.for_id(id.clone()) => router_target.emit(rm),
                            _ => target.emit(message),
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
        me: impl Identity + Clone,
        e: impl Stream<Item = Arc<RoutingMessage>>,
        nodes: Arc<RwLock<NodeRelationshipMap>>,
        transport: T,
        emit: impl Fn(FLESHMessage) + Clone,
    ) {
        e.for_each(|v| {
            let transport = transport.clone();
            let nodes = nodes.clone();
            let me = me.clone();
            let emit = emit.clone();

            async move {
                let reply = match RoutingMessage::clone(&*v) {
                    RoutingMessage::Announce(uuid) => {
                        nodes.read().await.knows(&uuid).not().then_some(RoutingMessage::RequestKey(uuid))
                    }
                    RoutingMessage::Ping(to, from) => to.eq(&me.id()).then_some(RoutingMessage::Pong(from, to)),
                    RoutingMessage::Pong(to, from) if to == me.id() => {
                        nodes.write().await.pong(from);
                        None
                    }
                    RoutingMessage::RequestKey(uuid) => {
                        if uuid == me.id() {
                            Some(RoutingMessage::ProvideKey(me.id(), me.key().as_bytes().to_vec()))
                        } else { nodes.read().await.key(&uuid).map(|key| RoutingMessage::ProvideKey(uuid, key.as_bytes().to_vec())) }
                    }
                    RoutingMessage::ProvideKey(uuid, key) => {
                        if let Ok(key) = VerifyingKey::from_bytes(key.as_slice().try_into().unwrap()) {
                            nodes.write().await.announced(uuid, key);
                        }
                        None
                    }
                    RoutingMessage::RequestRelayCapability(uuid) if nodes.read().await.can_relay(&uuid) => {
                        Some(RoutingMessage::ProvideRelayCapability(me.id(), uuid, true))
                    }
                    RoutingMessage::ProvideRelayCapability(from, to, status) if status => {
                        nodes.write().await.relayed(from, to);
                        None
                    }
                    RoutingMessage::Relay(uuid, msg) if uuid == me.id() => {
                        emit(msg.clone());
                        None
                    }
                    RoutingMessage::RelayFailure(uuid, msg) if uuid == me.id() => {
                        error!("Relay failed: {msg}");
                        None
                    }
                    _ => None,
                };

                // If a response or new request needs to be sent, serialize and send it.
                if let Some(msg) = reply {
                    transport.send(&FLESHMessage::new(msg.status()).serialize().unwrap()).await.unwrap();
                }
            }
        })
        .await;
    }

    /// Periodically broadcasts a request for its own ID to the network,
    /// serving as a discovery and presence mechanism.
    async fn periodic_announcements(my_id: Uuid, transport: T) {
        loop {
            tokio::time::sleep(Duration::from_secs(ANNOUNCE_DURATION_SECS)).await;
            let announce_msg = RoutingMessage::Announce(my_id);
            let _ = transport.send(&announce_msg.to_message().unwrap().serialize().unwrap()).await;
        }
    }

    /// Handles routing with or without a specified target via m.target
    pub async fn send(&self, m: FLESHMessage) -> anyhow::Result<()> {
        match m.target {
            None => self.transport.send(&m.serialize()?).await?,
            Some(id) => {
                let target = self.nodes.read().await.get(&id).ok_or(anyhow!("Unknown node {id}"))?;

                match target.0 {
                    NodeRelation::Local => self.transport.send(&m.serialize()?).await?,
                    NodeRelation::Relay { via } => {
                        let m = RoutingMessage::Relay(id, m).to_message()?.with_target(via);
                        self.transport.send(&m.serialize()?).await?
                    }
                }
            }
        }

        Ok(())
    }
}

// Allows treating `Network` as an `EventTarget<FLESHMessage>` directly.
impl<T: PacketTransport> Deref for Network<T> {
    type Target = EventTarget<FLESHMessage>;

    fn deref(&self) -> &Self::Target { &self.target }
}

#[derive(Debug, Clone)]
pub enum RoutingMessage {
    Announce(Uuid),
    Ping(Uuid, Uuid),
    Pong(Uuid, Uuid),
    RequestKey(Uuid),
    ProvideKey(Uuid, Vec<u8>),
    RequestRelayCapability(Uuid),
    ProvideRelayCapability(Uuid, Uuid, bool),
    Relay(Uuid, FLESHMessage),
    RelayFailure(Uuid, String),
}

impl RoutingMessage {
    pub fn status(&self) -> Status {
        match self {
            RoutingMessage::Announce(..) => Status::Acknowledge,
            RoutingMessage::RequestKey(..) => Status::RequestKey,
            RoutingMessage::ProvideKey(..) => Status::ProvideKey,
            RoutingMessage::RequestRelayCapability(..) => Status::RequestRelay,
            RoutingMessage::ProvideRelayCapability(..) => Status::ProvideRelay,
            RoutingMessage::Relay(..) => Status::Relay,
            RoutingMessage::RelayFailure(..) => Status::RelayFailure,
            RoutingMessage::Ping(..) => Status::Ping,
            RoutingMessage::Pong(..) => Status::Pong,
        }
    }
}

impl RoutingMessage {
    pub fn to_message(self) -> anyhow::Result<FLESHMessage> {
        let message = FLESHMessage::new(self.status());

        Ok(match self {
            RoutingMessage::Announce(uuid) => message.with_header("self", uuid),
            RoutingMessage::RequestKey(uuid) => message.with_header("for", uuid),
            RoutingMessage::ProvideKey(uuid, key) => message.with_header("for", uuid).with_header("key", key),
            RoutingMessage::RequestRelayCapability(uuid) => message.with_header("for", uuid),
            RoutingMessage::ProvideRelayCapability(from, to, status) => {
                message.with_header("from", from).with_header("to", to).with_header("status", status.to_string())
            }
            RoutingMessage::Relay(uuid, msg) => message.with_header("for", uuid).with_body(msg.serialize()?),
            RoutingMessage::RelayFailure(uuid, reason) => message.with_header("for", uuid).with_body(reason),
            RoutingMessage::Ping(to, from) => message.with_header("to", to).with_header("from", from),
            RoutingMessage::Pong(to, from) => message.with_header("to", to).with_header("from", from),
        })
    }

    pub fn from_message(m: &FLESHMessage) -> anyhow::Result<Option<Self>> {
        fn uuid(m: &FLESHMessage, h: &str) -> anyhow::Result<Uuid> {
            Ok(uuid::Uuid::from_bytes(m.headers.get(h).ok_or(anyhow!("Missing '{}' header", h))?.as_slice().try_into()?))
        }

        fn string(m: &FLESHMessage) -> anyhow::Result<String> { Ok(String::from_utf8(m.body.to_vec())?) }

        Ok(Some(match m.status {
            Status::Announce => Self::Announce(uuid(m, "for")?),
            Status::RequestKey => Self::RequestKey(uuid(m, "for")?),
            Status::ProvideKey => Self::ProvideKey(uuid(m, "for")?, m.body.clone()),
            Status::RequestRelay => Self::RequestKey(uuid(m, "for")?),
            Status::ProvideRelay => Self::ProvideRelayCapability(
                uuid(m, "from")?,
                uuid(m, "to")?,
                String::from_utf8(m.headers.get("status").ok_or(anyhow!("Missing 'status' header"))?.to_vec())?
                    .parse::<bool>()?,
            ),
            Status::Relay => Self::Relay(uuid(m, "for")?, FLESHMessage::deserialize(&m.body)?),
            Status::RelayFailure => Self::RelayFailure(uuid(m, "for")?, string(m)?),
            Status::Ping => Self::Ping(uuid(m, "to")?, uuid(m, "from")?),
            Status::Pong => {
                // TODO: Validate this is coming from who we think it is?
                Self::Ping(uuid(m, "to")?, uuid(m, "from")?)
            }
            _ => return Ok(None),
        }))
    }
}

pub enum RoutingStrategy {
    Direct(Uuid, VerifyingKey),
    Relayed(Uuid, VerifyingKey),
}

#[derive(Debug, Hash, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum NodeRelation {
    Local,
    Relay { via: Uuid },
}

#[derive(Clone, Debug, Default)]
pub struct NodeRelationshipMap(HashMap<Uuid, (Instant, NodeRelation, VerifyingKey)>);
impl NodeRelationshipMap {
    pub fn pong(&mut self, id: Uuid) {
        if let Some(existing) = self.0.get(&id) {
            self.0.insert(id, (Instant::now(), NodeRelation::Local, existing.2));
        }
    }

    pub fn announced(&mut self, id: Uuid, key: VerifyingKey) {
        if let Some(existing) = self.0.get(&id) {
            if existing.2 != key {
                warn!("Mismatching keys announced for {id}");
            }

            self.0.insert(id, (Instant::now(), existing.1.clone(), key));
        } else {
            self.0.insert(
                id,
                (
                    // We shouldnt assume we can reach this node unless we know otherwise, so we always disallow it by TTL
                    Instant::now().checked_sub(Duration::from_secs(RESOLUTION_TTL_SECS)).unwrap(),
                    NodeRelation::Local,
                    key,
                ),
            );
        }
    }

    pub fn relayed(&mut self, id: Uuid, via: Uuid) {
        if let Some(existing) = self.0.get(&id) {
            let relation = match existing.1 {
                NodeRelation::Local => {
                    trace!("Not downgrading local relationship to relay (for {id})");
                    NodeRelation::Local
                }
                _ => NodeRelation::Relay { via },
            };

            self.0.insert(id, (Instant::now(), relation, existing.2));
        } else {
            warn!("Relay found, but unknown node '{id}' to relay to.");
        }
    }

    pub fn knows(&self, id: &Uuid) -> bool {
        self.0.get(id).map(|v| v.0.elapsed() < Duration::from_secs(RESOLUTION_TTL_SECS)).unwrap_or_default()
    }

    pub fn key(&self, id: &Uuid) -> Option<VerifyingKey> {
        self.0.get(id).and_then(|v| (v.0.elapsed() < Duration::from_secs(RESOLUTION_TTL_SECS)).then_some(v.2))
    }

    pub fn can_relay(&self, id: &Uuid) -> bool {
        self.0
            .get(id)
            .map(|v| (v.0.elapsed() < Duration::from_secs(RESOLUTION_TTL_SECS)) && v.1 == NodeRelation::Local)
            .unwrap_or(false)
    }

    pub fn get(&self, id: &Uuid) -> Option<(NodeRelation, VerifyingKey)> {
        self.0.get(id).and_then(|v| (v.0.elapsed() < Duration::from_secs(RESOLUTION_TTL_SECS)).then_some((v.1.clone(), v.2)))
    }
}
