use bot_core::traits::Strategy;
use bot_core::types::{AlphaOutput, BookState, DesiredState, RiskDecision, Stake};

pub struct EnsembleStrategy {
    rsi_prices: Vec<f64>,
    bb_prices: Vec<f64>,
}

impl EnsembleStrategy {
    pub fn new() -> Self {
        Self { rsi_prices: Vec::new(), bb_prices: Vec::new() }
    }

    fn rsi_signal(&self) -> f64 {
        if self.rsi_prices.len() < 15 { return 0.0; }
        let mut gains = 0.0;
        let mut losses = 0.0;
        let start = self.rsi_prices.len() - 14;
        for i in start..self.rsi_prices.len() {
            let d = self.rsi_prices[i] - self.rsi_prices[i - 1];
            if d > 0.0 { gains += d; } else { losses -= d; }
        }
        let avg_gain = gains / 14.0;
        let avg_loss = losses / 14.0;
        if avg_loss == 0.0 { return 1.0; }
        let rs = avg_gain / avg_loss;
        let rsi = 100.0 - 100.0 / (1.0 + rs);
        if rsi < 30.0 { 1.0 } else if rsi > 70.0 { -1.0 } else { 0.0 }
    }

    fn bb_signal(&self) -> f64 {
        if self.bb_prices.len() < 20 { return 0.0; }
        let slice = &self.bb_prices[self.bb_prices.len() - 20..];
        let mean = slice.iter().sum::<f64>() / 20.0;
        let var = slice.iter().map(|&p| (p - mean).powi(2)).sum::<f64>() / 20.0;
        let std = var.sqrt();
        let last = *self.bb_prices.last().unwrap();
        if last < mean - 2.0 * std { 1.0 } else if last > mean + 2.0 * std { -1.0 } else { 0.0 }
    }
}

impl Strategy for EnsembleStrategy {
    fn evaluate_regime(&mut self, _time_left_sec: f64) -> String { "Ensemble".to_string() }

    fn generate_desired_state(&mut self, alpha: &AlphaOutput, risk: &RiskDecision, current_book: &BookState, _time_left_sec: f64) -> DesiredState {
        let mid = current_book.up.bid.map(|b| b.price.0).unwrap_or(0.5);
        self.rsi_prices.push(mid);
        self.bb_prices.push(mid);
        if self.rsi_prices.len() > 500 { self.rsi_prices.remove(0); }
        if self.bb_prices.len() > 500 { self.bb_prices.remove(0); }

        let score = self.rsi_signal() + self.bb_signal();
        let mut target_up = 0.0;

        if score > 0.5 && alpha.q_low.0 > 0.52 && risk.fraction > 0.0 {
            target_up = risk.fraction * risk.max_size.0;
        }

        DesiredState {
            target_position_up: Stake(target_up), target_position_down: Stake(0.0),
            maker_bid_price_up: current_book.up.bid.map(|b| b.price),
            maker_ask_price_up: None, maker_bid_price_down: None, maker_ask_price_down: None,
        }
    }
}
