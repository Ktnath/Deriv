use bot_core::traits::AlphaEngine;
use bot_core::types::{AlphaOutput, Prob, UnixMs};

/// Lognormal alpha engine (ported from Polymarket bot).
/// Estimates P(S_T >= S_0) using a lognormal model of price evolution.
pub struct LognormalAlphaEngine {
    pub weight: f64,
    pub spot_var_per_sec: f64,
    last_spot: Option<(f64, i64)>,
}

impl LognormalAlphaEngine {
    pub fn new(weight: f64) -> Self {
        Self { weight, spot_var_per_sec: 1e-8, last_spot: None }
    }

    /// Approximation of standard normal CDF (Abramowitz & Stegun).
    fn normal_cdf(x: f64) -> f64 {
        let a1 = 0.254829592;
        let a2 = -0.284496736;
        let a3 = 1.421413741;
        let a4 = -1.453152027;
        let a5 = 1.061405429;
        let p = 0.3275911;

        let sign = if x < 0.0 { -1.0 } else { 1.0 };
        let x_abs = x.abs() / std::f64::consts::SQRT_2;

        let t = 1.0 / (1.0 + p * x_abs);
        let y = 1.0 - (((((a5 * t + a4) * t) + a3) * t + a2) * t + a1) * t * (-x_abs * x_abs).exp();

        0.5 * (1.0 + sign * y)
    }

    fn logit(p: f64) -> f64 {
        let p = p.clamp(1e-9, 1.0 - 1e-9);
        (p / (1.0 - p)).ln()
    }

    fn inv_logit(l: f64) -> f64 {
        1.0 / (1.0 + (-l).exp())
    }
}

impl AlphaEngine for LognormalAlphaEngine {
    fn calculate_q_model(&mut self, s0: f64, s_hat_t: f64, time_left_sec: f64) -> Prob {
        if time_left_sec <= 0.0 {
            return Prob(if s_hat_t >= s0 { 1.0 } else { 0.0 });
        }

        let delta = time_left_sec;
        let mu = 0.0;

        // Adapted uncertainties for Deriv synthetics (smaller absolute values)
        // Volatility indices tick around 0.01-1.0 range, not $60k like BTC
        let sigma_basis_price = 0.05;
        let sigma_s0_price = 0.02;

        let var_log_basis = (sigma_basis_price / s_hat_t).powi(2);
        let var_log_s0 = (sigma_s0_price / s_hat_t).powi(2);
        let var_spot_h = self.spot_var_per_sec * delta;

        let total_variance = var_spot_h + var_log_basis + var_log_s0;
        let sigma_eff = total_variance.sqrt();

        if sigma_eff == 0.0 {
            return Prob(0.5);
        }

        let x = ((s0 / s_hat_t).ln() - mu * delta) / sigma_eff;
        Prob(1.0 - Self::normal_cdf(x))
    }

    fn shrink_logit(&self, q_model: Prob, q_mkt: Prob, weight: f64) -> AlphaOutput {
        let l_model = Self::logit(q_model.0);
        let l_mkt = Self::logit(q_mkt.0);

        let l_final = weight * l_model + (1.0 - weight) * l_mkt;
        let q_final = Self::inv_logit(l_final);

        let logit_spread = 0.5;
        let q_low = Self::inv_logit(l_final - logit_spread);
        let q_high = Self::inv_logit(l_final + logit_spread);

        AlphaOutput {
            q_model,
            q_mkt,
            q_final: Prob(q_final),
            q_low: Prob(q_low),
            q_high: Prob(q_high),
            confidence: 1.0,
        }
    }

    fn update_spot_return(&mut self, price: f64, timestamp: UnixMs) {
        if let Some((last_price, last_time)) = self.last_spot {
            let dt = (timestamp.0 - last_time) as f64 / 1000.0;
            if dt > 0.0 {
                let log_ret = (price / last_price).ln();
                let inst_var_per_sec = (log_ret * log_ret) / dt;
                self.spot_var_per_sec = 0.05 * inst_var_per_sec + 0.95 * self.spot_var_per_sec;
            }
        }
        self.last_spot = Some((price, timestamp.0));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normal_cdf_symmetry() {
        let cdf_0 = LognormalAlphaEngine::normal_cdf(0.0);
        assert!((cdf_0 - 0.5).abs() < 1e-6);
    }

    #[test]
    fn test_q_model_at_expiry() {
        let mut eng = LognormalAlphaEngine::new(0.5);
        let q = eng.calculate_q_model(100.0, 101.0, 0.0);
        assert_eq!(q.0, 1.0); // s_hat >= s0
        let q2 = eng.calculate_q_model(101.0, 100.0, 0.0);
        assert_eq!(q2.0, 0.0); // s_hat < s0
    }

    #[test]
    fn test_shrink_logit_midpoint() {
        let eng = LognormalAlphaEngine::new(0.5);
        let out = eng.shrink_logit(Prob(0.5), Prob(0.5), 0.5);
        assert!((out.q_final.0 - 0.5).abs() < 1e-6);
    }
}
