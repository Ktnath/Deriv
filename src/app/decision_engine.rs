use crate::{
    process::{FeatureExtractor, LiveDecisionInput, PriorEstimator, Regime},
    risk::{
        alpha::{AlphaEngine, BlendConfig, BlendInputs},
        kelly::KellyRisk,
        limits::RiskGate,
        settlement::Settlement,
    },
    types::{AlphaOutput, ContractType, Price, TickUpdate, UnixMs},
};

pub const MIN_PROCESS_POINTS: usize = 48;

#[derive(Debug, Clone, PartialEq)]
pub struct SimulatedTrade {
    pub entered_at_ms: i64,
    pub settle_at_ms: i64,
    pub direction: String,
    pub stake: f64,
}

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
    pub simulated_trade_closed: Option<SimulatedTradeSettlement>,
}

#[derive(Debug, Clone)]
pub struct SimulatedTradeSettlement {
    pub entered_at_ms: i64,
    pub settled_at_ms: i64,
    pub direction: String,
    pub stake: f64,
    pub pnl: f64,
    pub payout: f64,
    pub exit_reason: String,
    pub status: String,
}

#[derive(Debug, Clone)]
struct LivePosition {
    contract_id: Option<String>,
    direction: String,
    stake: f64,
    opened_at_ms: i64,
}

#[derive(Debug, Clone)]
enum LifecycleMode {
    Replay { open_trade: Option<SimulatedTrade> },
    LiveSynchronized { open_position: Option<LivePosition> },
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
    lifecycle: LifecycleMode,
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
        Self::new_with_mode(cfg, false)
    }

    pub fn new_live(cfg: DecisionEngineConfig) -> anyhow::Result<Self> {
        Self::new_with_mode(cfg, true)
    }

    fn new_with_mode(cfg: DecisionEngineConfig, live_synchronized: bool) -> anyhow::Result<Self> {
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
            lifecycle: if live_synchronized {
                LifecycleMode::LiveSynchronized {
                    open_position: None,
                }
            } else {
                LifecycleMode::Replay { open_trade: None }
            },
        })
    }

    pub fn notify_live_balance(&mut self, balance: f64) {
        if matches!(self.lifecycle, LifecycleMode::LiveSynchronized { .. }) {
            self.risk_gate.update_balance(balance);
        }
    }

    pub fn notify_live_trade_opened(
        &mut self,
        contract_id: Option<String>,
        direction: impl Into<String>,
        stake: f64,
        timestamp_ms: i64,
    ) {
        if let LifecycleMode::LiveSynchronized { open_position } = &mut self.lifecycle {
            let was_open = open_position.is_some();
            *open_position = Some(LivePosition {
                contract_id,
                direction: direction.into(),
                stake,
                opened_at_ms: timestamp_ms,
            });
            self.last_trade_time = timestamp_ms;
            if !was_open {
                self.risk_gate.on_trade_opened();
            }
        }
    }

    pub fn notify_live_trade_closed(
        &mut self,
        pnl: f64,
        timestamp_ms: i64,
        realized_balance: Option<f64>,
    ) {
        if let LifecycleMode::LiveSynchronized { open_position } = &mut self.lifecycle {
            if open_position.take().is_some() {
                self.risk_gate.on_trade_closed(pnl, UnixMs(timestamp_ms));
            }
            if let Some(balance) = realized_balance {
                self.risk_gate.update_balance(balance);
            }
        }
    }

    pub fn notify_live_trade_aborted(&mut self, timestamp_ms: i64, balance: Option<f64>) {
        if let LifecycleMode::LiveSynchronized { open_position } = &mut self.lifecycle {
            if open_position.take().is_some() {
                self.risk_gate.open_positions = self.risk_gate.open_positions.saturating_sub(1);
            }
            self.last_trade_time = timestamp_ms;
            if let Some(balance) = balance {
                self.risk_gate.update_balance(balance);
            }
        }
    }

    pub fn set_live_position_open(&mut self, is_open: bool) {
        if let LifecycleMode::LiveSynchronized { open_position } = &mut self.lifecycle {
            match (is_open, open_position.is_some()) {
                (true, false) => {
                    *open_position = Some(LivePosition {
                        contract_id: None,
                        direction: "UNKNOWN".into(),
                        stake: 0.0,
                        opened_at_ms: UnixMs::now().0,
                    });
                    self.risk_gate.on_trade_opened();
                }
                (false, true) => {
                    *open_position = None;
                    self.risk_gate.open_positions = self.risk_gate.open_positions.saturating_sub(1);
                }
                _ => {}
            }
        }
    }

    pub fn live_position_is_open(&self) -> bool {
        matches!(
            &self.lifecycle,
            LifecycleMode::LiveSynchronized {
                open_position: Some(_)
            }
        )
    }

    pub fn step(&mut self, tick: &TickUpdate, execution_enabled: bool) -> Option<DecisionContext> {
        let now = UnixMs(tick.epoch * 1000);
        let simulated_trade_closed = self.maybe_settle_simulated_trade(tick, now);
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
        let benchmark_signal = benchmark_signal(live_input.contract);
        let mut decision = "hold".to_string();
        let mut rejection_reason = live_input.rejection_reason.clone();
        let proposed_stake = risk_dec.max_size.0;
        let mut executed_stake = 0.0;
        if let Some(contract) = live_input.contract {
            let can = self.risk_gate.can_trade(now);
            if let Err(rej) = can {
                rejection_reason = Some(rej.to_string());
            } else if self.has_open_position() {
                rejection_reason = Some(self.open_position_rejection_reason().to_string());
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
                    if let LifecycleMode::Replay { open_trade } = &mut self.lifecycle {
                        *open_trade = Some(SimulatedTrade {
                            entered_at_ms: now.0,
                            settle_at_ms: now.0 + self.contract_duration as i64 * 1000,
                            direction: contract.to_string(),
                            stake: proposed_stake,
                        });
                    }
                }
            }
        }
        Some(DecisionContext {
            ts_ms: now.0,
            symbol: self.symbol.clone(),
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
            simulated_trade_closed,
        })
    }

    fn has_open_position(&self) -> bool {
        match &self.lifecycle {
            LifecycleMode::Replay { open_trade } => open_trade.is_some(),
            LifecycleMode::LiveSynchronized { open_position } => open_position.is_some(),
        }
    }

    fn open_position_rejection_reason(&self) -> &'static str {
        match &self.lifecycle {
            LifecycleMode::Replay { .. } => "simulated_trade_open",
            LifecycleMode::LiveSynchronized { .. } => "position_already_open",
        }
    }

    fn maybe_settle_simulated_trade(
        &mut self,
        tick: &TickUpdate,
        now: UnixMs,
    ) -> Option<SimulatedTradeSettlement> {
        let trade = match &self.lifecycle {
            LifecycleMode::Replay { open_trade } => open_trade.clone()?,
            LifecycleMode::LiveSynchronized { .. } => return None,
        };
        if now.0 < trade.settle_at_ms {
            return None;
        }
        let pnl = simulated_pnl(
            &trade.direction,
            self.settlement.current_price,
            tick.price,
            trade.stake,
        );
        self.risk_gate.on_trade_closed(pnl, now);
        if let LifecycleMode::Replay { open_trade } = &mut self.lifecycle {
            *open_trade = None;
        }
        Some(SimulatedTradeSettlement {
            entered_at_ms: trade.entered_at_ms,
            settled_at_ms: now.0,
            direction: trade.direction,
            stake: trade.stake,
            payout: trade.stake + pnl,
            pnl,
            exit_reason: "replay_contract_expiry".to_string(),
            status: "simulated_settled".to_string(),
        })
    }
}

