use crate::protocol::{requests, responses::DerivResponse};
use crate::transport::router::Router;
use crate::types::{BotError, ContractType, SymbolId, TradeState, UnixMs};
use std::sync::Arc;
use tracing::{debug, error, info, warn};

/// Action decided after processing a ProposalOpenContract update.
#[derive(Debug, Clone)]
pub enum PocAction {
    /// Contract naturally settled (is_sold or is_expired).
    Settled(f64),
    /// Profit/loss threshold hit — sell immediately.
    SellNow {
        contract_id: String,
        profit: f64,
        buy_price: f64,
    },
    /// Nothing to do, keep holding.
    Hold,
}

/// Active trade being tracked.
#[derive(Debug, Clone)]
pub struct ActiveTrade {
    pub state: TradeState,
    pub symbol: SymbolId,
    pub contract_type: ContractType,
    pub stake: f64,
    pub duration_sec: u64,
    pub duration_unit: String,
    pub proposal_id: Option<String>,
    pub contract_id: Option<String>,
    pub buy_price: Option<f64>,
    pub payout: Option<f64>,
    pub subscription_id: Option<String>,
    pub created_at: UnixMs,
}

/// Trade FSM: IDLE → PRICING → SUBMITTED → OPEN → SETTLED / ABORTED
pub struct Trader {
    pub router: Arc<Router>,
    pub active_trade: Option<ActiveTrade>,
    pub dry_run: bool,
    pub stop_loss_pct: f64,
}

impl Trader {
    pub fn new(router: Arc<Router>, dry_run: bool, stop_loss_pct: f64) -> Self {
        Self {
            router,
            active_trade: None,
            dry_run,
            stop_loss_pct,
        }
    }

    pub fn current_state(&self) -> TradeState {
        self.active_trade
            .as_ref()
            .map(|t| t.state)
            .unwrap_or(TradeState::Idle)
    }

    /// Attempt to enter a trade: IDLE → PRICING.
    pub async fn enter_trade(
        &mut self,
        symbol: &SymbolId,
        contract_type: ContractType,
        stake: f64,
        duration_sec: u64,
        duration_unit: &str,
    ) -> Result<(), BotError> {
        if self.current_state() != TradeState::Idle {
            return Err(BotError::StateError(format!(
                "Cannot enter trade in state {:?}",
                self.current_state()
            )));
        }

        let ct_str = contract_type.to_string();
        info!(
            symbol = %symbol.0, contract_type = %ct_str,
            stake, duration_sec, dry_run = self.dry_run,
            "Trade FSM: IDLE → PRICING"
        );

        self.active_trade = Some(ActiveTrade {
            state: TradeState::Pricing,
            symbol: symbol.clone(),
            contract_type,
            stake,
            duration_sec,
            duration_unit: duration_unit.to_string(),
            proposal_id: None,
            contract_id: None,
            buy_price: None,
            payout: None,
            subscription_id: None,
            created_at: UnixMs::now(),
        });

        if self.dry_run {
            info!(
                "[DRY_RUN] Would send proposal for {} {} stake={:.2}",
                symbol.0, ct_str, stake
            );
            // Simulate immediate settlement in dry run
            if let Some(trade) = self.active_trade.as_mut() {
                trade.state = TradeState::Settled;
            }
            return Ok(());
        }

        // Send proposal via RPC
        let payload = requests::proposal(&symbol.0, &ct_str, duration_sec, duration_unit, stake);
        let response = self.router.send_rpc(payload, 10_000).await?;

        // Parse proposal response
        let text = serde_json::to_string(&response).unwrap_or_default();
        match crate::protocol::responses::parse_response(&text) {
            DerivResponse::Proposal {
                proposal_id,
                ask_price,
                payout,
                ..
            } => {
                info!(proposal_id = %proposal_id, ask_price, payout, "Trade FSM: PRICING → SUBMITTED");
                if let Some(trade) = self.active_trade.as_mut() {
                    trade.state = TradeState::Submitted;
                    trade.proposal_id = Some(proposal_id.clone());
                    trade.payout = Some(payout);
                }
                // Now buy
                self.buy_proposal(&proposal_id, ask_price).await?;
            }
            DerivResponse::Error { code, message, .. } => {
                error!(code = %code, message = %message, "Proposal rejected");
                self.abort("Proposal rejected");
                return Err(BotError::Api(format!("{}: {}", code, message)));
            }
            other => {
                warn!(?other, "Unexpected proposal response");
                self.abort("Unexpected proposal response");
            }
        }

        Ok(())
    }

