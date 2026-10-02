use serde::{Deserialize, Serialize};

/// Default server port.
pub const DEFAULT_PORT: u16 = 6000;

/// Maximum number of connected clients.
pub const MAX_CLIENTS: usize = 2;

/// Messages exchanged between client and server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Message {
    /// A chat / game message carrying arbitrary text.
    GameMessage(String),
}

/// Serialize a [`Message`] to bytes using bincode.
pub fn serialize(msg: &Message) -> Vec<u8> {
    bincode::serialize(msg).expect("serialization should not fail")
}

/// Deserialize a [`Message`] from bytes.
pub fn deserialize(data: &[u8]) -> Result<Message, bincode::Error> {
    bincode::deserialize(data)
}
