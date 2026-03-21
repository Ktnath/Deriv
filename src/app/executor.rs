use crate::{
    config::ExecutorConfig,
    execution::trader::{PocAction, Trader},
    market_data::ticks::TickBuffer,
    observability::{
        db::{AlphaSignalRecord, DecisionSnapshotRecord, TelemetryDb},
        events,
        metrics::Metrics,
    },
    process::{FeatureExtractor, LiveDecisionInput, PriorEstimator, Regime},
    protocol::{
        self,
        responses::{parse_response, DerivResponse},
    },
    risk::{
        alpha::{AlphaEngine, BlendConfig, BlendInputs, ModelMode},
        kelly::KellyRisk,
        ledger::Ledger,
        limits::RiskGate,
        settlement::Settlement,
    },
    server,
    strategy::{Signal, StrategyEngine},
    transport::{
        router::Router,
        ws_client::{self, WsClient},
    },
    types::*,
};
use std::{sync::Arc, time::Duration};
use tokio::sync::{broadcast, mpsc};
use tracing::{debug, error, info, warn};

const MIN_PROCESS_POINTS: usize = 48;

pub async fn run_executor() -> anyhow::Result<()> {
    events::init_logging();
    let cfg = ExecutorConfig::from_env();
    info!(account_type = %cfg.account_type, base_stake = cfg.stake, telemetry_db = %cfg.telemetry_db_path, telemetry_bind = %cfg.telemetry_server_bind, "Executor configuration loaded");
    run_live_executor(cfg).await
}

