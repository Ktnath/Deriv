use serde::{Deserialize, Serialize};
use std::str::FromStr;

// ── Strategy selection (reused from Polymarket bot) ──────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StrategyType {
    Temporal,
    Rsi,
    BollingerBands,
    Macd,
    Ensemble,
    Lmsr,
    LateCycle,
    MtfPlaybook,
}

impl FromStr for StrategyType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "temporal" => Ok(StrategyType::Temporal),
            "rsi" => Ok(StrategyType::Rsi),
            "bb" | "bollinger_bands" | "bollingerbands" => Ok(StrategyType::BollingerBands),
            "macd" => Ok(StrategyType::Macd),
            "ensemble" | "dynamic" => Ok(StrategyType::Ensemble),
            "lmsr" | "bayesian" => Ok(StrategyType::Lmsr),
            "late_cycle" | "late" => Ok(StrategyType::LateCycle),
            "mtf" | "mtf_playbook" | "mtfplaybook" => Ok(StrategyType::MtfPlaybook),
            _ => Err(format!("Unknown strategy type: {}", s)),
        }
    }
}

// ── Domain primitives ────────────────────────────────────────────────

/// Symbol identifier on Deriv (e.g. "R_100", "1HZ100V", "frxEURUSD").
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SymbolId(pub String);

/// Unix timestamp in milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct UnixMs(pub i64);

impl UnixMs {
    pub fn now() -> Self {
        use std::time::{SystemTime, UNIX_EPOCH};
        Self(SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as i64)
    }

    pub fn elapsed_ms(&self, now: UnixMs) -> i64 {
        (now.0 - self.0).max(0)
    }
}

/// Raw price value.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct Price(pub f64);

/// USD amount.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct Usd(pub f64);

/// Stake amount in USD for a contract.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct Stake(pub f64);

/// Probability [0, 1].
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct Prob(pub f64);

// ── Contract types ───────────────────────────────────────────────────

/// Deriv contract type for Rise/Fall trading.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContractType {
    /// CALL — price will rise.
    Call,
    /// PUT — price will fall.
    Put,
}

impl std::fmt::Display for ContractType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ContractType::Call => write!(f, "CALL"),
            ContractType::Put => write!(f, "PUT"),
        }
    }
}

/// Specification of a tradeable contract on Deriv.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContractSpec {
    pub symbol: SymbolId,
    pub display_name: String,
    pub contract_type: ContractType,
    /// Contract duration in seconds.
    pub duration_sec: u64,
    /// Expected payout multiplier (e.g. 1.95 for 95% payout).
    pub payout_multiplier: f64,
}

// ── Market state ─────────────────────────────────────────────────────

/// Window lifecycle states (reused from Polymarket).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowState {
    Warmup,
    PreOpen,
    Open,
    Trading,
    Freeze,
    Close,
    ResolvedOrRollover,
}

/// Current tick state from the Deriv tick stream.
#[derive(Debug, Clone, Copy)]
pub struct TickState {
    pub last_price: Price,
    pub prev_price: Price,
    pub ts: UnixMs,
}

impl TickState {
    pub fn mid_price(&self) -> f64 {
        self.last_price.0
    }
}

/// Synthetic "book" representation for strategy compatibility.
/// Wraps tick data into the bid/ask like structure strategies expect.
#[derive(Debug, Clone, Copy)]
pub struct Level {
    pub price: Price,
    pub size: Stake,
}

#[derive(Debug, Clone, Copy)]
pub struct TopOfBook {
    pub bid: Option<Level>,
    pub ask: Option<Level>,
    pub ts: UnixMs,
}

#[derive(Debug, Clone)]
pub struct BookState {
    pub up: TopOfBook,
    pub down: TopOfBook,
    pub tick_up: f64,
    pub tick_down: f64,
}

/// Build a synthetic BookState from a tick probability q_up and a spread.
pub fn make_synth_book(q_up: f64, spread: f64, ts: i64) -> BookState {
    let up_bid = (q_up - spread).clamp(0.01, 0.99);
    let up_ask = (q_up + spread).clamp(0.01, 0.99);
    let down_mid = (1.0 - q_up).clamp(0.01, 0.99);
    let down_bid = (down_mid - spread).clamp(0.01, 0.99);
    let down_ask = (down_mid + spread).clamp(0.01, 0.99);

    BookState {
        up: TopOfBook {
            bid: Some(Level { price: Price(up_bid), size: Stake(1.0) }),
            ask: Some(Level { price: Price(up_ask), size: Stake(1.0) }),
            ts: UnixMs(ts),
        },
        down: TopOfBook {
            bid: Some(Level { price: Price(down_bid), size: Stake(1.0) }),
            ask: Some(Level { price: Price(down_ask), size: Stake(1.0) }),
            ts: UnixMs(ts),
        },
        tick_up: spread,
        tick_down: spread,
    }
}

// ── Settlement ───────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct SettlementEstimate {
    pub basis_ewma: f64,
    pub s0: Option<f64>,
    pub s_hat_t: Option<f64>,
    pub timestamp: UnixMs,
    pub staleness_ms: i64,
    pub sigma_basis: f64,
    pub confidence: f64,
    pub s0_uncertainty: Option<f64>,
}

// ── Alpha / Risk ─────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct AlphaOutput {
    pub q_model: Prob,
    pub q_mkt: Prob,
    pub q_final: Prob,
    pub q_low: Prob,
    pub q_high: Prob,
    pub confidence: f64,
}

#[derive(Debug, Clone)]
pub struct RiskDecision {
    pub fraction: f64,
    pub max_size: Usd,
}

/// Desired trading state produced by a strategy.
#[derive(Debug, Clone)]
pub struct DesiredState {
    pub target_position_up: Stake,
    pub target_position_down: Stake,
    pub maker_bid_price_up: Option<Price>,
    pub maker_ask_price_up: Option<Price>,
    pub maker_bid_price_down: Option<Price>,
    pub maker_ask_price_down: Option<Price>,
}

// ── Commands ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderSide {
    Buy,
    Sell,
}

/// Commands the bot can issue to the contract manager.
#[derive(Debug, Clone)]
pub enum ContractCommand {
    Buy {
        symbol: SymbolId,
        contract_type: ContractType,
        stake: Stake,
        duration_sec: u64,
    },
    Sell {
        contract_id: String,
    },
}

// ── Ledger ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct LedgerState {
    pub cash: Usd,
    pub open_calls: Stake,
    pub open_puts: Stake,
    pub realized_pnl: Usd,
    pub total_fees: Usd,
    pub mtm_pnl: Usd,
    pub peak_mtm: Usd,
}

// ── Tick update (from Deriv WS) ──────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TickUpdate {
    pub symbol: String,
    pub price: f64,
    pub epoch: i64,
}

// ── Errors ────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum BotError {
    Network(String),
    HeartbeatTimeout,
    StateError(String),
    Auth(String),
    Api(String),
    Other(String),
}

impl std::fmt::Display for BotError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BotError::Network(msg) => write!(f, "Network error: {}", msg),
            BotError::HeartbeatTimeout => write!(f, "Heartbeat timeout triggered"),
            BotError::StateError(msg) => write!(f, "State error: {}", msg),
            BotError::Auth(msg) => write!(f, "Auth error: {}", msg),
            BotError::Api(msg) => write!(f, "API error: {}", msg),
            BotError::Other(msg) => write!(f, "Other error: {}", msg),
        }
    }
}

impl std::error::Error for BotError {}
