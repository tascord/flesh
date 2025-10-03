use core::net;
use std::sync::Arc;

use futures::{lock::Mutex, stream_select};
use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::UnixStream};

use {
    crate::{Deserialize, Serialize, helpers::TaskList},
    anyhow::bail,
    fl_uid::Fluid,
    flesh::Network,
    futures::FutureExt,
    libloading::{Library, Symbol},
    signal_hook::{consts::signal::*, iterator::Signals},
    std::{
        env,
        ffi::{c_uint, c_void},
        fmt::Display,
        fs::create_dir_all,
        os::raw::c_int,
        path::PathBuf,
        process::ExitStatus,
    },
    tokio::{fs, process::Command},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct App {
    pub subdomain: String,
    pub module_path: String,
    pub root_dir: String,
}

#[derive(Clone)]
pub struct RunningApp {
    pub name: String,
    app: App,
    pub count_error: u8,
    pub stream: Arc<MessageStream>,
}


pub struct MessageStream(Mutex<tokio::net::UnixStream>);




impl MessageStream {
    pub fn new(socket: tokio::net::UnixStream) -> Self {
        Self(Mutex::new(socket))
    }

    pub async fn recv(&self) -> anyhow::Result<Message> {
        let mut buf = [0u8; 1024];
        let mut socket = self.0.lock().await;
        let n = socket.read(&mut buf).await?;
        if n == 0 {
            bail!("Socket closed");
        }
        let msg: Message = serde_json::from_slice(&buf[..n])?;
        Ok(msg)
    }

    pub async fn send(&self, msg: Message) -> anyhow::Result<()> {
        let buf = serde_json::to_vec(&msg)?;
        let mut socket = self.0.lock().await;
        let _ = socket.write(&buf).await?;
        Ok(())
    }
    pub fn blocking_send(&self, msg: Message) -> anyhow::Result<()> {
        let buf = serde_json::to_vec(&msg)?;
        futures::executor::block_on(async {
            let mut socket = self.0.lock().await;
            let _ = socket.write(&buf).await?;
            Ok(())
        })
    }
    pub fn blocking_recv(&self) -> anyhow::Result<Message> {
        let mut buf = [0u8; 1024];
        let n = futures::executor::block_on(async { self.0.lock().await.read(&mut buf).await })?;
        if n == 0 {
            bail!("Socket closed");
        }
        let msg: Message = serde_json::from_slice(&buf[..n])?;
        Ok(msg)
    }
}


#[derive(Deserialize, Serialize)]
pub enum Message {
    ErrorSignal(c_int),
    ErrorLoading(String),
    ErrorDone,
    QuitUrAss,
}

fn config_dir() -> PathBuf {
    let path = env::var("HOME")
        .map(|home| PathBuf::from(home).join(".config").join("flesh"))
        .unwrap_or_else(|_| PathBuf::from(".config").join("flesh"));

    if !path.exists() {
        let _ = create_dir_all(&path);
    }

    path
}



async fn find_so(p: PathBuf) -> anyhow::Result<PathBuf> {
    let target_dir = p.join("target").join("release");
    let mut entries = match fs::read_dir(&target_dir).await {
        Ok(e) => e,
        Err(e) => bail!("Failed to read target directory {:?}: {}", target_dir, e),
    };

    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        match path.extension() {
            Some(ext) if (ext == "so" || ext == "dylib") => {
                return Ok(path);
            }
            _ => continue,
        }
    }

    bail!("No .so file found in {:?}", target_dir);
}




impl App {
    pub async fn new(url: impl Display + Sync + Send + 'static) -> anyhow::Result<Self> {
        let name = url.to_string().replace(|c: char| !c.is_alphanumeric(), "_");
        let wd = config_dir().join(name);

        let tl = TaskList::new("Prepare app")
            .add_task("Clone repo", {
                let wd = wd.clone();
                async move {
                    Command::new("git")
                        .args(["clone", &url.to_string(), &wd.display().to_string(), "--quiet"])
                        .status()
                        .map(status_error)
                        .await
                }
            })
            .add_task("Cargo build", {
                let wd = wd.clone();
                async move {
                    Command::new("cargo")
                        .current_dir(wd)
                        .args(["build", "--release", "--quiet"])
                        .status()
                        .map(status_error)
                        .await
                }
            });

        tl.await?;
        Ok(Self {
            subdomain: Fluid::new().to_string(),
            module_path: find_so(wd.clone()).await?.display().to_string(),
            root_dir: wd.display().to_string(),
        })
    }

    pub async fn run(&self,   network: Network, port: usize) -> anyhow::Result<RunningApp> {
        unsafe {
            let path = PathBuf::from(format!("/tmp/flesh-{}.sock", self.subdomain));
            let server_socket = tokio::net::UnixSocket::new_stream()?;
            server_socket.bind(&path)?;
            let client_socket = tokio::net::UnixSocket::new_stream()?;
            let (server_socket,_    ) = server_socket.listen(1)?.accept().await?;
            let stream = Arc::new(MessageStream::new(client_socket.connect(path).await?));
            let lib = Library::new(self.module_path.clone())?;

            std::thread::spawn(move || {

            let network_ptr = &network as *const Network as *mut c_void;
                macro_rules! send_if_error {
                    ($msg:expr, $val:expr) => {
                        match $val {
                            Ok(v) => v,
                            Err(e) => {
                                let _ = stream.send(Message::ErrorLoading(anyhow::anyhow!(concat!("Failed to ", $msg, ": {}"), e).to_string()));
                                return;
                            }
                        }
                    };
                }
                let func: Symbol<unsafe extern "C" fn(*mut c_void, usize)> = send_if_error!(
                    "load entrypoint symbol (__flesh_entrypoint) from dynamic library",
                    lib.get(b"__flesh_entrypoint")
                );
                let mut signals = send_if_error!(
                    "create signal handler (SIGINT, SIGTERM, SIGQUIT, SIGSEGV)",
                    Signals::new([SIGINT, SIGTERM, SIGQUIT, SIGSEGV])
                );
                let stream_a = stream.clone();
                std::thread::spawn(move || {
                    for sig in signals.forever() {
                        let _ = stream_a.send(Message::ErrorSignal(sig as c_int));
                    }
                });

                func(network_ptr, port);
                while let Ok(msg) = stream.blocking_recv(){
                    match msg {
                        Message::QuitUrAss => break,
                        _ => {
                            func(network_ptr, port);
                        }
                    }
                }
                
            });
            Ok(RunningApp { name: self.subdomain.clone(), app: self.clone(), stream: Arc::new(MessageStream::new(server_socket)), count_error: 0})
        }
    }
}

fn status_error<E: std::fmt::Debug>(r: Result<ExitStatus, E>) -> anyhow::Result<()> {
    match r {
        Ok(v) if v.success() => Ok(()),
        _ => {
            bail!("Failed to run{}", if let Err(e) = r { format!(": {:?}", e) } else { String::new() })
        }
    }
}
