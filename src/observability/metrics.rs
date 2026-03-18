use crate::types::UnixMs;
use std::sync::atomic::{AtomicU64, Ordering};

/// Runtime metrics for the bot.
pub struct Metrics {
    pub tick_count: AtomicU64,
    pub trade_count: AtomicU64,
    pub win_count: AtomicU64,
    pub loss_count: AtomicU64,
    pub reconnect_count: AtomicU64,
    pub ping_latency_ms: AtomicU64,
    pub start_time: UnixMs,
}

impl Metrics {
    pub fn new() -> Self {
        Self {
            tick_count: AtomicU64::new(0),
            trade_count: AtomicU64::new(0),
            win_count: AtomicU64::new(0),
            loss_count: AtomicU64::new(0),
            reconnect_count: AtomicU64::new(0),
            ping_latency_ms: AtomicU64::new(0),
            start_time: UnixMs::now(),
        }
    }

    pub fn inc_ticks(&self) {
        self.tick_count.fetch_add(1, Ordering::Relaxed);
    }
    pub fn inc_trades(&self) {
        self.trade_count.fetch_add(1, Ordering::Relaxed);
    }
    pub fn inc_wins(&self) {
        self.win_count.fetch_add(1, Ordering::Relaxed);
    }
    pub fn inc_losses(&self) {
        self.loss_count.fetch_add(1, Ordering::Relaxed);
    }
    pub fn inc_reconnects(&self) {
        self.reconnect_count.fetch_add(1, Ordering::Relaxed);
    }

    pub fn set_ping_latency(&self, ms: u64) {
        self.ping_latency_ms.store(ms, Ordering::Relaxed);
    }

    pub fn win_rate(&self) -> f64 {
        let w = self.win_count.load(Ordering::Relaxed) as f64;
        let l = self.loss_count.load(Ordering::Relaxed) as f64;
        if w + l == 0.0 {
            0.0
        } else {
            w / (w + l)
        }
    }

    pub fn uptime_sec(&self) -> f64 {
        let now = UnixMs::now();
        (now.0 - self.start_time.0) as f64 / 1000.0
    }

    /// Summary string for periodic logging.
    pub fn summary(&self) -> String {
        format!(
            "ticks={} trades={} wins={} losses={} wr={:.1}% ping={}ms reconnects={} uptime={:.0}s",
            self.tick_count.load(Ordering::Relaxed),
            self.trade_count.load(Ordering::Relaxed),
            self.win_count.load(Ordering::Relaxed),
            self.loss_count.load(Ordering::Relaxed),
            self.win_rate() * 100.0,
            self.ping_latency_ms.load(Ordering::Relaxed),
            self.reconnect_count.load(Ordering::Relaxed),
            self.uptime_sec(),
        )
    }
}
