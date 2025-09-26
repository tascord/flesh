use {
    crate::widgets::{MessageList, Textbox},
    anyhow::{Context, Result},
    crossterm::{
        event::{self, Event, KeyCode, KeyModifiers},
        execute,
        terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
    },
    fl_uid::Fluid,
    flesh::{
        modes::lora::{Lora, LoraSettings},
        transport::{PacketTransport, encoding::FLESHMessage, network::Network},
    },
    futures::StreamExt,
    futures_signals::signal::Mutable,
    itertools::Itertools,
    ratatui::prelude::*,
    serde::{Deserialize, Serialize},
    std::{
        env,
        fs::OpenOptions,
        io::{self, Stdout},
        path::Path,
        process::Command,
        time::Duration,
    },
    tokio::{
        spawn,
        sync::mpsc::{UnboundedSender, unbounded_channel},
    },
    tracing_subscriber::{field::debug, filter::LevelFilter},
};

mod widgets;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum Message {
    Text { author: String, content: String },
    Join(String),
}

pub struct App {
    messages: Mutable<Vec<Message>>,
    send: UnboundedSender<Message>,
    textbox_state: (String, usize),
    should_quit: bool,
    name: String,
}

impl App {
    pub async fn new() -> Self {
        tracing_subscriber::fmt()
            .with_thread_names(true)
            .with_level(true)
            .with_max_level(LevelFilter::TRACE)
            .with_writer(OpenOptions::new().create(true).write(true).truncate(true).open("./demo.log").unwrap())
            .pretty()
            .with_ansi(false)
            .init();

        let lora = Lora::new(
            Path::new(&env::var("LORA").expect("No LoRa env")).to_path_buf(),
            9600,
            LoraSettings { spread_factor: 9, frequency_hz: 915_000_000, bandwidth_khz: 10 },
            false,
        )
        .await
        .expect("Failed to setup LoRa");

        let msgs = Mutable::<Vec<Message>>::new(Vec::new());

        let s = Self {
            messages: msgs.clone(),
            send: Self::lora(lora, msgs.clone()),
            textbox_state: Default::default(),
            should_quit: false,
            name: std::env::var("USER")
                .ok()
                .and_then(|v| {
                    Command::new("hostname")
                        .output()
                        .map(|h| format!("{v}@{}", String::from_utf8_lossy(&h.stdout).to_string().trim()))
                        .ok()
                })
                .unwrap_or(Fluid::new().to_string()),
        };

        s.send.send(Message::Join(s.name.to_string())).unwrap();
        s
    }

    pub fn lora(lora: Lora, msgs: Mutable<Vec<Message>>) -> UnboundedSender<Message> {
        let (tx, mut rx) = unbounded_channel::<Message>();

        spawn({
            let msgs = msgs.clone();
            let lora = lora.clone();
            async move {
                let msgs = msgs.clone();
                Network::new(lora)
                    .as_stream()
                    .filter_map(|m| async move { serde_json::from_slice(&m.body).ok() })
                    .for_each({
                        let msgs = msgs.clone();
                        move |m| {
                            let msgs = msgs.clone();
                            async move {
                                msgs.clone().set({
                                    let mut l = msgs.get_cloned();
                                    l.push(m);
                                    l
                                })
                            }
                        }
                    })
                    .await
            }
        });

        spawn(async move {
            while let Some(msg) = rx.recv().await {
                let encoded = FLESHMessage::new(flesh::transport::encoding::MessageType::Data { status: 100 })
                    .with_body(serde_json::to_vec(&msg).unwrap());

                msgs.clone().set({
                    let mut l = msgs.get_cloned();
                    l.push(msg);
                    l.dedup();
                    l
                });

                lora.send(&encoded.serialize().unwrap()).await.unwrap();
            }
        });

        tx
    }

    pub fn tick(&mut self, f: &mut Frame<'_>) -> anyhow::Result<()> {
        if event::poll(Duration::from_millis(50)).context("event poll failed")? {
            if let Event::Key(key) = event::read().context("event read failed")? {
                match key.code {
                    // Quit
                    KeyCode::Esc => {
                        self.should_quit = true;
                    }

                    // Send
                    KeyCode::Enter if !self.textbox_state.0.trim().is_empty() => {
                        self.send
                            .send(Message::Text { author: self.name.clone(), content: self.textbox_state.0.clone() })?;
                        self.textbox_state = (String::new(), 0);
                    }

                    KeyCode::Backspace => {
                        if self.textbox_state.1 > 0 {
                            self.textbox_state.0.remove(self.textbox_state.1 - 1);
                            self.textbox_state.1 -= 1;
                        }
                    }
                    KeyCode::Home | KeyCode::Left if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        self.textbox_state.1 = 0;
                    }
                    KeyCode::End | KeyCode::Right if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        self.textbox_state.1 = self.textbox_state.0.len();
                    }
                    KeyCode::Left => {
                        if self.textbox_state.1 > 0 {
                            self.textbox_state.1 -= 1;
                        }
                    }
                    KeyCode::Right => {
                        if self.textbox_state.1 < self.textbox_state.0.len() {
                            self.textbox_state.1 += 1;
                        }
                    }
                    KeyCode::Delete => {
                        self.textbox_state.0.remove(self.textbox_state.1);
                    }
                    KeyCode::Char(c) => {
                        self.textbox_state.0.insert(self.textbox_state.1, c);
                        self.textbox_state.1 += 1;
                    }

                    _ => {}
                }
            }
        }

        let binding = Layout::new(Direction::Vertical, [Constraint::Fill(1), Constraint::Length(3)]).split(f.area());
        let (lay_list, lay_input) = binding.into_iter().collect_tuple().unwrap();

        f.render_widget(MessageList::new(self.messages.get_cloned()), *lay_list);
        f.render_stateful_widget(Textbox::new("Compose"), *lay_input, &mut self.textbox_state);
        std::thread::sleep(Duration::from_millis(10));

        Ok(())
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut terminal = setup_terminal().context("setup failed")?;
    terminal.clear().unwrap();
    run(&mut terminal, App::new().await).await.context("run failed")?;
    restore_terminal(&mut terminal).context("restore terminal failed")?;
    Ok(())
}

fn setup_terminal() -> Result<Terminal<CrosstermBackend<Stdout>>> {
    let mut stdout = io::stdout();
    enable_raw_mode().context("failed to enable raw mode")?;
    execute!(stdout, EnterAlternateScreen).context("unable to enter alternate screen")?;
    Terminal::new(CrosstermBackend::new(stdout)).context("creating terminal failed")
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    disable_raw_mode().context("failed to disable raw mode")?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen).context("unable to switch to main screen")?;
    terminal.show_cursor().context("unable to show cursor")
}

async fn run(terminal: &mut Terminal<CrosstermBackend<Stdout>>, mut app: App) -> Result<()> {
    loop {
        terminal.draw(|f| {
            app.tick(f).unwrap();
        })?;

        if app.should_quit {
            break;
        }
    }

    Ok(())
}
