use rusqlite::{params, Connection, OptionalExtension, Result};

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

#[derive(Debug, Clone)]
pub struct RunMetadata {
    pub run_id: String,
    pub binary_type: String,
    pub model_version: String,
    pub strategy_version: String,
    pub prior_version: String,
    pub config_fingerprint: String,
    pub started_at_ms: i64,
}

#[derive(Debug, Clone)]
pub struct RunDecisionRecord<'a> {
    pub run_id: &'a str,
    pub timestamp_ms: i64,
    pub symbol: &'a str,
    pub price: f64,
    pub regime: &'a str,
    pub prior_mode: &'a str,
    pub strategy_mode: &'a str,
    pub model_metadata: &'a str,
    pub contract_direction: Option<&'a str>,
    pub benchmark_signal: &'a str,
    pub decision: &'a str,
    pub rejection_reason: Option<&'a str>,
    pub edge: f64,
    pub q_prior: f64,
    pub q_model: f64,
    pub q_final: f64,
    pub q_low: f64,
    pub q_high: f64,
    pub confidence: f64,
    pub time_left_sec: f64,
    pub proposed_stake: f64,
    pub executed_stake: f64,
    pub feature_summary: &'a str,
}

#[derive(Debug, Clone)]
pub struct TradeIntentRecord<'a> {
    pub run_id: &'a str,
    pub decision_id: i64,
    pub timestamp_ms: i64,
    pub contract_direction: &'a str,
    pub proposed_stake: f64,
    pub executed_stake: f64,
    pub execution_enabled: bool,
    pub intent_status: &'a str,
    pub rejection_reason: Option<&'a str>,
}

#[derive(Debug, Clone)]
pub struct ExecutedTradeRecord<'a> {
    pub run_id: &'a str,
    pub trade_intent_id: i64,
    pub timestamp_ms: i64,
    pub contract_id: Option<&'a str>,
    pub contract_direction: &'a str,
    pub stake: f64,
    pub payout: Option<f64>,
    pub pnl: Option<f64>,
    pub exit_reason: Option<&'a str>,
    pub status: &'a str,
}

