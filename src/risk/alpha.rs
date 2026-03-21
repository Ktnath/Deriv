use crate::types::{AlphaOutput, Prob, UnixMs};
use bot_core::ml::onnx_runner::OnnxRunner;

#[derive(Debug, Clone)]
pub enum ModelMode {
    Onnx { path: String },
    QuantOnly { reason: String },
}

#[derive(Debug, Clone, Copy)]
pub struct BlendConfig {
    pub model_weight: f64,
    pub prior_weight: f64,
    pub confidence_bias: f64,
}

impl Default for BlendConfig {
    fn default() -> Self {
        Self {
            model_weight: 0.64,
            prior_weight: 0.36,
            confidence_bias: 0.25,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct BlendInputs {
    pub model_probability: Prob,
    pub process_prior: Option<Prob>,
    pub confidence_multiplier: Option<f64>,
    pub config: BlendConfig,
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

    pub fn finalize_probability(&self, inputs: BlendInputs) -> AlphaOutput {
        let prior = inputs.process_prior.unwrap_or(inputs.model_probability);
        let confidence_multiplier = inputs.confidence_multiplier.unwrap_or(1.0).clamp(0.5, 1.5);
        let model_weight = (inputs.config.model_weight * confidence_multiplier).clamp(0.05, 0.95);
        let prior_weight = inputs.config.prior_weight.clamp(0.05, 0.95);
        let total = model_weight + prior_weight;
        let normalized_model = model_weight / total;
        let normalized_prior = prior_weight / total;
        let logit_model = logit(inputs.model_probability.0.clamp(0.001, 0.999));
        let logit_prior = logit(prior.0.clamp(0.001, 0.999));
        let base_logit = normalized_model * logit_model + normalized_prior * logit_prior;
        let confidence_push = (confidence_multiplier - 1.0) * inputs.config.confidence_bias;
        let q_final = inv_logit(base_logit + confidence_push);
        let margin = 0.02 * (1.0 - self.confidence) / confidence_multiplier.max(0.75);
        AlphaOutput {
            q_model: inputs.model_probability,
            q_mkt: prior,
            q_final: Prob(q_final),
            q_low: Prob((q_final - margin).max(0.0)),
            q_high: Prob((q_final + margin).min(1.0)),
            confidence: confidence_multiplier,
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
    fn blend_respects_prior_and_confidence() {
        let engine = AlphaEngine::new(0.55, None, true).unwrap();
        let neutral = engine.finalize_probability(BlendInputs {
            model_probability: Prob(0.60),
            process_prior: Some(Prob(0.52)),
            confidence_multiplier: Some(1.0),
            config: BlendConfig::default(),
        });
        let more_confident = engine.finalize_probability(BlendInputs {
            model_probability: Prob(0.60),
            process_prior: Some(Prob(0.52)),
            confidence_multiplier: Some(1.25),
            config: BlendConfig::default(),
        });
        assert!(neutral.q_final.0 > 0.52 && neutral.q_final.0 < 0.60);
        assert!(more_confident.q_final.0 > neutral.q_final.0);
    }
}
