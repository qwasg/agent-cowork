//! Embedded WebSocket sync server.
//!
//! The previous Node server used y-websocket semantics with room = URL path and
//! broadcast-to-peers behavior. This module preserves that transport contract.
//! Binary y-sync frames are treated as opaque bytes and relayed unchanged, which
//! keeps the server compatible with Yjs/Yrs peers while the document semantics
//! live at the client side.

use futures_util::{SinkExt, StreamExt};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use thiserror::Error;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, RwLock};
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
use tokio_tungstenite::tungstenite::Message;

mod client;
pub use client::SyncClient;

#[derive(Debug, Error)]
pub enum SyncError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("websocket error: {0}")]
    WebSocket(#[from] tokio_tungstenite::tungstenite::Error),
    #[error("protocol error: {0}")]
    Protocol(String),
}

pub type Result<T> = std::result::Result<T, SyncError>;

#[derive(Debug, Clone)]
pub struct SyncServerConfig {
    pub host: String,
    pub port: u16,
    pub channel_capacity: usize,
}

impl Default for SyncServerConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 1234,
            channel_capacity: 1024,
        }
    }
}

#[derive(Clone)]
pub struct SyncServer {
    rooms: Arc<RwLock<HashMap<String, broadcast::Sender<RoomMessage>>>>,
    capacity: usize,
}

impl SyncServer {
    pub fn new(capacity: usize) -> Self {
        Self {
            rooms: Arc::new(RwLock::new(HashMap::new())),
            capacity: capacity.max(16),
        }
    }

    pub async fn listen(config: SyncServerConfig) -> Result<SocketAddr> {
        let addr = format!("{}:{}", config.host, config.port);
        let listener = TcpListener::bind(addr).await?;
        let local_addr = listener.local_addr()?;
        let server = SyncServer::new(config.channel_capacity);
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let server = server.clone();
                tokio::spawn(async move {
                    let _ = server.handle_connection(stream).await;
                });
            }
        });
        Ok(local_addr)
    }

    pub async fn handle_connection(&self, stream: TcpStream) -> Result<()> {
        let room_slot: Arc<std::sync::Mutex<Option<String>>> =
            Arc::new(std::sync::Mutex::new(None));
        let room_capture = room_slot.clone();
        let ws =
            tokio_tungstenite::accept_hdr_async(stream, move |req: &Request, resp: Response| {
                let room = room_from_uri(req.uri().path());
                *room_capture.lock().unwrap() = Some(room);
                Ok(resp)
            })
            .await?;
        let room = room_slot
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_else(|| "/".to_string());
        self.run_socket(room, ws).await
    }

    async fn run_socket<S>(
        &self,
        room: String,
        ws: tokio_tungstenite::WebSocketStream<S>,
    ) -> Result<()>
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    {
        let sender = self.room_sender(&room).await;
        let mut rx = sender.subscribe();
        let peer_id = next_peer_id();
        let (mut write, mut read) = ws.split();
        loop {
            tokio::select! {
                inbound = read.next() => {
                    match inbound {
                        Some(Ok(Message::Binary(bytes))) => {
                            let _ = sender.send(RoomMessage { peer_id, message: Message::Binary(bytes) });
                        }
                        Some(Ok(Message::Text(text))) => {
                            let _ = sender.send(RoomMessage { peer_id, message: Message::Text(text) });
                        }
                        Some(Ok(Message::Close(close))) => {
                            let _ = write.send(Message::Close(close)).await;
                            break;
                        }
                        Some(Ok(_)) => {}
                        Some(Err(err)) => return Err(SyncError::WebSocket(err)),
                        None => break,
                    }
                }
                outbound = rx.recv() => {
                    match outbound {
                        Ok(frame) if frame.peer_id != peer_id => {
                            write.send(frame.message).await?;
                        }
                        Ok(_) => {}
                        Err(broadcast::error::RecvError::Lagged(_)) => {}
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
            }
        }
        Ok(())
    }

    async fn room_sender(&self, room: &str) -> broadcast::Sender<RoomMessage> {
        {
            let rooms = self.rooms.read().await;
            if let Some(sender) = rooms.get(room) {
                return sender.clone();
            }
        }
        let mut rooms = self.rooms.write().await;
        rooms
            .entry(room.to_string())
            .or_insert_with(|| broadcast::channel(self.capacity).0)
            .clone()
    }
}

#[derive(Debug, Clone)]
struct RoomMessage {
    peer_id: u64,
    message: Message,
}

fn next_peer_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

pub fn room_from_uri(path: &str) -> String {
    let room = path.trim();
    if room.is_empty() || room == "/" {
        "/".to_string()
    } else {
        room.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn room_path_is_preserved() {
        assert_eq!(room_from_uri("/docforge-word"), "/docforge-word");
        assert_eq!(room_from_uri("/docforge-ppt"), "/docforge-ppt");
        assert_eq!(room_from_uri(""), "/");
    }

    #[tokio::test]
    async fn can_bind_ephemeral_port() {
        let addr = SyncServer::listen(SyncServerConfig {
            host: "127.0.0.1".to_string(),
            port: 0,
            channel_capacity: 32,
        })
        .await
        .unwrap();
        assert!(addr.port() > 0);
    }
}
