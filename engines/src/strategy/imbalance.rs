use bot_core::traits::Strategy;
use bot_core::types::{AlphaOutput, BookState, DesiredState, RiskDecision, Stake};

pub struct ImbalanceStrategy {
    pub threshold: f64,
}

impl ImbalanceStrategy {
    pub fn new(threshold: f64) -> Self { Self { threshold } }
}

impl Strategy for ImbalanceStrategy {
    fn evaluate_regime(&mut self, _time_left_sec: f64) -> String { "Imbalance".to_string() }

    fn generate_desired_state(&mut self, alpha: &AlphaOutput, risk: &RiskDecision, current_book: &BookState, _time_left_sec: f64) -> DesiredState {
        let mut target_up = 0.0;
        if alpha.q_low.0 > 0.5 + self.threshold && risk.fraction > 0.0 {
            target_up = risk.fraction * risk.max_size.0;
        }
        DesiredState {
            target_position_up: Stake(target_up), target_position_down: Stake(0.0),
            maker_bid_price_up: current_book.up.bid.map(|b| b.price),
            maker_ask_price_up: None, maker_bid_price_down: None, maker_ask_price_down: None,
        }
    }
}
