use crate::{
    config::ResearchConfig,
    observability::{db::TelemetryDb, events},
};
use chrono::{TimeZone, Utc};

pub fn run_research_cli(args: &[String]) -> anyhow::Result<()> {
    events::init_logging();
    let cfg = ResearchConfig::from_env();
    let db = TelemetryDb::new(&cfg.db_path)?;
    match parse_command(args) {
        ResearchCommand::Summarize { symbol } => summarize(&db, symbol.as_deref()),
        ResearchCommand::Replay { symbol, limit } => replay(&db, &symbol, limit),
        ResearchCommand::InspectRegimes { symbol, window } => inspect_regimes(&db, &symbol, window),
        ResearchCommand::Help => {
            print_help();
            Ok(())
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ResearchCommand {
    Summarize { symbol: Option<String> },
    Replay { symbol: String, limit: usize },
    InspectRegimes { symbol: String, window: usize },
    Help,
}

fn parse_command(args: &[String]) -> ResearchCommand {
    match args.first().map(String::as_str) {
        Some("summarize") => ResearchCommand::Summarize {
            symbol: args.get(1).cloned(),
        },
        Some("replay") => ResearchCommand::Replay {
            symbol: args.get(1).cloned().unwrap_or_else(|| "R_100".to_string()),
            limit: args.get(2).and_then(|s| s.parse().ok()).unwrap_or(25),
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

fn replay(db: &TelemetryDb, symbol: &str, limit: usize) -> anyhow::Result<()> {
    let ticks = db.load_ticks(symbol, limit)?;
    if ticks.is_empty() {
        println!("No ticks found for symbol={symbol}.");
        return Ok(());
    }
    for tick in ticks {
        println!(
            "{} symbol={} price={:.5} source={} latency_ms={}",
            fmt_ms(tick.event_time_ms),
            tick.symbol,
            tick.price,
            tick.source,
            tick.received_at_ms - tick.event_time_ms
        );
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
    println!("research <subcommand>\n\nSubcommands:\n  summarize [symbol]\n  replay [symbol] [limit]\n  inspect-regimes [symbol] [window]");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_known_commands() {
        assert_eq!(
            parse_command(&["summarize".into()]),
            ResearchCommand::Summarize { symbol: None }
        );
        assert_eq!(
            parse_command(&["replay".into(), "R_50".into(), "10".into()]),
            ResearchCommand::Replay {
                symbol: "R_50".into(),
                limit: 10
            }
        );
        assert_eq!(
            parse_command(&["inspect-regimes".into(), "R_75".into()]),
            ResearchCommand::InspectRegimes {
                symbol: "R_75".into(),
                window: 50
            }
        );
    }
}
