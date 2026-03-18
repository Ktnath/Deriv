use rusqlite::{Connection, Result};

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
        Ok(())
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
