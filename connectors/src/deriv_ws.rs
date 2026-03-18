use bot_core::types::{BotError, TickUpdate};
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, Mutex};
use tokio_tungstenite::{
    connect_async,
    tungstenite::Message,
    MaybeTlsStream, WebSocketStream,
};
#[allow(unused_imports)]
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

pub type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Central Deriv WebSocket client.
///
/// Architecture:
/// - Connects to `wss://ws.derivws.com/websockets/v3?app_id=APP_ID`
/// - Authorizes with API token
/// - Sends JSON requests, dispatches responses to typed channels
pub struct DerivWsClient {
    pub write: Arc<Mutex<futures_util::stream::SplitSink<WsStream, Message>>>,
    pub app_id: String,
    pub endpoint: String,
    req_id_counter: Arc<Mutex<u64>>,
}

impl DerivWsClient {
    /// Connect to Deriv WebSocket and return (client, read_stream).
    pub async fn connect(
        app_id: &str,
        endpoint: &str,
    ) -> Result<(Self, futures_util::stream::SplitStream<WsStream>), BotError> {
        let url_str = format!("{}?app_id={}", endpoint, app_id);

        let (ws_stream, _response) = connect_async(&url_str)
            .await
            .map_err(|e| BotError::Network(format!("WS connect failed: {}", e)))?;

        println!("DEB: Connected to Deriv WS at {}", endpoint);

        let (write, read) = ws_stream.split();

        let client = Self {
            write: Arc::new(Mutex::new(write)),
            app_id: app_id.to_string(),
            endpoint: endpoint.to_string(),
            req_id_counter: Arc::new(Mutex::new(1)),
        };

        Ok((client, read))
    }

    /// Get next unique request ID.
    async fn next_req_id(&self) -> u64 {
        let mut counter = self.req_id_counter.lock().await;
        let id = *counter;
        *counter += 1;
        id
    }

    /// Send a raw JSON message.
    pub async fn send_json(&self, msg: Value) -> Result<(), BotError> {
        let text = serde_json::to_string(&msg)
            .map_err(|e| BotError::Other(format!("JSON serialize: {}", e)))?;
        let mut w = self.write.lock().await;
        w.send(Message::Text(text.into()))
            .await
            .map_err(|e| BotError::Network(format!("WS send: {}", e)))?;
        Ok(())
    }

    /// Authorize with an API token.
    pub async fn authorize(&self, api_token: &str) -> Result<(), BotError> {
        let req_id = self.next_req_id().await;
        let msg = serde_json::json!({
            "authorize": api_token,
            "req_id": req_id
        });
        self.send_json(msg).await?;
        println!("DEB: Authorization request sent (req_id={})", req_id);
        Ok(())
    }

    /// Request server time for time sync.
    pub async fn request_time(&self) -> Result<(), BotError> {
        let req_id = self.next_req_id().await;
        let msg = serde_json::json!({
            "time": 1,
            "req_id": req_id
        });
        self.send_json(msg).await
    }

    /// Subscribe to ticks for a symbol.
    pub async fn subscribe_ticks(&self, symbol: &str) -> Result<(), BotError> {
        let req_id = self.next_req_id().await;
        let msg = serde_json::json!({
            "ticks": symbol,
            "subscribe": 1,
            "req_id": req_id
        });
        self.send_json(msg).await?;
        println!("DEB: Subscribed to ticks for {} (req_id={})", symbol, req_id);
        Ok(())
    }

    /// Request a price proposal for a Rise/Fall contract.
    pub async fn request_proposal(
        &self,
        symbol: &str,
        contract_type: &str, // "CALL" or "PUT"
        duration: u64,
        duration_unit: &str, // "s" for seconds, "t" for ticks, "m" for minutes
        stake: f64,
    ) -> Result<u64, BotError> {
        let req_id = self.next_req_id().await;
        let msg = serde_json::json!({
            "proposal": 1,
            "amount": stake,
            "basis": "stake",
            "contract_type": contract_type,
            "currency": "USD",
            "duration": duration,
            "duration_unit": duration_unit,
            "symbol": symbol,
            "req_id": req_id
        });
        self.send_json(msg).await?;
        println!("DEB: Proposal request for {} {} {}{}  stake={:.2} (req_id={})",
            symbol, contract_type, duration, duration_unit, stake, req_id);
        Ok(req_id)
    }

