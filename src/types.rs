use serde::{Deserialize, Serialize};
use std::str::FromStr;

// ── Strategy selection ───────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StrategyType {
    Process,
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
            "process" | "synthetic" | "regime" => Ok(Self::Process),
            "temporal" => Ok(Self::Temporal),
            "rsi" => Ok(Self::Rsi),
            "bb" | "bollinger_bands" | "bollingerbands" => Ok(Self::BollingerBands),
            "macd" => Ok(Self::Macd),
            "ensemble" | "dynamic" => Ok(Self::Ensemble),
            "lmsr" | "bayesian" => Ok(Self::Lmsr),
            "late_cycle" | "late" => Ok(Self::LateCycle),
            "mtf" | "mtf_playbook" | "mtfplaybook" => Ok(Self::MtfPlaybook),
            _ => Err(format!("Unknown strategy: {}", s)),
        }
    }
}

// ── Primitives ───────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SymbolId(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct UnixMs(pub i64);

impl UnixMs {
    pub fn now() -> Self {
        use std::time::{SystemTime, UNIX_EPOCH};
        Self(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis() as i64,
        )
    }
    pub fn elapsed_ms(&self, now: UnixMs) -> i64 {
        (now.0 - self.0).max(0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct Price(pub f64);

#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct Usd(pub f64);

#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct Stake(pub f64);

#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct Prob(pub f64);

// ── Contract types ───────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContractType {
    Call,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContractSpec {
    pub symbol: SymbolId,
    pub display_name: String,
    pub contract_type: ContractType,
    pub duration_sec: u64,
    pub payout_multiplier: f64,
}

// ── Connection FSM ───────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Authorized,
    Running,
    Reconnecting,
    ShuttingDown,
}

// ── Trade FSM ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TradeState {
    Idle,
    Pricing,
    Submitted,
    Open,
    Settled,
    Aborted,
}

// ── Window state ─────────────────────────────────────────────────────

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

// ── Tick data ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TickUpdate {
    pub symbol: String,
    pub price: f64,
    pub epoch: i64,
}

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

// ── Book (synthetic for strategy compat) ─────────────────────────────

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

pub fn make_synth_book(q_up: f64, spread: f64, ts: i64) -> BookState {
    let up_bid = (q_up - spread).clamp(0.01, 0.99);
    let up_ask = (q_up + spread).clamp(0.01, 0.99);
    let down_mid = (1.0 - q_up).clamp(0.01, 0.99);
    let down_bid = (down_mid - spread).clamp(0.01, 0.99);
    let down_ask = (down_mid + spread).clamp(0.01, 0.99);
    BookState {
        up: TopOfBook {
            bid: Some(Level {
                price: Price(up_bid),
                size: Stake(1.0),
            }),
            ask: Some(Level {
                price: Price(up_ask),
                size: Stake(1.0),
            }),
            ts: UnixMs(ts),
        },
        down: TopOfBook {
            bid: Some(Level {
                price: Price(down_bid),
                size: Stake(1.0),
            }),
            ask: Some(Level {
                price: Price(down_ask),
                size: Stake(1.0),
            }),
            ts: UnixMs(ts),
        },
        tick_up: spread,
        tick_down: spread,
    }
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

#[derive(Debug, Clone)]
pub struct DesiredState {
    pub target_position_up: Stake,
    pub target_position_down: Stake,
    pub maker_bid_price_up: Option<Price>,
    pub maker_ask_price_up: Option<Price>,
    pub maker_bid_price_down: Option<Price>,
    pub maker_ask_price_down: Option<Price>,
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

// ── Commands ─────────────────────────────────────────────────────────

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

// ── Errors ────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum BotError {
    #[error("Network: {0}")]
    Network(String),
    #[error("Heartbeat timeout")]
    HeartbeatTimeout,
    #[error("State: {0}")]
    StateError(String),
    #[error("Auth: {0}")]
    Auth(String),
    #[error("API: {0}")]
    Api(String),
    #[error("Risk rejected: {0}")]
    RiskRejected(String),
    #[error("{0}")]
    Other(String),
}

// ── Risk rejection reasons ───────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum RiskRejection {
    MaxOpenPositions,
    MaxDailyLoss,
    CooldownActive,
    ConsecutiveLossLimit,
    InsufficientBalance,
}

impl std::fmt::Display for RiskRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MaxOpenPositions => write!(f, "max open positions reached"),
            Self::MaxDailyLoss => write!(f, "daily loss limit reached"),
            Self::CooldownActive => write!(f, "cooldown active after loss"),
            Self::ConsecutiveLossLimit => write!(f, "consecutive loss limit reached"),
            Self::InsufficientBalance => write!(f, "insufficient balance"),
        }
    }
}
