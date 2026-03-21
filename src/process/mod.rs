use crate::types::{ContractType, Prob, UnixMs};

const MIN_RET_EPS: f64 = 1e-9;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Regime {
    Calm,
    Transitional,
    Expansion,
}

impl Regime {
    pub fn as_str(self) -> &'static str {
        match self {
            Regime::Calm => "calm",
            Regime::Transitional => "transitional",
            Regime::Expansion => "expansion",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProcessFeatures {
    pub sample_size: usize,
    pub last_return: f64,
    pub short_drift: f64,
    pub long_drift: f64,
    pub drift_gap: f64,
    pub run_length: usize,
    pub sign_persistence: f64,
    pub variance_ratio: f64,
    pub return_clustering: f64,
    pub direction_concentration: f64,
    pub shock_frequency: f64,
    pub time_since_large_move: usize,
    pub move_zscore: f64,
    pub transition_instability: f64,
}

impl ProcessFeatures {
    pub fn neutral() -> Self {
        Self {
            sample_size: 0,
            last_return: 0.0,
            short_drift: 0.0,
            long_drift: 0.0,
            drift_gap: 0.0,
            run_length: 0,
            sign_persistence: 0.0,
            variance_ratio: 1.0,
            return_clustering: 0.0,
            direction_concentration: 0.0,
            shock_frequency: 0.0,
            time_since_large_move: 0,
            move_zscore: 0.0,
            transition_instability: 0.0,
        }
    }

    pub fn directional_bias(&self) -> f64 {
        let persistence_component = self.sign_persistence * self.run_length as f64 / 8.0;
        let drift_component = self.short_drift * 1.5 + self.drift_gap;
        let shock_penalty = self.transition_instability * 0.35;
        (drift_component + persistence_component * self.last_return.signum() - shock_penalty)
            .clamp(-1.0, 1.0)
    }

    pub fn confidence_score(&self) -> f64 {
        let stability = (1.0 - self.transition_instability).clamp(0.0, 1.0);
        let variance = ((self.variance_ratio - 1.0).abs() / 2.0).clamp(0.0, 1.0);
        let clustering = self.return_clustering.clamp(0.0, 1.0);
        (0.45 * stability + 0.30 * variance + 0.25 * clustering).clamp(0.0, 1.0)
    }
}

#[derive(Debug, Clone)]
pub struct ProcessSnapshot {
    pub ts: UnixMs,
    pub regime: Regime,
    pub prior: Prob,
    pub features: ProcessFeatures,
}

#[derive(Debug, Clone)]
pub struct PriorEstimator {
    priors: Vec<f64>,
    last_regime: Regime,
}

impl PriorEstimator {
    pub fn new() -> Self {
        Self {
            priors: Vec::new(),
            last_regime: Regime::Transitional,
        }
    }

    pub fn update(&mut self, features: &ProcessFeatures, ts: UnixMs) -> ProcessSnapshot {
        let regime = detect_regime(features);
        self.last_regime = regime;
        let directional_bias = features.directional_bias();
        let regime_tilt = match regime {
            Regime::Calm => 0.05 * directional_bias.signum(),
            Regime::Transitional => 0.10 * directional_bias,
            Regime::Expansion => 0.18 * directional_bias,
        };
        let confidence = features.confidence_score();
        let raw_prior =
            (0.5 + directional_bias * (0.12 + 0.20 * confidence) + regime_tilt).clamp(0.05, 0.95);
        self.priors.push(raw_prior);
        if self.priors.len() > 256 {
            self.priors.drain(..self.priors.len() - 128);
        }
        let smoothed = self.current_prior().unwrap_or(Prob(raw_prior));
        ProcessSnapshot {
            ts,
            regime,
            prior: smoothed,
            features: features.clone(),
        }
    }

    pub fn current_prior(&self) -> Option<Prob> {
        if self.priors.is_empty() {
            return None;
        }
        let half_life = 12.0;
        let mut weighted_sum = 0.0;
        let mut total_weight = 0.0;
        for (idx, prior) in self.priors.iter().rev().enumerate() {
            let weight = 0.5f64.powf(idx as f64 / half_life);
            weighted_sum += prior * weight;
            total_weight += weight;
        }
        Some(Prob((weighted_sum / total_weight).clamp(0.0, 1.0)))
    }

    pub fn last_regime(&self) -> Regime {
        self.last_regime
    }
}

impl Default for PriorEstimator {
    fn default() -> Self {
        Self::new()
    }
}

pub struct FeatureExtractor {
    prices: Vec<f64>,
    max_history: usize,
}

impl FeatureExtractor {
    pub fn new(max_history: usize) -> Self {
        Self {
            prices: Vec::new(),
            max_history: max_history.max(64),
        }
    }

    pub fn push_price(&mut self, price: f64) {
        self.prices.push(price);
        if self.prices.len() > self.max_history {
            self.prices.drain(..self.prices.len() - self.max_history);
        }
    }

    pub fn is_ready(&self, min_points: usize) -> bool {
        self.prices.len() >= min_points
    }

    pub fn extract(&self) -> Option<ProcessFeatures> {
        if self.prices.len() < 24 {
            return None;
        }
        let returns = returns(&self.prices);
        if returns.len() < 20 {
            return None;
        }
        let sample_size = returns.len();
        let short = tail(&returns, 12);
        let long = tail(&returns, 48.min(sample_size));
        let abs_returns: Vec<f64> = returns.iter().map(|r| r.abs()).collect();
        let abs_long = tail(&abs_returns, 48.min(abs_returns.len()));
        let long_std = std_dev(long);
        let short_std = std_dev(short);
        let last_return = *returns.last().unwrap_or(&0.0);
        let run_length = current_run_length(&returns);
        let sign_persistence = sign_persistence(&returns);
        let shock_threshold = mean(abs_long) + 1.5 * std_dev(abs_long);
        let shock_count = returns
            .iter()
            .filter(|r| r.abs() >= shock_threshold && shock_threshold > MIN_RET_EPS)
            .count();
        let time_since_large_move = returns
            .iter()
            .rposition(|r| r.abs() >= shock_threshold && shock_threshold > MIN_RET_EPS)
            .map(|idx| returns.len() - 1 - idx)
            .unwrap_or(returns.len());
        let long_mean = mean(long);
        let short_mean = mean(short);
        let move_zscore = if long_std > MIN_RET_EPS {
            (last_return - long_mean) / long_std
        } else {
            0.0
        };

        Some(ProcessFeatures {
            sample_size,
            last_return,
            short_drift: short_mean,
            long_drift: long_mean,
            drift_gap: short_mean - long_mean,
            run_length,
            sign_persistence,
            variance_ratio: (short_std.powi(2) / long_std.powi(2).max(MIN_RET_EPS))
                .clamp(0.0, 10.0),
            return_clustering: volatility_clustering(short, long),
            direction_concentration: direction_concentration(&returns),
            shock_frequency: shock_count as f64 / returns.len() as f64,
            time_since_large_move,
            move_zscore: move_zscore.clamp(-8.0, 8.0),
            transition_instability: transition_instability(&returns),
        })
    }
}

pub fn detect_regime(features: &ProcessFeatures) -> Regime {
    if features.variance_ratio > 1.6
        || features.shock_frequency > 0.18
        || features.move_zscore.abs() > 2.2
    {
        Regime::Expansion
    } else if features.transition_instability > 0.42
        || features.direction_concentration < 0.55
        || (features.variance_ratio > 1.15 && features.variance_ratio < 1.6)
    {
        Regime::Transitional
    } else {
        Regime::Calm
    }
}

#[derive(Debug, Clone)]
pub struct LiveDecisionInput {
    pub regime: Regime,
    pub prior: Prob,
    pub model_probability: Prob,
    pub final_probability: Prob,
    pub confidence_multiplier: f64,
    pub edge: f64,
    pub contract: Option<ContractType>,
    pub rejection_reason: Option<String>,
    pub features: ProcessFeatures,
}

impl LiveDecisionInput {
    pub fn benchmark_direction(&self) -> Option<ContractType> {
        self.contract
    }
}

fn returns(prices: &[f64]) -> Vec<f64> {
    prices
        .windows(2)
        .filter_map(|window| {
            let prev = window[0];
            let next = window[1];
            if prev.abs() <= MIN_RET_EPS {
                None
            } else {
                Some((next - prev) / prev)
            }
        })
        .collect()
}

fn tail(data: &[f64], n: usize) -> &[f64] {
    let start = data.len().saturating_sub(n);
    &data[start..]
}

fn mean(data: &[f64]) -> f64 {
    if data.is_empty() {
        0.0
    } else {
        data.iter().sum::<f64>() / data.len() as f64
    }
}

fn std_dev(data: &[f64]) -> f64 {
    if data.len() < 2 {
        return 0.0;
    }
    let avg = mean(data);
    let variance = data.iter().map(|value| (value - avg).powi(2)).sum::<f64>() / data.len() as f64;
    variance.sqrt()
}

fn current_run_length(returns: &[f64]) -> usize {
    let mut iter = returns.iter().rev();
    let Some(&last) = iter.next() else {
        return 0;
    };
    let sign = last.signum();
    if sign == 0.0 {
        return 0;
    }
    let mut len = 1;
    for &ret in iter {
        if ret.signum() == sign && ret.signum() != 0.0 {
            len += 1;
        } else {
            break;
        }
    }
    len
}

fn sign_persistence(returns: &[f64]) -> f64 {
    if returns.len() < 2 {
        return 0.0;
    }
    let same = returns
        .windows(2)
        .filter(|w| w[0].signum() != 0.0 && w[0].signum() == w[1].signum())
        .count();
    same as f64 / (returns.len() - 1) as f64
}

fn direction_concentration(returns: &[f64]) -> f64 {
    let up = returns.iter().filter(|r| **r > 0.0).count() as f64;
    let down = returns.iter().filter(|r| **r < 0.0).count() as f64;
    let total = (up + down).max(1.0);
    (up.max(down) / total).clamp(0.0, 1.0)
}

fn transition_instability(returns: &[f64]) -> f64 {
    if returns.len() < 2 {
        return 0.0;
    }
    let switches = returns
        .windows(2)
        .filter(|w| w[0].signum() != 0.0 && w[1].signum() != 0.0 && w[0].signum() != w[1].signum())
        .count();
    switches as f64 / (returns.len() - 1) as f64
}

fn volatility_clustering(short: &[f64], long: &[f64]) -> f64 {
    let short_mean = mean(short);
    let long_mean = mean(long);
    let short_abs =
        short.iter().map(|r| (r - short_mean).abs()).sum::<f64>() / short.len().max(1) as f64;
    let long_abs =
        long.iter().map(|r| (r - long_mean).abs()).sum::<f64>() / long.len().max(1) as f64;
    (short_abs / long_abs.max(MIN_RET_EPS)).clamp(0.0, 4.0) / 4.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp(start: f64, deltas: &[f64]) -> Vec<f64> {
        let mut out = vec![start];
        let mut price = start;
        for delta in deltas {
            price *= 1.0 + delta;
            out.push(price);
        }
        out
    }

    #[test]
    fn regime_detection_distinguishes_sequences() {
        let calm = ProcessFeatures {
            variance_ratio: 0.8,
            shock_frequency: 0.02,
            move_zscore: 0.4,
            transition_instability: 0.12,
            direction_concentration: 0.8,
            ..ProcessFeatures::neutral()
        };
        assert_eq!(detect_regime(&calm), Regime::Calm);

        let transitional = ProcessFeatures {
            variance_ratio: 1.25,
            shock_frequency: 0.08,
            move_zscore: 1.1,
            transition_instability: 0.48,
            direction_concentration: 0.52,
            ..ProcessFeatures::neutral()
        };
        assert_eq!(detect_regime(&transitional), Regime::Transitional);

        let expansion = ProcessFeatures {
            variance_ratio: 2.1,
            shock_frequency: 0.24,
            move_zscore: 3.0,
            transition_instability: 0.33,
            direction_concentration: 0.72,
            ..ProcessFeatures::neutral()
        };
        assert_eq!(detect_regime(&expansion), Regime::Expansion);
    }

    #[test]
    fn prior_estimator_tracks_directional_bias() {
        let mut estimator = PriorEstimator::new();
        let bull = ProcessFeatures {
            last_return: 0.004,
            short_drift: 0.002,
            long_drift: 0.0005,
            drift_gap: 0.0015,
            run_length: 6,
            sign_persistence: 0.85,
            variance_ratio: 1.8,
            return_clustering: 0.7,
            direction_concentration: 0.75,
            shock_frequency: 0.05,
            time_since_large_move: 9,
            move_zscore: 1.8,
            transition_instability: 0.18,
            sample_size: 48,
        };
        let snap = estimator.update(&bull, UnixMs(1));
        assert_eq!(snap.regime, Regime::Expansion);
        assert!(snap.prior.0 > 0.5);

        let bear = ProcessFeatures {
            last_return: -0.004,
            short_drift: -0.0025,
            long_drift: -0.001,
            drift_gap: -0.0015,
            run_length: 5,
            sign_persistence: 0.80,
            variance_ratio: 1.7,
            return_clustering: 0.7,
            direction_concentration: 0.78,
            shock_frequency: 0.07,
            time_since_large_move: 4,
            move_zscore: -2.1,
            transition_instability: 0.16,
            sample_size: 48,
        };
        let snap = estimator.update(&bear, UnixMs(2));
        assert!(snap.prior.0 < 0.55);
    }

    #[test]
    fn feature_extractor_is_stable_on_monotone_series() {
        let mut extractor = FeatureExtractor::new(128);
        for price in ramp(100.0, &vec![0.0005; 80]) {
            extractor.push_price(price);
        }
        let features = extractor.extract().unwrap();
        assert!(features.sign_persistence > 0.95);
        assert!(features.transition_instability < 0.05);
        assert!(features.run_length >= 10);
    }
}