pub async fn run_live_executor(cfg: ExecutorConfig) -> anyhow::Result<()> {
    let mut prior_estimator = PriorEstimator::new();
    let mut feature_extractor = FeatureExtractor::new(512);
    let mut alpha = AlphaEngine::new(0.55, cfg.model_path.as_deref(), cfg.allow_model_fallback)?;
    let blend_config = BlendConfig::default();
    let model_mode = match alpha.model_mode() {
        ModelMode::Onnx { path } => format!("onnx:{path}"),
        ModelMode::QuantOnly { reason } => format!("quant-only:{reason}"),
    };
    info!(symbol = %cfg.symbol, primary_pipeline = "process_prior_alpha", strategy = ?cfg.strategy, duration = cfg.contract_duration, duration_unit = %cfg.duration_unit, dry_run = cfg.dry_run, model_mode = %model_mode, model_path = ?cfg.model_path, allow_model_fallback = cfg.allow_model_fallback, max_open_positions = cfg.max_open_positions, max_daily_loss = cfg.max_daily_loss, cooldown_ms = cfg.cooldown_after_loss_ms, max_consecutive_losses = cfg.max_consecutive_losses, min_stake = cfg.min_stake, stake_sizing_mode = "kelly_live", early_exit_loss_threshold_pct = cfg.stop_loss_pct * 100.0, legacy_strategy_mode = ?cfg.strategy, "Startup configuration loaded");

    let (telemetry_tx, _telemetry_rx) = broadcast::channel::<String>(100);
    tokio::spawn(server::start_server(
        telemetry_tx.clone(),
        cfg.telemetry_server_bind.clone(),
    ));

    let mut tick_buf = TickBuffer::new(1000);
    let mut settlement = Settlement::new();
    let kelly = KellyRisk {
        max_fraction: 0.15,
        max_loss: cfg.initial_balance * 0.5,
        min_stake: cfg.min_stake,
    };
    let mut legacy_strategy = StrategyEngine::new(cfg.strategy);
    let mut ledger = Ledger::new(cfg.initial_balance);
    let mut risk_gate = RiskGate::new(
        cfg.max_open_positions,
        cfg.max_daily_loss,
        cfg.cooldown_after_loss_ms,
        cfg.max_consecutive_losses,
        cfg.initial_balance,
    );
    let metrics = Arc::new(Metrics::new());

    let db = match TelemetryDb::new(&cfg.telemetry_db_path) {
        Ok(db) => Some(Arc::new(db)),
        Err(e) => {
            error!(error = ?e, "Failed to initialize TelemetryDb");
            None
        }
    };

    let mut contract_start: Option<i64> = None;
    let mut last_trade_time = 0i64;
    let mut last_metrics_log = 0i64;

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
                            settlement.handle_tick(&tick);
                            legacy_strategy.on_tick(tick.price);
                            feature_extractor.push_price(tick.price);
                            alpha.update_spot_return(tick.price, UnixMs(tick.epoch * 1000));
                            if contract_start.is_none() {
                                contract_start = Some(now.0);
                                settlement.capture_s0();
                            }
                            let window_elapsed = contract_start.map(|s| (now.0 - s) as f64 / 1000.0).unwrap_or(0.0);
                            let time_left = (cfg.contract_duration as f64) - window_elapsed;
                            if time_left <= 0.0 {
                                contract_start = Some(now.0);
                                settlement.reset_s0();
                                settlement.capture_s0();
                                if trader.is_idle() { trader.reset(); }
                                continue;
                            }
                            let est = settlement.estimate();
                            let Some(s0) = est.s0 else { continue; };
                            let Some(s_hat) = est.s_hat_t else { continue; };
                            if !feature_extractor.is_ready(MIN_PROCESS_POINTS) {
                                continue;
                            }
                            let Some(features) = feature_extractor.extract() else { continue; };
                            let process_snapshot = prior_estimator.update(&features, now);
                            let q_model = alpha.calculate_q_model(s0, s_hat, time_left);
                            let confidence_multiplier = 0.85 + process_snapshot.features.confidence_score() * 0.5;
                            let alpha_out = alpha.finalize_probability(BlendInputs {
                                model_probability: q_model,
                                process_prior: Some(process_snapshot.prior),
                                confidence_multiplier: Some(confidence_multiplier),
                                config: blend_config,
                            });
                            let risk_dec = kelly.size(alpha_out.q_low, Price(0.50), 0.5, risk_gate.balance);
                            let live_input = build_live_decision(&process_snapshot.regime, &process_snapshot.features, process_snapshot.prior, q_model, alpha_out.q_final, confidence_multiplier, time_left);
                            let benchmark_signal = legacy_strategy.generate_signal(&alpha_out, &risk_dec, time_left);
                            let signal = live_input.contract.map(Signal::Enter).unwrap_or(Signal::Hold);

                            if let Some(ref db) = db {
                                let _ = db.insert_alpha_signal(AlphaSignalRecord {
                                    timestamp: now.0,
                                    q_model: Some(q_model.0),
                                    q_prior: Some(process_snapshot.prior.0),
                                    q_final: alpha_out.q_final.0,
                                    q_low: alpha_out.q_low.0,
                                    q_high: alpha_out.q_high.0,
                                    confidence: confidence_multiplier,
                                    time_left_sec: time_left,
                                    regime: process_snapshot.regime.as_str(),
                                });
                            }

                            let telemetry_msg = serde_json::json!({
                                "type": "decision",
                                "price": tick.price,
                                "time": tick.epoch,
                                "regime": process_snapshot.regime.as_str(),
                                "prior": process_snapshot.prior.0,
                                "q_model": q_model.0,
                                "q_final": alpha_out.q_final.0,
                                "edge": alpha_out.q_final.0 - 0.5,
                                "decision": format!("{:?}", signal),
                                "benchmark_signal": format!("{:?}", benchmark_signal),
                                "pnl": ledger.realized_pnl
                            });
                            let _ = telemetry_tx.send(telemetry_msg.to_string());

                            let mut rejection_reason = live_input.rejection_reason.clone();
                            let edge = live_input.edge;
                            let proposed_stake = risk_dec.max_size.0;
                            let mut decision = "hold";
                            if let Signal::Enter(ct) = signal {
                                if trader.is_idle() {
                                    let can = risk_gate.can_trade(now);
                                    if can.is_ok() && (now.0 - last_trade_time) > (cfg.contract_duration as i64 * 1000) {
                                        if proposed_stake < cfg.min_stake {
                                            rejection_reason = Some("below_min_stake".to_string());
                                            info!(regime = %process_snapshot.regime.as_str(), predicted_probability = alpha_out.q_final.0, prior_probability = process_snapshot.prior.0, model_probability = q_model.0, edge, proposed_stake, min_stake = cfg.min_stake, rejection = "below_min_stake", "Trade rejected by stake sizing");
                                        } else {
                                            match trader.enter_trade(&SymbolId(cfg.symbol.clone()), ct, proposed_stake, cfg.contract_duration, &cfg.duration_unit).await {
                                                Ok(()) => {
                                                    metrics.inc_trades();
                                                    risk_gate.on_trade_opened();
                                                    ledger.on_buy(ct, proposed_stake);
                                                    last_trade_time = now.0;
                                                    decision = "enter";
                                                    info!(regime = %process_snapshot.regime.as_str(), prior_probability = process_snapshot.prior.0, model_probability = q_model.0, final_probability = alpha_out.q_final.0, edge, contract = %ct, benchmark_signal = ?benchmark_signal, "Process-oriented trade entered");
                                                }
                                                Err(e) => {
                                                    rejection_reason = Some(format!("execution_failed:{e}"));
                                                    error!(error = %e, regime = %process_snapshot.regime.as_str(), predicted_probability = alpha_out.q_final.0, edge, proposed_stake, "Trade execution failed");
                                                    trader.reset();
                                                }
                                            }
                                        }
                                    } else if let Err(rej) = can {
                                        rejection_reason = Some(rej.to_string());
                                        debug!(rejection = %rej, regime = %process_snapshot.regime.as_str(), prior_probability = process_snapshot.prior.0, model_probability = q_model.0, final_probability = alpha_out.q_final.0, edge, proposed_stake, executed_stake = 0.0, "Risk gate blocked trade");
                                    } else {
                                        rejection_reason = Some("trade_cooldown_active".to_string());
                                    }
                                } else {
                                    rejection_reason = Some("trader_busy".to_string());
                                }
                            }

                            if let Some(ref db) = db {
                                let feature_summary = serde_json::json!({
                                    "sample_size": process_snapshot.features.sample_size,
                                    "last_return": process_snapshot.features.last_return,
                                    "short_drift": process_snapshot.features.short_drift,
                                    "long_drift": process_snapshot.features.long_drift,
                                    "drift_gap": process_snapshot.features.drift_gap,
                                    "run_length": process_snapshot.features.run_length,
                                    "sign_persistence": process_snapshot.features.sign_persistence,
                                    "variance_ratio": process_snapshot.features.variance_ratio,
                                    "return_clustering": process_snapshot.features.return_clustering,
                                    "direction_concentration": process_snapshot.features.direction_concentration,
                                    "shock_frequency": process_snapshot.features.shock_frequency,
                                    "time_since_large_move": process_snapshot.features.time_since_large_move,
                                    "move_zscore": process_snapshot.features.move_zscore,
                                    "transition_instability": process_snapshot.features.transition_instability,
                                    "benchmark_signal": format!("{:?}", benchmark_signal),
                                }).to_string();
                                let contract_direction = live_input.contract.map(|ct| ct.to_string());
                                let _ = db.insert_decision_snapshot(DecisionSnapshotRecord {
                                    timestamp: now.0,
                                    symbol: &cfg.symbol,
                                    regime: process_snapshot.regime.as_str(),
                                    contract_direction: contract_direction.as_deref(),
                                    decision,
                                    rejection_reason: rejection_reason.as_deref(),
                                    edge,
                                    q_prior: process_snapshot.prior.0,
                                    q_model: q_model.0,
                                    q_final: alpha_out.q_final.0,
                                    confidence: confidence_multiplier,
                                    feature_summary: &feature_summary,
                                });
                            }

                            if now.0 - last_metrics_log > 10_000 {
                                last_metrics_log = now.0;
                                info!(symbol = %cfg.symbol, regime = %process_snapshot.regime.as_str(), prior = format!("{:.4}", process_snapshot.prior.0), s0 = format!("{:.4}", s0), s_hat = format!("{:.4}", s_hat), q_model = format!("{:.4}", q_model.0), q_final = format!("{:.4}", alpha_out.q_final.0), edge = format!("{:.4}", alpha_out.q_final.0 - 0.5), benchmark_signal = ?benchmark_signal, pnl = format!("{:.2}", ledger.realized_pnl), win_rate = format!("{:.1}%", ledger.win_rate() * 100.0), metrics = %metrics.summary(), "Executor status");
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
                                    let now = UnixMs::now();
                                    risk_gate.on_trade_closed(pnl, now);
                                    let ct = trader.active_trade.as_ref().map(|t| t.contract_type).unwrap_or(ContractType::Call);
                                    let stake = trader.active_trade.as_ref().map(|t| t.stake).unwrap_or(0.0);
                                    if pnl > 0.0 {
                                        metrics.inc_wins();
                                        let payout = trader.active_trade.as_ref().and_then(|t| t.payout).unwrap_or(0.0);
                                        ledger.on_settle(ct, payout, stake);
                                    } else {
                                        metrics.inc_losses();
                                        ledger.on_settle(ct, 0.0, stake);
                                    }
                                    trader.reset();
                                    info!(contract_id = %cid, pnl, "Contract sold early after early-exit loss threshold");
                                }
                                PocAction::Settled(pnl) => {
                                    let now = UnixMs::now();
                                    risk_gate.on_trade_closed(pnl, now);
                                    let ct = trader.active_trade.as_ref().map(|t| t.contract_type).unwrap_or(ContractType::Call);
                                    let stake = trader.active_trade.as_ref().map(|t| t.stake).unwrap_or(0.0);
                                    if pnl > 0.0 {
                                        metrics.inc_wins();
                                        let payout = trader.active_trade.as_ref().and_then(|t| t.payout).unwrap_or(0.0);
                                        ledger.on_settle(ct, payout, stake);
                                    } else {
                                        metrics.inc_losses();
                                        ledger.on_settle(ct, 0.0, stake);
                                    }
                                    trader.reset();
                                    info!(contract_id = %contract_id, profit = pnl, "Contract settled naturally");
                                }
                                PocAction::Hold => {}
                            }
                        }
                        DerivResponse::Balance { balance, currency } => {
                            debug!(balance, currency = %currency, "Balance update");
                            risk_gate.update_balance(balance);
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
            trader.abort("Disconnect");
            trader.reset();
        }
        router.clear_pending().await;
        contract_start = None;
        settlement.reset_s0();
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

