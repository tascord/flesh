use {
    crate::{events::EventTarget, transport::PacketTransport},
    async_trait::async_trait,
    bytes::Bytes,
    futures::{SinkExt, StreamExt},
    std::{io, ops::Deref, path::PathBuf, time::Duration},
    tokio::{
        io::{AsyncReadExt, ReadHalf, WriteHalf, split},
        spawn,
        sync::mpsc::{UnboundedSender, unbounded_channel},
        time::timeout,
    },
    tokio_serial::{SerialPortBuilderExt, SerialStream},
    tokio_util::codec::{FramedRead, FramedWrite, LengthDelimitedCodec, LinesCodec},
    tracing::debug,
};

const LENGTH_FIELD_SIZE: usize = 1;
const MAX_PAYLOAD_SIZE: usize = 1200;
const CHUNK_SIZE: usize = 16;

#[derive(Debug, Clone, Copy)]
pub struct LoraSettings {
    pub spread_factor: u8,
    pub frequency_hz: u32,
    pub bandwidth_khz: u16,
}

#[derive(Clone)]
pub struct Lora {
    writer: UnboundedSender<Vec<u8>>,
    reader: EventTarget<Vec<u8>>,
}

impl Lora {
    pub async fn new(device: PathBuf, baud: u32, settings: LoraSettings, configure: bool) -> io::Result<Self> {
        debug!("Initializing LoRa with settings: {:?}", settings);

        let serial = tokio_serial::new(device.display().to_string(), baud).open_native_async()?;
        let (reader, writer) = split(serial);

        let config_codec = tokio_util::codec::LinesCodec::new();
        let mut reader = tokio_util::codec::FramedRead::new(reader, config_codec);

        let data_codec = LengthDelimitedCodec::builder()
            .length_field_length(LENGTH_FIELD_SIZE)
            .max_frame_length(MAX_PAYLOAD_SIZE)
            .little_endian()
            .new_codec();

        let mut writer = FramedWrite::new(writer, data_codec);
        if configure {
            Self::configure(settings, &mut writer, &mut reader).await?;
        }
        
        let reader = reader.into_inner();

        let (writer, reader) = Self::inner(reader, writer);
        Ok(Self { writer, reader })
    }

    async fn wait_for_ok(reader: &mut FramedRead<ReadHalf<SerialStream>, LinesCodec>, command_name: &str) -> io::Result<()> {
        match timeout(Duration::from_secs(5), reader.next()).await {
            Ok(Some(Ok(response))) => {
                if response.trim() == "OK" {
                    debug!("{} command successful.", command_name);
                    Ok(())
                } else {
                    Err(io::Error::other(format!("{} failed with response: {}", command_name, response)))
                }
            }
            Ok(Some(Err(e))) => Err(io::Error::other(format!("{} read error: {}", command_name, e))),
            Ok(None) => Err(io::Error::other(format!("{} failed: serial stream closed.", command_name))),
            Err(_) => Err(io::Error::other(format!("{} failed: Timeout waiting for response.", command_name))),
        }
    }

    async fn configure(
        settings: LoraSettings,
        writer: &mut FramedWrite<WriteHalf<SerialStream>, LengthDelimitedCodec>,
        reader: &mut FramedRead<ReadHalf<SerialStream>, LinesCodec>,
    ) -> io::Result<()> {
        // 1. Send the Spread Factor (SF) command
        let sf_command = format!("AT+SF={}\r\n", settings.spread_factor);
        writer
            .send(sf_command.as_bytes().to_vec().into())
            .await
            .map_err(|e| io::Error::other(format!("Failed to send SF command: {}", e)))?;
        Self::wait_for_ok(reader, "SF").await?;

        // 2. Send the Frequency command
        let freq_command = format!("AT+FREQ={}\r\n", settings.frequency_hz);
        writer
            .send(freq_command.as_bytes().to_vec().into())
            .await
            .map_err(|e| io::Error::other(format!("Failed to send FREQ command: {}", e)))?;
        Self::wait_for_ok(reader, "FREQ").await?;

        // 3. Send the Bandwidth command
        let bw_command = format!("AT+BW={}\r\n", settings.bandwidth_khz);
        writer
            .send(bw_command.as_bytes().to_vec().into())
            .await
            .map_err(|e| io::Error::other(format!("Failed to send BW command: {}", e)))?;
        Self::wait_for_ok(reader, "BW").await?;

        Ok(())
    }

