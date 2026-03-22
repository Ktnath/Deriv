use crate::{
    app::decision_engine::{
        DecisionContext, DecisionEngine, DecisionEngineConfig, SimulatedTradeSettlement,
    },
    config::ResearchConfig,
    observability::db::{
        ExecutedTradeRecord, RunDecisionRecord, RunMetadata, TelemetryDb, TradeIntentRecord,
    },
    types::TickUpdate,
};
use chrono::{TimeZone, Utc};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

pub fn run_research_cli(args: &[String]) -> anyhow::Result<()> {
    crate::observability::events::init_logging();
    let cfg = ResearchConfig::from_env();
    let db = TelemetryDb::new(&cfg.db_path)?;
    match parse_command(args) {
        ResearchCommand::Summarize { symbol } => summarize(&db, symbol.as_deref()),
        ResearchCommand::Replay {
            symbol,
            limit,
            persist,
            execution_mode,
        } => replay(&db, &cfg, &symbol, limit, persist, execution_mode),
        ResearchCommand::Report { run_id } => report(&db, run_id.as_deref()),
        ResearchCommand::InspectRegimes { symbol, window } => inspect_regimes(&db, &symbol, window),
        ResearchCommand::Help => {
            print_help();
            Ok(())
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ResearchCommand {
    Summarize {
        symbol: Option<String>,
    },
    Replay {
        symbol: String,
        limit: usize,
        persist: bool,
        execution_mode: bool,
    },
    Report {
        run_id: Option<String>,
    },
    InspectRegimes {
        symbol: String,
        window: usize,
    },
    Help,
}

fn parse_command(args: &[String]) -> ResearchCommand {
    match args.first().map(String::as_str) {
        Some("summarize") => ResearchCommand::Summarize {
            symbol: args.get(1).cloned(),
        },
        Some("replay") => ResearchCommand::Replay {
            symbol: args.get(1).cloned().unwrap_or_else(|| "R_100".to_string()),
            limit: args.get(2).and_then(|s| s.parse().ok()).unwrap_or(250),
            persist: !args.iter().any(|arg| arg == "--no-persist"),
            execution_mode: args.iter().any(|arg| arg == "--with-execution"),
        },
        Some("report") => ResearchCommand::Report {
            run_id: args.get(1).cloned(),
        },
        Some("inspect-regimes") => ResearchCommand::InspectRegimes {
            symbol: args.get(1).cloned().unwrap_or_else(|| "R_100".to_string()),
            window: args.get(2).and_then(|s| s.parse().ok()).unwrap_or(50),
        },
        _ => ResearchCommand::Help,
    }
}

fn summarize(db: &TelemetryDb, symbol: Option<&str>) -> anyhow::Result<()> {
    let summaries = db.summarize_ticks(symbol)?;
    if summaries.is_empty() {
        println!("No raw ticks found.");
        return Ok(());
    }
    for summary in summaries {
        println!(
            "symbol={} ticks={} first={} last={} min={:.5} max={:.5}",
            summary.symbol,
            summary.tick_count,
            fmt_ms(summary.first_event_time_ms),
            fmt_ms(summary.last_event_time_ms),
            summary.min_price,
            summary.max_price
        );
    }
    Ok(())
}

fn replay(
    db: &TelemetryDb,
    cfg: &ResearchConfig,
    symbol: &str,
    limit: usize,
    persist: bool,
    execution_mode: bool,
) -> anyhow::Result<()> {
    let ticks = db.load_ticks(symbol, limit)?;
    if ticks.is_empty() {
        println!("No ticks found for symbol={symbol}.");
        return Ok(());
    }
    let run = RunMetadata {
        run_id: format!(
            "research-{}-{}",
            symbol,
            ticks.first().map(|t| t.event_time_ms).unwrap_or(0)
        ),
        binary_type: "research".into(),
        model_version: cfg
            .model_path
            .clone()
            .unwrap_or_else(|| "quant-only".into()),
        strategy_version: cfg.strategy_version.clone(),
        prior_version: cfg.prior_version.clone(),
        config_fingerprint: cfg.config_fingerprint(),
        started_at_ms: Utc::now().timestamp_millis(),
    };
    if persist {
        db.upsert_run_metadata(&run)?;
    }
    let mut engine = DecisionEngine::new(DecisionEngineConfig {
        symbol: symbol.to_string(),
        contract_duration: cfg.contract_duration,
        min_stake: cfg.min_stake,
        initial_balance: cfg.initial_balance,
        max_open_positions: cfg.max_open_positions,
        max_daily_loss: cfg.max_daily_loss,
        cooldown_after_loss_ms: cfg.cooldown_after_loss_ms,
        max_consecutive_losses: cfg.max_consecutive_losses,
        model_path: cfg.model_path.clone(),
        allow_model_fallback: cfg.allow_model_fallback,
        strategy_mode: cfg.strategy_version.clone(),
        prior_mode: cfg.prior_version.clone(),
    })?;
    let mut seen = 0;
    for tick in ticks {
        let update = TickUpdate {
            symbol: tick.symbol,
            price: tick.price,
            epoch: tick.event_time_ms / 1000,
        };
        if let Some(decision) = engine.step(&update, execution_mode) {
            seen += 1;
            println!("{} decision={} direction={} edge={:.4} proposed_stake={:.2} regime={} rejection={}", fmt_ms(decision.ts_ms), decision.decision, decision.contract_direction.as_deref().unwrap_or("NONE"), decision.edge, decision.proposed_stake, decision.regime, decision.rejection_reason.as_deref().unwrap_or("none"));
            if persist {
                persist_decision_bundle(db, &run.run_id, &decision)?;
            }
        }
    }
    println!(
        "replay_complete run_id={} symbol={} ticks={} decisions={} persist={} execution_mode={}",
        run.run_id, symbol, limit, seen, persist, execution_mode
    );
    if persist {
        print_report(db, &run.run_id)?;
    }
    Ok(())
}

fn persist_decision_bundle(
    db: &TelemetryDb,
    run_id: &str,
    decision: &DecisionContext,
) -> anyhow::Result<()> {
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
        executed_stake: decision.executed_stake,
        feature_summary: &decision.feature_summary,
    })?;
    if let Some(direction) = decision.contract_direction.as_deref() {
        let intent_status = if decision.executed_stake > 0.0 {
            "executed"
        } else if decision.decision == "signal" {
            "signal_only"
        } else {
            "rejected"
        };
        let intent_id = db.insert_trade_intent(&TradeIntentRecord {
            run_id,
            decision_id,
            timestamp_ms: decision.ts_ms,
            contract_direction: direction,
            proposed_stake: decision.proposed_stake,
            executed_stake: decision.executed_stake,
            execution_enabled: decision.execution_enabled,
            intent_status,
            rejection_reason: decision.rejection_reason.as_deref(),
        })?;
        if decision.executed_stake > 0.0 {
            let trade_id = db.insert_executed_trade(&ExecutedTradeRecord {
                run_id,
                trade_intent_id: intent_id,
                timestamp_ms: decision.ts_ms,
                contract_id: None,
                contract_direction: direction,
                stake: decision.executed_stake,
                payout: None,
                pnl: None,
                exit_reason: None,
                status: "open",
            })?;
            if let Some(settlement) = decision.simulated_trade_closed.as_ref() {
                db.update_executed_trade_lifecycle(
                    trade_id,
                    settlement.settled_at_ms,
                    Some(settlement.payout),
                    Some(settlement.pnl),
                    Some(settlement.exit_reason.as_str()),
                    &settlement.status,
                )?;
            }
        }
    }
    if let Some(settlement) = decision.simulated_trade_closed.as_ref() {
        persist_simulated_settlement(db, run_id, settlement)?;
    }
    Ok(())
}

fn persist_simulated_settlement(
    db: &TelemetryDb,
    run_id: &str,
    settlement: &SimulatedTradeSettlement,
) -> anyhow::Result<()> {
    if let Some(trade_id) =
        db.find_latest_executed_trade_for_entry(run_id, settlement.entered_at_ms)?
    {
        db.update_executed_trade_lifecycle(
            trade_id,
            settlement.settled_at_ms,
            Some(settlement.payout),
            Some(settlement.pnl),
            Some(settlement.exit_reason.as_str()),
            &settlement.status,
        )?;
    }
    Ok(())
}

fn report(db: &TelemetryDb, run_id: Option<&str>) -> anyhow::Result<()> {
    let run_id = match run_id {
        Some(id) => id.to_string(),
        None => db
            .latest_run_id_for_binary("research")?
            .unwrap_or_else(|| "".into()),
    };
    if run_id.is_empty() {
        println!("No research runs found.");
        return Ok(());
    }
    print_report(db, &run_id)
}

fn print_report(db: &TelemetryDb, run_id: &str) -> anyhow::Result<()> {
    let summary = db.latest_run_report(run_id)?;
    let outcomes = db.trade_outcome_summary(run_id)?;
    println!("report run_id={run_id}");
    println!(
        "  decisions={} signal_intents={} trades={} avg_edge={:.5} pnl={:.4}",
        summary.decisions,
        summary.signal_intents,
        summary.trades,
        summary.average_edge,
        outcomes.pnl_sum
    );
    println!(
        "  wins={} losses={} open_trades={} unresolved_trades={} aborted_without_pnl={}",
        outcomes.wins,
        outcomes.losses,
        outcomes.open_trades,
        outcomes.unresolved_trades,
        outcomes.aborted_without_pnl
    );
    println!("  regimes:");
    for (regime, count) in db.regime_counts(run_id)? {
        println!("    {regime}: {count}");
    }
    println!("  rejection_reasons:");
    for (reason, count) in db.rejection_counts(run_id)? {
        println!("    {reason}: {count}");
    }
    Ok(())
}

fn inspect_regimes(db: &TelemetryDb, symbol: &str, window: usize) -> anyhow::Result<()> {
    let ticks = db.load_ticks(symbol, window.max(2))?;
    if ticks.len() < 2 {
        println!("Not enough ticks found for symbol={symbol}.");
        return Ok(());
    }
    let first = ticks.first().unwrap().price;
    let last = ticks.last().unwrap().price;
    let returns: Vec<f64> = ticks
        .windows(2)
        .map(|pair| pair[1].price - pair[0].price)
        .collect();
    let drift = last - first;
    let avg_abs_move = returns.iter().map(|r| r.abs()).sum::<f64>() / returns.len() as f64;
    let regime = if drift.abs() < avg_abs_move {
        "sideways"
    } else if drift > 0.0 {
        "uptrend"
    } else {
        "downtrend"
    };
    println!(
        "symbol={symbol} samples={} drift={:.5} avg_abs_move={:.5} inferred_regime={regime}",
        ticks.len(),
        drift,
        avg_abs_move
    );
    Ok(())
}

fn fmt_ms(ts_ms: i64) -> String {
    Utc.timestamp_millis_opt(ts_ms)
        .single()
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_else(|| ts_ms.to_string())
}

fn print_help() {
    println!("research <subcommand>\n\nSubcommands:\n  summarize [symbol]\n  replay [symbol] [limit] [--no-persist] [--with-execution]\n  report [run_id]\n  inspect-regimes [symbol] [window]");
}

impl ResearchConfig {
    fn config_fingerprint(&self) -> String {
        let mut hasher = DefaultHasher::new();
        self.db_path.hash(&mut hasher);
        self.contract_duration.hash(&mut hasher);
        self.min_stake.to_bits().hash(&mut hasher);
        self.initial_balance.to_bits().hash(&mut hasher);
        self.max_open_positions.hash(&mut hasher);
        self.max_daily_loss.to_bits().hash(&mut hasher);
        self.cooldown_after_loss_ms.hash(&mut hasher);
        self.max_consecutive_losses.hash(&mut hasher);
        self.model_path.hash(&mut hasher);
        self.allow_model_fallback.hash(&mut hasher);
        self.strategy_version.hash(&mut hasher);
        self.prior_version.hash(&mut hasher);
        format!("{:016x}", hasher.finish())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observability::db::TelemetryDb;
    use crate::process::Regime;

    #[test]
    fn parse_known_commands() {
        assert_eq!(
            parse_command(&["summarize".into()]),
            ResearchCommand::Summarize { symbol: None }
        );
        assert_eq!(
            parse_command(&[
                "replay".into(),
                "R_50".into(),
                "10".into(),
                "--with-execution".into()
            ]),
            ResearchCommand::Replay {
                symbol: "R_50".into(),
                limit: 10,
                persist: true,
                execution_mode: true
            }
        );
        assert_eq!(
            parse_command(&["report".into(), "run-1".into()]),
            ResearchCommand::Report {
                run_id: Some("run-1".into())
            }
        );
    }

    #[test]
    fn report_aggregates_fixture_run() {
        let db = TelemetryDb::new(":memory:").unwrap();
        db.upsert_run_metadata(&RunMetadata {
            run_id: "fixture".into(),
            binary_type: "research".into(),
            model_version: "m1".into(),
            strategy_version: "s1".into(),
            prior_version: "p1".into(),
            config_fingerprint: "fp".into(),
            started_at_ms: 1,
        })
        .unwrap();
        persist_decision_bundle(
            &db,
            "fixture",
            &DecisionContext {
                ts_ms: 1,
                symbol: "R_100".into(),
                price: 100.0,
                regime: Regime::Calm.as_str().into(),
                prior: 0.51,
                q_model: 0.55,
                q_final: 0.54,
                q_low: 0.52,
                q_high: 0.56,
                confidence: 1.0,
                edge: 0.04,
                time_left_sec: 120.0,
                contract_direction: Some("CALL".into()),
                benchmark_signal: "CALL".into(),
                decision: "signal".into(),
                rejection_reason: None,
                proposed_stake: 1.0,
                executed_stake: 0.0,
                execution_enabled: false,
                prior_mode: "prior-v1".into(),
                strategy_mode: "strategy-v1".into(),
                model_metadata: "quant-only".into(),
                feature_summary: "{}".into(),
                simulated_trade_closed: None,
            },
        )
        .unwrap();
        let report = db.latest_run_report("fixture").unwrap();
        assert_eq!(report.decisions, 1);
        assert_eq!(report.signal_intents, 1);
        assert_eq!(report.trades, 0);
        assert_eq!(
            db.trade_outcome_summary("fixture").unwrap(),
            crate::observability::db::WinLossSummary {
                wins: 0,
                losses: 0,
                open_trades: 0,
                unresolved_trades: 0,
                aborted_without_pnl: 0,
                pnl_sum: 0.0
            }
        );
    }
}