    /// Buy a contract using a proposal ID.
    pub async fn buy_contract(&self, proposal_id: &str, price: f64) -> Result<u64, BotError> {
        let req_id = self.next_req_id().await;
        let msg = serde_json::json!({
            "buy": proposal_id,
            "price": price,
            "req_id": req_id
        });
        self.send_json(msg).await?;
        println!("DEB: Buy request for proposal {} price={:.2} (req_id={})", proposal_id, price, req_id);
        Ok(req_id)
    }

    /// Request current balance with optional subscription.
    pub async fn get_balance(&self, subscribe: bool) -> Result<(), BotError> {
        let req_id = self.next_req_id().await;
        let mut msg = serde_json::json!({
            "balance": 1,
            "req_id": req_id
        });
        if subscribe {
            msg["subscribe"] = serde_json::json!(1);
        }
        self.send_json(msg).await
    }

    /// Request active symbols.
    pub async fn get_active_symbols(&self) -> Result<(), BotError> {
        let req_id = self.next_req_id().await;
        let msg = serde_json::json!({
            "active_symbols": "brief",
            "product_type": "basic",
            "req_id": req_id
        });
        self.send_json(msg).await
    }

    /// Request available contracts for a symbol.
    pub async fn get_contracts_for(&self, symbol: &str) -> Result<(), BotError> {
        let req_id = self.next_req_id().await;
        let msg = serde_json::json!({
            "contracts_for": symbol,
            "currency": "USD",
            "product_type": "basic",
            "req_id": req_id
        });
        self.send_json(msg).await
    }

    /// Forget a subscription by ID.
    pub async fn forget(&self, subscription_id: &str) -> Result<(), BotError> {
        let req_id = self.next_req_id().await;
        let msg = serde_json::json!({
            "forget": subscription_id,
            "req_id": req_id
        });
        self.send_json(msg).await
    }
}

/// Parse a raw WS text message into a categorized event.
#[derive(Debug)]
pub enum DerivEvent {
    Authorize { balance: f64, currency: String, login_id: String },
    Tick(TickUpdate),
    Proposal { req_id: u64, proposal_id: String, ask_price: f64, payout: f64 },
    Buy { contract_id: String, buy_price: f64, payout: f64 },
    Balance { balance: f64, currency: String },
    Time { server_time: i64 },
    Error { code: String, message: String },
    Unknown(Value),
}

