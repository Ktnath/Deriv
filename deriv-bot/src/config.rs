use std::{env, str::FromStr};
use bot_core::types::StrategyType;

/// Runtime config loaded from environment variables.
///
/// Required:
/// - DERIV_API_TOKEN       (create at api.deriv.com)
/// - DERIV_APP_ID          (register at api.deriv.com dashboard)
///
/// Optional:
/// - DERIV_ENDPOINT        (default: wss://ws.derivws.com/websockets/v3)
/// - DERIV_SYMBOL          (default: R_100)
/// - DERIV_ACCOUNT_TYPE    (demo | real, default: demo)
/// - DRY_RUN               (1 | 0, default: 1)
/// - DERIV_INITIAL_BALANCE (default: 10000.0 for demo)
/// - DERIV_STRATEGY        (temporal | rsi | bb | macd | ensemble | mtf_playbook, default: temporal)
/// - DERIV_CONTRACT_DURATION (seconds, default: 300)
/// - DERIV_DURATION_UNIT   (s | t | m, default: s)
/// - DERIV_STAKE           (USD, default: 1.0)
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
}

impl BotConfig {
    pub fn from_env() -> Self {
        dotenvy::dotenv().ok();

        let api_token = env::var("DERIV_API_TOKEN")
            .expect("DERIV_API_TOKEN must be set");

        let app_id = env::var("DERIV_APP_ID")
            .expect("DERIV_APP_ID must be set");

        let endpoint = env::var("DERIV_ENDPOINT")
            .unwrap_or_else(|_| "wss://ws.derivws.com/websockets/v3".to_string());

        let symbol = env::var("DERIV_SYMBOL")
            .unwrap_or_else(|_| "R_100".to_string());

        let account_type = env::var("DERIV_ACCOUNT_TYPE")
            .unwrap_or_else(|_| "demo".to_string());

        let dry_run = env::var("DRY_RUN")
            .unwrap_or_else(|_| "1".to_string()) == "1";

        let initial_balance = env::var("DERIV_INITIAL_BALANCE")
            .unwrap_or_else(|_| "10000.0".to_string())
            .parse::<f64>()
            .unwrap_or(10000.0);

        let strategy = env::var("DERIV_STRATEGY")
            .ok()
            .and_then(|s| StrategyType::from_str(&s).ok())
            .unwrap_or(StrategyType::Temporal);

        let contract_duration = env::var("DERIV_CONTRACT_DURATION")
            .unwrap_or_else(|_| "300".to_string())
            .parse::<u64>()
            .unwrap_or(300);

        let duration_unit = env::var("DERIV_DURATION_UNIT")
            .unwrap_or_else(|_| "s".to_string());

        let stake = env::var("DERIV_STAKE")
            .unwrap_or_else(|_| "1.0".to_string())
            .parse::<f64>()
            .unwrap_or(1.0);

        Self {
            api_token,
            app_id,
            endpoint,
            symbol,
            account_type,
            dry_run,
            initial_balance,
            strategy,
            contract_duration,
            duration_unit,
            stake,
        }
    }
}
