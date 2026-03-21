use crate::{
    config::ExecutorConfig,
    execution::trader::{PocAction, Trader},
    market_data::ticks::TickBuffer,
    observability::{db::TelemetryDb, events, metrics::Metrics},
    protocol::{
        self,
        responses::{parse_response, DerivResponse},
    },
    risk::{
        alpha::{AlphaEngine, ModelMode},
        kelly::KellyRisk,
        ledger::Ledger,
        limits::RiskGate,
        prior::{PriorEngine, PriorMode},
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

pub async fn run_executor() -> anyhow::Result<()> {
    events::init_logging();
    let cfg = ExecutorConfig::from_env();
    info!(account_type = %cfg.account_type, base_stake = cfg.stake, telemetry_db = %cfg.telemetry_db_path, telemetry_bind = %cfg.telemetry_server_bind, "Executor configuration loaded");
    run_live_executor(cfg).await
}

pub async fn run_live_executor(cfg: ExecutorConfig) -> anyhow::Result<()> {
    let prior_engine = PriorEngine::new(cfg.market_prior);
    let mut alpha = AlphaEngine::new(0.55, cfg.model_path.as_deref(), cfg.allow_model_fallback)?;
    let model_mode = match alpha.model_mode() {
        ModelMode::Onnx { path } => format!("onnx:{path}"),
        ModelMode::QuantOnly { reason } => format!("quant-only:{reason}"),
    };
    let prior_mode = match prior_engine.mode() {
        PriorMode::ModelOnly => "model_only".to_string(),
        PriorMode::Fixed(prob) => format!("fixed:{:.4}", prob.0),
    };
    info!(symbol = %cfg.symbol, strategy = ?cfg.strategy, duration = cfg.contract_duration, duration_unit = %cfg.duration_unit, dry_run = cfg.dry_run, model_mode = %model_mode, model_path = ?cfg.model_path, allow_model_fallback = cfg.allow_model_fallback, prior_mode = %prior_mode, max_open_positions = cfg.max_open_positions, max_daily_loss = cfg.max_daily_loss, cooldown_ms = cfg.cooldown_after_loss_ms, max_consecutive_losses = cfg.max_consecutive_losses, min_stake = cfg.min_stake, stake_sizing_mode = "kelly_live", early_exit_loss_threshold_pct = cfg.stop_loss_pct * 100.0, "Startup configuration loaded");

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
    let mut strat = StrategyEngine::new(cfg.strategy);
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
                            strat.on_tick(tick.price);
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
                            let q_model = alpha.calculate_q_model(s0, s_hat, time_left);
                            let alpha_out = alpha.finalize_probability(q_model, prior_engine.current_prior(), 0.55);
                            let risk_dec = kelly.size(alpha_out.q_low, Price(0.50), 0.5, risk_gate.balance);
                            let signal = strat.generate_signal(&alpha_out, &risk_dec, time_left);
                            if let Some(ref db) = db {
                                let _ = db.insert_alpha_signal(now.0, Some(q_model.0), prior_engine.current_prior().map(|p| p.0), alpha_out.q_low.0, time_left);
                            }
                            let telemetry_msg = serde_json::json!({
                                "type": "tick",
                                "price": tick.price,
                                "time": tick.epoch,
                                "signal": format!("{:?}", signal),
                                "edge": format!("{:.4}", alpha_out.q_final.0 - 0.5),
                                "pnl": format!("{:.2}", ledger.realized_pnl)
                            });
                            let _ = telemetry_tx.send(telemetry_msg.to_string());
                            if let Signal::Enter(ct) = signal {
                                if trader.is_idle() {
                                    let can = risk_gate.can_trade(now);
                                    if can.is_ok() && (now.0 - last_trade_time) > (cfg.contract_duration as i64 * 1000) {
                                        let edge = alpha_out.q_final.0 - 0.5;
                                        let proposed_stake = risk_dec.max_size.0;
                                        if proposed_stake < cfg.min_stake {
                                            info!(predicted_probability = alpha_out.q_final.0, edge, proposed_stake, min_stake = cfg.min_stake, rejection = "below_min_stake", "Trade rejected by stake sizing");
                                        } else {
                                            match trader.enter_trade(&SymbolId(cfg.symbol.clone()), ct, proposed_stake, cfg.contract_duration, &cfg.duration_unit).await {
                                                Ok(()) => {
                                                    metrics.inc_trades();
                                                    risk_gate.on_trade_opened();
                                                    ledger.on_buy(ct, proposed_stake);
                                                    last_trade_time = now.0;
                                                }
                                                Err(e) => {
                                                    error!(error = %e, predicted_probability = alpha_out.q_final.0, edge, proposed_stake, "Trade execution failed");
                                                    trader.reset();
                                                }
                                            }
                                        }
                                    } else if let Err(rej) = can {
                                        debug!(rejection = %rej, predicted_probability = alpha_out.q_final.0, edge = alpha_out.q_final.0 - 0.5, proposed_stake = risk_dec.max_size.0, executed_stake = 0.0, "Risk gate blocked trade");
                                    }
                                }
                            }
                            if now.0 - last_metrics_log > 10_000 {
                                last_metrics_log = now.0;
                                let regime = strat.evaluate_regime(time_left);
                                info!(symbol = %cfg.symbol, regime, s0 = format!("{:.4}", s0), s_hat = format!("{:.4}", s_hat), q_model = format!("{:.4}", q_model.0), edge = format!("{:.4}", alpha_out.q_final.0 - 0.5), pnl = format!("{:.2}", ledger.realized_pnl), win_rate = format!("{:.1}%", ledger.win_rate() * 100.0), metrics = %metrics.summary(), "Executor status");
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