/// Parse a text WS message into a DerivEvent.
pub fn parse_deriv_message(text: &str) -> DerivEvent {
    let v: Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(_) => return DerivEvent::Unknown(Value::Null),
    };

    // Check for error
    if let Some(err) = v.get("error") {
        return DerivEvent::Error {
            code: err.get("code").and_then(|c| c.as_str()).unwrap_or("").to_string(),
            message: err.get("message").and_then(|m| m.as_str()).unwrap_or("").to_string(),
        };
    }

    // Dispatch by msg_type
    let msg_type = v.get("msg_type").and_then(|m| m.as_str()).unwrap_or("");

    match msg_type {
        "authorize" => {
            let auth = &v["authorize"];
            DerivEvent::Authorize {
                balance: auth.get("balance").and_then(|b| b.as_f64())
                    .or_else(|| auth.get("balance").and_then(|b| b.as_str()).and_then(|s| s.parse().ok()))
                    .unwrap_or(0.0),
                currency: auth.get("currency").and_then(|c| c.as_str()).unwrap_or("USD").to_string(),
                login_id: auth.get("loginid").and_then(|l| l.as_str()).unwrap_or("").to_string(),
            }
        }
        "tick" => {
            let tick = &v["tick"];
            DerivEvent::Tick(TickUpdate {
                symbol: tick.get("symbol").and_then(|s| s.as_str()).unwrap_or("").to_string(),
                price: tick.get("quote").and_then(|q| q.as_f64()).unwrap_or(0.0),
                epoch: tick.get("epoch").and_then(|e| e.as_i64()).unwrap_or(0),
            })
        }
        "proposal" => {
            let prop = &v["proposal"];
            DerivEvent::Proposal {
                req_id: v.get("req_id").and_then(|r| r.as_u64()).unwrap_or(0),
                proposal_id: prop.get("id").and_then(|i| i.as_str()).unwrap_or("").to_string(),
                ask_price: prop.get("ask_price").and_then(|a| a.as_f64())
                    .or_else(|| prop.get("ask_price").and_then(|a| a.as_str()).and_then(|s| s.parse().ok()))
                    .unwrap_or(0.0),
                payout: prop.get("payout").and_then(|p| p.as_f64())
                    .or_else(|| prop.get("payout").and_then(|p| p.as_str()).and_then(|s| s.parse().ok()))
                    .unwrap_or(0.0),
            }
        }
        "buy" => {
            let buy = &v["buy"];
            DerivEvent::Buy {
                contract_id: buy.get("contract_id").and_then(|c| c.as_u64()).map(|c| c.to_string())
                    .or_else(|| buy.get("contract_id").and_then(|c| c.as_str()).map(|s| s.to_string()))
                    .unwrap_or_default(),
                buy_price: buy.get("buy_price").and_then(|b| b.as_f64())
                    .or_else(|| buy.get("buy_price").and_then(|b| b.as_str()).and_then(|s| s.parse().ok()))
                    .unwrap_or(0.0),
                payout: buy.get("payout").and_then(|p| p.as_f64())
                    .or_else(|| buy.get("payout").and_then(|p| p.as_str()).and_then(|s| s.parse().ok()))
                    .unwrap_or(0.0),
            }
        }
        "balance" => {
            let bal = &v["balance"];
            DerivEvent::Balance {
                balance: bal.get("balance").and_then(|b| b.as_f64())
                    .or_else(|| bal.get("balance").and_then(|b| b.as_str()).and_then(|s| s.parse().ok()))
                    .unwrap_or(0.0),
                currency: bal.get("currency").and_then(|c| c.as_str()).unwrap_or("USD").to_string(),
            }
        }
        "time" => {
            DerivEvent::Time {
                server_time: v.get("time").and_then(|t| t.as_i64()).unwrap_or(0),
            }
        }
        _ => DerivEvent::Unknown(v),
    }
}

/// Central dispatcher: reads from WS stream and routes tick updates to mpsc channel.
/// Returns on WS close or fatal error.
pub async fn run_tick_dispatcher(
    mut read: futures_util::stream::SplitStream<WsStream>,
    tick_tx: mpsc::Sender<TickUpdate>,
    event_tx: mpsc::Sender<DerivEvent>,
) {
    use tokio_tungstenite::tungstenite::Message;

    while let Some(msg_result) = read.next().await {
        match msg_result {
            Ok(Message::Text(text)) => {
                let event = parse_deriv_message(&text);
                match event {
                    DerivEvent::Tick(ref tick) => {
                        let _ = tick_tx.try_send(tick.clone());
                    }
                    _ => {}
                }
                let _ = event_tx.try_send(event);
            }
            Ok(Message::Ping(data)) => {
                // Pong is sent automatically by tungstenite
                let _ = data;
            }
            Ok(Message::Close(_)) => {
                println!("DEB: WS connection closed by server.");
                break;
            }
            Err(e) => {
                eprintln!("WS read error: {}", e);
                break;
            }
            _ => {}
        }
    }
    println!("DEB: Tick dispatcher ended.");
}
