use bot_core::traits::Ledger;
use bot_core::types::{BookState, ContractType, LedgerState, Stake, Usd};

/// Local ledger for tracking P&L on Deriv contracts.
pub struct LocalLedger {
    pub cash: Usd,
    pub open_calls: Stake,
    pub open_puts: Stake,
    pub realized_pnl: Usd,
    pub total_fees: Usd,
    pub mtm_pnl: Usd,
    pub peak_mtm: Usd,
    pub base_cash: Usd,
    pub total_contracts: u64,
    pub wins: u64,
    pub losses: u64,
}

impl LocalLedger {
    pub fn new(initial_cash: f64) -> Self {
        Self {
            cash: Usd(initial_cash),
            open_calls: Stake(0.0),
            open_puts: Stake(0.0),
            realized_pnl: Usd(0.0),
            total_fees: Usd(0.0),
            mtm_pnl: Usd(0.0),
            peak_mtm: Usd(0.0),
            base_cash: Usd(initial_cash),
            total_contracts: 0,
            wins: 0,
            losses: 0,
        }
    }

    pub fn win_rate(&self) -> f64 {
        if self.total_contracts == 0 { return 0.0; }
        self.wins as f64 / self.total_contracts as f64 * 100.0
    }
}

impl Ledger for LocalLedger {
    fn update_contract_buy(&mut self, contract_type: ContractType, stake: Stake) {
        self.cash.0 -= stake.0;
        match contract_type {
            ContractType::Call => self.open_calls.0 += stake.0,
            ContractType::Put => self.open_puts.0 += stake.0,
        }
        self.total_contracts += 1;
    }

    fn update_contract_result(&mut self, contract_type: ContractType, payout: Usd) {
        let stake_returned = match contract_type {
            ContractType::Call => {
                let s = self.open_calls.0;
                self.open_calls.0 = 0.0;
                s
            }
            ContractType::Put => {
                let s = self.open_puts.0;
                self.open_puts.0 = 0.0;
                s
            }
        };

        self.cash.0 += payout.0;

        let profit = payout.0 - stake_returned;
        self.realized_pnl.0 += profit;

        if profit > 0.0 {
            self.wins += 1;
        } else {
            self.losses += 1;
        }

        println!("LEDGER: Contract {} settled: stake={:.2} payout={:.2} profit={:.2} (W/L={}/{})",
            contract_type, stake_returned, payout.0, profit, self.wins, self.losses);
    }

    fn mark_to_market(&mut self, _book: &BookState) {
        // For Deriv contracts, MTM is simplified: open contracts are valued at stake cost
        let open_value = self.open_calls.0 + self.open_puts.0;
        let current_value = self.cash.0 + open_value;
        self.mtm_pnl.0 = current_value - self.base_cash.0;

        if self.mtm_pnl.0 > self.peak_mtm.0 {
            self.peak_mtm.0 = self.mtm_pnl.0;
        }
    }

    fn get_state(&self) -> LedgerState {
        LedgerState {
            cash: self.cash,
            open_calls: self.open_calls,
            open_puts: self.open_puts,
            realized_pnl: self.realized_pnl,
            total_fees: self.total_fees,
            mtm_pnl: self.mtm_pnl,
            peak_mtm: self.peak_mtm,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bot_core::types::*;

    #[test]
    fn test_ledger_buy_and_settle() {
        let mut l = LocalLedger::new(100.0);
        l.update_contract_buy(ContractType::Call, Stake(5.0));
        assert!((l.cash.0 - 95.0).abs() < 0.001);
        assert!((l.open_calls.0 - 5.0).abs() < 0.001);

        // Win: payout = 9.50 (1.9x stake)
        l.update_contract_result(ContractType::Call, Usd(9.50));
        assert!((l.cash.0 - 104.50).abs() < 0.001);
        assert_eq!(l.wins, 1);
        assert!((l.realized_pnl.0 - 4.50).abs() < 0.001);
    }

    #[test]
    fn test_ledger_loss() {
        let mut l = LocalLedger::new(100.0);
        l.update_contract_buy(ContractType::Put, Stake(5.0));

        // Loss: payout = 0
        l.update_contract_result(ContractType::Put, Usd(0.0));
        assert!((l.cash.0 - 95.0).abs() < 0.001);
        assert_eq!(l.losses, 1);
        assert!((l.realized_pnl.0 - (-5.0)).abs() < 0.001);
    }

    #[test]
    fn test_win_rate() {
        let mut l = LocalLedger::new(100.0);
        l.update_contract_buy(ContractType::Call, Stake(5.0));
        l.update_contract_result(ContractType::Call, Usd(9.50));
        l.update_contract_buy(ContractType::Call, Stake(5.0));
        l.update_contract_result(ContractType::Call, Usd(0.0));
        assert!((l.win_rate() - 50.0).abs() < 0.001);
    }
}
