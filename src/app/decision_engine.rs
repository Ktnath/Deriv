use crate::{
    process::{FeatureExtractor, LiveDecisionInput, PriorEstimator, Regime},
    risk::{
        alpha::{AlphaEngine, AlphaOutput, BlendConfig, BlendInputs},
        kelly::KellyRisk,
        limits::RiskGate,
        settlement::Settlement,
    },
    types::{ContractType, Price, Prob, RiskDecision, TickUpdate, UnixMs},
};

pub const MIN_PROCESS_POINTS: usize = 48;

#[derive(Debug, Clone)]
pub struct DecisionContext {
    pub ts_ms: i64,
    pub symbol: String,
    pub price: f64,
    pub regime: String,
    pub prior: f64,
    pub q_model: f64,
    pub q_final: f64,
    pub q_low: f64,
    pub q_high: f64,
    pub confidence: f64,
    pub edge: f64,
    pub time_left_sec: f64,
    pub contract_direction: Option<String>,
    pub benchmark_signal: String,
    pub decision: String,
    pub rejection_reason: Option<String>,
    pub proposed_stake: f64,
    pub executed_stake: f64,
    pub execution_enabled: bool,
    pub prior_mode: String,
    pub strategy_mode: String,
    pub model_metadata: String,
    pub feature_summary: String,
}

pub struct DecisionEngine {
    symbol: String,
    contract_duration: u64,
    min_stake: f64,
    strategy_mode: String,
    prior_mode: String,
    model_metadata: String,
    alpha: AlphaEngine,
    blend_config: BlendConfig,
    prior_estimator: PriorEstimator,
    feature_extractor: FeatureExtractor,
    settlement: Settlement,
    kelly: KellyRisk,
    risk_gate: RiskGate,
    last_trade_time: i64,
    contract_start: Option<i64>,
}

pub struct DecisionEngineConfig {
    pub symbol: String,
    pub contract_duration: u64,
    pub min_stake: f64,
    pub initial_balance: f64,
    pub max_open_positions: usize,
    pub max_daily_loss: f64,
    pub cooldown_after_loss_ms: u64,
    pub max_consecutive_losses: usize,
    pub model_path: Option<String>,
    pub allow_model_fallback: bool,
    pub strategy_mode: String,
    pub prior_mode: String,
}

impl DecisionEngine {
    pub fn new(cfg: DecisionEngineConfig) -> anyhow::Result<Self> {
        let alpha = AlphaEngine::new(0.55, cfg.model_path.as_deref(), cfg.allow_model_fallback)?;
        let model_metadata = match alpha.model_mode() {
            crate::risk::alpha::ModelMode::Onnx { path } => format!("onnx:{path}"),
            crate::risk::alpha::ModelMode::QuantOnly { reason } => format!("quant-only:{reason}"),
        };
        Ok(Self {
            symbol: cfg.symbol,
            contract_duration: cfg.contract_duration,
            min_stake: cfg.min_stake,
            strategy_mode: cfg.strategy_mode,
            prior_mode: cfg.prior_mode,
            model_metadata,
            alpha,
            blend_config: BlendConfig::default(),
            prior_estimator: PriorEstimator::new(),
            feature_extractor: FeatureExtractor::new(512),
            settlement: Settlement::new(),
            kelly: KellyRisk {
                max_fraction: 0.15,
                max_loss: cfg.initial_balance * 0.5,
                min_stake: cfg.min_stake,
            },
            risk_gate: RiskGate::new(
                cfg.max_open_positions,
                cfg.max_daily_loss,
                cfg.cooldown_after_loss_ms,
                cfg.max_consecutive_losses,
                cfg.initial_balance,
            ),
            last_trade_time: 0,
            contract_start: None,
        })
    }

