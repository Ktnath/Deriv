use crate::{
    app::decision_engine::{DecisionContext, DecisionEngine, DecisionEngineConfig},
    config::ExecutorConfig,
    execution::trader::{PocAction, Trader},
    market_data::ticks::TickBuffer,
    observability::{
        db::{ExecutedTradeRecord, RunDecisionRecord, RunMetadata, TelemetryDb, TradeIntentRecord},
        events,
        metrics::Metrics,
    },
    protocol::{
        self,
        responses::{parse_response, DerivResponse},
    },
    risk::ledger::Ledger,
    server,
    transport::{
        router::Router,
        ws_client::{self, WsClient},
    },
    types::*,
};
use std::{sync::Arc, time::Duration};
use tokio::sync::{broadcast, mpsc};
use tracing::{debug, error, info, warn};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExecutionMode {
    Live,
    DryRun,
}

#[derive(Debug, Clone)]
struct PendingExecutionTelemetry {
    intent_id: i64,
    executed_trade_id: i64,
    stake: f64,
    contract_type: ContractType,
    mode: ExecutionMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CloseReason {
    NaturalExpiry,
    EarlyExit,
    DryRunExpiry,
}

pub async fn run_executor() -> anyhow::Result<()> {
    events::init_logging();
    let cfg = ExecutorConfig::from_env();
    info!(account_type = %cfg.account_type, base_stake = cfg.stake, telemetry_db = %cfg.telemetry_db_path, telemetry_bind = %cfg.telemetry_server_bind, "Executor configuration loaded");
    run_live_executor(cfg).await
}

pub async fn run_live_executor(cfg: ExecutorConfig) -> anyhow::Result<()> {
    let mut engine = DecisionEngine::new_live(DecisionEngineConfig {
        symbol: cfg.symbol.clone(),
        contract_duration: cfg.contract_duration,
        min_stake: cfg.min_stake,
        initial_balance: cfg.initial_balance,
        max_open_positions: cfg.max_open_positions,
        max_daily_loss: cfg.max_daily_loss,
        cooldown_after_loss_ms: cfg.cooldown_after_loss_ms,
        max_consecutive_losses: cfg.max_consecutive_losses,
        model_path: cfg.model_path.clone(),
        allow_model_fallback: cfg.allow_model_fallback,
        strategy_mode: format!("{:?}", cfg.strategy),
        prior_mode: "process-v1".into(),
    })?;
    info!(symbol = %cfg.symbol, primary_pipeline = "decision_engine", strategy = ?cfg.strategy, duration = cfg.contract_duration, duration_unit = %cfg.duration_unit, dry_run = cfg.dry_run, model_path = ?cfg.model_path, allow_model_fallback = cfg.allow_model_fallback, max_open_positions = cfg.max_open_positions, max_daily_loss = cfg.max_daily_loss, cooldown_ms = cfg.cooldown_after_loss_ms, max_consecutive_losses = cfg.max_consecutive_losses, min_stake = cfg.min_stake, stake_sizing_mode = "kelly_live", early_exit_loss_threshold_pct = cfg.stop_loss_pct * 100.0, "Startup configuration loaded");

    let (telemetry_tx, _telemetry_rx) = broadcast::channel::<String>(100);
    tokio::spawn(server::start_server(
        telemetry_tx.clone(),
        cfg.telemetry_server_bind.clone(),
    ));

    let mut tick_buf = TickBuffer::new(1000);
    let mut ledger = Ledger::new(cfg.initial_balance);
    let metrics = Arc::new(Metrics::new());

    let db = match TelemetryDb::new(&cfg.telemetry_db_path) {
        Ok(db) => Some(Arc::new(db)),
        Err(e) => {
            error!(error = ?e, "Failed to initialize TelemetryDb");
            None
        }
    };

    let run_id = format!("executor-{}-{}", cfg.symbol, UnixMs::now().0);
    if let Some(ref db) = db {
        let _ = db.upsert_run_metadata(&RunMetadata {
            run_id: run_id.clone(),
            binary_type: "executor".into(),
            model_version: cfg
                .model_path
                .clone()
                .unwrap_or_else(|| "quant-only".into()),
            strategy_version: format!("{:?}", cfg.strategy),
            prior_version: "process-v1".into(),
            config_fingerprint: format!(
                "{}:{}:{}:{}",
                cfg.symbol, cfg.contract_duration, cfg.min_stake, cfg.max_open_positions
            ),
            started_at_ms: UnixMs::now().0,
        });
    }

    let mut last_metrics_log = 0i64;
    let mut pending_execution: Option<PendingExecutionTelemetry> = None;

    loop {
        info!(endpoint = %cfg.shared.endpoint, "Establishing executor WebSocket connection");
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

        let auth_payload = protocol::requests::authorize(&cfg.shared.api_token);
        if let Err(e) = router.send_fire(auth_payload).await {
            error!(error = %e, "Failed to authorize executor session, reconnecting");
            tokio::time::sleep(Duration::from_secs(5)).await;
            continue;
        }

        let mut trader = Trader::new(Arc::clone(&router), cfg.dry_run, cfg.stop_loss_pct);
        if trader.active_trade.is_some() {
            warn!("Aborting stale trade from previous connection");
            trader.abort("Reconnection");
            trader.reset();
        }

        info!(symbol = %cfg.symbol, strategy = ?cfg.strategy, "Executor loop started");
        metrics.inc_reconnects();

        let disconnect_reason = loop {
            tokio::select! {
                msg = raw_msg_rx.recv() => {
                    let Some(raw_msg) = msg else { break "WS read channel closed"; };
                    let response = parse_response(&raw_msg);
                    match response {
                        DerivResponse::Authorize { balance, currency, login_id } => {
                            info!(login_id = %login_id, balance, currency = %currency, "Authorized executor session");
                            engine.notify_live_balance(balance);
                            ws_client.set_state(ConnectionState::Authorized).await;
                            let _ = router.send_fire(protocol::requests::ticks(&cfg.symbol)).await;
                            let _ = router.send_fire(protocol::requests::balance_subscribe()).await;
                            ws_client.set_state(ConnectionState::Running).await;
                        }
                        DerivResponse::Tick(tick) => {
                            let now = UnixMs::now();
                            metrics.inc_ticks();
                            if let Some(ref db) = db {
                                let _ = db.insert_tick(now.0, tick.price, &cfg.symbol);
                            }
                            tick_buf.push(tick.clone());

                            let Some(decision) = engine.step(&tick, false) else { continue; };
                            emit_decision_telemetry(&telemetry_tx, &decision, ledger.realized_pnl);

                            if decision.decision == "signal" {
                                if trader.is_idle() {
                                    if let Some(contract_type) = parse_contract_direction(decision.contract_direction.as_deref()) {
                                        match trader.enter_trade(&SymbolId(cfg.symbol.clone()), contract_type, decision.proposed_stake, cfg.contract_duration, &cfg.duration_unit).await {
                                            Ok(()) => {
                                                if trader.current_state() == TradeState::Open {
                                                    metrics.inc_trades();
                                                    ledger.on_buy(contract_type, decision.proposed_stake);
                                                    let opened_at_ms = UnixMs::now().0;
                                                    let contract_id = trader.active_trade.as_ref().and_then(|t| t.contract_id.clone());
                                                    engine.notify_live_trade_opened(
                                                        contract_id,
                                                        contract_type.to_string(),
                                                        decision.proposed_stake,
                                                        opened_at_ms,
                                                    );
                                                    if let Some(ref db) = db {
                                                        pending_execution = persist_execution_open(
                                                            db,
                                                            &run_id,
                                                            &decision,
                                                            contract_type,
                                                            &trader,
                                                            if cfg.dry_run { ExecutionMode::DryRun } else { ExecutionMode::Live },
                                                        ).ok();
                                                    }
                                                    if cfg.dry_run {
                                                        settle_trade(
                                                            &mut engine,
                                                            &mut pending_execution,
                                                            &db,
                                                            &trader,
                                                            0.0,
                                                            CloseReason::DryRunExpiry,
                                                            &metrics,
                                                            &mut ledger,
                                                        );
                                                        trader.reset();
                                                    }
                                                    info!(regime = %decision.regime, q_final = decision.q_final, edge = decision.edge, benchmark_signal = %decision.benchmark_signal, contract = %decision.contract_direction.as_deref().unwrap_or("NONE"), "DecisionEngine trade entered");
                                                } else {
                                                    info!(state = ?trader.current_state(), "Trade request completed without an open live contract; skipping live-open synchronization");
                                                    trader.reset();
                                                }
                                            }
                                            Err(e) => {
                                                error!(error = %e, edge = decision.edge, proposed_stake = decision.proposed_stake, "Trade execution failed");
                                                if let Some(ref db) = db {
                                                    let _ = mark_live_execution_failed(db, &run_id, &decision);
                                                }
                                                trader.reset();
                                            }
                                        }
                                    }
                                } else if let Some(ref db) = db {
                                    let mut rejected = decision.clone();
                                    rejected.rejection_reason = Some("position_already_open".into());
                                    rejected.executed_stake = 0.0;
                                    if let Err(err) = persist_live_rejected_intent(db, &run_id, &rejected) {
                                        warn!(error = ?err, "Failed to persist rejected live intent");
                                    }
                                }
                            } else if let Some(ref db) = db {
                                if let Err(err) = persist_live_decision(db, &run_id, &decision) {
                                    warn!(error = ?err, "Failed to persist decision telemetry");
                                }
                            }

                            if now.0 - last_metrics_log > 10_000 {
                                last_metrics_log = now.0;
                                info!(symbol = %cfg.symbol, regime = %decision.regime, q_model = format!("{:.4}", decision.q_model), q_final = format!("{:.4}", decision.q_final), edge = format!("{:.4}", decision.edge), benchmark_signal = %decision.benchmark_signal, pnl = format!("{:.2}", ledger.realized_pnl), win_rate = format!("{:.1}%", ledger.win_rate() * 100.0), metrics = %metrics.summary(), "Executor status");
                            }
                        }
                        DerivResponse::ProposalOpenContract { contract_id, is_sold, is_expired, is_valid_to_sell, profit, buy_price, .. } => {
                            match trader.handle_poc_update(&contract_id, is_sold, is_expired, is_valid_to_sell, profit, buy_price) {
                                PocAction::SellNow { contract_id: cid, profit: p, buy_price: bp } => {
                                    let pnl = match trader.sell_contract(&cid, bp + p).await {
                                        Ok(realized) => realized,
                                        Err(e) => {
                                            error!(error = %e, "Sell failed, waiting for natural settlement");
                                            continue;
                                        }
                                    };
                                    settle_trade(
                                        &mut engine,
                                        &mut pending_execution,
                                        &db,
                                        &trader,
                                        pnl,
                                        CloseReason::EarlyExit,
                                        &metrics,
                                        &mut ledger,
                                    );
                                    trader.reset();
                                    info!(contract_id = %cid, pnl, "Contract sold early after early-exit loss threshold");
                                }
                                PocAction::Settled(pnl) => {
                                    settle_trade(
                                        &mut engine,
                                        &mut pending_execution,
                                        &db,
                                        &trader,
                                        pnl,
                                        CloseReason::NaturalExpiry,
                                        &metrics,
                                        &mut ledger,
                                    );
                                    trader.reset();
                                    info!(contract_id = %contract_id, profit = pnl, "Contract settled naturally");
                                }
                                PocAction::Hold => {}
                            }
                        }
                        DerivResponse::Balance { balance, currency } => {
                            engine.notify_live_balance(balance);
                            debug!(balance, currency = %currency, "Balance update");
                        }
                        DerivResponse::Time { server_time } => {
                            let now_epoch = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
                            let latency_ms = ((now_epoch - server_time) * 1000).unsigned_abs();
                            metrics.set_ping_latency(latency_ms);
                        }
                        DerivResponse::Error { code, message, .. } => warn!(code = %code, message = %message, "API error"),
                        _ => {}
                    }
                }
                _ = tokio::time::sleep(Duration::from_secs(1)) => {
                    if let Some(ref trade) = trader.active_trade {
                        if trade.state == TradeState::Open {
                            if let Some(ref cid) = trade.contract_id {
                                let _ = router.send_fire(protocol::requests::proposal_open_contract_poll(cid)).await;
                            }
                        }
                    }
                }
                _ = tokio::time::sleep(Duration::from_secs(30)) => {
                    if let Ok(req_id) = router.next_req_id().await.try_into() {
                        if let Err(e) = WsClient::send_ping(&write_tx, req_id).await {
                            warn!(error = %e, "Ping failed, reconnecting");
                            break "Ping send failed";
                        }
                    }
                }
            }
        };

        warn!(
            reason = disconnect_reason,
            "Executor connection lost, reconnecting in 5s"
        );
        if !trader.is_idle() {
            warn!("Open trade aborted due to disconnect");
            if let Some(ref db) = db {
                if let Some(pending) = pending_execution.take() {
                    let _ = db.update_trade_intent_status(
                        pending.intent_id,
                        "executed",
                        pending.stake,
                        Some("disconnect_aborted"),
                    );
                    let _ = db.update_executed_trade_lifecycle(
                        pending.executed_trade_id,
                        UnixMs::now().0,
                        None,
                        None,
                        Some("disconnect_aborted"),
                        "aborted",
                    );
                }
            }
            engine.notify_live_trade_aborted(UnixMs::now().0, None);
            trader.abort("Disconnect");
            trader.reset();
        }
        router.clear_pending().await;
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

fn emit_decision_telemetry(
    telemetry_tx: &broadcast::Sender<String>,
    decision: &DecisionContext,
    pnl: f64,
) {
    let msg = serde_json::json!({
        "type": "decision",
        "price": decision.price,
        "time": decision.ts_ms / 1000,
        "regime": decision.regime,
        "prior": decision.prior,
        "q_model": decision.q_model,
        "q_final": decision.q_final,
        "edge": decision.edge,
        "decision": decision.decision,
        "benchmark_signal": decision.benchmark_signal,
        "pnl": pnl
    });
    let _ = telemetry_tx.send(msg.to_string());
}

fn persist_live_decision(
    db: &TelemetryDb,
    run_id: &str,
    decision: &DecisionContext,
) -> rusqlite::Result<i64> {
    let decision_id = db.insert_run_decision(&RunDecisionRecord {
        run_id,
        timestamp_ms: decision.ts_ms,
        symbol: &decision.symbol,
        price: decision.price,
        regime: &decision.regime,
        prior_mode: &decision.prior_mode,
        strategy_mode: &decision.strategy_mode,
        model_metadata: &decision.model_metadata,
        contract_direction: decision.contract_direction.as_deref(),
        benchmark_signal: &decision.benchmark_signal,
        decision: &decision.decision,
        rejection_reason: decision.rejection_reason.as_deref(),
        edge: decision.edge,
        q_prior: decision.prior,
        q_model: decision.q_model,
        q_final: decision.q_final,
        q_low: decision.q_low,
        q_high: decision.q_high,
        confidence: decision.confidence,
        time_left_sec: decision.time_left_sec,
        proposed_stake: decision.proposed_stake,
        executed_stake: 0.0,
        feature_summary: &decision.feature_summary,
    })?;
    if let Some(direction) = decision.contract_direction.as_deref() {
        let intent_status = if decision.decision == "signal" {
            "submitted"
        } else {
            "rejected"
        };
        let _ = db.insert_trade_intent(&TradeIntentRecord {
            run_id,
            decision_id,
            timestamp_ms: decision.ts_ms,
            contract_direction: direction,
            proposed_stake: decision.proposed_stake,
            executed_stake: 0.0,
            execution_enabled: true,
            intent_status,
            rejection_reason: decision.rejection_reason.as_deref(),
        })?;
    }
    Ok(decision_id)
}

fn mark_live_execution_failed(
    db: &TelemetryDb,
    run_id: &str,
    decision: &DecisionContext,
) -> rusqlite::Result<()> {
    let decision_id = db.insert_run_decision(&RunDecisionRecord {
        run_id,
        timestamp_ms: decision.ts_ms,
        symbol: &decision.symbol,
        price: decision.price,
        regime: &decision.regime,
        prior_mode: &decision.prior_mode,
        strategy_mode: &decision.strategy_mode,
        model_metadata: &decision.model_metadata,
        contract_direction: decision.contract_direction.as_deref(),
        benchmark_signal: &decision.benchmark_signal,
        decision: "signal",
        rejection_reason: Some("execution_failed"),
        edge: decision.edge,
        q_prior: decision.prior,
        q_model: decision.q_model,
        q_final: decision.q_final,
        q_low: decision.q_low,
        q_high: decision.q_high,
        confidence: decision.confidence,
        time_left_sec: decision.time_left_sec,
        proposed_stake: decision.proposed_stake,
        executed_stake: 0.0,
        feature_summary: &decision.feature_summary,
    })?;
    if let Some(direction) = decision.contract_direction.as_deref() {
        db.insert_trade_intent(&TradeIntentRecord {
            run_id,
            decision_id,
            timestamp_ms: decision.ts_ms,
            contract_direction: direction,
            proposed_stake: decision.proposed_stake,
            executed_stake: 0.0,
            execution_enabled: true,
            intent_status: "execution_failed",
            rejection_reason: Some("execution_failed"),
        })?;
    }
    Ok(())
}

fn persist_live_rejected_intent(
    db: &TelemetryDb,
    run_id: &str,
    decision: &DecisionContext,
) -> rusqlite::Result<()> {
    let decision_id = db.insert_run_decision(&RunDecisionRecord {
        run_id,
        timestamp_ms: decision.ts_ms,
        symbol: &decision.symbol,
        price: decision.price,
        regime: &decision.regime,
        prior_mode: &decision.prior_mode,
        strategy_mode: &decision.strategy_mode,
        model_metadata: &decision.model_metadata,
        contract_direction: decision.contract_direction.as_deref(),
        benchmark_signal: &decision.benchmark_signal,
        decision: "signal",
        rejection_reason: decision.rejection_reason.as_deref(),
        edge: decision.edge,
        q_prior: decision.prior,
        q_model: decision.q_model,
        q_final: decision.q_final,
        q_low: decision.q_low,
        q_high: decision.q_high,
        confidence: decision.confidence,
        time_left_sec: decision.time_left_sec,
        proposed_stake: decision.proposed_stake,
        executed_stake: 0.0,
        feature_summary: &decision.feature_summary,
    })?;
    if let Some(direction) = decision.contract_direction.as_deref() {
        db.insert_trade_intent(&TradeIntentRecord {
            run_id,
            decision_id,
            timestamp_ms: decision.ts_ms,
            contract_direction: direction,
            proposed_stake: decision.proposed_stake,
            executed_stake: 0.0,
            execution_enabled: true,
            intent_status: "rejected",
            rejection_reason: decision.rejection_reason.as_deref(),
        })?;
    }
    Ok(())
}

fn persist_execution_open(
    db: &TelemetryDb,
    run_id: &str,
    decision: &DecisionContext,
    contract_type: ContractType,
    trader: &Trader,
    mode: ExecutionMode,
) -> rusqlite::Result<PendingExecutionTelemetry> {
    let decision_id = db.insert_run_decision(&RunDecisionRecord {
        run_id,
        timestamp_ms: decision.ts_ms,
        symbol: &decision.symbol,
        price: decision.price,
        regime: &decision.regime,
        prior_mode: &decision.prior_mode,
        strategy_mode: &decision.strategy_mode,
        model_metadata: &decision.model_metadata,
        contract_direction: decision.contract_direction.as_deref(),
        benchmark_signal: &decision.benchmark_signal,
        decision: "signal",
        rejection_reason: None,
        edge: decision.edge,
        q_prior: decision.prior,
        q_model: decision.q_model,
        q_final: decision.q_final,
        q_low: decision.q_low,
        q_high: decision.q_high,
        confidence: decision.confidence,
        time_left_sec: decision.time_left_sec,
        proposed_stake: decision.proposed_stake,
        executed_stake: 0.0,
        feature_summary: &decision.feature_summary,
    })?;
    let intent_id = db.insert_trade_intent(&TradeIntentRecord {
        run_id,
        decision_id,
        timestamp_ms: decision.ts_ms,
        contract_direction: decision.contract_direction.as_deref().unwrap_or("UNKNOWN"),
        proposed_stake: decision.proposed_stake,
        executed_stake: 0.0,
        execution_enabled: true,
        intent_status: if mode == ExecutionMode::DryRun {
            "dry_run_executed"
        } else {
            "submitted"
        },
        rejection_reason: None,
    })?;
    let executed_trade_id = db.insert_executed_trade(&ExecutedTradeRecord {
        run_id,
        trade_intent_id: intent_id,
        timestamp_ms: decision.ts_ms,
        contract_id: trader
            .active_trade
            .as_ref()
            .and_then(|t| t.contract_id.as_deref()),
        contract_direction: decision.contract_direction.as_deref().unwrap_or("UNKNOWN"),
        stake: decision.proposed_stake,
        payout: trader.active_trade.as_ref().and_then(|t| t.payout),
        pnl: None,
        exit_reason: None,
        status: if mode == ExecutionMode::DryRun {
            "dry_run_open"
        } else {
            "open"
        },
    })?;
    Ok(PendingExecutionTelemetry {
        intent_id,
        executed_trade_id,
        stake: decision.proposed_stake,
        contract_type,
        mode,
    })
}

fn settle_trade(
    engine: &mut DecisionEngine,
    pending_execution: &mut Option<PendingExecutionTelemetry>,
    db: &Option<Arc<TelemetryDb>>,
    trader: &Trader,
    pnl: f64,
    close_reason: CloseReason,
    metrics: &Metrics,
    ledger: &mut Ledger,
) {
    if pnl > 0.0 {
        metrics.inc_wins();
    } else {
        metrics.inc_losses();
    }
    if let Some(pending) = pending_execution.take() {
        let (trade_status, intent_status, exit_reason, payout) = match pending.mode {
            ExecutionMode::Live => {
                let (trade_status, exit_reason) = match close_reason {
                    CloseReason::NaturalExpiry => ("settled", Some("contract_expired")),
                    CloseReason::EarlyExit => ("closed_early", Some("stop_loss_exit")),
                    CloseReason::DryRunExpiry => ("settled", Some("contract_expired")),
                };
                (
                    trade_status,
                    "executed",
                    exit_reason,
                    trader
                        .active_trade
                        .as_ref()
                        .and_then(|t| t.payout)
                        .map(|p| {
                            if trade_status == "settled" && pnl > 0.0 {
                                p
                            } else {
                                0.0
                            }
                        }),
                )
            }
            ExecutionMode::DryRun => (
                "dry_run_settled",
                "dry_run_executed",
                Some("dry_run_simulated_expiry"),
                Some(pending.stake + pnl),
            ),
        };
        ledger.on_settle(pending.contract_type, payout.unwrap_or(0.0), pending.stake);
        engine.notify_live_trade_closed(pnl, UnixMs::now().0, Some(ledger.cash));
        if let Some(db) = db {
            let _ = db.update_trade_intent_status(
                pending.intent_id,
                intent_status,
                pending.stake,
                None,
            );
            let _ = db.update_executed_trade_lifecycle(
                pending.executed_trade_id,
                UnixMs::now().0,
                payout,
                Some(pnl),
                exit_reason,
                trade_status,
            );
        }
    }
}

fn parse_contract_direction(direction: Option<&str>) -> Option<ContractType> {
    match direction {
        Some("CALL") => Some(ContractType::Call),
        Some("PUT") => Some(ContractType::Put),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::decision_engine::DecisionContext;
    use crate::observability::db::RunMetadata;

    fn sample_decision() -> DecisionContext {
        DecisionContext {
            ts_ms: 10,
            symbol: "R_100".into(),
            price: 100.0,
            regime: "calm".into(),
            prior: 0.51,
            q_model: 0.54,
            q_final: 0.53,
            q_low: 0.51,
            q_high: 0.55,
            confidence: 1.0,
            edge: 0.03,
            time_left_sec: 120.0,
            contract_direction: Some("CALL".into()),
            benchmark_signal: "CALL".into(),
            decision: "signal".into(),
            rejection_reason: None,
            proposed_stake: 1.2,
            executed_stake: 0.0,
            execution_enabled: true,
            prior_mode: "process-v1".into(),
            strategy_mode: "process".into(),
            model_metadata: "quant-only".into(),
            feature_summary: "{}".into(),
            simulated_trade_closed: None,
        }
    }

    #[test]
    fn live_execution_failure_persists_single_failed_lifecycle() {
        let db = TelemetryDb::new(":memory:").unwrap();
        db.upsert_run_metadata(&RunMetadata {
            run_id: "live-fail".into(),
            binary_type: "executor".into(),
            model_version: "quant-only".into(),
            strategy_version: "process".into(),
            prior_version: "process-v1".into(),
            config_fingerprint: "fp".into(),
            started_at_ms: 1,
        })
        .unwrap();

        mark_live_execution_failed(&db, "live-fail", &sample_decision()).unwrap();

        assert_eq!(db.count_rows("decision_events", "live-fail").unwrap(), 1);
        assert_eq!(
            db.count_trade_intents_by_status("live-fail", "execution_failed")
                .unwrap(),
            1
        );
        assert_eq!(db.count_rows("executed_trades", "live-fail").unwrap(), 0);
    }

    #[test]
    fn settle_live_trade_clears_engine_open_state() {
        let mut engine = DecisionEngine::new_live(DecisionEngineConfig {
            symbol: "R_100".into(),
            contract_duration: 5,
            min_stake: 0.35,
            initial_balance: 100.0,
            max_open_positions: 1,
            max_daily_loss: 100.0,
            cooldown_after_loss_ms: 0,
            max_consecutive_losses: 10,
            model_path: None,
            allow_model_fallback: true,
            strategy_mode: "process".into(),
            prior_mode: "process-v1".into(),
        })
        .unwrap();
        engine.notify_live_trade_opened(Some("c_123".into()), "CALL", 1.2, 10);

        let router = Arc::new(Router::new(tokio::sync::mpsc::channel(1).0));
        let trader = Trader {
            router,
            active_trade: Some(crate::execution::trader::ActiveTrade {
                state: TradeState::Open,
                symbol: SymbolId("R_100".into()),
                contract_type: ContractType::Call,
                stake: 1.2,
                duration_sec: 5,
                duration_unit: "s".into(),
                proposal_id: None,
                contract_id: Some("c_123".into()),
                buy_price: Some(1.2),
                payout: Some(2.34),
                subscription_id: None,
                created_at: UnixMs(10),
            }),
            dry_run: false,
            stop_loss_pct: 0.5,
        };
        let metrics = Metrics::new();
        let mut ledger = Ledger::new(100.0);
        ledger.on_buy(ContractType::Call, 1.2);
        let mut pending = Some(PendingExecutionTelemetry {
            intent_id: 1,
            executed_trade_id: 1,
            stake: 1.2,
            contract_type: ContractType::Call,
            mode: ExecutionMode::Live,
        });

        settle_trade(
            &mut engine,
            &mut pending,
            &None,
            &trader,
            1.14,
            CloseReason::NaturalExpiry,
            &metrics,
            &mut ledger,
        );

        assert!(pending.is_none());
        assert!(!engine.live_position_is_open());
    }

    #[tokio::test]
    async fn dry_run_execution_persists_open_then_settled_lifecycle() {
        let db = Arc::new(TelemetryDb::new(":memory:").unwrap());
        db.upsert_run_metadata(&RunMetadata {
            run_id: "dry-run".into(),
            binary_type: "executor".into(),
            model_version: "quant-only".into(),
            strategy_version: "process".into(),
            prior_version: "process-v1".into(),
            config_fingerprint: "fp".into(),
            started_at_ms: 1,
        })
        .unwrap();

        let mut engine = DecisionEngine::new_live(DecisionEngineConfig {
            symbol: "R_100".into(),
            contract_duration: 5,
            min_stake: 0.35,
            initial_balance: 100.0,
            max_open_positions: 1,
            max_daily_loss: 100.0,
            cooldown_after_loss_ms: 0,
            max_consecutive_losses: 10,
            model_path: None,
            allow_model_fallback: true,
            strategy_mode: "process".into(),
            prior_mode: "process-v1".into(),
        })
        .unwrap();
        let router = Arc::new(Router::new(tokio::sync::mpsc::channel(1).0));
        let mut trader = Trader::new(router, true, 0.5);
        trader
            .enter_trade(&SymbolId("R_100".into()), ContractType::Call, 1.2, 5, "s")
            .await
            .unwrap();
        assert_eq!(trader.current_state(), TradeState::Open);

        let decision = sample_decision();
        engine.notify_live_trade_opened(
            trader
                .active_trade
                .as_ref()
                .and_then(|t| t.contract_id.clone()),
            "CALL",
            1.2,
            decision.ts_ms,
        );
        let persisted = persist_execution_open(
            &db,
            "dry-run",
            &decision,
            ContractType::Call,
            &trader,
            ExecutionMode::DryRun,
        )
        .unwrap();

        assert_eq!(db.count_rows("decision_events", "dry-run").unwrap(), 1);
        assert_eq!(
            db.count_trade_intents_by_status("dry-run", "dry_run_executed")
                .unwrap(),
            1
        );
        assert!(engine.live_position_is_open());

        let metrics = Metrics::new();
        let mut ledger = Ledger::new(100.0);
        ledger.on_buy(ContractType::Call, 1.2);
        let mut pending = Some(persisted);
        settle_trade(
            &mut engine,
            &mut pending,
            &Some(Arc::clone(&db)),
            &trader,
            0.0,
            CloseReason::DryRunExpiry,
            &metrics,
            &mut ledger,
        );

        assert!(pending.is_none());
        assert!(!engine.live_position_is_open());
        assert_eq!(
            db.count_executed_trades_by_status("dry-run", "dry_run_settled")
                .unwrap(),
            1
        );
        let dry_run_contract_ids = db.executed_trade_contract_ids("dry-run").unwrap();
        assert!(dry_run_contract_ids
            .iter()
            .all(|id| id.starts_with("dry_run:")));
    }

    #[tokio::test]
    async fn multiple_dry_run_trades_do_not_leave_stale_open_state() {
        let db = Arc::new(TelemetryDb::new(":memory:").unwrap());
        db.upsert_run_metadata(&RunMetadata {
            run_id: "dry-run-multi".into(),
            binary_type: "executor".into(),
            model_version: "quant-only".into(),
            strategy_version: "process".into(),
            prior_version: "process-v1".into(),
            config_fingerprint: "fp".into(),
            started_at_ms: 1,
        })
        .unwrap();
        let mut engine = DecisionEngine::new_live(DecisionEngineConfig {
            symbol: "R_100".into(),
            contract_duration: 5,
            min_stake: 0.35,
            initial_balance: 100.0,
            max_open_positions: 1,
            max_daily_loss: 100.0,
            cooldown_after_loss_ms: 0,
            max_consecutive_losses: 10,
            model_path: None,
            allow_model_fallback: true,
            strategy_mode: "process".into(),
            prior_mode: "process-v1".into(),
        })
        .unwrap();
        let metrics = Metrics::new();
        let mut ledger = Ledger::new(100.0);

        for ts in [10_i64, 20_i64] {
            let router = Arc::new(Router::new(tokio::sync::mpsc::channel(1).0));
            let mut trader = Trader::new(router, true, 0.5);
            trader
                .enter_trade(&SymbolId("R_100".into()), ContractType::Call, 1.2, 5, "s")
                .await
                .unwrap();
            let mut decision = sample_decision();
            decision.ts_ms = ts;
            engine.notify_live_trade_opened(
                trader
                    .active_trade
                    .as_ref()
                    .and_then(|t| t.contract_id.clone()),
                "CALL",
                1.2,
                ts,
            );
            let pending = persist_execution_open(
                &db,
                "dry-run-multi",
                &decision,
                ContractType::Call,
                &trader,
                ExecutionMode::DryRun,
            )
            .unwrap();
            ledger.on_buy(ContractType::Call, 1.2);
            let mut pending = Some(pending);
            settle_trade(
                &mut engine,
                &mut pending,
                &Some(Arc::clone(&db)),
                &trader,
                0.0,
                CloseReason::DryRunExpiry,
                &metrics,
                &mut ledger,
            );
            assert!(!engine.live_position_is_open());
        }

        assert_eq!(
            db.count_executed_trades_by_status("dry-run-multi", "dry_run_settled")
                .unwrap(),
            2
        );
    }

    #[test]
    fn rejected_live_signal_persists_without_trade_row() {
        let db = TelemetryDb::new(":memory:").unwrap();
        db.upsert_run_metadata(&RunMetadata {
            run_id: "live-rejected".into(),
            binary_type: "executor".into(),
            model_version: "quant-only".into(),
            strategy_version: "process".into(),
            prior_version: "process-v1".into(),
            config_fingerprint: "fp".into(),
            started_at_ms: 1,
        })
        .unwrap();

        let mut decision = sample_decision();
        decision.rejection_reason = Some("position_already_open".into());
        persist_live_rejected_intent(&db, "live-rejected", &decision).unwrap();

        assert_eq!(
            db.count_rows("decision_events", "live-rejected").unwrap(),
            1
        );
        assert_eq!(
            db.count_trade_intents_by_status("live-rejected", "rejected")
                .unwrap(),
            1
        );
        assert_eq!(
            db.count_rows("executed_trades", "live-rejected").unwrap(),
            0
        );
    }
}