#[derive(Debug, Clone)]
pub struct ReplayReport {
    pub decisions: i64,
    pub signal_intents: i64,
    pub trades: i64,
    pub average_edge: f64,
    pub pnl_sum: f64,
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
        self.conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS ticks (
                timestamp INTEGER PRIMARY KEY,
                price REAL NOT NULL,
                symbol TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS raw_ticks (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                event_time_ms INTEGER NOT NULL,
                received_at_ms INTEGER NOT NULL,
                price REAL NOT NULL,
                symbol TEXT NOT NULL,
                source TEXT NOT NULL DEFAULT 'ws'
            );
            CREATE INDEX IF NOT EXISTS idx_raw_ticks_symbol_time ON raw_ticks(symbol, event_time_ms);
            CREATE TABLE IF NOT EXISTS recorder_metadata (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                recorded_at_ms INTEGER NOT NULL,
                event_type TEXT NOT NULL,
                symbol TEXT,
                payload TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_recorder_metadata_type_time ON recorder_metadata(event_type, recorded_at_ms);
            CREATE TABLE IF NOT EXISTS trade_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp INTEGER NOT NULL,
                event_type TEXT NOT NULL,
                contract_type TEXT,
                setup TEXT,
                pnl REAL,
                details TEXT
            );
            CREATE TABLE IF NOT EXISTS experiment_runs (
                run_id TEXT PRIMARY KEY,
                binary_type TEXT NOT NULL,
                model_version TEXT NOT NULL,
                strategy_version TEXT NOT NULL,
                prior_version TEXT NOT NULL,
                config_fingerprint TEXT NOT NULL,
                started_at_ms INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS decision_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                run_id TEXT NOT NULL,
                timestamp_ms INTEGER NOT NULL,
                symbol TEXT NOT NULL,
                price REAL NOT NULL,
                regime TEXT NOT NULL,
                prior_mode TEXT NOT NULL,
                strategy_mode TEXT NOT NULL,
                model_metadata TEXT NOT NULL,
                contract_direction TEXT,
                benchmark_signal TEXT NOT NULL,
                decision TEXT NOT NULL,
                rejection_reason TEXT,
                edge REAL NOT NULL,
                q_prior REAL NOT NULL,
                q_model REAL NOT NULL,
                q_final REAL NOT NULL,
                q_low REAL NOT NULL,
                q_high REAL NOT NULL,
                confidence REAL NOT NULL,
                time_left_sec REAL NOT NULL,
                proposed_stake REAL NOT NULL,
                executed_stake REAL NOT NULL,
                feature_summary TEXT NOT NULL,
                FOREIGN KEY(run_id) REFERENCES experiment_runs(run_id)
            );
            CREATE INDEX IF NOT EXISTS idx_decision_events_run_time ON decision_events(run_id, timestamp_ms);
            CREATE TABLE IF NOT EXISTS trade_intents (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                run_id TEXT NOT NULL,
                decision_id INTEGER NOT NULL,
                timestamp_ms INTEGER NOT NULL,
                contract_direction TEXT NOT NULL,
                proposed_stake REAL NOT NULL,
                executed_stake REAL NOT NULL,
                execution_enabled INTEGER NOT NULL,
                intent_status TEXT NOT NULL,
                rejection_reason TEXT,
                FOREIGN KEY(run_id) REFERENCES experiment_runs(run_id),
                FOREIGN KEY(decision_id) REFERENCES decision_events(id)
            );
            CREATE INDEX IF NOT EXISTS idx_trade_intents_run_time ON trade_intents(run_id, timestamp_ms);
            CREATE TABLE IF NOT EXISTS executed_trades (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                run_id TEXT NOT NULL,
                trade_intent_id INTEGER NOT NULL,
                timestamp_ms INTEGER NOT NULL,
                contract_id TEXT,
                contract_direction TEXT NOT NULL,
                stake REAL NOT NULL,
                payout REAL,
                pnl REAL,
                exit_reason TEXT,
                status TEXT NOT NULL,
                FOREIGN KEY(run_id) REFERENCES experiment_runs(run_id),
                FOREIGN KEY(trade_intent_id) REFERENCES trade_intents(id)
            );
            CREATE INDEX IF NOT EXISTS idx_executed_trades_run_time ON executed_trades(run_id, timestamp_ms);
            CREATE VIEW IF NOT EXISTS alpha_signals AS
                SELECT timestamp_ms AS timestamp, q_model, q_prior, q_final, q_low, q_high, confidence, time_left_sec, regime
                FROM decision_events;
            CREATE VIEW IF NOT EXISTS decision_snapshots AS
                SELECT id, timestamp_ms AS timestamp, symbol, regime, contract_direction, decision, rejection_reason, edge,
                       q_prior, q_model, q_final, confidence, feature_summary
                FROM decision_events;
            "
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
            "INSERT INTO raw_ticks (event_time_ms, received_at_ms, price, symbol, source) VALUES (?1, ?2, ?3, ?4, ?5)",
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
            "INSERT INTO recorder_metadata (recorded_at_ms, event_type, symbol, payload) VALUES (?1, ?2, ?3, ?4)",
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
            "SELECT symbol, COUNT(*), MIN(event_time_ms), MAX(event_time_ms), MIN(price), MAX(price) FROM raw_ticks WHERE symbol = ?1 GROUP BY symbol ORDER BY symbol"
        } else {
            "SELECT symbol, COUNT(*), MIN(event_time_ms), MAX(event_time_ms), MIN(price), MAX(price) FROM raw_ticks GROUP BY symbol ORDER BY symbol"
        };
        let mut stmt = self.conn.prepare(sql)?;
        let rows = if let Some(symbol) = symbol {
            stmt.query_map([symbol], map_summary)?
        } else {
            stmt.query_map([], map_summary)?
        };
        rows.collect()
    }

    pub fn load_ticks(&self, symbol: &str, limit: usize) -> Result<Vec<TickRecord>> {
        let mut stmt = self.conn.prepare("SELECT event_time_ms, received_at_ms, price, symbol, source FROM raw_ticks WHERE symbol = ?1 ORDER BY event_time_ms ASC LIMIT ?2")?;
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

    pub fn upsert_run_metadata(&self, run: &RunMetadata) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO experiment_runs (run_id, binary_type, model_version, strategy_version, prior_version, config_fingerprint, started_at_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![run.run_id, run.binary_type, run.model_version, run.strategy_version, run.prior_version, run.config_fingerprint, run.started_at_ms],
        )?;
        Ok(())
    }

    pub fn insert_run_decision(&self, record: &RunDecisionRecord<'_>) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO decision_events (run_id, timestamp_ms, symbol, price, regime, prior_mode, strategy_mode, model_metadata, contract_direction, benchmark_signal, decision, rejection_reason, edge, q_prior, q_model, q_final, q_low, q_high, confidence, time_left_sec, proposed_stake, executed_stake, feature_summary) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23)",
            params![record.run_id, record.timestamp_ms, record.symbol, record.price, record.regime, record.prior_mode, record.strategy_mode, record.model_metadata, record.contract_direction, record.benchmark_signal, record.decision, record.rejection_reason, record.edge, record.q_prior, record.q_model, record.q_final, record.q_low, record.q_high, record.confidence, record.time_left_sec, record.proposed_stake, record.executed_stake, record.feature_summary],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn insert_trade_intent(&self, record: &TradeIntentRecord<'_>) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO trade_intents (run_id, decision_id, timestamp_ms, contract_direction, proposed_stake, executed_stake, execution_enabled, intent_status, rejection_reason) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![record.run_id, record.decision_id, record.timestamp_ms, record.contract_direction, record.proposed_stake, record.executed_stake, i64::from(record.execution_enabled), record.intent_status, record.rejection_reason],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn insert_executed_trade(&self, record: &ExecutedTradeRecord<'_>) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO executed_trades (run_id, trade_intent_id, timestamp_ms, contract_id, contract_direction, stake, payout, pnl, exit_reason, status) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![record.run_id, record.trade_intent_id, record.timestamp_ms, record.contract_id, record.contract_direction, record.stake, record.payout, record.pnl, record.exit_reason, record.status],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn update_trade_intent_status(
        &self,
        intent_id: i64,
        intent_status: &str,
        executed_stake: f64,
        rejection_reason: Option<&str>,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE trade_intents SET intent_status = ?2, executed_stake = ?3, rejection_reason = ?4 WHERE id = ?1",
            params![intent_id, intent_status, executed_stake, rejection_reason],
        )?;
        Ok(())
    }

    pub fn update_executed_trade_lifecycle(
        &self,
        trade_id: i64,
        timestamp_ms: i64,
        payout: Option<f64>,
        pnl: Option<f64>,
        exit_reason: Option<&str>,
        status: &str,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE executed_trades SET timestamp_ms = ?2, payout = ?3, pnl = ?4, exit_reason = ?5, status = ?6 WHERE id = ?1",
            params![trade_id, timestamp_ms, payout, pnl, exit_reason, status],
        )?;
        Ok(())
    }

    pub fn find_latest_executed_trade_for_entry(
        &self,
        run_id: &str,
        entered_at_ms: i64,
    ) -> Result<Option<i64>> {
        self.conn.query_row(
            "SELECT et.id FROM executed_trades et JOIN trade_intents ti ON ti.id = et.trade_intent_id WHERE et.run_id = ?1 AND ti.timestamp_ms = ?2 ORDER BY et.id DESC LIMIT 1",
            params![run_id, entered_at_ms],
            |row| row.get(0),
        ).optional()
    }

    pub fn latest_run_report(&self, run_id: &str) -> Result<ReplayReport> {
        self.conn.query_row(
            "SELECT COUNT(*), COALESCE((SELECT COUNT(*) FROM trade_intents WHERE run_id = ?1 AND intent_status = 'signal_only'),0), COALESCE((SELECT COUNT(*) FROM executed_trades WHERE run_id = ?1),0), COALESCE(AVG(edge),0), COALESCE((SELECT SUM(COALESCE(pnl,0)) FROM executed_trades WHERE run_id = ?1),0) FROM decision_events WHERE run_id = ?1",
            [run_id],
            |row| Ok(ReplayReport { decisions: row.get(0)?, signal_intents: row.get(1)?, trades: row.get(2)?, average_edge: row.get(3)?, pnl_sum: row.get(4)? }),
        )
    }

    pub fn regime_counts(&self, run_id: &str) -> Result<Vec<(String, i64)>> {
        let mut stmt = self.conn.prepare("SELECT regime, COUNT(*) FROM decision_events WHERE run_id = ?1 GROUP BY regime ORDER BY COUNT(*) DESC, regime")?;
        stmt.query_map([run_id], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect()
    }

    pub fn rejection_counts(&self, run_id: &str) -> Result<Vec<(String, i64)>> {
        let mut stmt = self.conn.prepare("SELECT COALESCE(rejection_reason, 'none'), COUNT(*) FROM decision_events WHERE run_id = ?1 GROUP BY COALESCE(rejection_reason, 'none') ORDER BY COUNT(*) DESC, 1")?;
        stmt.query_map([run_id], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect()
    }

    pub fn win_loss_summary(&self, run_id: &str) -> Result<(i64, i64, f64)> {
        self.conn.query_row(
            "SELECT SUM(CASE WHEN COALESCE(pnl,0) > 0 THEN 1 ELSE 0 END), SUM(CASE WHEN COALESCE(pnl,0) <= 0 THEN 1 ELSE 0 END), COALESCE(SUM(COALESCE(pnl,0)),0) FROM executed_trades WHERE run_id = ?1",
            [run_id],
            |row| Ok((row.get::<_, Option<i64>>(0)?.unwrap_or(0), row.get::<_, Option<i64>>(1)?.unwrap_or(0), row.get(2)?)),
        )
    }

    pub fn latest_run_id_for_binary(&self, binary_type: &str) -> Result<Option<String>> {
        self.conn.query_row(
            "SELECT run_id FROM experiment_runs WHERE binary_type = ?1 ORDER BY started_at_ms DESC LIMIT 1",
            [binary_type],
            |row| row.get(0),
        ).optional()
    }
}

fn map_summary(row: &rusqlite::Row<'_>) -> Result<TickSummary> {
    Ok(TickSummary {
        symbol: row.get(0)?,
        tick_count: row.get(1)?,
        first_event_time_ms: row.get(2)?,
        last_event_time_ms: row.get(3)?,
        min_price: row.get(4)?,
        max_price: row.get(5)?,
    })
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
        assert_eq!(summaries[0].tick_count, 2);
        assert_eq!(db.load_ticks("R_100", 10).unwrap().len(), 2);
    }

    #[test]
    fn run_decision_trade_round_trip() {
        let db = TelemetryDb::new(":memory:").unwrap();
        db.upsert_run_metadata(&RunMetadata {
            run_id: "r1".into(),
            binary_type: "research".into(),
            model_version: "quant-only".into(),
            strategy_version: "process".into(),
            prior_version: "process-v1".into(),
            config_fingerprint: "abc".into(),
            started_at_ms: 1,
        })
        .unwrap();
        let decision_id = db
            .insert_run_decision(&RunDecisionRecord {
                run_id: "r1",
                timestamp_ms: 10,
                symbol: "R_100",
                price: 100.0,
                regime: "calm",
                prior_mode: "process-v1",
                strategy_mode: "process",
                model_metadata: "quant-only",
                contract_direction: Some("CALL"),
                benchmark_signal: "CALL",
                decision: "signal",
                rejection_reason: None,
                edge: 0.03,
                q_prior: 0.51,
                q_model: 0.54,
                q_final: 0.53,
                q_low: 0.51,
                q_high: 0.55,
                confidence: 1.0,
                time_left_sec: 120.0,
                proposed_stake: 1.2,
                executed_stake: 0.0,
                feature_summary: "{}",
            })
            .unwrap();
        let intent_id = db
            .insert_trade_intent(&TradeIntentRecord {
                run_id: "r1",
                decision_id,
                timestamp_ms: 10,
                contract_direction: "CALL",
                proposed_stake: 1.2,
                executed_stake: 0.0,
                execution_enabled: false,
                intent_status: "signal_only",
                rejection_reason: None,
            })
            .unwrap();
        db.insert_executed_trade(&ExecutedTradeRecord {
            run_id: "r1",
            trade_intent_id: intent_id,
            timestamp_ms: 20,
            contract_id: Some("c1"),
            contract_direction: "CALL",
            stake: 1.2,
            payout: Some(2.34),
            pnl: Some(1.14),
            exit_reason: Some("fixture_win"),
            status: "settled",
        })
        .unwrap();
        let report = db.latest_run_report("r1").unwrap();
        assert_eq!(report.decisions, 1);
        assert_eq!(report.signal_intents, 1);
        assert_eq!(report.trades, 1);
        assert!(report.pnl_sum > 1.0);
    }
}
