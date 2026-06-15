//! y-websocket sync client.
//!
//! Connects a local [`yrs::Doc`] to a y-websocket room (the embedded
//! [`crate::SyncServer`] or any compatible server) using the standard y-sync
//! protocol, so edits made in the native editor propagate to browser Yjs peers
//! sharing the same room and vice versa.

use std::sync::Arc;

use futures_channel::mpsc::unbounded;
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message as WsMessage;
use yrs::sync::{Message, SyncMessage};
use yrs::updates::decoder::Decode;
use yrs::updates::encoder::Encode;
use yrs::{Doc, ReadTxn, Subscription, Transact, Update};

use crate::Result;

/// Origin tag applied to remote updates so the local-update observer does not
/// echo them back onto the wire.
const REMOTE_ORIGIN: &str = "moonlit.sync.remote";

/// A live sync connection. Dropping it stops forwarding local updates.
pub struct SyncClient {
    _subscription: Subscription,
    handle: tokio::task::JoinHandle<()>,
}

impl SyncClient {
    /// Connect `doc` to `url` (e.g. `ws://127.0.0.1:1234/docforge-word`).
    ///
    /// `on_change` is invoked after every remote update is applied, so the UI
    /// can request a repaint.
    pub async fn connect<F>(url: String, doc: Doc, on_change: F) -> Result<SyncClient>
    where
        F: Fn() + Send + Sync + 'static,
    {
        let (ws, _resp) = tokio_tungstenite::connect_async(url.as_str()).await?;
        let (mut write, mut read) = ws.split();

        // Outgoing frame channel shared by the local-update observer and the
        // protocol handler.
        let (out_tx, mut out_rx) = unbounded::<Vec<u8>>();

        // Forward local document updates (skip the ones we applied from remote).
        let observer_tx = out_tx.clone();
        let subscription = doc
            .observe_update_v1(move |txn, event| {
                let is_remote = txn
                    .origin()
                    .map(|o| o.as_ref() == REMOTE_ORIGIN.as_bytes())
                    .unwrap_or(false);
                if is_remote {
                    return;
                }
                let msg = Message::Sync(SyncMessage::Update(event.update.clone()));
                let _ = observer_tx.unbounded_send(msg.encode_v1());
            })
            .map_err(|e| crate::SyncError::Protocol(e.to_string()))?;

        // Kick off the handshake: send our state vector (SyncStep1).
        let sv = doc.transact().state_vector();
        let _ = out_tx.unbounded_send(Message::Sync(SyncMessage::SyncStep1(sv)).encode_v1());

        let on_change = Arc::new(on_change);
        let handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    outbound = out_rx.next() => {
                        match outbound {
                            Some(bytes) => {
                                if write.send(WsMessage::Binary(bytes)).await.is_err() {
                                    break;
                                }
                            }
                            None => break,
                        }
                    }
                    inbound = read.next() => {
                        match inbound {
                            Some(Ok(WsMessage::Binary(bytes))) => {
                                if let Some(reply) = handle_message(&doc, &bytes, on_change.as_ref()) {
                                    let _ = out_tx.unbounded_send(reply);
                                }
                            }
                            Some(Ok(WsMessage::Close(_))) | None => break,
                            Some(Ok(_)) => {}
                            Some(Err(_)) => break,
                        }
                    }
                }
            }
        });

        Ok(SyncClient {
            _subscription: subscription,
            handle,
        })
    }

    pub fn abort(&self) {
        self.handle.abort();
    }
}

/// Apply an incoming y-sync message to `doc`, returning an optional reply.
fn handle_message(
    doc: &Doc,
    bytes: &[u8],
    on_change: &(dyn Fn() + Send + Sync),
) -> Option<Vec<u8>> {
    let msg = Message::decode_v1(bytes).ok()?;
    match msg {
        Message::Sync(SyncMessage::SyncStep1(sv)) => {
            let update = doc.transact().encode_state_as_update_v1(&sv);
            Some(Message::Sync(SyncMessage::SyncStep2(update)).encode_v1())
        }
        Message::Sync(SyncMessage::SyncStep2(update))
        | Message::Sync(SyncMessage::Update(update)) => {
            if let Ok(update) = Update::decode_v1(&update) {
                let mut txn = doc.transact_mut_with(REMOTE_ORIGIN.to_string());
                if txn.apply_update(update).is_ok() {
                    drop(txn);
                    on_change();
                }
            }
            None
        }
        _ => None,
    }
}
