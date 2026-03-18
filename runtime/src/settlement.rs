use bot_core::traits::SettlementPriceEstimator;
use bot_core::types::{SettlementEstimate, TickUpdate, UnixMs};
use std::collections::VecDeque;

/// EWMA-based settlement price estimator, simplified for Deriv.
/// Uses a single tick source instead of Polymarket's dual Binance/Chainlink feeds.
pub struct EWMAEstimator {
    pub current_price: f64,
    pub price_ewma: f64,
    pub price_var_ewma: f64,
    pub s0: Option<f64>,
    pub timestamp: UnixMs,
    pub s0_uncertainty: Option<f64>,
    price_buffer: VecDeque<(f64, i64)>,
    last_tick_ts: Option<i64>,
}

impl EWMAEstimator {
    pub fn new() -> Self {
        Self {
            current_price: 0.0,
            price_ewma: 0.0,
            price_var_ewma: 0.0,
            s0: None,
            timestamp: UnixMs(0),
            s0_uncertainty: None,
            price_buffer: VecDeque::new(),
            last_tick_ts: None,
        }
    }

    /// Capture the opening price (s0) from the tick buffer.
    pub fn capture_s0(&mut self, target_t0: UnixMs) {
        if self.s0.is_some() { return; }

        let mut best_sample = None;
        let mut min_diff = i64::MAX;
        let mut before = None;
        let mut after = None;

        for &(price, ts) in &self.price_buffer {
            let diff = (ts - target_t0.0).abs();
            if diff < min_diff {
                min_diff = diff;
                best_sample = Some(price);
            }
            if ts <= target_t0.0 {
                before = Some(price);
            }
            if ts >= target_t0.0 && after.is_none() {
                after = Some(price);
            }
        }

        self.s0 = best_sample;
        if let (Some(b), Some(a)) = (before, after) {
            self.s0_uncertainty = Some((a - b).abs());
            println!("DEB: Captured s0 at {}: val={:.4}, uncertainty={:.6}",
                target_t0.0, best_sample.unwrap_or(0.0), self.s0_uncertainty.unwrap());
        }
    }

    /// Process a tick update from the Deriv WebSocket.
    pub fn handle_update(&mut self, update: TickUpdate) {
        self.update_tick(update.price, UnixMs(update.epoch * 1000));
    }
}

impl Default for EWMAEstimator {
    fn default() -> Self {
        Self::new()
    }
}

impl SettlementPriceEstimator for EWMAEstimator {
    fn update_tick(&mut self, price: f64, timestamp: UnixMs) {
        let _old_price = self.current_price;
        self.current_price = price;
        self.timestamp = timestamp;
        self.last_tick_ts = Some(timestamp.0);

        self.price_buffer.push_back((price, timestamp.0));
        while self.price_buffer.len() > 200 {
            self.price_buffer.pop_front();
        }

        // EWMA update
        if self.price_ewma == 0.0 {
            self.price_ewma = price;
            self.price_var_ewma = 0.0;
        } else {
            let diff = price - self.price_ewma;
            self.price_ewma = 0.1 * price + 0.9 * self.price_ewma;
            self.price_var_ewma = 0.1 * (diff * diff) + 0.9 * self.price_var_ewma;
        }
    }

    fn estimate(&self, now_ms: UnixMs) -> SettlementEstimate {
        let s_hat = if self.current_price > 0.0 {
            Some(self.current_price)
        } else {
            None
        };

        let staleness_ms = if let Some(last_ts) = self.last_tick_ts {
            now_ms.0 - last_ts
        } else {
            i64::MAX
        };

        let sigma = self.price_var_ewma.sqrt();
        let confidence = if staleness_ms > 10_000 { 0.0 }
            else if staleness_ms > 3_000 { 0.5 }
            else { 1.0 };

        SettlementEstimate {
            basis_ewma: self.price_ewma,
            s0: self.s0,
            s_hat_t: s_hat,
            timestamp: self.timestamp,
            staleness_ms: staleness_ms.max(0),
            sigma_basis: sigma,
            confidence,
            s0_uncertainty: self.s0_uncertainty,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ewma_estimator_basic() {
        let mut est = EWMAEstimator::new();
        est.update_tick(100.0, UnixMs(1000));
        est.update_tick(101.0, UnixMs(2000));
        est.update_tick(99.5, UnixMs(3000));

        let e = est.estimate(UnixMs(3500));
        assert!(e.s_hat_t.is_some());
        assert!((e.s_hat_t.unwrap() - 99.5).abs() < 0.001);
        assert!(e.staleness_ms >= 0);
    }

    #[test]
    fn test_capture_s0() {
        let mut est = EWMAEstimator::new();
        est.update_tick(100.0, UnixMs(900));
        est.update_tick(100.5, UnixMs(1100));
        est.capture_s0(UnixMs(1000));
        assert!(est.s0.is_some());
    }
}
