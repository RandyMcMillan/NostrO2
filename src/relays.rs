use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use futures_util::{StreamExt, stream::SplitSink, SinkExt};
use tokio::{task::spawn_blocking, sync::mpsc::{UnboundedReceiver, unbounded_channel}};
use tokio_tungstenite::{tungstenite::{protocol::{Message as WsMessage, CloseFrame, frame::coding::CloseCode}, Error as TungsteniteError}, connect_async, WebSocketStream};
use std::sync::{Arc,Mutex};

use super::{utils::{new_keys}};
use super::{notes::{SignedNote}};

pub struct NostrRelay {
    _url: Arc<str>,
    ws_write: Arc<
      Mutex<
        SplitSink<
          WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>
          >,
          WsMessage
        >
      >
    >,
    notes_receiver: tokio::sync::Mutex<
      UnboundedReceiver<Result<WsMessage, TungsteniteError>>
    >,
}

impl NostrRelay {
    pub async fn new(relay_url:&str) -> Self {
        let url = relay_url;
        let url_object = url::Url::parse(url).unwrap();

        if let Ok((ws_stream, _)) = connect_async(url_object).await {
            let (ws_write, mut ws_read) = ws_stream.split();

            let (tx, rx) = unbounded_channel();
            tokio::spawn(async move {
                while let Some(note) = ws_read.next().await {
                    match tx.send(note) {
                        Ok(_) => (),
                        Err(e) => {
                            println!("Error sending note to channel: {:?}", e);
                        }
                    }
                }
            });

            NostrRelay {
                _url: Arc::from(url),
                ws_write: Arc::new(Mutex::new(ws_write)),
                notes_receiver: tokio::sync::Mutex::new(rx),
            }
        } else {
            panic!("Failed to connect to Nostr Relay");
        }
    }

    pub async fn subscribe(&self, filter: Value) -> Result<(), Box<dyn std::error::Error>> {
        let nostr_subscription = NostrSubscription::new(filter);
        let ws_stream = Arc::clone(&self.ws_write);
        spawn_blocking(move || {
            let mut write = ws_stream.lock().unwrap();
            match tokio::runtime::Handle::current().block_on(write.send(nostr_subscription)) {
                Ok(_) => (),
                Err(e) => {
                    println!("Error subscribing: {:?}", e);
                }
            }
        });

        Ok(())
    }

    pub async fn send_note(&self, note: SignedNote) {
        let ws_stream = Arc::clone(&self.ws_write);
        spawn_blocking(move || {
            let mut write = ws_stream.lock().unwrap();
            match tokio::runtime::Handle::current().block_on(
              write.send(note.prepare_ws_message())
            ) {
                Ok(_) => {
                    ()
                },
                Err(e) => {
                    println!("Error sending note to relay: {:?}", e);
                }
            }
        });
    }

    pub async fn read_notes(&self) -> Option<Result<WsMessage, TungsteniteError>> {
        let mut lock = self.notes_receiver.lock().await;
        lock.recv().await
    }

   pub async fn close(&self) -> Result<(), Box<dyn std::error::Error>> {
        let ws_write = Arc::clone(&self.ws_write);
        let close_msg = WsMessage::Close(Some(CloseFrame {
            code: CloseCode::Normal,
            reason: "Bye bye".into(),
        }));
        tokio::task::spawn_blocking(move || {
            let mut write_guard = ws_write.lock().unwrap();
            match tokio::runtime::Handle::current().block_on(write_guard.send(close_msg)) {
                Ok(_) => (),
                Err(e) => {
                    println!("Error closing the connection: {:?}", e);
                }
            }
        });
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
struct NostrSubscription {
    id: String,
    filters: Value,
}

impl NostrSubscription {
    fn new(filter: Value) -> WsMessage {
        let id = hex::encode(&new_keys()[..]);
        let nostr_subscription = NostrSubscription {
            id,
            filters: filter,
        };
        let nostr_subscription_string = WsMessage::Text(
            json!(["REQ", nostr_subscription.id, nostr_subscription.filters]).to_string(),
        );
        nostr_subscription_string
    }
}