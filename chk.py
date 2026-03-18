import sqlite3
import datetime

db_path = "d:/Deriv/deriv_metrics.db"
try:
    with sqlite3.connect(db_path) as conn:
        c = conn.cursor()
        c.execute("SELECT COUNT(timestamp), MIN(timestamp) FROM ticks WHERE symbol='R_100'")
        count, oldest = c.fetchone()
        oldest_date = datetime.datetime.fromtimestamp(oldest).strftime('%Y-%m-%d %H:%M:%S') if oldest else "N/A"
        print(f"Total Ticks: {count}")
        print(f"Oldest record: {oldest_date}")
except Exception as e:
    print(e)
