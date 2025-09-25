use {
    bincode::config::Configuration,
    flesh::{
        modes::lora::{Lora, LoraSettings},
        transport::PacketTransport,
    },
    futures::StreamExt,
    inquire::Text,
    std::{
        fs::OpenOptions,
        io::{self, Write},
        path::Path,
        sync::{Arc, RwLock, mpsc},
        thread,
        time::Duration,
    },
    termion::{clear, cursor, screen::IntoAlternateScreen, terminal_size},
    tokio::spawn,
    tracing::level_filters::LevelFilter,
};

// --- App State Struct ---
struct App {
    lora: Lora,
    messages: Arc<RwLock<Vec<String>>>,
}

#[derive(bincode::Decode, bincode::Encode)]
pub struct Message {
    inner: String,
}

impl From<String> for Message {
    fn from(val: String) -> Self { Message { inner: val } }
}

impl App {
    async fn new() -> App {
        tracing_subscriber::fmt()
            .pretty()
            .with_thread_names(true)
            .with_max_level(LevelFilter::TRACE)
            .with_ansi(false)
            .with_writer(OpenOptions::new().create(true).truncate(true).write(true).open("./logs").unwrap())
            .init();

        let lora = Lora::new(
            Path::new("/dev/serial/by-id/usb-Silicon_Labs_CP2102_USB_to_UART_Bridge_Controller_0001-if00-port0")
                .to_path_buf(),
            9600,
            LoraSettings { spread_factor: 7, frequency_hz: 915_000_000, bandwidth_khz: 125 },
        )
        .await
        .unwrap();

        let messages = Arc::new(RwLock::new(vec![
            String::from("Welcome to the Simple Rust Messenger!"),
            String::from("Type your message below and press Enter."),
        ]));

        spawn({
            let lora = lora.clone();
            let messages = messages.clone();
            async move {
                lora.as_stream()
                    .for_each(move |v| {
                        let messages = messages.clone();
                        async move {
                            if let Ok(m) = bincode::decode_from_slice::<Message, Configuration>(&v, Configuration::default())
                            {
                                messages.write().unwrap().push(m.0.inner);
                            }
                        }
                    })
                    .await
            }
        });

        App { lora, messages }
    }

    fn push_message(&mut self, text: String) { self.messages.write().unwrap().push(text); }
}

#[tokio::main]
async fn main() -> io::Result<()> {
    let mut stdout = io::stdout().into_alternate_screen()?;
    let mut app = App::new().await;
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut count = 0;
        loop {
            thread::sleep(Duration::from_secs(5));
            count += 1;
            let new_msg = format!("System (Tick {}): New messages just arrived!", count);
            if tx.send(new_msg).is_err() {
                break; // UI thread has shut down
            }
        }
    });

    // --- Main UI Loop ---
    loop {
        // A. Clear the screen for a fresh draw
        write!(stdout, "{}{}", clear::All, cursor::Goto(1, 1))?;

        // B. Draw Messages
        let (width, _height) = terminal_size()?;
        for msg in app.messages.read().unwrap().iter().rev().take(10).rev() {
            // Display last 10 messages
            writeln!(stdout, "{}", msg)?;
        }

        // C. Draw Input Prompt Title (always placed at the bottom, before the input)
        writeln!(stdout, "{}", "-".repeat(width as usize))?;
        writeln!(stdout, "Enter your message (Esc to quit):")?;
        write!(stdout, " > ")?;
        stdout.flush()?;

        // D. Use inquire for robust, line-editing input
        let prompt = Text::new(""); // We've already drawn the prompt " > "

        match prompt.prompt() {
            Ok(input) => {
                if input.trim() == "/quit" {
                    break;
                }
                app.push_message(format!("You: {}", input));
            }
            Err(inquire::error::InquireError::OperationCanceled) => {
                // User pressed Ctrl+C or Esc - we treat this as a signal to quit
                break;
            }
            Err(_) => {
                // Ignore other errors (e.g., failed to read input)
            }
        }

        // E. Check for new incoming messages from the backend thread
        while let Ok(new_message) = rx.try_recv() {
            let lora = app.lora.clone();
            spawn(async move {
                lora.send(
                    &bincode::encode_to_vec::<Message, Configuration>(
                        Into::<Message>::into(new_message),
                        Configuration::default(),
                    )
                    .unwrap(),
                )
                .await
            });
        }
    }

    // Restore terminal state when done
    Ok(())
}
