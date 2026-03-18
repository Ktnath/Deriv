use bot_core::traits::Strategy;
use bot_core::types::{AlphaOutput, BookState, DesiredState, RiskDecision, Stake};

/// MTF Playbook Strategy based on mtf.md research.
/// HTF (10m Context) -> TTF (1m Setup) -> LTF (Ticks Timing)
pub struct MtfPlaybookStrategy {
    pub min_roi: f64,
    pub max_roi: f64,
    mid_history: Vec<f64>,
    vwap_cum_pv: f64,
    vwap_cum_v: f64,
}

impl MtfPlaybookStrategy {
    pub fn new(min_roi: f64, max_roi: f64) -> Self {
        Self {
            min_roi,
            max_roi,
            mid_history: Vec::with_capacity(2400),
            vwap_cum_pv: 0.0,
            vwap_cum_v: 0.0,
        }
    }

    fn calculate_roi(&self, price: f64) -> f64 {
        if price <= 0.0 || price >= 1.0 { return 0.0; }
        (1.0 - price) / price
    }

    fn calculate_ema(&self, period: usize) -> Option<f64> {
        if self.mid_history.len() < period { return None; }
        let alpha = 2.0 / (period as f64 + 1.0);
        let mut ema = self.mid_history[0];
        for &price in self.mid_history.iter().skip(1) {
            ema = price * alpha + ema * (1.0 - alpha);
        }
        Some(ema)
    }

    fn calculate_bbw(&self, period: usize) -> Option<f64> {
        if self.mid_history.len() < period { return None; }
        let slice = &self.mid_history[self.mid_history.len() - period..];
        let mean = slice.iter().sum::<f64>() / period as f64;
        let variance = slice.iter().map(|&p| (p - mean).powi(2)).sum::<f64>() / period as f64;
        let std_dev = variance.sqrt();
        if mean == 0.0 { return None; }
        Some((4.0 * std_dev) / mean)
    }

    fn get_mid_velocity(&self, period: usize) -> f64 {
        if self.mid_history.len() <= period { return 0.0; }
        let current = self.mid_history.last().unwrap();
        let prev = self.mid_history[self.mid_history.len() - 1 - period];
        current - prev
    }

    fn update_vwap(&mut self, price: f64, size: f64) {
        self.vwap_cum_pv += price * size;
        self.vwap_cum_v += size;
    }

    fn get_vwap(&self) -> Option<f64> {
        if self.vwap_cum_v == 0.0 { return None; }
        Some(self.vwap_cum_pv / self.vwap_cum_v)
    }
}

impl Strategy for MtfPlaybookStrategy {
    fn evaluate_regime(&mut self, _time_left_sec: f64) -> String {
        let bbw = self.calculate_bbw(2400).unwrap_or(0.0);
        if bbw < 0.005 {
            "MTF-Range".to_string()
        } else {
            "MTF-Trend".to_string()
        }
    }

    fn generate_desired_state(&mut self, alpha: &AlphaOutput, _risk: &RiskDecision, current_book: &BookState, time_left_sec: f64) -> DesiredState {
        let mid = current_book.up.bid.map(|b| b.price.0).unwrap_or(0.5);
        let vol = 1.0;

        self.mid_history.push(mid);
        self.update_vwap(mid, vol);

        if self.mid_history.len() > 2400 { self.mid_history.remove(0); }

        let mut target_up = 0.0;
        let mut target_down = 0.0;
        let mut maker_bid_up = None;
        let mut maker_bid_down = None;

        // HTF Context
        let htf_period = self.mid_history.len().max(2).min(2400);
        let ema200_htf = self.calculate_ema(htf_period).unwrap_or(mid);
        let bbw_htf = self.calculate_bbw(htf_period).unwrap_or(1.0);
        let regime_is_range = bbw_htf < 0.005;

        // TTF Setup
        let ttf_period = self.mid_history.len().max(2).min(80);
        let ema20_ttf = self.calculate_ema(ttf_period).unwrap_or(mid);
        let vwap = self.get_vwap().unwrap_or(mid);

        // LTF Timing
        let vel_fast = self.get_mid_velocity(5.min(self.mid_history.len().max(2) - 1));

        // Logic Playbook (T-240s onwards)
        if time_left_sec < 240.0 && time_left_sec > 1.0 {
            let q_model = alpha.q_model.0;

            if !regime_is_range {
                // TREND PLAYBOOK
                if mid > ema200_htf && mid < ema20_ttf && vel_fast > -0.001 && q_model > 0.505 {
                    if let Some(bid) = current_book.up.bid {
                        let roi = self.calculate_roi(bid.price.0);
                        if roi >= self.min_roi && roi <= self.max_roi {
                            target_up = (_risk.max_size.0 * 0.075) / bid.price.0;
                            maker_bid_up = Some(bid.price);
                            println!("[MTF-Trend] UP Pullback: roi={:.2} q={:.2} v_fast={:.4}", roi, q_model, vel_fast);
                        }
                    }
                }
                if mid < ema200_htf && mid > ema20_ttf && vel_fast < 0.001 && q_model < 0.495 {
                    if let Some(bid) = current_book.down.bid {
                        let roi = self.calculate_roi(bid.price.0);
                        if roi >= self.min_roi && roi <= self.max_roi {
                            target_down = (_risk.max_size.0 * 0.075) / bid.price.0;
                            maker_bid_down = Some(bid.price);
                            println!("[MTF-Trend] DOWN Pullback: roi={:.2} q={:.2} v_fast={:.4}", roi, q_model, vel_fast);
                        }
                    }
                }
            } else {
                // RANGE PLAYBOOK
                if mid < vwap - 0.005 && vel_fast > 0.001 && q_model > 0.505 {
                    if let Some(bid) = current_book.up.bid {
                        let roi = self.calculate_roi(bid.price.0);
                        if roi >= self.min_roi && roi <= self.max_roi {
                            target_up = (_risk.max_size.0 * 0.075) / bid.price.0;
                            maker_bid_up = Some(bid.price);
                            println!("[MTF-Range] UP Reversion: roi={:.2} vwap={:.4} dist={:.4}", roi, vwap, vwap - mid);
                        }
                    }
                }
                if mid > vwap + 0.005 && vel_fast < -0.001 && q_model < 0.495 {
                    if let Some(bid) = current_book.down.bid {
                        let roi = self.calculate_roi(bid.price.0);
                        if roi >= self.min_roi && roi <= self.max_roi {
                            target_down = (_risk.max_size.0 * 0.075) / bid.price.0;
                            maker_bid_down = Some(bid.price);
                            println!("[MTF-Range] DOWN Reversion: roi={:.2} vwap={:.4} dist={:.4}", roi, vwap, mid - vwap);
                        }
                    }
                }
            }
        }

        DesiredState {
            target_position_up: Stake(target_up),
            target_position_down: Stake(target_down),
            maker_bid_price_up: maker_bid_up,
            maker_ask_price_up: None,
            maker_bid_price_down: maker_bid_down,
            maker_ask_price_down: None,
        }
    }
}