fn build_live_decision(
    regime: &Regime,
    features: &crate::process::ProcessFeatures,
    prior: Prob,
    model_probability: Prob,
    final_probability: Prob,
    confidence_multiplier: f64,
    time_left_sec: f64,
) -> LiveDecisionInput {
    let edge = final_probability.0 - 0.5;
    let edge_threshold = match regime {
        Regime::Calm => 0.035,
        Regime::Transitional => 0.05,
        Regime::Expansion => 0.065,
    };
    let mut rejection_reason = None;
    let contract = if time_left_sec < 10.0 {
        rejection_reason = Some("late_window".to_string());
        None
    } else if features.sample_size < 24 {
        rejection_reason = Some("insufficient_process_history".to_string());
        None
    } else if features.transition_instability > 0.78 {
        rejection_reason = Some("transition_instability".to_string());
        None
    } else if edge.abs() < edge_threshold {
        rejection_reason = Some("edge_below_threshold".to_string());
        None
    } else if matches!(regime, Regime::Calm) && features.variance_ratio > 1.2 {
        rejection_reason = Some("variance_out_of_regime".to_string());
        None
    } else if edge > 0.0 {
        Some(ContractType::Call)
    } else {
        Some(ContractType::Put)
    };

    LiveDecisionInput {
        regime: *regime,
        prior,
        model_probability,
        final_probability,
        confidence_multiplier,
        edge,
        contract,
        rejection_reason,
        features: features.clone(),
    }
}
