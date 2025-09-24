use {
    crate::{events::EventTarget, resolution::resolver::Resolver, transport::Transport},
    async_trait::async_trait,
    std::{io, ops::Deref, path::PathBuf},
    tokio::{
        io::{AsyncReadExt, AsyncWriteExt, split},
        select, spawn,
        sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel},
    },
    tokio_serial::SerialPortBuilderExt,
    tracing::debug,
};

pub struct LoraTransport {
    tx: UnboundedSender<Vec<u8>>,
    target: EventTarget<Vec<u8>>,
    resolver: Resolver,
}

impl Deref for LoraTransport {
    type Target = EventTarget<Vec<u8>>;

    fn deref(&self) -> &Self::Target { &self.target }
}

impl LoraTransport {
    pub fn new(device: PathBuf, baud: u32) -> io::Result<Self> {
        let ev = EventTarget::new();
        let (o_tx, i_rx) = unbounded_channel();

        // Spawn a single async task to handle all serial communication
        spawn({
            let rx_from_lora_clone = ev.clone();
            async move {
                Self::inner(device, baud, i_rx, rx_from_lora_clone).await.unwrap();
            }
        });

        Ok(Self {
            resolver: Resolver::new(
                ev.as_stream(),
                {
                    let tx = o_tx.clone();
                    move |v| {
                        let _ = tx.clone().send(v);
                    }
                },
                Self::max_length(),
            ),
            target: ev,
            tx: o_tx,
        })
    }

    async fn inner(
        device: PathBuf,
        baud: u32,
        mut rx: UnboundedReceiver<Vec<u8>>,
        tx_to_app: EventTarget<Vec<u8>>,
    ) -> io::Result<()> {
        let serial = tokio_serial::new(device.display().to_string(), baud).open_native_async()?;
        let (mut reader, mut writer) = split(serial);

        let mut buf = vec![0; 4096];
        loop {
            select! {
                res = reader.read(&mut buf) => {
                    let bytes = res?;
                    if bytes > 0 {
                        debug!("Got {}b", bytes);
                        tx_to_app.emit(buf[..bytes].to_vec());
                    }
                }

                res = rx.recv() => {
                    if let Some(data) = res {
                        writer.write_all(&data).await?;
                        debug!("Sent {}b", data.len());
                    }
                },
            }
        }
    }
}

#[async_trait]
impl Transport for LoraTransport {
    fn resolver(&self) -> &Resolver { &self.resolver }

    fn max_length() -> usize { 4096 }

    async fn send(&self, data: &[u8]) { let _ = self.tx.send(data.to_vec()); }
}
