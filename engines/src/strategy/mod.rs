use bot_core::traits::Strategy;
use bot_core::types::{AlphaOutput, BookState, DesiredState, Price, RiskDecision, Stake, StrategyType};

pub mod rsi;
pub mod bb;
pub mod macd;
pub mod breakout;
pub mod imbalance;
pub mod lmsr;
pub mod ensemble;
pub mod late_cycle;
pub mod mtf_playbook;

// ── Temporal Strategy (default, reused from Polymarket) ──────────────

pub struct TemporalStrategy;

impl Strategy for TemporalStrategy {
    fn evaluate_regime(&mut self, time_left_sec: f64) -> String {
        if time_left_sec > 120.0 {
            "Early".to_string()
        } else if time_left_sec > 30.0 {
            "Mid".to_string()
        } else if time_left_sec > 10.0 {
            "Late".to_string()
        } else {
            "Freeze".to_string()
        }
    }

    fn generate_desired_state(
        &mut self,
        alpha: &AlphaOutput,
        risk: &RiskDecision,
        current_book: &BookState,
        _time_left_sec: f64,
    ) -> DesiredState {
        let mut target_position_up = 0.0;
        let mut maker_bid_up = None;

        if let Some(ask) = current_book.up.ask {
            let edge_up = alpha.q_low.0 - ask.price.0;

            if edge_up > 0.02 && risk.fraction > 0.0 {
                let stake_usd = risk.fraction * risk.max_size.0;
                target_position_up = stake_usd;
            }

            let tick = current_book.tick_up;
            if tick > 0.0 {
                let diff = (alpha.q_low.0 - 0.01) / tick;
                let rounded_price = (diff.round() * tick).clamp(0.01, 0.99);
                maker_bid_up = Some(Price(rounded_price));
            }
        }

        DesiredState {
            target_position_up: Stake(target_position_up),
            target_position_down: Stake(0.0),
            maker_bid_price_up: maker_bid_up,
            maker_ask_price_up: None,
            maker_bid_price_down: None,
            maker_ask_price_down: None,
        }
    }
}

// ── Strategy Selector (dispatch enum) ────────────────────────────────

pub enum StrategySelector {
    Temporal(TemporalStrategy),
    Rsi(rsi::RsiStrategy),
    BollingerBands(bb::BollingerBandsStrategy),
    Macd(macd::MacdStrategy),
    Lmsr(lmsr::LmsrStrategy),
    Ensemble(ensemble::EnsembleStrategy),
    LateCycle(late_cycle::LateCycleStrategy),
    MtfPlaybook(mtf_playbook::MtfPlaybookStrategy),
}

impl StrategySelector {
    pub fn new(strategy_type: StrategyType) -> Self {
        let config_str = std::fs::read_to_string("strategy_config.json").unwrap_or_else(|_| "{}".to_string());
        let config: serde_json::Value = serde_json::from_str(&config_str).unwrap_or(serde_json::json!({}));

        match strategy_type {
            StrategyType::Temporal => StrategySelector::Temporal(TemporalStrategy),
            StrategyType::Rsi => {
                let p = config["rsi"]["period"].as_u64().unwrap_or(14) as usize;
                let ob = config["rsi"]["overbought"].as_f64().unwrap_or(70.0);
                let os = config["rsi"]["oversold"].as_f64().unwrap_or(30.0);
                StrategySelector::Rsi(rsi::RsiStrategy::new(p, ob, os))
            },
            StrategyType::BollingerBands => {
                let p = config["bb"]["period"].as_u64().unwrap_or(20) as usize;
                let std_dev = config["bb"]["num_std"].as_f64().unwrap_or(2.0);
                StrategySelector::BollingerBands(bb::BollingerBandsStrategy::new(p, std_dev))
            },
            StrategyType::Macd => {
                let fast = config["macd"]["fast"].as_u64().unwrap_or(12) as usize;
                let slow = config["macd"]["slow"].as_u64().unwrap_or(26) as usize;
                let sig = config["macd"]["signal"].as_u64().unwrap_or(9) as usize;
                StrategySelector::Macd(macd::MacdStrategy::new(fast, slow, sig))
            },
            StrategyType::Lmsr => StrategySelector::Lmsr(lmsr::LmsrStrategy::default()),
            StrategyType::Ensemble => StrategySelector::Ensemble(ensemble::EnsembleStrategy::new()),
            StrategyType::LateCycle => {
                let t1 = config["late_cycle"]["threshold_1"].as_f64().unwrap_or(0.10);
                let t2 = config["late_cycle"]["threshold_2"].as_f64().unwrap_or(0.20);
                let t3 = config["late_cycle"]["threshold_3"].as_f64().unwrap_or(0.15);
                StrategySelector::LateCycle(late_cycle::LateCycleStrategy::new(t1, t2, t3))
            },
            StrategyType::MtfPlaybook => {
                let t1 = config["mtf_playbook"]["threshold_1"].as_f64().unwrap_or(0.10);
                let t2 = config["mtf_playbook"]["threshold_2"].as_f64().unwrap_or(0.20);
                StrategySelector::MtfPlaybook(mtf_playbook::MtfPlaybookStrategy::new(t1, t2))
            },
        }
    }
}

impl Strategy for StrategySelector {
    fn evaluate_regime(&mut self, time_left_sec: f64) -> String {
        match self {
            StrategySelector::Temporal(s) => s.evaluate_regime(time_left_sec),
            StrategySelector::LateCycle(_) => "LateCycle".to_string(),
            StrategySelector::MtfPlaybook(s) => s.evaluate_regime(time_left_sec),
            _ => "Quants".to_string(),
        }
    }

    fn generate_desired_state(
        &mut self,
        alpha: &AlphaOutput,
        risk: &RiskDecision,
        current_book: &BookState,
        time_left_sec: f64,
    ) -> DesiredState {
        match self {
            StrategySelector::Temporal(s) => s.generate_desired_state(alpha, risk, current_book, time_left_sec),
            StrategySelector::Rsi(s) => s.generate_desired_state(alpha, risk, current_book, time_left_sec),
            StrategySelector::BollingerBands(s) => s.generate_desired_state(alpha, risk, current_book, time_left_sec),
            StrategySelector::Macd(s) => s.generate_desired_state(alpha, risk, current_book, time_left_sec),
            StrategySelector::Lmsr(s) => s.generate_desired_state(alpha, risk, current_book, time_left_sec),
            StrategySelector::Ensemble(s) => s.generate_desired_state(alpha, risk, current_book, time_left_sec),
            StrategySelector::LateCycle(s) => s.generate_desired_state(alpha, risk, current_book, time_left_sec),
            StrategySelector::MtfPlaybook(s) => s.generate_desired_state(alpha, risk, current_book, time_left_sec),
        }
    }
}