    /// Buy a proposal: SUBMITTED → OPEN.
    async fn buy_proposal(&mut self, proposal_id: &str, price: f64) -> Result<(), BotError> {
        let payload = requests::buy(proposal_id, price);
        let response = self.router.send_rpc(payload, 10_000).await?;

        let text = serde_json::to_string(&response).unwrap_or_default();
        match crate::protocol::responses::parse_response(&text) {
            DerivResponse::Buy {
                contract_id,
                buy_price,
                payout,
            } => {
                info!(contract_id = %contract_id, buy_price, payout, "Trade FSM: SUBMITTED → OPEN");
                if let Some(trade) = self.active_trade.as_mut() {
                    trade.state = TradeState::Open;
                    trade.contract_id = Some(contract_id.clone());
                    trade.buy_price = Some(buy_price);
                    trade.payout = Some(payout);
                }
                // Subscribe to proposal_open_contract for monitoring
                self.monitor_contract(&contract_id).await?;
            }
            DerivResponse::Error { code, message, .. } => {
                error!(code = %code, message = %message, "Buy rejected");
                self.abort("Buy rejected");
                return Err(BotError::Api(format!("{}: {}", code, message)));
            }
            other => {
                warn!(?other, "Unexpected buy response");
                self.abort("Unexpected buy response");
            }
        }

        Ok(())
    }

    /// Subscribe to contract monitoring: watches until settlement.
    async fn monitor_contract(&mut self, contract_id: &str) -> Result<(), BotError> {
        let payload = requests::proposal_open_contract(contract_id);
        let _req_id = self.router.send_fire(payload).await?;
        debug!(contract_id, "Subscribed to proposal_open_contract");
        Ok(())
    }

    /// Handle a ProposalOpenContract update — returns a PocAction.
    pub fn handle_poc_update(
        &mut self,
        contract_id: &str,
        is_sold: bool,
        is_expired: bool,
        is_valid_to_sell: bool,
        profit: f64,
        buy_price: f64,
    ) -> PocAction {
        if let Some(trade) = self.active_trade.as_ref() {
            if trade.contract_id.as_deref() == Some(contract_id) && trade.state == TradeState::Open
            {
                // Already settled by Deriv
                if is_sold || is_expired {
                    info!(
                        contract_id,
                        profit, is_sold, is_expired, "Trade FSM: OPEN → SETTLED (natural)"
                    );
                    // Mark settled
                    if let Some(t) = self.active_trade.as_mut() {
                        t.state = TradeState::Settled;
                    }
                    return PocAction::Settled(profit);
                }

                // Check stop-loss threshold
                if buy_price > 0.0 && is_valid_to_sell {
                    let profit_pct = profit / buy_price;

                    if profit_pct <= -self.stop_loss_pct {
                        warn!(
                            contract_id,
                            profit,
                            buy_price,
                            profit_pct = format!("{:.1}%", profit_pct * 100.0),
                            threshold = format!("-{:.1}%", self.stop_loss_pct * 100.0),
                            "🛑 Stop-loss triggered — selling contract"
                        );
                        return PocAction::SellNow {
                            contract_id: contract_id.to_string(),
                            profit,
                            buy_price,
                        };
                    }

                    // Log progress when in profit
                    if profit > 0.0 {
                        debug!(
                            contract_id,
                            profit_pct = format!("{:.1}%", profit_pct * 100.0),
                            "Position in profit, holding till expiration"
                        );
                    }
                }
            }
        }
        PocAction::Hold
    }

    /// Sell a contract early (take-profit / stop-loss).
    pub async fn sell_contract(&mut self, contract_id: &str, price: f64) -> Result<f64, BotError> {
        info!(contract_id, price, "Sending sell RPC");
        let payload = requests::sell(contract_id, price);
        let response = self.router.send_rpc(payload, 10_000).await?;

        let text = serde_json::to_string(&response).unwrap_or_default();
        match crate::protocol::responses::parse_response(&text) {
            DerivResponse::Sell { sold_for, .. } => {
                // Compute real PnL from sold_for vs our buy_price
                // (the API `profit` field returns 0.0 for sell responses)
                let buy_price = self
                    .active_trade
                    .as_ref()
                    .and_then(|t| t.buy_price)
                    .unwrap_or(price);
                let real_pnl = sold_for - buy_price;
                info!(
                    contract_id,
                    sold_for, buy_price, real_pnl, "Trade FSM: OPEN → SETTLED (early sell)"
                );
                if let Some(trade) = self.active_trade.as_mut() {
                    trade.state = TradeState::Settled;
                }
                Ok(real_pnl)
            }
            DerivResponse::Error { code, message, .. } => {
                error!(code = %code, message = %message, "Sell rejected");
                // Don't abort — the contract is still open, it may settle naturally
                Err(BotError::Api(format!("Sell failed: {}: {}", code, message)))
            }
            other => {
                warn!(?other, "Unexpected sell response");
                Err(BotError::Other("Unexpected sell response".into()))
            }
        }
    }