    pub fn step(&mut self, tick: &TickUpdate, execution_enabled: bool) -> Option<DecisionContext> {
        let now = UnixMs(tick.epoch * 1000);
        self.settlement.handle_tick(tick);
        self.feature_extractor.push_price(tick.price);
        self.alpha.update_spot_return(tick.price, now);
        if self.contract_start.is_none() {
            self.contract_start = Some(now.0);
            self.settlement.capture_s0();
        }
        let window_elapsed = self
            .contract_start
            .map(|s| (now.0 - s) as f64 / 1000.0)
            .unwrap_or(0.0);
        let time_left = self.contract_duration as f64 - window_elapsed;
        if time_left <= 0.0 {
            self.contract_start = Some(now.0);
            self.settlement.reset_s0();
            self.settlement.capture_s0();
            return None;
        }
        let est = self.settlement.estimate();
        let s0 = est.s0?;
        let s_hat = est.s_hat_t?;
        if !self.feature_extractor.is_ready(MIN_PROCESS_POINTS) {
            return None;
        }
        let features = self.feature_extractor.extract()?;
        let process_snapshot = self.prior_estimator.update(&features, now);
        let q_model = self.alpha.calculate_q_model(s0, s_hat, time_left);
        let confidence_multiplier = 0.85 + process_snapshot.features.confidence_score() * 0.5;
        let alpha_out = self.alpha.finalize_probability(BlendInputs {
            model_probability: q_model,
            process_prior: Some(process_snapshot.prior),
            confidence_multiplier: Some(confidence_multiplier),
            config: self.blend_config,
        });
        let risk_dec = self
            .kelly
            .size(alpha_out.q_low, Price(0.50), 0.5, self.risk_gate.balance);
        let live_input = build_decision(
            &process_snapshot.regime,
            &process_snapshot.features,
            process_snapshot.prior,
            q_model,
            alpha_out.q_final,
            confidence_multiplier,
            time_left,
        );
        let benchmark_signal = live_input
            .contract
            .map(|c| c.to_string())
            .unwrap_or_else(|| "HOLD".to_string());
        let mut decision = "hold".to_string();
        let mut rejection_reason = live_input.rejection_reason.clone();
        let proposed_stake = risk_dec.max_size.0;
        let mut executed_stake = 0.0;
        if let Some(contract) = live_input.contract {
            let can = self.risk_gate.can_trade(now);
            if let Err(rej) = can {
                rejection_reason = Some(rej.to_string());
            } else if (now.0 - self.last_trade_time) <= self.contract_duration as i64 * 1000 {
                rejection_reason = Some("trade_cooldown_active".to_string());
            } else if proposed_stake < self.min_stake {
                rejection_reason = Some("below_min_stake".to_string());
            } else {
                decision = if execution_enabled { "enter" } else { "signal" }.to_string();
                if execution_enabled {
                    executed_stake = proposed_stake;
                    self.last_trade_time = now.0;
                    self.risk_gate.on_trade_opened();
                }
                let _ = contract;
            }
        }
        Some(DecisionContext {
            ts_ms: now.0,
            symbol: tick.symbol.clone(),
            price: tick.price,
            regime: process_snapshot.regime.as_str().to_string(),
            prior: process_snapshot.prior.0,
            q_model: q_model.0,
            q_final: alpha_out.q_final.0,
            q_low: alpha_out.q_low.0,
            q_high: alpha_out.q_high.0,
            confidence: confidence_multiplier,
            edge: live_input.edge,
            time_left_sec: time_left,
            contract_direction: live_input.contract.map(|ct| ct.to_string()),
            benchmark_signal,
            decision,
            rejection_reason,
            proposed_stake,
            executed_stake,
            execution_enabled,
            prior_mode: self.prior_mode.clone(),
            strategy_mode: self.strategy_mode.clone(),
            model_metadata: self.model_metadata.clone(),
            feature_summary: feature_summary(&live_input, &alpha_out),
        })
    }
}

fn feature_summary(input: &LiveDecisionInput, alpha_out: &AlphaOutput) -> String {
    serde_json::json!({
        "sample_size": input.features.sample_size,
        "last_return": input.features.last_return,
        "short_drift": input.features.short_drift,
        "long_drift": input.features.long_drift,
        "drift_gap": input.features.drift_gap,
        "run_length": input.features.run_length,
        "sign_persistence": input.features.sign_persistence,
        "variance_ratio": input.features.variance_ratio,
        "return_clustering": input.features.return_clustering,
        "direction_concentration": input.features.direction_concentration,
        "shock_frequency": input.features.shock_frequency,
        "time_since_large_move": input.features.time_since_large_move,
        "move_zscore": input.features.move_zscore,
        "transition_instability": input.features.transition_instability,
        "q_low": alpha_out.q_low.0,
        "q_high": alpha_out.q_high.0,
    })
    .to_string()
}

pub fn build_decision(
    regime: &Regime,
    features: &crate::process::ProcessFeatures,
    prior: Prob,
    model_probability: Prob,
    final_probability: Prob,
    confidence_multiplier: f64,
    time_left_sec: f64,
) -> LiveDecisionInput {
    let edge = final_probability.0 - 0.5;
    let edge_threshold = match regime {
        Regime::Calm => 0.035,
        Regime::Transitional => 0.05,
        Regime::Expansion => 0.065,
    };
    let mut rejection_reason = None;
    let contract = if time_left_sec < 10.0 {
        rejection_reason = Some("late_window".to_string());
        None
    } else if features.sample_size < 24 {
        rejection_reason = Some("insufficient_process_history".to_string());
        None
    } else if features.transition_instability > 0.78 {
        rejection_reason = Some("transition_instability".to_string());
        None
    } else if edge.abs() < edge_threshold {
        rejection_reason = Some("edge_below_threshold".to_string());
        None
    } else if matches!(regime, Regime::Calm) && features.variance_ratio > 1.2 {
        rejection_reason = Some("variance_out_of_regime".to_string());
        None
    } else if edge > 0.0 {
        Some(ContractType::Call)
    } else {
        Some(ContractType::Put)
    };
    LiveDecisionInput {
        regime: *regime,
        prior,
        model_probability,
        final_probability,
        confidence_multiplier,
        edge,
        contract,
        rejection_reason,
        features: features.clone(),
    }
}
