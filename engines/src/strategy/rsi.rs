use bot_core::traits::Strategy;
use bot_core::types::{AlphaOutput, BookState, DesiredState, RiskDecision, Stake};

pub struct RsiStrategy {
    pub period: usize,
    pub overbought: f64,
    pub oversold: f64,
    prices: Vec<f64>,
}

impl RsiStrategy {
    pub fn new(period: usize, overbought: f64, oversold: f64) -> Self {
        Self { period, overbought, oversold, prices: Vec::with_capacity(period + 1) }
    }

    fn calculate_rsi(&self) -> Option<f64> {
        if self.prices.len() <= self.period { return None; }
        let mut gains = 0.0;
        let mut losses = 0.0;
        let start = self.prices.len() - self.period;
        for i in start..self.prices.len() {
            let diff = self.prices[i] - self.prices[i - 1];
            if diff > 0.0 { gains += diff; } else { losses -= diff; }
        }
        let avg_gain = gains / self.period as f64;
        let avg_loss = losses / self.period as f64;
        if avg_loss == 0.0 { return Some(100.0); }
        let rs = avg_gain / avg_loss;
        Some(100.0 - 100.0 / (1.0 + rs))
    }
}

impl Strategy for RsiStrategy {
    fn evaluate_regime(&mut self, _time_left_sec: f64) -> String {
        match self.calculate_rsi() {
            Some(rsi) if rsi > self.overbought => "Overbought".to_string(),
            Some(rsi) if rsi < self.oversold => "Oversold".to_string(),
            _ => "Neutral".to_string(),
        }
    }

    fn generate_desired_state(&mut self, alpha: &AlphaOutput, risk: &RiskDecision, current_book: &BookState, _time_left_sec: f64) -> DesiredState {
        let mid = current_book.up.bid.map(|b| b.price.0).unwrap_or(0.5);
        self.prices.push(mid);
        if self.prices.len() > 500 { self.prices.remove(0); }

        let mut target_up = 0.0;
        let mut maker_bid_up = None;

        if let Some(rsi) = self.calculate_rsi() {
            if rsi < self.oversold && alpha.q_low.0 > 0.52 && risk.fraction > 0.0 {
                target_up = risk.fraction * risk.max_size.0;
                maker_bid_up = current_book.up.bid.map(|b| b.price);
            }
        }

        DesiredState {
            target_position_up: Stake(target_up),
            target_position_down: Stake(0.0),
            maker_bid_price_up: maker_bid_up,
            maker_ask_price_up: None,
            maker_bid_price_down: None,
            maker_ask_price_down: None,
        }
    }
}