fn simulated_pnl(direction: &str, entry_price: f64, settle_price: f64, stake: f64) -> f64 {
    let won = match direction {
        "CALL" => settle_price >= entry_price,
        "PUT" => settle_price <= entry_price,
        _ => false,
    };
    if won {
        stake * 0.95
    } else {
        -stake
    }
}

fn benchmark_signal(contract: Option<ContractType>) -> String {
    contract
        .map(|c| c.to_string())
        .unwrap_or_else(|| "HOLD".to_string())
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
        "benchmark_signal": benchmark_signal(input.contract),
        "q_low": alpha_out.q_low.0,
        "q_high": alpha_out.q_high.0,
    })
    .to_string()
}

pub fn build_decision(
    regime: &Regime,
    features: &crate::process::ProcessFeatures,
    prior: crate::types::Prob,
    model_probability: crate::types::Prob,
    final_probability: crate::types::Prob,
    confidence_multiplier: f64,
    time_left_sec: f64,
) -> LiveDecisionInput {
    let mut edge = final_probability.0 - 0.5;
    if matches!(regime, Regime::Transitional) {
        edge *= 0.5; // Penalize Edge during transitional regime to reduce false positives
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn make_tick(price: f64, epoch: i64) -> TickUpdate {
        TickUpdate {
            symbol: "R_100".into(),
            price,
            epoch,
        }
    }

    fn replay_engine() -> DecisionEngine {
        DecisionEngine::new(DecisionEngineConfig {
            symbol: "R_100".into(),
            contract_duration: 60,
            min_stake: 0.35,
            initial_balance: 100.0,
            max_open_positions: 1,
            max_daily_loss: 100.0,
            cooldown_after_loss_ms: 0,
            max_consecutive_losses: 10,
            model_path: None,
            allow_model_fallback: true,
            strategy_mode: "process".into(),
            prior_mode: "process-v1".into(),
        })
        .unwrap()
    }

    fn live_engine(balance: f64) -> DecisionEngine {
        DecisionEngine::new_live(DecisionEngineConfig {
            symbol: "R_100".into(),
            contract_duration: 60,
            min_stake: 0.35,
            initial_balance: balance,
            max_open_positions: 1,
            max_daily_loss: 100.0,
            cooldown_after_loss_ms: 0,
            max_consecutive_losses: 10,
            model_path: None,
            allow_model_fallback: true,
            strategy_mode: "process".into(),
            prior_mode: "process-v1".into(),
        })
        .unwrap()
    }

    fn first_signal(engine: &mut DecisionEngine) -> DecisionContext {
        for idx in 0..120 {
            let price = 100.0 + idx as f64 * 0.02;
            if let Some(ctx) = engine.step(&make_tick(price, idx as i64), false) {
                if ctx.contract_direction.is_some() {
                    return ctx;
                }
            }
        }
        panic!("expected at least one signal");
    }

    #[test]
    fn replay_execution_closes_positions_and_allows_multiple_trades() {
        let mut engine = replay_engine();

        let mut enters = 0;
        let mut settlements = 0;
        for idx in 0..200 {
            let price = 100.0 + idx as f64 * 0.02;
            if let Some(ctx) = engine.step(&make_tick(price, idx as i64), true) {
                if ctx.decision == "enter" {
                    enters += 1;
                }
                if ctx.simulated_trade_closed.is_some() {
                    settlements += 1;
                }
            }
        }

        assert!(enters >= 2, "expected multiple entries, got {enters}");
        assert!(
            settlements >= 1,
            "expected at least one simulated settlement"
        );
    }

    #[test]
    fn live_balance_update_changes_kelly_stake_sizing() {
        let mut engine = live_engine(100.0);
        let before = first_signal(&mut engine).proposed_stake;

        let mut engine = live_engine(100.0);
        engine.notify_live_balance(25.0);
        let after = first_signal(&mut engine).proposed_stake;

        assert!(after < before, "expected {after} < {before}");
    }

    #[test]
    fn live_trade_open_blocks_entries_until_close() {
        let mut engine = live_engine(100.0);
        let signal = first_signal(&mut engine);
        engine.notify_live_trade_opened(
            Some("c1".into()),
            "CALL",
            signal.proposed_stake,
            signal.ts_ms,
        );

        for idx in 60..120 {
            if let Some(ctx) = engine.step(&make_tick(101.0 + idx as f64 * 0.01, idx as i64), false)
            {
                if ctx.contract_direction.is_some() {
                    assert_eq!(
                        ctx.rejection_reason.as_deref(),
                        Some("max open positions reached")
                    );
                    return;
                }
            }
        }
        panic!("expected a blocked signal while live position is open");
    }

    #[test]
    fn live_trade_close_reenables_entries() {
        let mut engine = live_engine(100.0);
        let signal = first_signal(&mut engine);
        engine.notify_live_trade_opened(
            Some("c1".into()),
            "CALL",
            signal.proposed_stake,
            signal.ts_ms,
        );
        engine.notify_live_trade_closed(5.0, signal.ts_ms + 6_000, Some(105.0));

        for idx in 70..150 {
            if let Some(ctx) = engine.step(&make_tick(102.0 + idx as f64 * 0.01, idx as i64), false)
            {
                if ctx.contract_direction.is_some()
                    && ctx.rejection_reason.as_deref() != Some("trade_cooldown_active")
                {
                    assert_ne!(
                        ctx.rejection_reason.as_deref(),
                        Some("position_already_open")
                    );
                    return;
                }
            }
        }
        panic!("expected live entries to re-enable after close");
    }

    #[test]
    fn live_disconnect_abort_clears_open_position_state() {
        let mut engine = live_engine(100.0);
        let signal = first_signal(&mut engine);
        engine.notify_live_trade_opened(
            Some("c1".into()),
            "CALL",
            signal.proposed_stake,
            signal.ts_ms,
        );
        assert!(engine.live_position_is_open());

        engine.notify_live_trade_aborted(signal.ts_ms + 1_000, Some(100.0));
        assert!(!engine.live_position_is_open());
    }

    #[test]
    fn benchmark_signal_matches_contract_semantics() {
        assert_eq!(benchmark_signal(Some(ContractType::Call)), "CALL");
        assert_eq!(benchmark_signal(Some(ContractType::Put)), "PUT");
        assert_eq!(benchmark_signal(None), "HOLD");
    }
}
