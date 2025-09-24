use {
    crate::{events::EventTarget, resolution::resolver::Resolver, transport::Transport},
    async_trait::async_trait,
    socket2::{Domain, Protocol, SockAddr, Socket, Type},
    std::{net::SocketAddr, ops::Deref},
    tokio::{
        net::UdpSocket,
        select, spawn,
        sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel},
    },
    tracing::debug,
};

pub struct UdpTransport {
    target: EventTarget<Vec<u8>>,
    tx: UnboundedSender<Vec<u8>>,
    resolver: Resolver,
}

impl Deref for UdpTransport {
    type Target = EventTarget<Vec<u8>>;

    fn deref(&self) -> &Self::Target { &self.target }
}

impl UdpTransport {
    pub fn new(bind_addr: SocketAddr) -> std::io::Result<Self> {
        let ev = EventTarget::new();

        let (o_tx, i_rx) = unbounded_channel();
        spawn(Self::inner(bind_addr, i_rx, ev.clone()));

        Ok(Self {
            resolver: Resolver::new(
                ev.as_stream(),
                {
                    let tx = o_tx.clone();
                    move |v| {
                        let _ = tx.clone().send(v);
                    }
                },
            ),
            target: ev,
            tx: o_tx.clone(),
        })
    }

    async fn inner(
        bind_addr: SocketAddr,
        mut rx: UnboundedReceiver<Vec<u8>>,
        tx: EventTarget<Vec<u8>>,
    ) -> std::io::Result<()> {
        let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
        socket.set_reuse_port(true)?;
        socket.set_broadcast(true)?;

        // Bind the socket to a local address, not the multicast group address.
        let listen_addr = format!("0.0.0.0:{}", bind_addr.port()).parse::<SocketAddr>().unwrap();
        socket.bind(&SockAddr::from(listen_addr))?;

        // Set to non-blocking before handing to Tokio
        socket.set_nonblocking(true)?;
        let udp_tok = UdpSocket::from_std(socket.into())?;

        // Join the multicast group.
        udp_tok.join_multicast_v4(
            match bind_addr {
                SocketAddr::V4(socket_addr_v4) => *socket_addr_v4.ip(),
                _ => unreachable!(),
            },
            "0.0.0.0".parse().unwrap(),
        )?;

        let mut buf = vec![0; 4096];

        loop {
            select! {
                res = udp_tok.recv_from(&mut buf) => {
                    if let Ok((bytes, _sender_addr)) = res {
                        debug!("Got {bytes}b");
                        buf.truncate(bytes);
                        tx.emit(buf.clone());
                        buf = vec![0; 4096];
                    }
                },
                res = rx.recv() => {
                    if let Some(data) = res && let Ok(bytes) = udp_tok.send_to(&data, &bind_addr).await {
                        debug!("Sent {}b", bytes);
                    }
                }
            };
        }
    }
}

#[async_trait]
impl Transport for UdpTransport {
    fn resolver(&self) -> &Resolver { &self.resolver }

    async fn send(&self, data: &[u8]) { let _ = self.tx.send(data.to_vec()); }
}
