use crate::types::*;

/// Discover tradeable symbols on Deriv.
pub trait SymbolCatalog {
    fn discover_symbols(&mut self) -> Result<Vec<ContractSpec>, BotError>;
}

/// Server time synchronisation.
pub trait TimeSync {
    fn calculate_offset(&mut self) -> Result<i64, BotError>;
    fn now_ms(&self) -> UnixMs;
}

/// Contract duration window lifecycle tracking.
pub trait WindowTracker {
    fn update_state(&mut self, current_time: UnixMs) -> WindowState;
    fn current_state(&self) -> WindowState;
}

/// Estimate settlement price from tick data.
pub trait SettlementPriceEstimator {
    fn update_tick(&mut self, price: f64, timestamp: UnixMs);
    fn estimate(&self, now_ms: UnixMs) -> SettlementEstimate;
}

/// Alpha engine: model probability and blend with market.
pub trait AlphaEngine {
    fn calculate_q_model(&mut self, s0: f64, s_hat_t: f64, time_left_sec: f64) -> Prob;
    fn shrink_logit(&self, q_model: Prob, q_mkt: Prob, weight: f64) -> AlphaOutput;
    fn update_spot_return(&mut self, price: f64, timestamp: UnixMs);
}

/// Risk sizing engine.
pub trait RiskEngine {
    fn size_fractional_kelly(&self, q_low: Prob, price: Price, kelly_fraction: f64) -> RiskDecision;
    fn check_breakers(&self) -> Result<(), BotError>;
}

/// Strategy: evaluate market regime and produce desired state.
pub trait Strategy {
    fn evaluate_regime(&mut self, time_left_sec: f64) -> String;
    fn generate_desired_state(
        &mut self,
        alpha: &AlphaOutput,
        risk: &RiskDecision,
        current_book: &BookState,
        time_left_sec: f64,
    ) -> DesiredState;
}

/// Contract lifecycle management (replaces OrderManager).
pub trait ContractManager {
    fn execute_commands(&mut self, commands: Vec<ContractCommand>) -> Result<(), BotError>;
}

/// Local ledger for P&L tracking.
pub trait Ledger {
    fn update_contract_buy(&mut self, contract_type: ContractType, stake: Stake);
    fn update_contract_result(&mut self, contract_type: ContractType, payout: Usd);
    fn mark_to_market(&mut self, book: &BookState);
    fn get_state(&self) -> LedgerState;
}
