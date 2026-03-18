use bot_core::traits::Strategy;
use bot_core::types::{AlphaOutput, BookState, DesiredState, RiskDecision, Stake};

pub struct MacdStrategy {
    pub fast: usize,
    pub slow: usize,
    pub signal: usize,
    prices: Vec<f64>,
}

impl MacdStrategy {
    pub fn new(fast: usize, slow: usize, signal: usize) -> Self {
        Self { fast, slow, signal, prices: Vec::with_capacity(slow + signal) }
    }

    fn ema(data: &[f64], period: usize) -> f64 {
        if data.is_empty() { return 0.0; }
        let alpha = 2.0 / (period as f64 + 1.0);
        let mut ema = data[0];
        for &p in data.iter().skip(1) {
            ema = p * alpha + ema * (1.0 - alpha);
        }
        ema
    }

    fn calculate_macd(&self) -> Option<(f64, f64)> {
        if self.prices.len() < self.slow + self.signal { return None; }
        let fast_ema = Self::ema(&self.prices, self.fast);
        let slow_ema = Self::ema(&self.prices, self.slow);
        let macd_line = fast_ema - slow_ema;

        let mut macd_hist = Vec::new();
        for i in 0..self.prices.len() {
            let slice = &self.prices[..=i];
            let f = Self::ema(slice, self.fast);
            let s = Self::ema(slice, self.slow);
            macd_hist.push(f - s);
        }
        let signal = Self::ema(&macd_hist, self.signal);
        Some((macd_line, signal))
    }
}

impl Strategy for MacdStrategy {
    fn evaluate_regime(&mut self, _time_left_sec: f64) -> String {
        match self.calculate_macd() {
            Some((macd, signal)) if macd > signal => "Bullish".to_string(),
            Some(_) => "Bearish".to_string(),
            None => "Warmup".to_string(),
        }
    }

    fn generate_desired_state(&mut self, alpha: &AlphaOutput, risk: &RiskDecision, current_book: &BookState, _time_left_sec: f64) -> DesiredState {
        let mid = current_book.up.bid.map(|b| b.price.0).unwrap_or(0.5);
        self.prices.push(mid);
        if self.prices.len() > 500 { self.prices.remove(0); }

        let mut target_up = 0.0;

        if let Some((macd, signal)) = self.calculate_macd() {
            if macd > signal && alpha.q_low.0 > 0.52 && risk.fraction > 0.0 {
                target_up = risk.fraction * risk.max_size.0;
            }
        }

        DesiredState {
            target_position_up: Stake(target_up),
            target_position_down: Stake(0.0),
            maker_bid_price_up: current_book.up.bid.map(|b| b.price),
            maker_ask_price_up: None,
            maker_bid_price_down: None,
            maker_ask_price_down: None,
        }
    }
}
