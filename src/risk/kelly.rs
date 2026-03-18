use crate::types::{Price, Prob, RiskDecision, Usd};

/// Fractional Kelly risk engine.
pub struct KellyRisk {
    pub max_fraction: f64,
    pub global_cap: f64,
    pub max_loss: f64,
    pub min_stake: f64,
}

impl KellyRisk {
    pub fn size(&self, q_low: Prob, price: Price, kelly_fraction: f64) -> RiskDecision {
        let p = q_low.0;
        let b = if price.0 > 0.0 {
            (1.0 / price.0) - 1.0
        } else {
            0.0
        };
        let q = 1.0 - p;
        let kelly = if b > 0.0 { (p * b - q) / b } else { 0.0 };
        let fraction = (kelly * kelly_fraction).clamp(0.0, self.max_fraction);
        let size = (fraction * self.global_cap).min(self.max_loss);
        let final_size = if size < self.min_stake { 0.0 } else { size };
        RiskDecision {
            fraction,
            max_size: Usd(final_size),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_edge() {
        let risk = KellyRisk {
            max_fraction: 0.15,
            global_cap: 1000.0,
            max_loss: 500.0,
            min_stake: 0.35,
        };
        let dec = risk.size(Prob(0.5), Price(0.5), 0.5);
        assert!(dec.max_size.0 < 0.01);
    }

    #[test]
    fn test_positive_edge() {
        let risk = KellyRisk {
            max_fraction: 0.15,
            global_cap: 1000.0,
            max_loss: 500.0,
            min_stake: 0.35,
        };
        let dec = risk.size(Prob(0.6), Price(0.5), 0.5);
        assert!(dec.max_size.0 > 0.0);
    }

    #[test]
    fn test_min_stake_enforcement() {
        let risk = KellyRisk {
            max_fraction: 0.15,
            global_cap: 10.0,
            max_loss: 5.0,
            min_stake: 0.35,
        };
        let dec = risk.size(Prob(0.51), Price(0.5), 0.5);
        assert!(dec.max_size.0 == 0.0 || dec.max_size.0 >= 0.35);
    }
}
