use bot_core::traits::Strategy;
use bot_core::types::{AlphaOutput, BookState, DesiredState, RiskDecision, Stake};

pub struct BollingerBandsStrategy {
    pub period: usize,
    pub std_mult: f64,
    prices: Vec<f64>,
}

impl BollingerBandsStrategy {
    pub fn new(period: usize, std_mult: f64) -> Self {
        Self { period, std_mult, prices: Vec::with_capacity(period) }
    }

    fn calculate_bands(&self) -> Option<(f64, f64, f64)> {
        if self.prices.len() < self.period { return None; }
        let slice = &self.prices[self.prices.len() - self.period..];
        let mean = slice.iter().sum::<f64>() / self.period as f64;
        let var = slice.iter().map(|&p| (p - mean).powi(2)).sum::<f64>() / self.period as f64;
        let std = var.sqrt();
        Some((mean - self.std_mult * std, mean, mean + self.std_mult * std))
    }
}

impl Strategy for BollingerBandsStrategy {
    fn evaluate_regime(&mut self, _time_left_sec: f64) -> String {
        if let Some((lower, _mid, upper)) = self.calculate_bands() {
            let last = *self.prices.last().unwrap_or(&0.5);
            if last < lower { "Below-Lower".to_string() }
            else if last > upper { "Above-Upper".to_string() }
            else { "In-Band".to_string() }
        } else {
            "Warmup".to_string()
        }
    }

    fn generate_desired_state(&mut self, alpha: &AlphaOutput, risk: &RiskDecision, current_book: &BookState, _time_left_sec: f64) -> DesiredState {
        let mid = current_book.up.bid.map(|b| b.price.0).unwrap_or(0.5);
        self.prices.push(mid);
        if self.prices.len() > 500 { self.prices.remove(0); }

        let mut target_up = 0.0;

        if let Some((lower, _mid, _upper)) = self.calculate_bands() {
            if mid < lower && alpha.q_low.0 > 0.52 && risk.fraction > 0.0 {
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
