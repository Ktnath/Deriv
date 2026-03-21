use crate::types::{AlphaOutput, Prob, UnixMs};
use bot_core::ml::onnx_runner::OnnxRunner;

#[derive(Debug, Clone)]
pub enum ModelMode {
    Onnx { path: String },
    QuantOnly { reason: String },
}

/// Lognormal alpha engine — ported from Polymarket bot.
pub struct AlphaEngine {
    pub confidence: f64,
    prices: Vec<(f64, i64)>,
    sigma_basis_price: f64,
    sigma_s0_price: f64,
    ml_runner: Option<OnnxRunner>,
    model_mode: ModelMode,
}

impl AlphaEngine {
    pub fn new(
        confidence: f64,
        model_path: Option<&str>,
        allow_fallback: bool,
    ) -> anyhow::Result<Self> {
        let (ml_runner, model_mode) = match model_path.map(str::trim).filter(|s| !s.is_empty()) {
            Some(path) => match OnnxRunner::new(path) {
                Ok(runner) => {
                    tracing::info!(model_path = path, "Loaded ONNX model for live inference");
                    (
                        Some(runner),
                        ModelMode::Onnx {
                            path: path.to_string(),
                        },
                    )
                }
                Err(err) if allow_fallback => {
                    tracing::warn!(model_path = path, error = %err, "Failed to load ONNX model; continuing in quant-only fallback mode");
                    (
                        None,
                        ModelMode::QuantOnly {
                            reason: format!("model load failed: {err}"),
                        },
                    )
                }
                Err(err) => {
                    return Err(anyhow::anyhow!(
                        "failed to load ONNX model from {} and fallback is disabled: {}",
                        path,
                        err
                    ));
                }
            },
            None => {
                tracing::warn!("DERIV_MODEL_PATH not set; running in quant-only fallback mode");
                (
                    None,
                    ModelMode::QuantOnly {
                        reason: "no model path configured".to_string(),
                    },
                )
            }
        };

        Ok(Self {
            confidence,
            prices: Vec::new(),
            sigma_basis_price: 0.003,
            sigma_s0_price: 0.002,
            ml_runner,
            model_mode,
        })
    }

    pub fn model_mode(&self) -> &ModelMode {
        &self.model_mode
    }

    pub fn finalize_probability(
        &self,
        q_model: Prob,
        q_prior: Option<Prob>,
        alpha_weight: f64,
    ) -> AlphaOutput {
        match q_prior {
            Some(prior) => self.shrink_logit(q_model, prior, alpha_weight),
            None => {
                let margin = 0.02 * (1.0 - self.confidence);
                AlphaOutput {
                    q_model,
                    q_mkt: q_model,
                    q_final: q_model,
                    q_low: Prob((q_model.0 - margin).max(0.0)),
                    q_high: Prob((q_model.0 + margin).min(1.0)),
                    confidence: self.confidence,
                }
            }
        }
    }

    /// Record a new spot price.
    pub fn update_spot_return(&mut self, price: f64, ts: UnixMs) {
        self.prices.push((price, ts.0));
        if self.prices.len() > 1000 {
            self.prices.drain(..self.prices.len() - 500);
        }
    }

    /// Calculate q_model: probability that price ends above s0.
    pub fn calculate_q_model(&self, s0: f64, s_hat: f64, time_left_sec: f64) -> Prob {
        if s0 <= 0.0 || s_hat <= 0.0 || time_left_sec <= 0.0 {
            return Prob(0.5);
        }
        if let Some(ref runner) = self.ml_runner {
            if self.prices.len() >= 50 {
                let n = self.prices.len();
                let p = |i: usize| self.prices[n - 1 - i].0;
                let ret = (p(0) - p(1)) / p(1);
                let sma_10: f64 = (0..10).map(|i| p(i)).sum::<f64>() / 10.0;
                let sma_30: f64 = (0..30).map(|i| p(i)).sum::<f64>() / 30.0;
                let dist_sma10 = (p(0) - sma_10) / sma_10;
                let dist_sma30 = (p(0) - sma_30) / sma_30;
                let rsi_14 = {
                    let mut gains = 0.0;
                    let mut losses = 0.0;
                    for i in 0..14 {
                        let diff = p(i) - p(i + 1);
                        if diff > 0.0 {
                            gains += diff;
                        } else {
                            losses -= diff;
                        }
                    }
                    if losses == 0.0 {
                        100.0
                    } else {
                        let rs = (gains / 14.0) / (losses / 14.0);
                        100.0 - (100.0 / (1.0 + rs))
                    }
                };
                let sma_20: f64 = (0..20).map(|i| p(i)).sum::<f64>() / 20.0;
                let var_20: f64 = (0..20).map(|i| (p(i) - sma_20).powi(2)).sum::<f64>() / 20.0;
                let std_20 = var_20.sqrt();
                let bb_upper = sma_20 + 2.0 * std_20;
                let bb_lower = sma_20 - 2.0 * std_20;
                let bb_position = (p(0) - bb_lower) / (bb_upper - bb_lower).max(1e-9);
                let bb_width = (bb_upper - bb_lower) / sma_20.max(1e-9);
                let vol_20 = {
                    let rets: Vec<f64> = (0..20).map(|i| (p(i) - p(i + 1)) / p(i + 1)).collect();
                    let mean: f64 = rets.iter().sum::<f64>() / 20.0;
                    (rets.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / 20.0).sqrt()
                };
                let fractal_efficiency = {
                    let net_change = (p(0) - p(10)).abs();
                    let sum_abs_changes: f64 = (0..10).map(|i| (p(i) - p(i + 1)).abs()).sum();
                    net_change / sum_abs_changes.max(1e-9)
                };
                let mom_accel = (p(0) - p(5)) - (p(0) - p(20));
                let vol_heat = {
                    let std_10 =
                        ((0..10).map(|i| (p(i) - sma_10).powi(2)).sum::<f64>() / 10.0).sqrt();
                    let sma_50: f64 = (0..50).map(|i| p(i)).sum::<f64>() / 50.0;
                    let std_50 =
                        ((0..50).map(|i| (p(i) - sma_50).powi(2)).sum::<f64>() / 50.0).sqrt();
                    std_10 / std_50.max(1e-9)
                };
                let features = [
                    ret as f32,
                    dist_sma10 as f32,
                    dist_sma30 as f32,
                    rsi_14 as f32,
                    bb_position as f32,
                    bb_width as f32,
                    vol_20 as f32,
                    fractal_efficiency as f32,
                    mom_accel as f32,
                    vol_heat as f32,
                ];
                if let Ok(prob) = runner.predict(&features) {
                    return Prob(prob.0);
                }
            }
        }
        let sigma = self.sigma_basis_price + self.sigma_s0_price;
        let sigma_t = sigma * (time_left_sec / 300.0).sqrt();
        if sigma_t < 1e-12 {
            return Prob(0.5);
        }
        let z = (s_hat / s0).ln() / sigma_t;
        Prob(normal_cdf(z))
    }

