use {
    flesh::{
        modes::lora::{Lora, LoraSettings},
        transport::network::{self, Network},
    },
    futures::StreamExt,
    std::path::Path,
};

#[tokio::main]
async fn main() {
    let lora = Lora::new(
        Path::new("/dev/serial/by-id/usb-Silicon_Labs_CP2102_USB_to_UART_Bridge_Controller_0001-if00-port0").to_path_buf(),
        9600,
        LoraSettings { spread_factor: 9, frequency_hz: 915_000_000, bandwidth_khz: 10 },
        false,
    )
    .await
    .unwrap();

    let network = Network::new(lora.clone());

    loop {
        let message = network.as_stream().next().await;
        if let Some(message) = message {
            let body = message.body.clone();
            println!("{}b -- {}", body.len(), String::from_utf8_lossy(&body));
        }
    }
}