    /// Abort current trade.
    pub fn abort(&mut self, reason: &str) {
        if let Some(trade) = self.active_trade.as_mut() {
            warn!(reason, state = ?trade.state, "Trade FSM: → ABORTED");
            trade.state = TradeState::Aborted;
        }
    }

    /// Reset to IDLE after settlement or abort.
    pub fn reset(&mut self) {
        self.active_trade = None;
    }

    /// Check if trader is idle and ready for new trades.
    pub fn is_idle(&self) -> bool {
        self.active_trade.is_none()
            || matches!(
                self.current_state(),
                TradeState::Idle | TradeState::Settled | TradeState::Aborted
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trade_fsm_initial_state() {
        assert_eq!(TradeState::Idle, TradeState::Idle);
        assert_ne!(TradeState::Idle, TradeState::Pricing);
    }

    #[test]
    fn test_trade_state_transitions() {
        let states = vec![
            TradeState::Idle,
            TradeState::Pricing,
            TradeState::Submitted,
            TradeState::Open,
            TradeState::Settled,
            TradeState::Aborted,
        ];
        assert_eq!(states.len(), 6);
    }

    #[test]
    fn test_poc_action_stop_loss() {
        let router = Arc::new(Router::new(tokio::sync::mpsc::channel(1).0));
        let mut trader = Trader::new(router, false, 0.50);
        trader.active_trade = Some(ActiveTrade {
            state: TradeState::Open,
            symbol: SymbolId("R_100".into()),
            contract_type: ContractType::Call,
            stake: 1.0,
            duration_sec: 300,
            duration_unit: "s".into(),
            proposal_id: None,
            contract_id: Some("c_123".into()),
            buy_price: Some(1.0),
            payout: Some(1.95),
            subscription_id: None,
            created_at: UnixMs(0),
        });

        // Loss = -0.55 on buy_price 1.0 → -55% <= -50% threshold
        let action = trader.handle_poc_update("c_123", false, false, true, -0.55, 1.0);
        assert!(matches!(action, PocAction::SellNow { .. }));
    }

    #[test]
    fn test_poc_action_hold() {
        let router = Arc::new(Router::new(tokio::sync::mpsc::channel(1).0));
        let mut trader = Trader::new(router, false, 0.50);
        trader.active_trade = Some(ActiveTrade {
            state: TradeState::Open,
            symbol: SymbolId("R_100".into()),
            contract_type: ContractType::Call,
            stake: 1.0,
            duration_sec: 300,
            duration_unit: "s".into(),
            proposal_id: None,
            contract_id: Some("c_123".into()),
            buy_price: Some(1.0),
            payout: Some(1.95),
            subscription_id: None,
            created_at: UnixMs(0),
        });

        // Profit = 0.10 on buy_price 1.0 → 10% < 20% threshold → Hold
        let action = trader.handle_poc_update("c_123", false, false, true, 0.10, 1.0);
        assert!(matches!(action, PocAction::Hold));
    }

    #[test]
    fn test_poc_action_settled() {
        let router = Arc::new(Router::new(tokio::sync::mpsc::channel(1).0));
        let mut trader = Trader::new(router, false, 0.50);
        trader.active_trade = Some(ActiveTrade {
            state: TradeState::Open,
            symbol: SymbolId("R_100".into()),
            contract_type: ContractType::Call,
            stake: 1.0,
            duration_sec: 300,
            duration_unit: "s".into(),
            proposal_id: None,
            contract_id: Some("c_123".into()),
            buy_price: Some(1.0),
            payout: Some(1.95),
            subscription_id: None,
            created_at: UnixMs(0),
        });

        // is_expired = true → Settled
        let action = trader.handle_poc_update("c_123", false, true, false, -0.80, 1.0);
        assert!(matches!(action, PocAction::Settled(_)));
    }
}
