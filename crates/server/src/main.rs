use std::collections::HashMap;
use std::io::{self, Read};
use std::net::{TcpListener, TcpStream};

use anyhow::Result;
use log::{error, info};
use shared::{self, Message, DEFAULT_PORT, MAX_CLIENTS};

/// Per-connection state.
struct Client {
    stream: TcpStream,
    addr: std::net::SocketAddr,
    buf: Vec<u8>,
}

fn handle_data(client: &mut Client) -> Result<Vec<Message>> {
    let mut tmp = [0u8; 4096];
    let n = client.stream.read(&mut tmp)?;
    if n == 0 {
        anyhow::bail!("connection closed");
    }
    client.buf.extend_from_slice(&tmp[..n]);

    let mut messages = Vec::new();
    // Simple length-prefixed framing: [u32 le length][payload]
    while client.buf.len() >= 4 {
        let len = u32::from_le_bytes(client.buf[..4].try_into().unwrap()) as usize;
        if client.buf.len() < 4 + len {
            break;
        }
        let payload = &client.buf[4..4 + len];
        match shared::deserialize(payload) {
            Ok(msg) => messages.push(msg),
            Err(e) => error!("Failed to deserialize message from {}: {}", client.addr, e),
        }
        client.buf.drain(..4 + len);
    }
    Ok(messages)
}

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let addr = format!("0.0.0.0:{DEFAULT_PORT}");
    let listener = TcpListener::bind(&addr)?;
    listener.set_nonblocking(true)?;

    info!("Server started, waiting for connections on {addr}...");

    let mut clients: HashMap<usize, Client> = HashMap::new();
    let mut next_id: usize = 0;

    loop {
        // Accept new connections
        match listener.accept() {
            Ok((stream, addr)) => {
                if clients.len() >= MAX_CLIENTS {
                    info!("Rejecting connection from {addr} (server full)");
                    drop(stream);
                } else {
                    stream.set_nonblocking(true)?;
                    info!("New connection from {addr}");
                    clients.insert(
                        next_id,
                        Client {
                            stream,
                            addr,
                            buf: Vec::new(),
                        },
                    );
                    next_id += 1;
                }
            }
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {}
            Err(e) => error!("Accept error: {e}"),
        }

        // Poll existing clients
        let mut to_remove = Vec::new();
        for (&id, client) in clients.iter_mut() {
            match handle_data(client) {
                Ok(msgs) => {
                    for msg in msgs {
                        match &msg {
                            Message::GameMessage(text) => {
                                info!("Received custom message from {}: {}", client.addr, text);
                            }
                        }
                    }
                }
                Err(ref e)
                    if e.downcast_ref::<io::Error>()
                        .map_or(false, |io_e| io_e.kind() == io::ErrorKind::WouldBlock) => {}
                Err(_) => {
                    info!("Disconnection from {}", client.addr);
                    to_remove.push(id);
                }
            }
        }
        for id in to_remove {
            clients.remove(&id);
        }

        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}
