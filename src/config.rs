use crate::types::StrategyType;
use std::{env, str::FromStr};

#[derive(Clone, Debug)]
pub struct SharedConfig {
    pub api_token: String,
    pub app_id: String,
    pub endpoint: String,
}

#[derive(Clone, Debug)]
pub struct ExecutorConfig {
    pub shared: SharedConfig,
    pub symbol: String,
    pub account_type: String,
    pub dry_run: bool,
    pub initial_balance: f64,
    pub strategy: StrategyType,
    pub contract_duration: u64,
    pub duration_unit: String,
    pub stake: f64,
    pub min_stake: f64,
    pub model_path: Option<String>,
    pub allow_model_fallback: bool,
    pub market_prior: Option<f64>,
    pub max_open_positions: usize,
    pub max_daily_loss: f64,
    pub cooldown_after_loss_ms: u64,
    pub max_consecutive_losses: usize,
    pub stop_loss_pct: f64,
    pub telemetry_db_path: String,
    pub telemetry_server_bind: String,
}

#[derive(Clone, Debug)]
pub struct RecorderConfig {
    pub shared: SharedConfig,
    pub symbols: Vec<String>,
    pub db_path: String,
    pub subscribe_balance: bool,
    pub subscribe_time: bool,
    pub retention_days: Option<u64>,
}

#[derive(Clone, Debug)]
pub struct ResearchConfig {
    pub db_path: String,
}

#[derive(Clone, Debug)]
pub struct BotConfig {
    pub api_token: String,
    pub app_id: String,
    pub endpoint: String,
    pub symbol: String,
    pub account_type: String,
    pub dry_run: bool,
    pub initial_balance: f64,
    pub strategy: StrategyType,
    pub contract_duration: u64,
    pub duration_unit: String,
    pub stake: f64,
    pub min_stake: f64,
    pub model_path: Option<String>,
    pub allow_model_fallback: bool,
    pub market_prior: Option<f64>,
    pub max_open_positions: usize,
    pub max_daily_loss: f64,
    pub cooldown_after_loss_ms: u64,
    pub max_consecutive_losses: usize,
    pub stop_loss_pct: f64,
}

impl SharedConfig {
    pub fn from_env() -> Self {
        dotenvy::dotenv().ok();
        Self {
            api_token: env::var("DERIV_API_TOKEN").expect("DERIV_API_TOKEN must be set"),
            app_id: env::var("DERIV_APP_ID").expect("DERIV_APP_ID must be set"),
            endpoint: env::var("DERIV_ENDPOINT")
                .unwrap_or_else(|_| "wss://ws.binaryws.com/websockets/v3".to_string()),
        }
    }
}

