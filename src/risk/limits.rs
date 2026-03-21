use crate::types::{RiskRejection, UnixMs};
use chrono::{DateTime, Datelike, Utc};
use tracing::warn;

/// Risk gating: enforces limits before allowing a trade.
pub struct RiskGate {
    pub max_open_positions: usize,
    pub max_loss_per_day: f64,
    pub cooldown_after_loss_ms: u64,
    pub max_consecutive_losses: usize,
    pub open_positions: usize,
    pub daily_pnl: f64,
    pub consecutive_losses: usize,
    pub last_loss_time: Option<UnixMs>,
    pub daily_reset_day: i32,
    pub balance: f64,
    pub min_balance: f64,
}

impl RiskGate {
    pub fn new(
        max_open_positions: usize,
        max_loss_per_day: f64,
        cooldown_after_loss_ms: u64,
        max_consecutive_losses: usize,
        initial_balance: f64,
    ) -> Self {
        Self {
            max_open_positions,
            max_loss_per_day,
            cooldown_after_loss_ms,
            max_consecutive_losses,
            open_positions: 0,
            daily_pnl: 0.0,
            consecutive_losses: 0,
            last_loss_time: None,
            daily_reset_day: i32::MIN,
            balance: initial_balance,
            min_balance: initial_balance * 0.05,
        }
    }

    fn utc_day(now: UnixMs) -> i32 {
        DateTime::<Utc>::from_timestamp_millis(now.0)
            .map(|dt| dt.date_naive().num_days_from_ce())
            .unwrap_or(i32::MIN)
    }

    pub fn refresh_for_time(&mut self, now: UnixMs) {
        let current_day = Self::utc_day(now);
        if self.daily_reset_day != current_day {
            self.reset_daily();
            self.daily_reset_day = current_day;
        }
    }

    pub fn can_trade(&mut self, now: UnixMs) -> Result<(), RiskRejection> {
        self.refresh_for_time(now);

        if self.open_positions >= self.max_open_positions {
            warn!(
                open = self.open_positions,
                max = self.max_open_positions,
                "Risk: max positions"
            );
            return Err(RiskRejection::MaxOpenPositions);
        }
        if -self.daily_pnl >= self.max_loss_per_day {
            warn!(
                daily_pnl = self.daily_pnl,
                max_loss_per_day = self.max_loss_per_day,
                "Risk: daily loss limit"
            );
            return Err(RiskRejection::MaxDailyLoss);
        }
        if let Some(last_loss) = self.last_loss_time {
            if (now.0 - last_loss.0) < self.cooldown_after_loss_ms as i64 {
                warn!(
                    elapsed_ms = now.0 - last_loss.0,
                    cooldown_ms = self.cooldown_after_loss_ms,
                    "Risk: cooldown"
                );
                return Err(RiskRejection::CooldownActive);
            }
        }
        if self.consecutive_losses >= self.max_consecutive_losses {
            warn!(
                consec = self.consecutive_losses,
                limit = self.max_consecutive_losses,
                "Risk: consecutive losses"
            );
            return Err(RiskRejection::ConsecutiveLossLimit);
        }
        if self.balance < self.min_balance {
            warn!(
                balance = self.balance,
                min = self.min_balance,
                "Risk: insufficient balance"
            );
            return Err(RiskRejection::InsufficientBalance);
        }
        Ok(())
    }

    pub fn on_trade_opened(&mut self) {
        self.open_positions += 1;
    }

    pub fn on_trade_closed(&mut self, pnl: f64, now: UnixMs) {
        self.refresh_for_time(now);
        self.open_positions = self.open_positions.saturating_sub(1);
        self.daily_pnl += pnl;
        self.balance += pnl;
        if pnl < 0.0 {
            self.consecutive_losses += 1;
            self.last_loss_time = Some(now);
        } else {
            self.consecutive_losses = 0;
        }
    }

    pub fn reset_daily(&mut self) {
        self.daily_pnl = 0.0;
        self.consecutive_losses = 0;
        self.last_loss_time = None;
    }

    pub fn update_balance(&mut self, balance: f64) {
        self.balance = balance;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc_ms(y: i32, m: u32, d: u32, h: u32) -> UnixMs {
        let dt = chrono::NaiveDate::from_ymd_opt(y, m, d)
            .unwrap()
            .and_hms_opt(h, 0, 0)
            .unwrap();
        UnixMs(dt.and_utc().timestamp_millis())
    }

    #[test]
    fn test_can_trade_ok() {
        let mut gate = RiskGate::new(1, 50.0, 30_000, 5, 1000.0);
        assert!(gate.can_trade(utc_ms(2026, 3, 21, 0)).is_ok());
    }

    #[test]
    fn test_max_positions() {
        let mut gate = RiskGate::new(1, 50.0, 30_000, 5, 1000.0);
        gate.on_trade_opened();
        assert!(matches!(
            gate.can_trade(utc_ms(2026, 3, 21, 0)),
            Err(RiskRejection::MaxOpenPositions)
        ));
    }

    #[test]
    fn test_daily_loss_reached_blocks_trading() {
        let mut gate = RiskGate::new(5, 50.0, 0, 100, 1000.0);
        let now = utc_ms(2026, 3, 21, 10);
        gate.on_trade_closed(-55.0, now);
        assert!(matches!(
            gate.can_trade(now),
            Err(RiskRejection::MaxDailyLoss)
        ));
    }

    #[test]
    fn test_next_day_reset_allows_trading_again() {
        let mut gate = RiskGate::new(5, 50.0, 0, 100, 1000.0);
        gate.on_trade_closed(-55.0, utc_ms(2026, 3, 21, 10));
        assert!(matches!(
            gate.can_trade(utc_ms(2026, 3, 21, 12)),
            Err(RiskRejection::MaxDailyLoss)
        ));
        assert!(gate.can_trade(utc_ms(2026, 3, 22, 0)).is_ok());
    }

    #[test]
    fn test_consecutive_losses() {
        let mut gate = RiskGate::new(5, 1000.0, 0, 3, 10000.0);
        for i in 0..3 {
            gate.on_trade_closed(-1.0, UnixMs(i * 1000));
        }
        assert!(matches!(
            gate.can_trade(UnixMs(5000)),
            Err(RiskRejection::ConsecutiveLossLimit)
        ));
    }

    #[test]
    fn test_win_resets_consecutive() {
        let mut gate = RiskGate::new(5, 1000.0, 0, 3, 10000.0);
        gate.on_trade_closed(-1.0, UnixMs(0));
        gate.on_trade_closed(-1.0, UnixMs(1000));
        gate.on_trade_closed(5.0, UnixMs(2000));
        assert!(gate.can_trade(UnixMs(3000)).is_ok());
    }
}
