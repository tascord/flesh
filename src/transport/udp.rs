use {
    crate::{events::EventTarget, mesh::resolving::Resolver, transport::Transport},
    async_trait::async_trait,
    std::{net::SocketAddr, ops::Deref},
    tokio::{
        net::UdpSocket,
        select, spawn,
        sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel},
    },
};

pub struct UdpTransport {
    tx: UnboundedSender<Vec<u8>>,
    target: EventTarget<Vec<u8>>,
    resolver: Resolver,
}

impl Deref for UdpTransport {
    type Target = EventTarget<Vec<u8>>;

    fn deref(&self) -> &Self::Target { &self.target }
}

async fn inner(socket: SocketAddr, mut rx: UnboundedReceiver<Vec<u8>>, tx: EventTarget<Vec<u8>>) -> std::io::Result<()> {
    let udp_std = std::net::UdpSocket::bind(socket)?;
    let udp_tok = UdpSocket::from_std(udp_std)?;

    let mut buf = vec![0; 1024];

    loop {
        select! {
            res = udp_tok.recv_from(&mut buf) => {
                if let Ok((bytes, _)) = res {
                    buf.truncate(bytes);
                    tx.emit(buf.clone());
                }
            },

            res = rx.recv() => {
                if let Some(data) = res {
                    let _ = udp_tok.send(&data).await;
                }
            }
        };
    }
}

#[async_trait]
impl Transport for UdpTransport {
    fn new(socket: SocketAddr) -> std::io::Result<Self> {
        let ev = EventTarget::new();

        let (o_tx, i_rx) = unbounded_channel();
        spawn(inner(socket, i_rx, ev.clone()));

        Ok(Self {
            resolver: Resolver::new(ev.as_stream(), {
                let tx = o_tx.clone();
                move |v| {
                    let _ = tx.send(v);
                }
            }),
            tx: o_tx,
            target: ev,
        })
    }

    fn resolver(&self) -> &Resolver { &self.resolver }

    async fn send(&self, data: &[u8]) { let _ = self.tx.send(data.to_vec()); }
}