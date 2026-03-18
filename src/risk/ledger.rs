use crate::types::{ContractType, LedgerState, Stake, Usd};

/// PnL tracking ledger for Deriv contracts.
pub struct Ledger {
    pub cash: f64,
    pub initial_balance: f64,
    pub open_calls: f64,
    pub open_puts: f64,
    pub realized_pnl: f64,
    pub wins: u64,
    pub losses: u64,
    pub peak_balance: f64,
    pub total_traded: f64,
}

impl Ledger {
    pub fn new(initial_balance: f64) -> Self {
        Self {
            cash: initial_balance,
            initial_balance,
            open_calls: 0.0,
            open_puts: 0.0,
            realized_pnl: 0.0,
            wins: 0,
            losses: 0,
            peak_balance: initial_balance,
            total_traded: 0.0,
        }
    }

    /// Record a contract buy.
    pub fn on_buy(&mut self, contract_type: ContractType, stake: f64) {
        self.cash -= stake;
        self.total_traded += stake;
        match contract_type {
            ContractType::Call => self.open_calls += stake,
            ContractType::Put => self.open_puts += stake,
        }
    }

    /// Record a contract settlement.
    pub fn on_settle(&mut self, contract_type: ContractType, payout: f64, stake: f64) {
        match contract_type {
            ContractType::Call => self.open_calls = (self.open_calls - stake).max(0.0),
            ContractType::Put => self.open_puts = (self.open_puts - stake).max(0.0),
        }
        self.cash += payout;
        let pnl = payout - stake;
        self.realized_pnl += pnl;
        if pnl > 0.0 {
            self.wins += 1;
        } else {
            self.losses += 1;
        }
        if self.cash > self.peak_balance {
            self.peak_balance = self.cash;
        }
    }

    pub fn win_rate(&self) -> f64 {
        let total = (self.wins + self.losses) as f64;
        if total == 0.0 {
            return 0.0;
        }
        self.wins as f64 / total
    }

    pub fn drawdown(&self) -> f64 {
        ((self.peak_balance - self.cash) / self.peak_balance).max(0.0)
    }

    pub fn get_state(&self) -> LedgerState {
        LedgerState {
            cash: Usd(self.cash),
            open_calls: Stake(self.open_calls),
            open_puts: Stake(self.open_puts),
            realized_pnl: Usd(self.realized_pnl),
            total_fees: Usd(0.0),
            mtm_pnl: Usd(self.realized_pnl),
            peak_mtm: Usd(self.peak_balance - self.initial_balance),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_buy_and_settle_win() {
        let mut l = Ledger::new(100.0);
        l.on_buy(ContractType::Call, 10.0);
        assert!((l.cash - 90.0).abs() < 0.001);
        l.on_settle(ContractType::Call, 19.5, 10.0);
        assert!((l.realized_pnl - 9.5).abs() < 0.001);
        assert_eq!(l.wins, 1);
    }

    #[test]
    fn test_loss() {
        let mut l = Ledger::new(100.0);
        l.on_buy(ContractType::Put, 5.0);
        l.on_settle(ContractType::Put, 0.0, 5.0);
        assert!((l.realized_pnl - (-5.0)).abs() < 0.001);
        assert_eq!(l.losses, 1);
    }

    #[test]
    fn test_win_rate() {
        let mut l = Ledger::new(100.0);
        l.on_buy(ContractType::Call, 1.0);
        l.on_settle(ContractType::Call, 1.95, 1.0);
        l.on_buy(ContractType::Call, 1.0);
        l.on_settle(ContractType::Call, 0.0, 1.0);
        l.on_buy(ContractType::Call, 1.0);
        l.on_settle(ContractType::Call, 1.95, 1.0);
        assert!((l.win_rate() - 0.6667).abs() < 0.01);
    }

    #[test]
    fn test_drawdown() {
        let mut l = Ledger::new(100.0);
        l.on_buy(ContractType::Call, 1.0);
        l.on_settle(ContractType::Call, 1.95, 1.0);
        // peak = 100.95
        l.on_buy(ContractType::Call, 5.0);
        l.on_settle(ContractType::Call, 0.0, 5.0);
        // cash = 95.95, drawdown = (100.95 - 95.95) / 100.95
        assert!(l.drawdown() > 0.04);
    }
}
