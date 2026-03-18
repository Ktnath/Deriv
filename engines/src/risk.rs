use bot_core::traits::RiskEngine;
use bot_core::types::{BotError, Price, Prob, RiskDecision, Usd};

/// Fractional Kelly risk engine (ported from Polymarket bot).
pub struct FractionalKellyRiskEngine {
    pub max_fraction: f64,
    pub global_cap: Usd,
    pub max_loss: Usd,
    /// Deriv minimum stake per contract (typically $0.35).
    pub min_stake: Usd,
}

impl RiskEngine for FractionalKellyRiskEngine {
    fn size_fractional_kelly(&self, q_low: Prob, price: Price, _kelly_fraction: f64) -> RiskDecision {
        if q_low.0 <= price.0 || price.0 >= 1.0 {
            return RiskDecision { fraction: 0.0, max_size: self.global_cap };
        }

        // Fixed 7.5% stake per contract (matching Polymarket config)
        let f = 0.075;

        // Ensure stake meets Deriv minimum
        let raw_stake = f * self.global_cap.0;
        let clamped_stake = raw_stake.max(self.min_stake.0);

        RiskDecision {
            fraction: clamped_stake / self.global_cap.0,
            max_size: self.global_cap,
        }
    }

    fn check_breakers(&self) -> Result<(), BotError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_edge_no_trade() {
        let risk = FractionalKellyRiskEngine {
            max_fraction: 0.15,
            global_cap: Usd(100.0),
            max_loss: Usd(50.0),
            min_stake: Usd(0.35),
        };
        let dec = risk.size_fractional_kelly(Prob(0.45), Price(0.50), 0.5);
        assert_eq!(dec.fraction, 0.0);
    }

    #[test]
    fn test_positive_edge() {
        let risk = FractionalKellyRiskEngine {
            max_fraction: 0.15,
            global_cap: Usd(100.0),
            max_loss: Usd(50.0),
            min_stake: Usd(0.35),
        };
        let dec = risk.size_fractional_kelly(Prob(0.60), Price(0.45), 0.5);
        assert!(dec.fraction > 0.0);
    }

    #[test]
    fn test_min_stake_enforcement() {
        let risk = FractionalKellyRiskEngine {
            max_fraction: 0.15,
            global_cap: Usd(4.0), // Small balance → raw stake = 0.30 < min 0.35
            max_loss: Usd(2.0),
            min_stake: Usd(0.35),
        };
        let dec = risk.size_fractional_kelly(Prob(0.60), Price(0.45), 0.5);
        let actual_stake = dec.fraction * dec.max_size.0;
        assert!(actual_stake >= 0.35);
    }
}
