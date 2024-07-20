use std::collections::HashMap;
use std::io;
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use log::{error, info, warn};
use serde::{Deserialize, Serialize};
use simple_logger::SimpleLogger;

const DEFAULT_CONFIG_PATH: &str = "/etc/ssh_router/config.toml";
const DEFAULT_LISTEN_PORT: u16 = 2222;

#[derive(Debug, Deserialize, Serialize)]
struct Config {
    listen_port: Option<u16>,
    routes: HashMap<String, String>,
}

fn main() -> io::Result<()> {
    SimpleLogger::new()
        .with_level(log::LevelFilter::Info)
        .init()
        .unwrap();

    let config_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| DEFAULT_CONFIG_PATH.to_string());
    let config = load_config(&config_path)?;

    let listen_port = config.listen_port.unwrap_or(DEFAULT_LISTEN_PORT);
    let routes: Arc<HashMap<IpAddr, String>> = Arc::new(
        config
            .routes
            .into_iter()
            .map(|(k, v)| (k.parse().unwrap(), v))
            .collect(),
    );

    let listener = TcpListener::bind(format!("0.0.0.0:{}", listen_port))?;
    info!("Listening on port {}", listen_port);

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let routes = Arc::clone(&routes);
                thread::spawn(move || {
                    if let Err(e) = handle_client(stream, &routes) {
                        error!("Error handling client: {}", e);
                    }
                });
            }
            Err(e) => error!("Error accepting connection: {}", e),
        }
    }
    Ok(())
}

fn load_config<P: AsRef<Path>>(path: P) -> io::Result<Config> {
    let content = std::fs::read_to_string(path)?;
    toml::from_str(&content).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

fn handle_client(mut client: TcpStream, routes: &HashMap<IpAddr, String>) -> io::Result<()> {
    let local_addr = client.local_addr()?;
    info!("Incoming connection to: {}", local_addr);

    if let Some(target_ip) = routes.get(&local_addr.ip()) {
        info!("Routing connection to {}", target_ip);
        match TcpStream::connect_timeout(
            &(target_ip.parse::<SocketAddr>().unwrap()),
            Duration::from_secs(5),
        ) {
            Ok(mut target) => {
                let mut client_clone = client.try_clone()?;
                let mut target_clone = target.try_clone()?;

                thread::spawn(move || {
                    if let Err(e) = io::copy(&mut client_clone, &mut target) {
                        error!("Error forwarding client to target: {}", e);
                    }
                });

                thread::spawn(move || {
                    if let Err(e) = io::copy(&mut target_clone, &mut client) {
                        error!("Error forwarding target to client: {}", e);
                    }
                });

                Ok(())
            }
            Err(e) => {
                warn!("Failed to connect to target {}: {}", target_ip, e);
                Err(e)
            }
        }
    } else {
        let err = io::Error::new(io::ErrorKind::NotFound, "No matching route found");
        warn!("{}", err);
        Err(err)
    }
}

