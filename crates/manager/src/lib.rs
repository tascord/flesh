use std::sync::Arc;

use tokio::sync::Mutex;

use {
    crate::{app::App, helpers::TaskList},
    flesh::{Network, lora::Lora},
    owo_colors::OwoColorize,
    port_check::free_local_port,
    serde::{Deserialize, Serialize},
    std::{collections::HashMap, env, fs::OpenOptions, io::Write, path::Path},
};

pub mod app;
pub mod helpers;

pub const DNSMASQ_CONFIG: &str = "/tmp/flesh-dnsmasq";
pub const NGINX_CONFIG: &str = "/tmp/flesh-nginx";

#[derive(Debug, Default, Serialize, Deserialize,Clone)]
pub struct Config {
    apps: HashMap<String, App>,
}

impl Config {
    pub fn new() -> Self { Self::default() }

    pub fn add_app(&mut self, name: String, app: App) { self.apps.insert(name, app); }

    pub fn remove_app(&mut self, name: &str) -> bool { self.apps.remove(name).is_some() }

    pub fn apps(&self) -> &HashMap<String, App> { &self.apps }

    pub async fn start(self) -> anyhow::Result<()> {
        // TODO: Specify mode via CLI
        let network = Network::new(Lora::new(Path::new(&env::var("LORA").expect("Missing LORA env")).to_path_buf(), 6900)?);
        let ports =
            std::iter::repeat_n((), self.apps.len()).map(|_| free_local_port().unwrap() as usize).collect::<Vec<_>>();

        let mut tl = TaskList::new("Start FLESH")
            .add_task("Write dnsmasq", Self::write_dnsmasq(self.apps.clone()))
            .add_task("Write nginx", Self::write_nginx(self.apps.clone(), ports.clone()));

        let apps = self.apps.clone();
        let running_apps = std::sync::Arc::new(tokio::sync::RwLock::new(HashMap::new()));

        for (i, (name, app)) in apps.into_iter().enumerate() {
            let running_apps = running_apps.clone();
            tl = tl.add_task(format!("Run {name}"), {
                let ports = ports.clone();
                let network = network.clone();
                let app = app.clone();
                async move {
                    running_apps.write().await.insert(name.clone(), Arc::new(Mutex::new(app.run(network, *ports.get(i).ok_or(anyhow::anyhow!("No port available"))?).await?)));
                    Ok::<_,anyhow::Error>(())
                }
            })
        }
        

        tl.await?;

        // Run dnsmasq + nginx in the foreground.
        // Example: spawn dnsmasq and nginx using std::process::Command
        let mut dnsmasq =
            tokio::process::Command::new("dnsmasq").arg("--no-daemon").arg("--conf-file").arg(DNSMASQ_CONFIG).spawn()?;

        let mut nginx =
            tokio::process::Command::new("nginx").arg("-c").arg(NGINX_CONFIG).arg("-g").arg("daemon off;").spawn()?;

        println!("{}", "⟶ Running".bright_green().bold());
        loop {
            let mut apps = running_apps.write().await;
            for (name, app) in (*apps).clone().into_iter() {
                let app = app.lock().await;
                match app.stream.recv().await? {
                    app::Message::ErrorDone => {
                        println!("{} {}", "⟵".bright_yellow().bold(), format!("App {name} reported an error").bright_yellow().bold());
                        let mut count_error = app.count_error;
                        count_error += 1;
                        if count_error >= 3 {
                            println!("{} {}", "✖".bright_red().bold(), format!("App {name} has reached the maximum number of errors and will be stopped.",).bright_red().bold());
                            // Remove the app from the list to stop monitoring it
                            
                            apps.remove(&name);
                        }
                    },
                    app::Message::ErrorSignal(sig) => {
                        println!("{} {}", "⟵".bright_yellow().bold(), format!("App {name} received signal {sig}",).bright_yellow().bold());
                        let mut count_error = app.count_error;
                        count_error += 1;
                        if count_error >= 3 {
                            println!("{} {}", "✖".bright_red().bold(), format!("App {name} has reached the maximum number of errors and will be stopped.",).bright_red().bold());
                            // Remove the app from the list to stop monitoring it
                            app.stream.send(app::Message::QuitUrAss).await?;
                            apps.remove(&name);
                        }
                    },
                    _ => {}
                }
            }   
            if apps.len() == 0 {
                println!("{}", "✔ All apps have been stopped.".bright_green().bold());
                break;
            }
        }
        let _ = tokio::try_join!(dnsmasq.wait(), nginx.wait());

        Ok(())
    }

    // forward `app.subdomain`.local -> 127.0.0.1
    async fn write_dnsmasq(apps: HashMap<String, App>) -> anyhow::Result<()> {
        let mut config = String::new();

        // Add general dnsmasq configuration
        config.push_str("# FLESH DNS Configuration\n");
        config.push_str("no-resolv\n");
        config.push_str("server=8.8.8.8\n");
        config.push_str("server=8.8.4.4\n\n");

        // Add DNS entries for each app
        for app in apps.values() {
            config.push_str(&format!("address=/{}.local/127.0.0.1\n", app.subdomain));
        }

        // Write to dnsmasq config file
        let mut file = OpenOptions::new().create(true).write(true).truncate(true).open(DNSMASQ_CONFIG)?;

        file.write_all(config.as_bytes())?;
        Ok(())
    }

    // forward `app.subdomain`.local -> 127.0.0.1:{port}, return ports
    async fn write_nginx(apps: HashMap<String, App>, ports: Vec<usize>) -> anyhow::Result<()> {
        let mut config = String::new();

        // Add general nginx configuration
        config.push_str("# FLESH Nginx Configuration\n");
        config.push_str("events {\n");
        config.push_str("    worker_connections 1024;\n");
        config.push_str("}\n\n");
        config.push_str("http {\n");
        config.push_str("    include /etc/nginx/mime.types;\n");
        config.push_str("    default_type application/octet-stream;\n\n");

        // Add server blocks for each app
        for (i, app) in apps.values().enumerate() {
            if let Some(&port) = ports.get(i) {
                config.push_str(&format!(
                    "    server {{\n\
                     \x20       listen 80;\n\
                     \x20       server_name {}.local;\n\n\
                     \x20       location / {{\n\
                     \x20           proxy_pass http://127.0.0.1:{};\n\
                     \x20           proxy_set_header Host $host;\n\
                     \x20           proxy_set_header X-Real-IP $remote_addr;\n\
                     \x20           proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;\n\
                     \x20           proxy_set_header X-Forwarded-Proto $scheme;\n\
                     \x20       }}\n\
                     \x20   }}\n\n",
                    app.subdomain, port
                ));
            }
        }

        config.push_str("}\n");

        // Write to nginx config file
        let mut file = OpenOptions::new().create(true).write(true).truncate(true).open(NGINX_CONFIG)?;

        file.write_all(config.as_bytes())?;
        Ok(())
    }
}
