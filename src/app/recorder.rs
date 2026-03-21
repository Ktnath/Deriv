use crate::{
    config::RecorderConfig,
    observability::{db::TelemetryDb, events},
    protocol::{
        self,
        responses::{parse_response, DerivResponse},
    },
    transport::{
        router::Router,
        ws_client::{self, WsClient},
    },
    types::{ConnectionState, UnixMs},
};
use std::{sync::Arc, time::Duration};
use tokio::sync::mpsc;
use tracing::{error, info, warn};

pub async fn run_recorder() -> anyhow::Result<()> {
    events::init_logging();
    let cfg = RecorderConfig::from_env();
    info!(db_path = %cfg.db_path, symbols = ?cfg.symbols, subscribe_balance = cfg.subscribe_balance, subscribe_time = cfg.subscribe_time, retention_days = ?cfg.retention_days, "Recorder configuration loaded");
    let db = Arc::new(TelemetryDb::new(&cfg.db_path)?);

    loop {
        info!(endpoint = %cfg.shared.endpoint, "Establishing recorder WebSocket connection");
        let (ws_client, _state_rx) = WsClient::new(
            &cfg.shared.app_id,
            &cfg.shared.endpoint,
            &cfg.shared.api_token,
        );
        let (sink, source) = ws_client.connect_with_backoff().await?;
        let (write_tx, write_rx) = mpsc::channel::<ws_client::WsCommand>(2048);
        let (raw_msg_tx, mut raw_msg_rx) = mpsc::channel::<String>(4096);
        let router = Arc::new(Router::new(write_tx.clone()));
        tokio::spawn(ws_client::ws_read_loop(source, raw_msg_tx, router.clone()));
        tokio::spawn(ws_client::ws_write_loop(sink, write_rx));

        if let Err(e) = router
            .send_fire(protocol::requests::authorize(&cfg.shared.api_token))
            .await
        {
            error!(error = %e, "Failed to authorize recorder session, reconnecting");
            tokio::time::sleep(Duration::from_secs(5)).await;
            continue;
        }

        let disconnect_reason = loop {
            tokio::select! {
                msg = raw_msg_rx.recv() => {
                    let Some(raw_msg) = msg else { break "WS read channel closed"; };
                    match parse_response(&raw_msg) {
                        DerivResponse::Authorize { login_id, .. } => {
                            info!(login_id = %login_id, "Authorized recorder session");
                            ws_client.set_state(ConnectionState::Authorized).await;
                            for symbol in &cfg.symbols {
                                let _ = router.send_fire(protocol::requests::ticks(symbol)).await;
                                info!(symbol = %symbol, "Recorder subscribed to ticks");
                            }
                            if cfg.subscribe_balance {
                                let _ = router.send_fire(protocol::requests::balance_subscribe()).await;
                            }
                            ws_client.set_state(ConnectionState::Running).await;
                        }
                        DerivResponse::Tick(tick) => {
                            let received_at = UnixMs::now().0;
                            db.insert_raw_tick(tick.epoch * 1000, received_at, tick.price, &tick.symbol, "recorder")?;
                        }
                        DerivResponse::Balance { .. } => {
                            db.insert_recorder_metadata(UnixMs::now().0, "balance", None, &raw_msg)?;
                        }
                        DerivResponse::Time { .. } => {
                            db.insert_recorder_metadata(UnixMs::now().0, "time", None, &raw_msg)?;
                        }
                        DerivResponse::Error { code, message, .. } => warn!(code = %code, message = %message, "Recorder API error"),
                        _ => {}
                    }
                }
                _ = tokio::time::sleep(Duration::from_secs(60)) => {
                    if cfg.subscribe_time {
                        let _ = router.send_fire(protocol::requests::time()).await;
                    }
                    if let Some(days) = cfg.retention_days {
                        let cutoff = UnixMs::now().0 - (days as i64 * 24 * 60 * 60 * 1000);
                        match db.prune_raw_ticks_older_than(cutoff) {
                            Ok(deleted) if deleted > 0 => info!(deleted, retention_days = days, "Recorder pruned old raw ticks"),
                            Ok(_) => {},
                            Err(e) => warn!(error = ?e, retention_days = days, "Failed to prune raw ticks"),
                        }
                    }
                }
            }
        };

        warn!(
            reason = disconnect_reason,
            "Recorder connection lost, reconnecting in 5s"
        );
        router.clear_pending().await;
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}
