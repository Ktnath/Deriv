import sqlite3
import pandas as pd

def read_loss_signals():
    db_path = "data/telemetry_demo.sqlite"
    with open("loss_analysis_report.txt", "w", encoding="utf-8") as f:
        try:
            conn = sqlite3.connect(db_path)
            
            query = """
            SELECT 
                t.id as trade_id,
                t.status,
                t.exit_reason,
                t.pnl,
                t.stake,
                d.timestamp_ms,
                d.contract_direction,
                d.regime,
                d.price,
                d.edge,
                d.q_prior,
                d.q_model,
                d.q_final,
                d.q_low,
                d.q_high,
                d.confidence,
                d.model_metadata,
                d.feature_summary
            FROM executed_trades t
            JOIN trade_intents i ON t.trade_intent_id = i.id
            JOIN decision_events d ON i.decision_id = d.id
            WHERE t.pnl < 0
            ORDER BY t.timestamp_ms DESC
            """
            
            df = pd.read_sql(query, conn)
            
            f.write("=== ANALYSE DES TRADES PERDANTS ===\n")
            f.write(df.to_string() + "\n\n")
            
            # Formatted readable summary
            f.write("=== RESUME DETAILLE ===\n")
            for _, row in df.iterrows():
                f.write(f"Trade ID: {row['trade_id']} | Date: {row['timestamp_ms']}\n")
                f.write(f"PNL: {row['pnl']} | Stake: {row['stake']}\n")
                f.write(f"Signal: {row['contract_direction']} at Price: {row['price']}\n")
                f.write(f"Math Params:\n")
                f.write(f"  - Regime: {row['regime']}\n")
                f.write(f"  - Edge: {row['edge']}\n")
                f.write(f"  - Q-Model: {row['q_model']} | Q-Final: {row['q_final']}\n")
                f.write(f"  - Q-Low: {row['q_low']} | Q-High: {row['q_high']}\n")
                f.write(f"  - Confidence: {row['confidence']}\n")
                f.write(f"Feature summary: {row['feature_summary']}\n")
                f.write("-" * 50 + "\n")
                
        except Exception as e:
            f.write("Error: " + str(e))

if __name__ == "__main__":
    read_loss_signals()
