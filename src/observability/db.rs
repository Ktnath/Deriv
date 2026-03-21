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

pub struct AlphaSignalRecord<'a> {
    pub timestamp: i64,
    pub q_model: Option<f64>,
    pub q_prior: Option<f64>,
    pub q_final: f64,
    pub q_low: f64,
    pub q_high: f64,
    pub confidence: f64,
    pub time_left_sec: f64,
    pub regime: &'a str,
}

pub struct DecisionSnapshotRecord<'a> {
    pub timestamp: i64,
    pub symbol: &'a str,
    pub regime: &'a str,
    pub contract_direction: Option<&'a str>,
    pub decision: &'a str,
    pub rejection_reason: Option<&'a str>,
    pub edge: f64,
    pub q_prior: f64,
    pub q_model: f64,
    pub q_final: f64,
    pub confidence: f64,
    pub feature_summary: &'a str,
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
                q_prior REAL,
                q_final REAL NOT NULL,
                q_low REAL NOT NULL,
                q_high REAL NOT NULL,
                confidence REAL NOT NULL,
                time_left_sec REAL NOT NULL,
                regime TEXT NOT NULL DEFAULT 'unknown'
            )",
            [],
        )?;

        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS decision_snapshots (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp INTEGER NOT NULL,
                symbol TEXT NOT NULL,
                regime TEXT NOT NULL,
                contract_direction TEXT,
                decision TEXT NOT NULL,
                rejection_reason TEXT,
                edge REAL NOT NULL,
                q_prior REAL NOT NULL,
                q_model REAL NOT NULL,
                q_final REAL NOT NULL,
                confidence REAL NOT NULL,
                feature_summary TEXT NOT NULL
            )",
            [],
        )?;
        self.conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_decision_snapshots_symbol_time ON decision_snapshots(symbol, timestamp)",
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
                })
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

    pub fn insert_alpha_signal(&self, record: AlphaSignalRecord<'_>) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO alpha_signals (timestamp, q_model, q_prior, q_final, q_low, q_high, confidence, time_left_sec, regime)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                record.timestamp,
                record.q_model,
                record.q_prior,
                record.q_final,
                record.q_low,
                record.q_high,
                record.confidence,
                record.time_left_sec,
                record.regime,
            ],
        )?;
        Ok(())
    }

    pub fn insert_decision_snapshot(&self, record: DecisionSnapshotRecord<'_>) -> Result<()> {
        self.conn.execute(
            "INSERT INTO decision_snapshots (timestamp, symbol, regime, contract_direction, decision, rejection_reason, edge, q_prior, q_model, q_final, confidence, feature_summary)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                record.timestamp,
                record.symbol,
                record.regime,
                record.contract_direction,
                record.decision,
                record.rejection_reason,
                record.edge,
                record.q_prior,
                record.q_model,
                record.q_final,
                record.confidence,
                record.feature_summary,
            ],
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

    #[test]
    fn decision_snapshot_round_trip() {
        let db = TelemetryDb::new(":memory:").unwrap();
        db.insert_alpha_signal(AlphaSignalRecord {
            timestamp: 10,
            q_model: Some(0.54),
            q_prior: Some(0.51),
            q_final: 0.53,
            q_low: 0.51,
            q_high: 0.55,
            confidence: 1.0,
            time_left_sec: 120.0,
            regime: "calm",
        })
        .unwrap();
        db.insert_decision_snapshot(DecisionSnapshotRecord {
            timestamp: 10,
            symbol: "R_100",
            regime: "calm",
            contract_direction: Some("CALL"),
            decision: "enter",
            rejection_reason: None,
            edge: 0.03,
            q_prior: 0.51,
            q_model: 0.54,
            q_final: 0.53,
            confidence: 1.0,
            feature_summary: r#"{"run_length":4}"#,
        })
        .unwrap();

        let count: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM decision_snapshots", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 1);
    }
}
