use bot_core::traits::Strategy;
use bot_core::types::{AlphaOutput, BookState, DesiredState, RiskDecision, Stake};

pub struct LateCycleStrategy {
    pub min_roi: f64,
    pub max_roi: f64,
    pub edge_threshold: f64,
}

impl LateCycleStrategy {
    pub fn new(min_roi: f64, max_roi: f64, edge_threshold: f64) -> Self {
        Self { min_roi, max_roi, edge_threshold }
    }

    fn calculate_roi(price: f64) -> f64 {
        if price <= 0.0 || price >= 1.0 { return 0.0; }
        (1.0 - price) / price
    }
}

impl Strategy for LateCycleStrategy {
    fn evaluate_regime(&mut self, _time_left_sec: f64) -> String { "LateCycle".to_string() }

    fn generate_desired_state(&mut self, alpha: &AlphaOutput, risk: &RiskDecision, current_book: &BookState, time_left_sec: f64) -> DesiredState {
        let mut target_up = 0.0;
        let mut target_down = 0.0;
        let mut maker_bid_up = None;
        let mut maker_bid_down = None;

        if time_left_sec < 60.0 && time_left_sec > 1.0 {
            if alpha.q_low.0 > 0.5 + self.edge_threshold {
                if let Some(bid) = current_book.up.bid {
                    let roi = Self::calculate_roi(bid.price.0);
                    if roi >= self.min_roi && roi <= self.max_roi && risk.fraction > 0.0 {
                        target_up = risk.fraction * risk.max_size.0;
                        maker_bid_up = Some(bid.price);
                    }
                }
            }
            if alpha.q_low.0 < 0.5 - self.edge_threshold {
                if let Some(bid) = current_book.down.bid {
                    let roi = Self::calculate_roi(bid.price.0);
                    if roi >= self.min_roi && roi <= self.max_roi && risk.fraction > 0.0 {
                        target_down = risk.fraction * risk.max_size.0;
                        maker_bid_down = Some(bid.price);
                    }
                }
            }
        }

        DesiredState {
            target_position_up: Stake(target_up), target_position_down: Stake(target_down),
            maker_bid_price_up: maker_bid_up, maker_ask_price_up: None,
            maker_bid_price_down: maker_bid_down, maker_ask_price_down: None,
        }
    }
}