    pub fn shrink_logit(&self, q_model: Prob, q_mkt: Prob, alpha_weight: f64) -> AlphaOutput {
        let logit_model = logit(q_model.0.clamp(0.001, 0.999));
        let logit_mkt = logit(q_mkt.0.clamp(0.001, 0.999));
        let blended = alpha_weight * logit_model + (1.0 - alpha_weight) * logit_mkt;
        let q_final = inv_logit(blended);
        let margin = 0.02 * (1.0 - self.confidence);
        AlphaOutput {
            q_model,
            q_mkt,
            q_final: Prob(q_final),
            q_low: Prob((q_final - margin).max(0.0)),
            q_high: Prob((q_final + margin).min(1.0)),
            confidence: self.confidence,
        }
    }
}

fn logit(p: f64) -> f64 {
    (p / (1.0 - p)).ln()
}
fn inv_logit(x: f64) -> f64 {
    1.0 / (1.0 + (-x).exp())
}
fn normal_cdf(x: f64) -> f64 {
    0.5 * (1.0 + erf(x / std::f64::consts::SQRT_2))
}
fn erf(x: f64) -> f64 {
    let t = 1.0 / (1.0 + 0.3275911 * x.abs());
    let poly = t
        * (0.254829592
            + t * (-0.284496736 + t * (1.421413741 + t * (-1.453152027 + t * 1.061405429))));
    let result = 1.0 - poly * (-x * x).exp();
    if x >= 0.0 {
        result
    } else {
        -result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_q_model_at_expiry() {
        let engine = AlphaEngine::new(0.55, None, true).unwrap();
        let q = engine.calculate_q_model(100.0, 100.0, 0.001);
        assert!((q.0 - 0.5).abs() < 0.1);
    }

    #[test]
    fn test_model_mode_without_path_uses_fallback() {
        let engine = AlphaEngine::new(0.55, None, true).unwrap();
        assert!(matches!(engine.model_mode(), ModelMode::QuantOnly { .. }));
    }

    #[test]
    fn test_model_load_failure_respects_fallback_flag() {
        let ok = AlphaEngine::new(0.55, Some("/definitely/missing/model.onnx"), true).unwrap();
        assert!(matches!(ok.model_mode(), ModelMode::QuantOnly { .. }));
        assert!(AlphaEngine::new(0.55, Some("/definitely/missing/model.onnx"), false).is_err());
    }

    #[test]
    fn test_finalize_probability_model_only_passthrough() {
        let engine = AlphaEngine::new(0.55, None, true).unwrap();
        let out = engine.finalize_probability(Prob(0.61), None, 0.55);
        assert!((out.q_final.0 - 0.61).abs() < f64::EPSILON);
        assert!((out.q_mkt.0 - 0.61).abs() < f64::EPSILON);
    }

    #[test]
    fn test_normal_cdf_symmetry() {
        assert!((normal_cdf(0.0) - 0.5).abs() < 0.001);
        assert!((normal_cdf(1.0) + normal_cdf(-1.0) - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_shrink_logit_midpoint() {
        let engine = AlphaEngine::new(0.55, None, true).unwrap();
        let out = engine.shrink_logit(Prob(0.5), Prob(0.5), 0.55);
        assert!((out.q_final.0 - 0.5).abs() < 0.01);
    }
}