    fn inner(
        mut reader: ReadHalf<SerialStream>,
        mut writer: FramedWrite<WriteHalf<SerialStream>, LengthDelimitedCodec>,
    ) -> (UnboundedSender<Vec<u8>>, EventTarget<Vec<u8>>) {
        let (tx, mut rx) = unbounded_channel::<Vec<u8>>();
        let target = EventTarget::new();

        spawn({
            let target = target.clone();
            async move {
                while let Ok(v) = Self::recv(&mut reader).await {
                    target.emit(v);
                }
            }
        });

        spawn(async move {
            while let Some(v) = rx.recv().await {
                Self::send(&mut writer, &v).await.unwrap();
            }
        });

        (tx, target)
    }

    // Arc<tokio::sync::Mutex<FramedWrite<tokio::io::WriteHalf<SerialStream>, LengthDelimitedCodec>>>
    async fn send(stream: &mut FramedWrite<WriteHalf<SerialStream>, LengthDelimitedCodec>, data: &[u8]) -> io::Result<()> {
        let len = data.len();
        if len > MAX_PAYLOAD_SIZE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("Packet size of {} bytes exceeds max payload of {} bytes", len, MAX_PAYLOAD_SIZE),
            ));
        }

        debug!("Sending frame with length: {}", len);
        stream.send(Bytes::copy_from_slice(data)).await.map_err(|e| io::Error::other(e.to_string()))?;
        stream.flush().await.map_err(|e| io::Error::other(e.to_string()))?;

        Ok(())
    }

    async fn recv(reader: &mut ReadHalf<SerialStream>) -> io::Result<Vec<u8>> {
        let mut buffer = Vec::new();

        loop {
            let mut chunk = [0u8; CHUNK_SIZE];

            if reader.read_exact(&mut chunk).await.is_err() {
                return Err(io::Error::new(
                    io::ErrorKind::ConnectionAborted,
                    "Failed to read a full 16-byte chunk or connection closed",
                ));
            }

            buffer.extend_from_slice(&chunk);
            debug!("Read new 16-byte chunk. Buffer size: {}", buffer.len());

            let mut i = 0;
            while i < buffer.len() {
                if buffer.len() - i < LENGTH_FIELD_SIZE {
                    break;
                }

                let payload_len = buffer[i] as usize;
                let frame_len = LENGTH_FIELD_SIZE + payload_len;

                if payload_len == 0 || payload_len > MAX_PAYLOAD_SIZE {
                    debug!("Discarding byte at offset {} (value {}). Not a valid frame start.", i, payload_len);
                    i += 1;
                    continue;
                }

                if buffer.len() - i >= frame_len {
                    let payload_start = i + LENGTH_FIELD_SIZE;
                    let payload_end = i + frame_len;

                    let payload = buffer[payload_start..payload_end].to_vec();
                    debug!("Found complete valid frame of size {}. Extracted payload length: {}", frame_len, payload_len);

                    buffer.drain(..payload_end);

                    return Ok(payload);
                } else {
                    debug!(
                        "Found valid length prefix ({} bytes), but frame is incomplete. Waiting for next chunk.",
                        payload_len
                    );
                    break;
                }
            }

            if i > 0 {
                buffer.drain(..i);
            }

            if buffer.len() > MAX_PAYLOAD_SIZE * 2 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Buffer exceeded safety limit due to un-synchronized data.",
                ));
            }
        }
    }
}

#[async_trait]
impl PacketTransport for Lora {
    async fn send(&self, data: &[u8]) -> io::Result<()> { self.writer.send(data.to_vec()).map_err(std::io::Error::other) }

    async fn recv(&mut self) -> io::Result<Vec<u8>> {
        self.reader
            .as_stream()
            .next()
            .await
            .ok_or(std::io::Error::new(io::ErrorKind::BrokenPipe, "Reader channel was disconnected"))
            .map(|v| Vec::clone(&*v))
    }
}

impl Deref for Lora {
    type Target = EventTarget<Vec<u8>>;

    fn deref(&self) -> &Self::Target { &self.reader }
}
