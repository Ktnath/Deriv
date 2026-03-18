mod config;

use std::sync::Arc;
use std::time::Duration;

use bot_core::traits::{AlphaEngine, ContractManager, Ledger, RiskEngine, Strategy, SettlementPriceEstimator};
use bot_core::types::{
    ContractCommand, ContractType, Prob, Stake, SymbolId, UnixMs, Usd,
    make_synth_book,
};
use engines::alpha::LognormalAlphaEngine;
use engines::risk::FractionalKellyRiskEngine;
use engines::strategy::StrategySelector;
use runtime::ledger::LocalLedger;
use runtime::settlement::EWMAEstimator;
use runtime::contract_manager::DerivContractManager;
use connectors::deriv_ws::{DerivWsClient, DerivEvent, run_tick_dispatcher};

use crate::config::BotConfig;

use tokio::sync::mpsc;

fn now_ms_local() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as i64
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cfg = BotConfig::from_env();

    println!("╔══════════════════════════════════════════════════╗");
    println!("║         DERIV TRADING BOT v0.1.0                ║");
    println!("╠══════════════════════════════════════════════════╣");
    println!("║ Symbol:   {:<39}║", cfg.symbol);
    println!("║ Strategy: {:<39}║", format!("{:?}", cfg.strategy));
    println!("║ Duration: {}{}                                   ║", cfg.contract_duration, cfg.duration_unit);
    println!("║ Stake:    ${:<38.2}║", cfg.stake);
    println!("║ Dry Run:  {:<39}║", if cfg.dry_run { "YES (signals only)" } else { "NO (LIVE)" });
    println!("║ Account:  {:<39}║", cfg.account_type);
    println!("╚══════════════════════════════════════════════════╝");

    // 1. Connect to Deriv WebSocket
    let (ws_client, read_stream) = DerivWsClient::connect(&cfg.app_id, &cfg.endpoint).await
        .map_err(|e| anyhow::anyhow!("WS connect failed: {}", e))?;
    let ws = Arc::new(ws_client);

    // 2. Start event dispatcher
    let (tick_tx, mut tick_rx) = mpsc::channel(2048);
    let (event_tx, mut event_rx) = mpsc::channel(2048);

    tokio::spawn(async move {
        run_tick_dispatcher(read_stream, tick_tx, event_tx).await;
    });

    // 3. Authorize
    ws.authorize(&cfg.api_token).await
        .map_err(|e| anyhow::anyhow!("Auth failed: {}", e))?;

    // Wait for auth response
    let mut authorized = false;
    let auth_deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !authorized && tokio::time::Instant::now() < auth_deadline {
        tokio::select! {
            Some(event) = event_rx.recv() => {
                match event {
                    DerivEvent::Authorize { balance, currency, login_id } => {
                        println!("✓ Authorized: {} ({}) Balance: {:.2} {}",
                            login_id, cfg.account_type, balance, currency);
                        authorized = true;
                    }
                    DerivEvent::Error { code, message } => {
                        anyhow::bail!("Auth error [{}]: {}", code, message);
                    }
                    _ => {}
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(100)) => {}
        }
    }
    if !authorized {
        anyhow::bail!("Authorization timeout (10s)");
    }

    // 4. Subscribe to ticks
    ws.subscribe_ticks(&cfg.symbol).await
        .map_err(|e| anyhow::anyhow!("Tick sub failed: {}", e))?;

    // 5. Request balance subscription
    ws.get_balance(true).await.ok();

    // 6. Initialize engines
    let mut settlement = EWMAEstimator::new();
    let mut alpha = LognormalAlphaEngine::new(0.55);
    let risk = FractionalKellyRiskEngine {
        max_fraction: 0.15,
        global_cap: Usd(cfg.initial_balance),
        max_loss: Usd(cfg.initial_balance * 0.5),
        min_stake: Usd(0.35),
    };
    let mut strat = StrategySelector::new(cfg.strategy);
    let mut contract_mgr = DerivContractManager::new(
        Arc::clone(&ws),
        cfg.dry_run,
        cfg.contract_duration,
        cfg.duration_unit.clone(),
    );
    let mut ledger = LocalLedger::new(cfg.initial_balance);

    // Timing
    let mut contract_start_ms: Option<i64> = None;
    let mut contract_end_ms: Option<i64> = None;
    let mut last_contract_time = 0i64;
    let mut tick_count = 0u64;
    let q_mkt_up: f64 = 0.5; // market implied probability (starts at 50/50)

    println!("\n▶ Starting trading loop for {}...\n", cfg.symbol);

    loop {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let now = now_ms_local();

        // Process events from the dispatcher
        while let Ok(event) = event_rx.try_recv() {
            match event {
                DerivEvent::Proposal { req_id, proposal_id, ask_price, payout } => {
                    contract_mgr
                        .handle_proposal_response(req_id, &proposal_id, ask_price, payout)
                        .await
                        .ok();
                }
                DerivEvent::Buy { contract_id, buy_price, payout } => {
                    contract_mgr.handle_buy_confirmation(&contract_id, buy_price, payout);
                    ledger.update_contract_buy(ContractType::Call, Stake(buy_price));
                }
                DerivEvent::Balance { balance, currency } => {
                    if tick_count % 100 == 0 {
                        println!("[BALANCE] {:.2} {}", balance, currency);
                    }
                }
                DerivEvent::Error { code, message } => {
                    eprintln!("[API_ERROR] {}: {}", code, message);
                }
                _ => {}
            }
        }

        // Process ticks
        let mut _ticks_this_round = 0;
        while let Ok(tick) = tick_rx.try_recv() {
            tick_count += 1;
            _ticks_this_round += 1;

            // Update settlement estimator
            settlement.handle_update(tick.clone());

            // Update alpha engine with spot returns
            alpha.update_spot_return(tick.price, UnixMs(tick.epoch * 1000));

            // Auto-create contract windows based on tick timing
            if contract_start_ms.is_none() || now > contract_end_ms.unwrap_or(0) + 5000 {
                let start = now;
                let end = start + (cfg.contract_duration * 1000) as i64;
                contract_start_ms = Some(start);
                contract_end_ms = Some(end);
                settlement.s0 = None;
            }
        }

        // Skip if no contract window or no ticks yet
        let (Some(start_ms), Some(end_ms)) = (contract_start_ms, contract_end_ms) else { continue; };

        // Capture opening price
        let est = settlement.estimate(UnixMs(now));
        if est.s0.is_none() && est.s_hat_t.is_some() {
            settlement.capture_s0(UnixMs(start_ms));
        }

        let Some(s0) = est.s0 else { continue; };
        let Some(s_hat) = est.s_hat_t else { continue; };

        // Calculate time left in this contract window
        let time_left_sec = ((end_ms - now) as f64 / 1000.0).max(0.0);

        // Alpha + Strategy + Risk
        let q_model = alpha.calculate_q_model(s0, s_hat, time_left_sec);
        let alpha_out = alpha.shrink_logit(q_model, Prob(q_mkt_up), 0.55);
        let book = make_synth_book(q_mkt_up, 0.01, now);
        let risk_dec = risk.size_fractional_kelly(
            alpha_out.q_low,
            bot_core::types::Price(0.50),
            0.5,
        );
        let desired = strat.generate_desired_state(&alpha_out, &risk_dec, &book, time_left_sec);

        // Logging (every ~10 seconds)
        if tick_count % 50 == 0 {
            let regime = strat.evaluate_regime(time_left_sec);
            let l_state = ledger.get_state();
            println!(
                "[{}][{regime}] S0={:.4} S_hat={:.4} q_model={:.4} Edge={:.4} PnL={:.2} W/L={}/{} Ticks={}",
                cfg.symbol, s0, s_hat, q_model.0, alpha_out.q_final.0 - 0.5,
                l_state.realized_pnl.0, ledger.wins, ledger.losses, tick_count
            );
        }

        // Execution: place contract if desired stake > minimum
        let total_desired = desired.target_position_up.0 + desired.target_position_down.0;
        if total_desired > 0.0 && (now - last_contract_time) > (cfg.contract_duration as i64 * 1000) {
            last_contract_time = now;

            // Determine direction
            let (ct, stake_val) = if desired.target_position_up.0 > desired.target_position_down.0 {
                (ContractType::Call, desired.target_position_up.0.min(cfg.stake))
            } else {
                (ContractType::Put, desired.target_position_down.0.min(cfg.stake))
            };

            let cmd = ContractCommand::Buy {
                symbol: SymbolId(cfg.symbol.clone()),
                contract_type: ct,
                stake: Stake(stake_val.max(0.35)), // Enforce minimum stake
                duration_sec: cfg.contract_duration,
            };

            if let Err(e) = contract_mgr.execute_commands(vec![cmd]) {
                eprintln!("[EXEC_ERROR] {}", e);
            }

            // Reset contract window for next round
            contract_start_ms = None;
            contract_end_ms = None;
        }

        ledger.mark_to_market(&book);
    }
}
