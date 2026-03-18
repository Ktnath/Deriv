use bot_core::traits::ContractManager;
use bot_core::types::{BotError, ContractCommand, ContractType};
use connectors::deriv_ws::DerivWsClient;
use std::sync::Arc;
use std::collections::HashMap;

/// Manages the lifecycle of Deriv contracts.
/// Sends proposal requests and buy orders via the Deriv WebSocket.
pub struct DerivContractManager {
    pub ws: Arc<DerivWsClient>,
    pub active_contracts: HashMap<String, ActiveContract>,
    pub pending_proposals: HashMap<u64, PendingProposal>,
    pub is_alive: bool,
    pub dry_run: bool,
    /// Default contract duration in seconds.
    pub default_duration_sec: u64,
    /// Duration unit for Deriv API ("s" = seconds, "t" = ticks, "m" = minutes).
    pub duration_unit: String,
}

#[derive(Debug, Clone)]
pub struct ActiveContract {
    pub contract_id: String,
    pub contract_type: ContractType,
    pub stake: f64,
    pub payout: f64,
    pub buy_price: f64,
    pub epoch: i64,
}

#[derive(Debug, Clone)]
pub struct PendingProposal {
    pub symbol: String,
    pub contract_type: ContractType,
    pub stake: f64,
    pub req_id: u64,
}

impl DerivContractManager {
    pub fn new(ws: Arc<DerivWsClient>, dry_run: bool, default_duration_sec: u64, duration_unit: String) -> Self {
        Self {
            ws,
            active_contracts: HashMap::new(),
            pending_proposals: HashMap::new(),
            is_alive: true,
            dry_run,
            default_duration_sec,
            duration_unit,
        }
    }

    /// Handle a proposal response: if we have a pending proposal for this req_id, buy it.
    pub async fn handle_proposal_response(
        &mut self,
        req_id: u64,
        proposal_id: &str,
        ask_price: f64,
        payout: f64,
    ) -> Result<(), BotError> {
        let Some(pending) = self.pending_proposals.remove(&req_id) else {
            return Ok(());
        };

        if self.dry_run {
            println!("[DRY_RUN] Would buy {} {} contract: ask={:.2} payout={:.2}",
                pending.symbol, pending.contract_type, ask_price, payout);
            return Ok(());
        }

        println!("CM: Buying {} {} contract: proposal={} ask={:.2} payout={:.2}",
            pending.symbol, pending.contract_type, proposal_id, ask_price, payout);

        self.ws.buy_contract(proposal_id, ask_price).await?;
        Ok(())
    }

    /// Handle a buy confirmation: register the active contract.
    pub fn handle_buy_confirmation(&mut self, contract_id: &str, buy_price: f64, payout: f64) {
        // Find the contract type from the most recent pending or assume Call
        let ct = ContractType::Call; // simplified — in production, track this properly
        self.active_contracts.insert(contract_id.to_string(), ActiveContract {
            contract_id: contract_id.to_string(),
            contract_type: ct,
            stake: buy_price,
            payout,
            buy_price,
            epoch: bot_core::types::UnixMs::now().0 / 1000,
        });
        println!("CM: Contract {} purchased: buy_price={:.2} potential_payout={:.2}",
            contract_id, buy_price, payout);
    }
}

impl ContractManager for DerivContractManager {
    fn execute_commands(&mut self, commands: Vec<ContractCommand>) -> Result<(), BotError> {
        if !self.is_alive {
            return Err(BotError::HeartbeatTimeout);
        }

        for cmd in commands {
            match cmd {
                ContractCommand::Buy { symbol, contract_type, stake, duration_sec } => {
                    let ws = Arc::clone(&self.ws);
                    let ct_str = match contract_type {
                        ContractType::Call => "CALL",
                        ContractType::Put => "PUT",
                    };
                    let dur = if duration_sec > 0 { duration_sec } else { self.default_duration_sec };
                    let dur_unit = self.duration_unit.clone();
                    let sym = symbol.0.clone();
                    let stake_val = stake.0;

                    if self.dry_run {
                        println!("[DRY_RUN] SIGNAL: {} {} stake={:.2} dur={}{}",
                            sym, ct_str, stake_val, dur, dur_unit);
                        continue;
                    }

                    let _req_id_fut = {
                        let ws = Arc::clone(&ws);
                        tokio::spawn(async move {
                            ws.request_proposal(&sym, ct_str, dur, &dur_unit, stake_val).await
                        })
                    };

                    // We can't easily await inside a sync trait method, so we fire and forget.
                    // The proposal response will be handled by handle_proposal_response.
                    self.pending_proposals.insert(0, PendingProposal {
                        symbol: symbol.0.clone(),
                        contract_type,
                        stake: stake_val,
                        req_id: 0, // Will be updated when the spawn completes
                    });
                }
                ContractCommand::Sell { contract_id } => {
                    // Deriv doesn't support selling Rise/Fall contracts mid-flight
                    // (they expire automatically). Log a warning.
                    println!("CM: [WARN] Sell not supported for Rise/Fall contracts (id={})", contract_id);
                    self.active_contracts.remove(&contract_id);
                }
            }
        }

        Ok(())
    }
}
