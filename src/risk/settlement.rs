use crate::types::{SettlementEstimate, TickUpdate, UnixMs};

/// EWMA settlement price estimator — single source (Deriv ticks).
pub struct Settlement {
    pub current_price: f64,
    pub timestamp: UnixMs,
    pub ewma: f64,
    pub s0: Option<f64>,
    alpha: f64,
    tick_count: u64,
    last_tick_ts: Option<i64>,
}

impl Settlement {
    pub fn new() -> Self {
        Self {
            current_price: 0.0,
            timestamp: UnixMs(0),
            ewma: 0.0,
            s0: None,
            alpha: 0.05,
            tick_count: 0,
            last_tick_ts: None,
        }
    }

    /// Handle incoming tick.
    pub fn handle_tick(&mut self, tick: &TickUpdate) {
        let _old_price = self.current_price;
        self.current_price = tick.price;
        self.timestamp = UnixMs(tick.epoch * 1000);
        self.last_tick_ts = Some(tick.epoch);

        if self.tick_count == 0 {
            self.ewma = tick.price;
        } else {
            self.ewma = self.alpha * tick.price + (1.0 - self.alpha) * self.ewma;
        }
        self.tick_count += 1;
    }

    /// Capture opening price for contract window.
    pub fn capture_s0(&mut self) {
        if self.current_price > 0.0 {
            self.s0 = Some(self.current_price);
        }
    }

    /// Reset s0 for new contract window.
    pub fn reset_s0(&mut self) {
        self.s0 = None;
    }

    /// Get settlement estimate.
    pub fn estimate(&self) -> SettlementEstimate {
        let staleness = self
            .last_tick_ts
            .map(|ts| {
                let now_epoch = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs() as i64;
                (now_epoch - ts) * 1000
            })
            .unwrap_or(i64::MAX);

        SettlementEstimate {
            basis_ewma: self.ewma,
            s0: self.s0,
            s_hat_t: if self.tick_count > 0 {
                Some(self.current_price)
            } else {
                None
            },
            timestamp: self.timestamp,
            staleness_ms: staleness,
            sigma_basis: 0.003,
            confidence: if self.tick_count > 10 { 0.9 } else { 0.5 },
            s0_uncertainty: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_tick(price: f64, epoch: i64) -> TickUpdate {
        TickUpdate {
            symbol: "R_100".to_string(),
            price,
            epoch,
        }
    }

    #[test]
    fn test_basic_settlement() {
        let mut s = Settlement::new();
        s.handle_tick(&make_tick(100.0, 1));
        s.handle_tick(&make_tick(101.0, 2));
        let est = s.estimate();
        assert!(est.s_hat_t.is_some());
        assert!((est.s_hat_t.unwrap() - 101.0).abs() < 0.001);
    }

    #[test]
    fn test_capture_s0() {
        let mut s = Settlement::new();
        s.handle_tick(&make_tick(100.0, 1));
        assert!(s.s0.is_none());
        s.capture_s0();
        assert_eq!(s.s0, Some(100.0));
    }
}
