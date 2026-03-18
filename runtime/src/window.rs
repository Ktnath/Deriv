use bot_core::traits::WindowTracker;
use bot_core::types::{UnixMs, WindowState};

/// Reused from Polymarket bot — tracks contract window lifecycle.
pub struct TimeWindowTracker {
    pub start_time: UnixMs,
    pub end_time: UnixMs,
    pub state: WindowState,
}

impl TimeWindowTracker {
    pub fn new(start_time: UnixMs, end_time: UnixMs) -> Self {
        Self {
            start_time,
            end_time,
            state: WindowState::PreOpen,
        }
    }
}

impl WindowTracker for TimeWindowTracker {
    fn update_state(&mut self, current_time: UnixMs) -> WindowState {
        let time_left = (self.end_time.0 - current_time.0) as f64 / 1000.0;
        let time_to_start = (self.start_time.0 - current_time.0) as f64 / 1000.0;

        self.state = if time_to_start > 30.0 {
            WindowState::PreOpen
        } else if time_to_start > 0.0 {
            WindowState::Warmup
        } else if time_to_start == 0.0 || (time_to_start <= 0.0 && self.state == WindowState::Warmup) {
            WindowState::Open
        } else if time_left > 10.0 {
            WindowState::Trading
        } else if time_left > 0.0 {
            WindowState::Freeze
        } else if time_left > -60.0 {
            WindowState::Close
        } else {
            WindowState::ResolvedOrRollover
        };

        self.state
    }

    fn current_state(&self) -> WindowState {
        self.state
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_window_state_transitions() {
        let start = UnixMs(50_000);  // start at 50s
        let end = UnixMs(350_000);   // 300s contract
        let mut w = TimeWindowTracker::new(start, end);

        // Before start (>30s away): time_to_start = (50000-0)/1000 = 50s > 30
        assert_eq!(w.update_state(UnixMs(0)), WindowState::PreOpen);

        // Warmup (< 30s to start): time_to_start = (50000-30000)/1000 = 20s < 30
        assert_eq!(w.update_state(UnixMs(30_000)), WindowState::Warmup);

        // Open (at start)
        w.state = WindowState::Warmup; // simulate transition
        assert_eq!(w.update_state(UnixMs(50_000)), WindowState::Open);

        // Trading
        assert_eq!(w.update_state(UnixMs(100_000)), WindowState::Trading);

        // Freeze (< 10s left)
        assert_eq!(w.update_state(UnixMs(345_000)), WindowState::Freeze);

        // Close
        assert_eq!(w.update_state(UnixMs(355_000)), WindowState::Close);

        // Resolved
        assert_eq!(w.update_state(UnixMs(450_000)), WindowState::ResolvedOrRollover);
    }
}
