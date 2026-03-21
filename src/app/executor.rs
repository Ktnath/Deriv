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

#[derive(Debug, Clone)]
struct PendingExecutionTelemetry {
    intent_id: i64,
    executed_trade_id: i64,
    stake: f64,
    contract_type: ContractType,
}

pub async fn run_executor() -> anyhow::Result<()> {
    events::init_logging();
    let cfg = ExecutorConfig::from_env();
    info!(account_type = %cfg.account_type, base_stake = cfg.stake, telemetry_db = %cfg.telemetry_db_path, telemetry_bind = %cfg.telemetry_server_bind, "Executor configuration loaded");
    run_live_executor(cfg).await
}

pub async fn run_live_executor(cfg: ExecutorConfig) -> anyhow::Result<()> {
    let mut engine = DecisionEngine::new(DecisionEngineConfig {
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

                            if let Some(ref db) = db {
                                if let Err(err) = persist_live_decision(db, &run_id, &decision, false) {
                                    warn!(error = ?err, "Failed to persist decision telemetry");
                                }
                            }

                            if decision.decision == "signal" {
                                if trader.is_idle() {
                                    if let Some(contract_type) = parse_contract_direction(decision.contract_direction.as_deref()) {
                                        match trader.enter_trade(&SymbolId(cfg.symbol.clone()), contract_type, decision.proposed_stake, cfg.contract_duration, &cfg.duration_unit).await {
                                            Ok(()) => {
                                                metrics.inc_trades();
                                                ledger.on_buy(contract_type, decision.proposed_stake);
                                                if let Some(ref db) = db {
                                                    pending_execution = persist_live_execution_open(
                                                        db,
                                                        &run_id,
                                                        &decision,
                                                        contract_type,
                                                        &trader,
                                                    ).ok();
                                                }
                                                info!(regime = %decision.regime, q_final = decision.q_final, edge = decision.edge, benchmark_signal = %decision.benchmark_signal, contract = %decision.contract_direction.as_deref().unwrap_or("NONE"), "DecisionEngine trade entered");
                                            }
                                            Err(e) => {
                                                error!(error = %e, edge = decision.edge, proposed_stake = decision.proposed_stake, "Trade execution failed");
                                                if let Some(ref db) = db {
                                                    let _ = persist_live_decision(db, &run_id, &decision, true);
                                                }
                                                trader.reset();
                                            }
                                        }
                                    }
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
                                    settle_live_trade(&mut pending_execution, &db, &trader, pnl, "closed_early", Some("stop_loss_exit"), &metrics, &mut ledger);
                                    trader.reset();
                                    info!(contract_id = %cid, pnl, "Contract sold early after early-exit loss threshold");
                                }
                                PocAction::Settled(pnl) => {
                                    settle_live_trade(&mut pending_execution, &db, &trader, pnl, "settled", Some("contract_expired"), &metrics, &mut ledger);
                                    trader.reset();
                                    info!(contract_id = %contract_id, profit = pnl, "Contract settled naturally");
                                }
                                PocAction::Hold => {}
                            }
                        }
                        DerivResponse::Balance { balance, currency } => {
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
                        "submitted",
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
    mark_execution_failed: bool,
) -> rusqlite::Result<i64> {
    let decision_label = if mark_execution_failed && decision.decision == "signal" {
        "hold"
    } else {
        &decision.decision
    };
    let rejection_reason = if mark_execution_failed {
        Some("execution_failed")
    } else {
        decision.rejection_reason.as_deref()
    };
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
        decision: decision_label,
        rejection_reason,
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
            rejection_reason,
        })?;
    }
    Ok(decision_id)
}

fn persist_live_execution_open(
    db: &TelemetryDb,
    run_id: &str,
    decision: &DecisionContext,
    contract_type: ContractType,
    trader: &Trader,
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
        decision: "enter",
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
        executed_stake: decision.proposed_stake,
        feature_summary: &decision.feature_summary,
    })?;
    let intent_id = db.insert_trade_intent(&TradeIntentRecord {
        run_id,
        decision_id,
        timestamp_ms: decision.ts_ms,
        contract_direction: decision.contract_direction.as_deref().unwrap_or("UNKNOWN"),
        proposed_stake: decision.proposed_stake,
        executed_stake: decision.proposed_stake,
        execution_enabled: true,
        intent_status: "executed",
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
        status: "open",
    })?;
    Ok(PendingExecutionTelemetry {
        intent_id,
        executed_trade_id,
        stake: decision.proposed_stake,
        contract_type,
    })
}

fn settle_live_trade(
    pending_execution: &mut Option<PendingExecutionTelemetry>,
    db: &Option<Arc<TelemetryDb>>,
    trader: &Trader,
    pnl: f64,
    status: &str,
    exit_reason: Option<&str>,
    metrics: &Metrics,
    ledger: &mut Ledger,
) {
    if pnl > 0.0 {
        metrics.inc_wins();
    } else {
        metrics.inc_losses();
    }
    if let Some(pending) = pending_execution.take() {
        let payout = trader
            .active_trade
            .as_ref()
            .and_then(|t| t.payout)
            .map(|p| if pnl > 0.0 { p } else { 0.0 });
        ledger.on_settle(pending.contract_type, payout.unwrap_or(0.0), pending.stake);
        if let Some(db) = db {
            let _ =
                db.update_trade_intent_status(pending.intent_id, "executed", pending.stake, None);
            let _ = db.update_executed_trade_lifecycle(
                pending.executed_trade_id,
                UnixMs::now().0,
                payout,
                Some(pnl),
                exit_reason,
                status,
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
