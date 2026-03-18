use crate::types::{BotError, ConnectionState};
use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, watch, Mutex};
use tokio_tungstenite::{connect_async, tungstenite::Message, MaybeTlsStream, WebSocketStream};
use tracing::{debug, error, info, warn};

pub type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;
pub type WsSink = futures_util::stream::SplitSink<WsStream, Message>;
pub type WsSource = futures_util::stream::SplitStream<WsStream>;

/// WebSocket command sent from engine to write loop.
#[derive(Debug)]
pub struct WsCommand {
    pub payload: String,
}

/// WebSocket client with Connection FSM, reconnection, and health checks.
pub struct WsClient {
    pub app_id: String,
    pub endpoint: String,
    pub api_token: String,
    state: Arc<Mutex<ConnectionState>>,
    state_tx: watch::Sender<ConnectionState>,
}

impl WsClient {
    pub fn new(
        app_id: &str,
        endpoint: &str,
        api_token: &str,
    ) -> (Self, watch::Receiver<ConnectionState>) {
        let (state_tx, state_rx) = watch::channel(ConnectionState::Disconnected);
        (
            Self {
                app_id: app_id.to_string(),
                endpoint: endpoint.to_string(),
                api_token: api_token.to_string(),
                state: Arc::new(Mutex::new(ConnectionState::Disconnected)),
                state_tx,
            },
            state_rx,
        )
    }

    pub async fn set_state(&self, new_state: ConnectionState) {
        let mut s = self.state.lock().await;
        info!(from = ?*s, to = ?new_state, "Connection FSM transition");
        *s = new_state;
        let _ = self.state_tx.send(new_state);
    }

    pub async fn current_state(&self) -> ConnectionState {
        *self.state.lock().await
    }

    /// Connect with exponential backoff. Returns (sink, source) on success.
    pub async fn connect_with_backoff(&self) -> Result<(WsSink, WsSource), BotError> {
        let url = format!("{}?app_id={}", self.endpoint, self.app_id);
        let mut attempt = 0u32;
        let max_delay = Duration::from_secs(30);

        loop {
            self.set_state(ConnectionState::Connecting).await;
            info!(attempt, url = %url, "Connecting to Deriv WebSocket");

            match connect_async(&url).await {
                Ok((ws_stream, _resp)) => {
                    self.set_state(ConnectionState::Connected).await;
                    info!("WebSocket connected");
                    let (sink, source) = ws_stream.split();
                    return Ok((sink, source));
                }
                Err(e) => {
                    attempt += 1;
                    let delay = std::cmp::min(
                        Duration::from_millis(1000 * 2u64.pow(attempt.min(5))),
                        max_delay,
                    );
                    warn!(attempt, error = %e, delay_ms = delay.as_millis(), "Connect failed, retrying");
                    self.set_state(ConnectionState::Reconnecting).await;
                    tokio::time::sleep(delay).await;
                }
            }
        }
    }

    /// Authorize the session. Sends authorize request and waits for response via event channel.
    pub async fn send_authorize(
        write_tx: &mpsc::Sender<WsCommand>,
        api_token: &str,
        req_id: u64,
    ) -> Result<(), BotError> {
        let msg = serde_json::json!({
            "authorize": api_token,
            "req_id": req_id
        });
        let payload = serde_json::to_string(&msg).map_err(|e| BotError::Other(e.to_string()))?;
        write_tx
            .send(WsCommand { payload })
            .await
            .map_err(|e| BotError::Network(format!("Write channel closed: {}", e)))?;
        debug!(req_id, "Authorization request sent");
        Ok(())
    }

    /// Send a ping/time request for health checks.
    pub async fn send_ping(
        write_tx: &mpsc::Sender<WsCommand>,
        req_id: u64,
    ) -> Result<(), BotError> {
        let msg = serde_json::json!({ "time": 1, "req_id": req_id });
        let payload = serde_json::to_string(&msg).map_err(|e| BotError::Other(e.to_string()))?;
        write_tx
            .send(WsCommand { payload })
            .await
            .map_err(|e| BotError::Network(format!("Write channel closed: {}", e)))?;
        Ok(())
    }

    /// Send forget_all + logout for graceful shutdown.
    pub async fn send_shutdown(
        write_tx: &mpsc::Sender<WsCommand>,
        req_id_base: u64,
    ) -> Result<(), BotError> {
        // forget_all ticks
        let msg1 = serde_json::json!({ "forget_all": "ticks", "req_id": req_id_base });
        let _ = write_tx
            .send(WsCommand {
                payload: serde_json::to_string(&msg1).unwrap(),
            })
            .await;
        // forget_all proposal_open_contract
        let msg2 = serde_json::json!({ "forget_all": "proposal_open_contract", "req_id": req_id_base + 1 });
        let _ = write_tx
            .send(WsCommand {
                payload: serde_json::to_string(&msg2).unwrap(),
            })
            .await;
        // logout
        let msg3 = serde_json::json!({ "logout": 1, "req_id": req_id_base + 2 });
        let _ = write_tx
            .send(WsCommand {
                payload: serde_json::to_string(&msg3).unwrap(),
            })
            .await;
        info!("Shutdown sequence sent (forget_all + logout)");
        Ok(())
    }
}

/// ws_write_loop: receives WsCommands from mpsc, sends to WebSocket sink.
pub async fn ws_write_loop(mut sink: WsSink, mut cmd_rx: mpsc::Receiver<WsCommand>) {
    while let Some(cmd) = cmd_rx.recv().await {
        if let Err(e) = sink.send(Message::Text(cmd.payload.into())).await {
            error!(error = %e, "WS write error");
            break;
        }
    }
    debug!("ws_write_loop ended");
}

pub async fn ws_read_loop(
    mut source: WsSource,
    raw_msg_tx: mpsc::Sender<String>,
    router: Arc<crate::transport::router::Router>,
) {
    while let Some(msg_result) = source.next().await {
        match msg_result {
            Ok(Message::Text(text)) => {
                let text_str = text.to_string();
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text_str) {
                    if router.route_message(&v).await {
                        continue; // Was an RPC response, handled
                    }
                }
                if raw_msg_tx.send(text_str).await.is_err() {
                    break;
                }
            }
            Ok(Message::Ping(_)) => { /* pong auto-sent by tungstenite */ }
            Ok(Message::Close(_)) => {
                info!("WebSocket closed by server");
                break;
            }
            Err(e) => {
                error!(error = %e, "WS read error");
                break;
            }
            _ => {}
        }
    }
    debug!("ws_read_loop ended");
}
