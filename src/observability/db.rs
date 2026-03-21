use rusqlite::{params, Connection, Result};

#[derive(Debug, Clone)]
pub struct TickRecord {
    pub event_time_ms: i64,
    pub received_at_ms: i64,
    pub price: f64,
    pub symbol: String,
    pub source: String,
}

#[derive(Debug, Clone)]
pub struct TickSummary {
    pub symbol: String,
    pub tick_count: i64,
    pub first_event_time_ms: i64,
    pub last_event_time_ms: i64,
    pub min_price: f64,
    pub max_price: f64,
}

pub struct TelemetryDb {
    conn: Connection,
}

impl TelemetryDb {
    pub fn new(db_path: &str) -> Result<Self> {
        let conn = Connection::open(db_path)?;
        let mut db = Self { conn };
        db.init_schema()?;
        Ok(db)
    }

    fn init_schema(&mut self) -> Result<()> {
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS ticks (
                timestamp INTEGER PRIMARY KEY,
                price REAL NOT NULL,
                symbol TEXT NOT NULL
            )",
            [],
        )?;

        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS raw_ticks (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                event_time_ms INTEGER NOT NULL,
                received_at_ms INTEGER NOT NULL,
                price REAL NOT NULL,
                symbol TEXT NOT NULL,
                source TEXT NOT NULL DEFAULT 'ws'
            )",
            [],
        )?;
        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_raw_ticks_symbol_time ON raw_ticks(symbol, event_time_ms)",
            [],
        )?;

        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS recorder_metadata (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                recorded_at_ms INTEGER NOT NULL,
                event_type TEXT NOT NULL,
                symbol TEXT,
                payload TEXT NOT NULL
            )",
            [],
        )?;
        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_recorder_metadata_type_time ON recorder_metadata(event_type, recorded_at_ms)",
            [],
        )?;

        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS trade_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp INTEGER NOT NULL,
                event_type TEXT NOT NULL,
                contract_type TEXT,
                setup TEXT,
                pnl REAL,
                details TEXT
            )",
            [],
        )?;

        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS alpha_signals (
                timestamp INTEGER PRIMARY KEY,
                q_model REAL,
                q_mkt REAL,
                q_low REAL,
                time_left_sec REAL
            )",
            [],
        )?;

        Ok(())
    }

    pub fn insert_tick(&self, timestamp: i64, price: f64, symbol: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO ticks (timestamp, price, symbol) VALUES (?1, ?2, ?3)",
            (timestamp, price, symbol),
        )?;
        self.insert_raw_tick(timestamp, timestamp, price, symbol, "executor")?;
        Ok(())
    }

    pub fn insert_raw_tick(
        &self,
        event_time_ms: i64,
        received_at_ms: i64,
        price: f64,
        symbol: &str,
        source: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO raw_ticks (event_time_ms, received_at_ms, price, symbol, source)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![event_time_ms, received_at_ms, price, symbol, source],
        )?;
        Ok(())
    }

    pub fn insert_recorder_metadata(
        &self,
        recorded_at_ms: i64,
        event_type: &str,
        symbol: Option<&str>,
        payload: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO recorder_metadata (recorded_at_ms, event_type, symbol, payload)
             VALUES (?1, ?2, ?3, ?4)",
            params![recorded_at_ms, event_type, symbol, payload],
        )?;
        Ok(())
    }

    pub fn prune_raw_ticks_older_than(&self, min_event_time_ms: i64) -> Result<usize> {
        self.conn.execute(
            "DELETE FROM raw_ticks WHERE event_time_ms < ?1",
            params![min_event_time_ms],
        )
    }

    pub fn summarize_ticks(&self, symbol: Option<&str>) -> Result<Vec<TickSummary>> {
        let sql = if symbol.is_some() {
            "SELECT symbol, COUNT(*), MIN(event_time_ms), MAX(event_time_ms), MIN(price), MAX(price)
             FROM raw_ticks WHERE symbol = ?1 GROUP BY symbol ORDER BY symbol"
        } else {
            "SELECT symbol, COUNT(*), MIN(event_time_ms), MAX(event_time_ms), MIN(price), MAX(price)
             FROM raw_ticks GROUP BY symbol ORDER BY symbol"
        };

        let mut stmt = self.conn.prepare(sql)?;
        let rows = if let Some(symbol) = symbol {
            stmt.query_map([symbol], |row| {
                Ok(TickSummary {
                    symbol: row.get(0)?,
                    tick_count: row.get(1)?,
                    first_event_time_ms: row.get(2)?,
                    last_event_time_ms: row.get(3)?,
                    min_price: row.get(4)?,
                    max_price: row.get(5)?,
                })?
            })?
        } else {
            stmt.query_map([], |row| {
                Ok(TickSummary {
                    symbol: row.get(0)?,
                    tick_count: row.get(1)?,
                    first_event_time_ms: row.get(2)?,
                    last_event_time_ms: row.get(3)?,
                    min_price: row.get(4)?,
                    max_price: row.get(5)?,
                })
            })?
        };

        rows.collect()
    }

    pub fn load_ticks(&self, symbol: &str, limit: usize) -> Result<Vec<TickRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT event_time_ms, received_at_ms, price, symbol, source
             FROM raw_ticks
             WHERE symbol = ?1
             ORDER BY event_time_ms ASC
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![symbol, limit as i64], |row| {
            Ok(TickRecord {
                event_time_ms: row.get(0)?,
                received_at_ms: row.get(1)?,
                price: row.get(2)?,
                symbol: row.get(3)?,
                source: row.get(4)?,
            })
        })?;

        rows.collect()
    }

    pub fn insert_trade_event(
        &self,
        timestamp: i64,
        event_type: &str,
        contract_type: Option<&str>,
        setup: Option<&str>,
        pnl: Option<f64>,
        details: Option<&str>,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO trade_events (timestamp, event_type, contract_type, setup, pnl, details)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            (timestamp, event_type, contract_type, setup, pnl, details),
        )?;
        Ok(())
    }

    pub fn insert_alpha_signal(
        &self,
        timestamp: i64,
        q_model: Option<f64>,
        q_mkt: Option<f64>,
        q_low: f64,
        time_left_sec: f64,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO alpha_signals (timestamp, q_model, q_mkt, q_low, time_left_sec)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            (timestamp, q_model, q_mkt, q_low, time_left_sec),
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_tick_summary_round_trip() {
        let db = TelemetryDb::new(":memory:").unwrap();
        db.insert_raw_tick(1_000, 1_001, 100.0, "R_100", "recorder")
            .unwrap();
        db.insert_raw_tick(2_000, 2_001, 101.5, "R_100", "recorder")
            .unwrap();

        let summaries = db.summarize_ticks(Some("R_100")).unwrap();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].tick_count, 2);

        let ticks = db.load_ticks("R_100", 10).unwrap();
        assert_eq!(ticks.len(), 2);
        assert_eq!(ticks[0].source, "recorder");
    }
}
