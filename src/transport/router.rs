use crate::transport::ws_client::WsCommand;
use crate::types::BotError;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot, Mutex};
use tracing::{debug, warn};

/// Subscription info tracked for replay on reconnect.
#[derive(Debug, Clone)]
pub struct SubscriptionInfo {
    pub sub_id: String,
    pub request_payload: Value,
    pub msg_type: String,
}

/// Router: correlates req_id → oneshot (RPC) and manages subscription registry.
pub struct Router {
    /// Pending RPC requests: req_id → oneshot sender.
    pending_rpcs: Arc<Mutex<HashMap<u64, oneshot::Sender<Value>>>>,
    /// Active subscriptions: sub_id → info.
    subscriptions: Arc<Mutex<HashMap<String, SubscriptionInfo>>>,
    /// Channel to send WS write commands.
    write_tx: mpsc::Sender<WsCommand>,
    /// Next req_id counter.
    req_id_counter: Arc<Mutex<u64>>,
}

impl Router {
    pub fn new(write_tx: mpsc::Sender<WsCommand>) -> Self {
        Self {
            pending_rpcs: Arc::new(Mutex::new(HashMap::new())),
            subscriptions: Arc::new(Mutex::new(HashMap::new())),
            write_tx,
            req_id_counter: Arc::new(Mutex::new(1)),
        }
    }

    /// Allocate next req_id.
    pub async fn next_req_id(&self) -> u64 {
        let mut c = self.req_id_counter.lock().await;
        let id = *c;
        *c += 1;
        id
    }

    /// Send an RPC request and await the response (with timeout).
    pub async fn send_rpc(&self, mut payload: Value, timeout_ms: u64) -> Result<Value, BotError> {
        let req_id = self.next_req_id().await;
        payload["req_id"] = serde_json::json!(req_id);

        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending_rpcs.lock().await;
            pending.insert(req_id, tx);
        }

        let text = serde_json::to_string(&payload)
            .map_err(|e| BotError::Other(format!("serialize: {}", e)))?;
        self.write_tx
            .send(WsCommand { payload: text })
            .await
            .map_err(|e| BotError::Network(format!("write channel: {}", e)))?;

        debug!(req_id, "RPC sent, awaiting response");

        let result = tokio::time::timeout(std::time::Duration::from_millis(timeout_ms), rx).await;

        match result {
            Ok(Ok(val)) => Ok(val),
            Ok(Err(_)) => Err(BotError::Other("RPC oneshot dropped".into())),
            Err(_) => {
                // Cleanup
                self.pending_rpcs.lock().await.remove(&req_id);
                Err(BotError::Network(format!(
                    "RPC timeout after {}ms (req_id={})",
                    timeout_ms, req_id
                )))
            }
        }
    }

    /// Send a fire-and-forget request (no response expected).
    pub async fn send_fire(&self, mut payload: Value) -> Result<u64, BotError> {
        let req_id = self.next_req_id().await;
        payload["req_id"] = serde_json::json!(req_id);
        let text = serde_json::to_string(&payload)
            .map_err(|e| BotError::Other(format!("serialize: {}", e)))?;
        self.write_tx
            .send(WsCommand { payload: text })
            .await
            .map_err(|e| BotError::Network(format!("write channel: {}", e)))?;
        Ok(req_id)
    }

    /// Route an incoming message: resolve RPC or dispatch to event handler.
    /// Returns true if it was an RPC response (resolved), false otherwise.
    pub async fn route_message(&self, msg: &Value) -> bool {
        // Check for RPC response (has req_id and matches pending)
        if let Some(req_id) = msg.get("req_id").and_then(|r| r.as_u64()) {
            let mut pending = self.pending_rpcs.lock().await;
            if let Some(tx) = pending.remove(&req_id) {
                let _ = tx.send(msg.clone());
                return true;
            }
        }

        // Track subscription IDs
        if let Some(sub) = msg
            .get("subscription")
            .and_then(|s| s.get("id"))
            .and_then(|i| i.as_str())
        {
            let msg_type = msg
                .get("msg_type")
                .and_then(|m| m.as_str())
                .unwrap_or("")
                .to_string();
            debug!(sub_id = sub, msg_type = %msg_type, "Subscription message received");
        }

        false
    }

    /// Register a subscription for replay on reconnect.
    pub async fn register_subscription(
        &self,
        sub_id: &str,
        request_payload: Value,
        msg_type: &str,
    ) {
        let mut subs = self.subscriptions.lock().await;
        subs.insert(
            sub_id.to_string(),
            SubscriptionInfo {
                sub_id: sub_id.to_string(),
                request_payload,
                msg_type: msg_type.to_string(),
            },
        );
        debug!(sub_id, "Subscription registered");
    }

    /// Remove a subscription.
    pub async fn remove_subscription(&self, sub_id: &str) {
        self.subscriptions.lock().await.remove(sub_id);
    }

    /// Get all subscription payloads for replay after reconnect.
    pub async fn get_replay_payloads(&self) -> Vec<Value> {
        let subs = self.subscriptions.lock().await;
        subs.values().map(|s| s.request_payload.clone()).collect()
    }

    /// Send forget for a specific subscription.
    pub async fn forget(&self, sub_id: &str) -> Result<(), BotError> {
        let payload = serde_json::json!({ "forget": sub_id });
        self.send_fire(payload).await?;
        self.remove_subscription(sub_id).await;
        Ok(())
    }

    /// Send forget_all for a message type.
    pub async fn forget_all(&self, msg_type: &str) -> Result<(), BotError> {
        let payload = serde_json::json!({ "forget_all": msg_type });
        self.send_fire(payload).await?;
        // Remove matching subscriptions
        let mut subs = self.subscriptions.lock().await;
        subs.retain(|_, v| v.msg_type != msg_type);
        Ok(())
    }

    /// Clear all pending RPCs (on disconnect).
    pub async fn clear_pending(&self) {
        let mut pending = self.pending_rpcs.lock().await;
        let count = pending.len();
        pending.clear();
        if count > 0 {
            warn!(count, "Cleared pending RPCs on disconnect");
        }
    }
}
