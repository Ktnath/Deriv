import sqlite3
import pandas as pd

def read_telemetry():
    db_path = "data/telemetry_demo.sqlite"
    with open("telemetry_report.txt", "w", encoding="utf-8") as f:
        f.write(f"Reading DB: {db_path}\n\n")
        try:
            conn = sqlite3.connect(db_path)
            tables = pd.read_sql("SELECT name FROM sqlite_master WHERE type='table'", conn)["name"].tolist()
            f.write("=== TABLES ===\n")
            f.write(str(tables) + "\n\n")
            f.write("=== RECENT DECISIONS (10) ===\n")
            f.write(pd.read_sql("SELECT timestamp_ms, contract_direction, decision, proposed_stake, q_model, q_final, edge, regime FROM decision_events ORDER BY timestamp_ms DESC LIMIT 10", conn).to_string() + "\n\n")
            f.write("=== TRADES (from executed_trades) ===\n")
            f.write(pd.read_sql("SELECT * FROM executed_trades ORDER BY timestamp_ms DESC LIMIT 10", conn).to_string() + "\n\n")
            f.write("=== INTENTS (from trade_intents) ===\n")
            f.write(pd.read_sql("SELECT * FROM trade_intents ORDER BY timestamp_ms DESC LIMIT 10", conn).to_string() + "\n")
        except Exception as e:
            f.write("Error accessing DB: " + str(e) + "\n")

if __name__ == "__main__":
    read_telemetry()
