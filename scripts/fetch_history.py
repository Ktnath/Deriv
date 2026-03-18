import asyncio
import json
import websockets
import os
import sqlite3
import time
from dotenv import load_dotenv

load_dotenv("d:/Deriv/.env")

API_TOKEN = os.getenv("DERIV_API_TOKEN", "")
APP_ID = "36544" 
SYMBOL = "R_100"

async def fetch_history(months_back=6):
    if not API_TOKEN:
        print("Please set DERIV_API_TOKEN in .env")
        return

    uri = f"wss://ws.derivws.com/websockets/v3?app_id={APP_ID}"
    
    # Initialize DB
    db_path = "d:/Deriv/deriv_metrics.db"
    conn = sqlite3.connect(db_path)
    c = conn.cursor()
    c.execute('''CREATE TABLE IF NOT EXISTS ticks
                 (timestamp INTEGER PRIMARY KEY, price REAL NOT NULL, symbol TEXT NOT NULL)''')
    conn.commit()

    async with websockets.connect(uri) as websocket:
        # Authorize
        auth_req = {"authorize": API_TOKEN}
        await websocket.send(json.dumps(auth_req))
        auth_res = json.loads(await websocket.recv())
        if "error" in auth_res:
            print("Auth Error:", auth_res["error"])
            return
        
        print("Authorized! Starting massive historical data fetch...")

        # Time targets
        end = int(time.time())
        target_start = end - (months_back * 30 * 24 * 60 * 60) # roughly X months in seconds
        
        total_fetched = 0
        requests_count = 0
        
        while end > target_start:
            req = {
                "ticks_history": SYMBOL,
                "end": "latest" if requests_count == 0 else str(end),
                "count": 5000,
                "style": "ticks"
            }
            await websocket.send(json.dumps(req))
            res = json.loads(await websocket.recv())
            
            if "error" in res:
                print("History Error:", res["error"])
                # Could be rate limit, let's wait a bit and retry
                await asyncio.sleep(5)
                continue
                
            history = res.get("history", {})
            times = history.get("times", [])
            prices = history.get("prices", [])
            
            if not times:
                print("No more historical data available from broker.")
                break
                
            # Insert into DB
            records = [(t, p, SYMBOL) for t, p in zip(times, prices)]
            c.executemany("INSERT OR IGNORE INTO ticks (timestamp, price, symbol) VALUES (?, ?, ?)", records)
            conn.commit()
            
            fetched = len(times)
            total_fetched += fetched
            oldest_ts = times[0]
            
            requests_count += 1
            if requests_count % 10 == 0:
                print(f"Progress: Fetched {total_fetched} ticks so far. Reached date: {time.strftime('%Y-%m-%d %H:%M:%S', time.localtime(oldest_ts))}")
            
            end = oldest_ts - 1 # Next batch ends exactly before the oldest tick
            
            await asyncio.sleep(0.5) # Gentle rate limiting

        print(f"Done! Total ticks fetched: {total_fetched}")
    
    conn.close()

if __name__ == "__main__":
    import sys
    months = 6
    if len(sys.argv) > 1:
        months = int(sys.argv[1])
    asyncio.run(fetch_history(months_back=months))
