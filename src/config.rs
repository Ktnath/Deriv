use crate::types::StrategyType;
use std::{env, str::FromStr};

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
    // Risk limits
    pub max_open_positions: usize,
    pub max_daily_loss: f64,
    pub cooldown_after_loss_ms: u64,
    pub max_consecutive_losses: usize,
    // Stop-loss only
    pub stop_loss_pct: f64,
}

impl BotConfig {
    pub fn from_env() -> Self {
        dotenvy::dotenv().ok();
        Self {
            api_token: env::var("DERIV_API_TOKEN").expect("DERIV_API_TOKEN must be set"),
            app_id: env::var("DERIV_APP_ID").expect("DERIV_APP_ID must be set"),
            endpoint: env::var("DERIV_ENDPOINT")
                .unwrap_or_else(|_| "wss://ws.binaryws.com/websockets/v3".to_string()),
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
                .unwrap_or(StrategyType::Temporal),
            contract_duration: env::var("DERIV_CONTRACT_DURATION")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(300),
            duration_unit: env::var("DERIV_DURATION_UNIT").unwrap_or_else(|_| "s".to_string()),
            stake: env::var("DERIV_STAKE")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(1.0),
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
        }
    }
}
