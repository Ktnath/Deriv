use rusqlite::Connection;
use bot_core::types::{Tick, UnixMs};
use std::path::Path;

pub struct OfflineTickFeeder {
    pub db_path: String,
    pub symbol: String,
}

impl OfflineTickFeeder {
    pub fn new(db_path: &str, symbol: &str) -> Self {
        Self {
            db_path: db_path.to_string(),
            symbol: symbol.to_string(),
        }
    }

    /// Load all ticks for a specific symbol from SQLite
    pub fn load_ticks(&self) -> Result<Vec<Tick>, rusqlite::Error> {
        let conn = Connection::open(&self.db_path)?;
        let mut stmt = conn.prepare(
            "SELECT timestamp, price FROM ticks WHERE symbol = ?1 ORDER BY timestamp ASC",
        )?;
        
        let tick_iter = stmt.query_map([&self.symbol], |row| {
            let ts: i64 = row.get(0)?;
            let price: f64 = row.get(1)?;
            Ok(Tick {
                epoch: ts,
                price,
                symbol: self.symbol.clone(),
                quote_ms: Some(ts * 1000), // convert to ms
            })
        })?;

        let mut ticks = Vec::new();
        for tick in tick_iter {
            ticks.push(tick?);
        }
        
        Ok(ticks)
    }
}
