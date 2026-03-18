use bot_core::traits::Strategy;
use bot_core::types::{AlphaOutput, BookState, DesiredState, RiskDecision, Stake};

pub struct BreakoutStrategy {
    pub lookback: usize,
    pub threshold: f64,
    highs: Vec<f64>,
    lows: Vec<f64>,
}

impl BreakoutStrategy {
    pub fn new(lookback: usize, threshold: f64) -> Self {
        Self { lookback, threshold, highs: Vec::new(), lows: Vec::new() }
    }
}

impl Strategy for BreakoutStrategy {
    fn evaluate_regime(&mut self, _time_left_sec: f64) -> String { "Breakout".to_string() }

    fn generate_desired_state(&mut self, alpha: &AlphaOutput, risk: &RiskDecision, current_book: &BookState, _time_left_sec: f64) -> DesiredState {
        let mid = current_book.up.bid.map(|b| b.price.0).unwrap_or(0.5);
        self.highs.push(mid);
        self.lows.push(mid);
        if self.highs.len() > self.lookback { self.highs.remove(0); }
        if self.lows.len() > self.lookback { self.lows.remove(0); }

        let mut target_up = 0.0;
        if self.highs.len() >= self.lookback {
            let max = self.highs.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
            if mid > max - self.threshold && alpha.q_low.0 > 0.52 && risk.fraction > 0.0 {
                target_up = risk.fraction * risk.max_size.0;
            }
        }

        DesiredState {
            target_position_up: Stake(target_up), target_position_down: Stake(0.0),
            maker_bid_price_up: current_book.up.bid.map(|b| b.price),
            maker_ask_price_up: None, maker_bid_price_down: None, maker_ask_price_down: None,
        }
    }
}
