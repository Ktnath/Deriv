use crate::types::{Price, TickUpdate, UnixMs};
use std::collections::VecDeque;

/// Ring buffer storing the last N ticks.
pub struct TickBuffer {
    buf: VecDeque<TickUpdate>,
    capacity: usize,
}

impl TickBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            buf: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    /// Push a tick into the ring buffer.
    pub fn push(&mut self, tick: TickUpdate) {
        if self.buf.len() >= self.capacity {
            self.buf.pop_front();
        }
        self.buf.push_back(tick);
    }

    pub fn len(&self) -> usize {
        self.buf.len()
    }
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    /// Get the last tick.
    pub fn last(&self) -> Option<&TickUpdate> {
        self.buf.back()
    }

    /// Get the second-to-last tick.
    pub fn prev(&self) -> Option<&TickUpdate> {
        if self.buf.len() >= 2 {
            Some(&self.buf[self.buf.len() - 2])
        } else {
            None
        }
    }

    /// Get all prices as a vec (oldest first).
    pub fn prices(&self) -> Vec<f64> {
        self.buf.iter().map(|t| t.price).collect()
    }

    /// Get last N prices.
    pub fn last_n_prices(&self, n: usize) -> Vec<f64> {
        let start = if self.buf.len() > n {
            self.buf.len() - n
        } else {
            0
        };
        self.buf.iter().skip(start).map(|t| t.price).collect()
    }

    /// Current tick state (last + prev prices).
    pub fn tick_state(&self) -> Option<(Price, Price, UnixMs)> {
        let last = self.last()?;
        let prev_price = self.prev().map(|p| p.price).unwrap_or(last.price);
        Some((
            Price(last.price),
            Price(prev_price),
            UnixMs(last.epoch * 1000),
        ))
    }

    /// Simple Moving Average of last N prices.
    pub fn sma(&self, period: usize) -> Option<f64> {
        let prices = self.last_n_prices(period);
        if prices.len() < period {
            return None;
        }
        Some(prices.iter().sum::<f64>() / prices.len() as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_tick(price: f64, epoch: i64) -> TickUpdate {
        TickUpdate {
            symbol: "R_100".to_string(),
            price,
            epoch,
        }
    }

    #[test]
    fn test_ring_buffer() {
        let mut buf = TickBuffer::new(3);
        buf.push(make_tick(100.0, 1));
        buf.push(make_tick(101.0, 2));
        buf.push(make_tick(102.0, 3));
        assert_eq!(buf.len(), 3);
        buf.push(make_tick(103.0, 4));
        assert_eq!(buf.len(), 3); // still 3, oldest dropped
        assert!((buf.last().unwrap().price - 103.0).abs() < 0.001);
        assert!((buf.prices()[0] - 101.0).abs() < 0.001); // oldest is now 101
    }

    #[test]
    fn test_sma() {
        let mut buf = TickBuffer::new(100);
        for i in 1..=5 {
            buf.push(make_tick(i as f64 * 10.0, i));
        }
        assert!((buf.sma(3).unwrap() - 40.0).abs() < 0.001); // (30+40+50)/3
        assert!(buf.sma(10).is_none()); // not enough data
    }

    #[test]
    fn test_tick_state() {
        let mut buf = TickBuffer::new(10);
        buf.push(make_tick(99.0, 1));
        buf.push(make_tick(100.5, 2));
        let (last, prev, _ts) = buf.tick_state().unwrap();
        assert!((last.0 - 100.5).abs() < 0.001);
        assert!((prev.0 - 99.0).abs() < 0.001);
    }
}