impl ExecutorConfig {
    pub fn from_env() -> Self {
        let shared = SharedConfig::from_env();
        Self {
            shared,
            symbol: env::var("DERIV_SYMBOL").unwrap_or_else(|_| "R_100".to_string()),
            account_type: env::var("DERIV_ACCOUNT_TYPE").unwrap_or_else(|_| "demo".to_string()),
            dry_run: env::var("DRY_RUN").unwrap_or_else(|_| "1".to_string()) == "1",
            initial_balance: env::var("DERIV_INITIAL_BALANCE")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(10000.0),
            strategy: env::var("DERIV_STRATEGY")
                .ok()
                .and_then(|s| StrategyType::from_str(&s).ok())
                .unwrap_or(StrategyType::Process),
            contract_duration: env::var("DERIV_CONTRACT_DURATION")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(300),
            duration_unit: env::var("DERIV_DURATION_UNIT").unwrap_or_else(|_| "s".to_string()),
            stake: env::var("DERIV_STAKE")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(1.0),
            min_stake: env::var("DERIV_MIN_STAKE")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0.35),
            model_path: env::var("DERIV_MODEL_PATH")
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
            allow_model_fallback: env::var("DERIV_ALLOW_MODEL_FALLBACK")
                .ok()
                .map(|s| parse_bool(&s))
                .unwrap_or(true),
            market_prior: env::var("DERIV_MARKET_PRIOR")
                .ok()
                .and_then(|s| s.parse::<f64>().ok())
                .map(|p| p.clamp(0.0, 1.0)),
            max_open_positions: env::var("DERIV_MAX_POSITIONS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(1),
            max_daily_loss: env::var("DERIV_MAX_DAILY_LOSS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(50.0),
            cooldown_after_loss_ms: env::var("DERIV_COOLDOWN_MS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(30_000),
            max_consecutive_losses: env::var("DERIV_MAX_CONSEC_LOSSES")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(5),
            stop_loss_pct: env::var("DERIV_STOP_LOSS_PCT")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0.80),
            telemetry_db_path: env::var("DERIV_TELEMETRY_DB_PATH")
                .unwrap_or_else(|_| "deriv_metrics.db".to_string()),
            telemetry_server_bind: env::var("DERIV_TELEMETRY_BIND")
                .unwrap_or_else(|_| "127.0.0.1:3000".to_string()),
        }
    }
}

impl RecorderConfig {
    pub fn from_env() -> Self {
        let shared = SharedConfig::from_env();
        Self {
            shared,
            symbols: env::var("DERIV_RECORDER_SYMBOLS")
                .ok()
                .map(|raw| parse_symbols(&raw))
                .filter(|symbols| !symbols.is_empty())
                .unwrap_or_else(|| {
                    vec![env::var("DERIV_SYMBOL").unwrap_or_else(|_| "R_100".to_string())]
                }),
            db_path: env::var("DERIV_RECORDER_DB_PATH")
                .unwrap_or_else(|_| "deriv_recorder.db".to_string()),
            subscribe_balance: env::var("DERIV_RECORDER_BALANCE")
                .ok()
                .map(|s| parse_bool(&s))
                .unwrap_or(false),
            subscribe_time: env::var("DERIV_RECORDER_TIME")
                .ok()
                .map(|s| parse_bool(&s))
                .unwrap_or(true),
            retention_days: env::var("DERIV_RECORDER_RETENTION_DAYS")
                .ok()
                .and_then(|s| s.parse().ok()),
        }
    }
}

impl ResearchConfig {
    pub fn from_env() -> Self {
        dotenvy::dotenv().ok();
        Self {
            db_path: env::var("DERIV_RESEARCH_DB_PATH")
                .or_else(|_| env::var("DERIV_RECORDER_DB_PATH"))
                .unwrap_or_else(|_| "deriv_recorder.db".to_string()),
        }
    }
}

impl BotConfig {
    pub fn from_env() -> Self {
        ExecutorConfig::from_env().into()
    }
}

impl From<ExecutorConfig> for BotConfig {
    fn from(value: ExecutorConfig) -> Self {
        Self {
            api_token: value.shared.api_token,
            app_id: value.shared.app_id,
            endpoint: value.shared.endpoint,
            symbol: value.symbol,
            account_type: value.account_type,
            dry_run: value.dry_run,
            initial_balance: value.initial_balance,
            strategy: value.strategy,
            contract_duration: value.contract_duration,
            duration_unit: value.duration_unit,
            stake: value.stake,
            min_stake: value.min_stake,
            model_path: value.model_path,
            allow_model_fallback: value.allow_model_fallback,
            market_prior: value.market_prior,
            max_open_positions: value.max_open_positions,
            max_daily_loss: value.max_daily_loss,
            cooldown_after_loss_ms: value.cooldown_after_loss_ms,
            max_consecutive_losses: value.max_consecutive_losses,
            stop_loss_pct: value.stop_loss_pct,
        }
    }
}

fn parse_symbols(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(|symbol| symbol.trim())
        .filter(|symbol| !symbol.is_empty())
        .map(str::to_string)
        .collect()
}

fn parse_bool(raw: &str) -> bool {
    matches!(
        raw.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_symbols_splits_csv() {
        assert_eq!(
            parse_symbols("R_100, R_50,,frxEURUSD"),
            vec!["R_100", "R_50", "frxEURUSD"]
        );
    }

    #[test]
    fn parse_bool_accepts_common_truthy_values() {
        assert!(parse_bool("true"));
        assert!(parse_bool("1"));
        assert!(!parse_bool("false"));
    }
}
