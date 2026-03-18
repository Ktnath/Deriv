import sqlite3
import pandas as pd
import numpy as np
import time

DB_PATH = "d:/Deriv/deriv_metrics.db"
OUTPUT_PATH = "d:/Deriv/dataset_ml.csv"

def calculate_rsi(prices, period=14):
    delta = prices.diff()
    gain = (delta.where(delta > 0, 0)).rolling(window=period).mean()
    loss = (-delta.where(delta < 0, 0)).rolling(window=period).mean()
    rs = gain / loss
    return 100 - (100 / (1 + rs))

def main():
    print("Connecting to DB to load ticks...")
    conn = sqlite3.connect(DB_PATH)
    
    # Load all ticks
    query = "SELECT timestamp, price FROM ticks WHERE symbol = 'R_100' ORDER BY timestamp ASC"
    df = pd.read_sql_query(query, conn)
    conn.close()
    
    print(f"Loaded {len(df)} ticks.")
    if len(df) < 500:
        print("Not enough data to build dataset.")
        return

    # Clean duplicates if any
    df = df.drop_duplicates(subset=['timestamp'])
    
    # ---------------------------------------------------------
    # 1. Labeling : The 5-Minute Projection (+300s)
    # ---------------------------------------------------------
    print("Projecting future prices (+300s) for labels...")
    
    # We create a copy of the dataframe to merge it as the future state
    df_future = df[['timestamp', 'price']].copy()
    df_future = df_future.rename(columns={'timestamp': 'future_timestamp', 'price': 'future_price'})
    
    # The target time is exactly timestamp + 300
    df['target_time'] = df['timestamp'] + 300
    
    # Sort both for merge_asof
    df = df.sort_values('target_time')
    df_future = df_future.sort_values('future_timestamp')
    
    # Match each tick's target_time with the closest future_timestamp (forward direction)
    # We allow a tolerance of up to 5 seconds. If no tick exists precisely 5 mins later, 
    # we take the nearest one within 5s.
    df = pd.merge_asof(
        df, 
        df_future, 
        left_on='target_time', 
        right_on='future_timestamp', 
        direction='forward',
        tolerance=5
    )
    
    # Compute Target
    # 1 = UP (future_price > price), 0 = DOWN (future_price <= price)
    # NA values mean there's no data 5 mins ahead (e.g. the last 5 mins of the dataset)
    df['target'] = np.where(df['future_price'] > df['price'], 1, 0)
    
    # Drop rows where we couldn't find a future price
    df = df.dropna(subset=['future_price'])
    
    # Restore sorting to natural time
    df = df.sort_values('timestamp').reset_index(drop=True)
    
    # ---------------------------------------------------------
    # 2. Feature Engineering
    # ---------------------------------------------------------
    print("Computing Technical Features (SMA, RSI, Bollinger)...")
    
    # Basic Return
    df['returns'] = df['price'].pct_change()
    
    # Moving Averages
    df['sma_10'] = df['price'].rolling(window=10).mean()
    df['sma_30'] = df['price'].rolling(window=30).mean()
    
    # Relationships
    df['dist_sma10'] = (df['price'] - df['sma_10']) / df['sma_10']
    df['dist_sma30'] = (df['price'] - df['sma_30']) / df['sma_30']
    
    # RSI 14
    df['rsi_14'] = calculate_rsi(df['price'], period=14)
    
    # Bollinger Bands (20 periods, 2 std dev)
    sma_20 = df['price'].rolling(window=20).mean()
    std_20 = df['price'].rolling(window=20).std()
    bb_upper = sma_20 + (2 * std_20)
    bb_lower = sma_20 - (2 * std_20)
    
    # BB Position: 0 means at lower band, 1 means at upper band
    df['bb_position'] = (df['price'] - bb_lower) / (bb_upper - bb_lower)
    # BB Width (Volatility)
    df['bb_width'] = (bb_upper - bb_lower) / sma_20
    
    # Volatility directly
    df['volatility_20'] = df['returns'].rolling(window=20).std()
    
    # --- Custom Indicators ---
    
    # 1. Fractal Efficiency (Kaufman Efficiency Ratio variant)
    # Ratio of net movement over sum of absolute ticks
    n_period = 10
    df['net_change'] = (df['price'] - df['price'].shift(n_period)).abs()
    df['sum_abs_changes'] = df['price'].diff().abs().rolling(window=n_period).sum()
    df['fractal_efficiency'] = df['net_change'] / df['sum_abs_changes']
    
    # 2. Momentum Acceleration
    # Difference between a fast momentum and a slow momentum
    mom_fast = df['price'].diff(5)
    mom_slow = df['price'].diff(20)
    df['mom_accel'] = mom_fast - mom_slow
    
    # 3. Volatility Heat
    # Ratio of short-term volatility over long-term volatility
    df['vol_heat'] = df['price'].rolling(window=10).std() / df['price'].rolling(window=50).std()
    
    # Drop rows with NaN features (the first 50 rows typically due to vol_heat)
    print("Dropping rows with incomplete features...")
    df = df.dropna()
    
    # Select final columns
    features = [
        'returns', 'dist_sma10', 'dist_sma30', 
        'rsi_14', 'bb_position', 'bb_width', 'volatility_20',
        'fractal_efficiency', 'mom_accel', 'vol_heat'
    ]
    
    final_cols = ['timestamp', 'price'] + features + ['future_timestamp', 'future_price', 'target']
    df_final = df[final_cols]
    
    print(df_final.head())
    
    print(f"\nFinal Dataset Size: {len(df_final)} samples.")
    print("Target Distribution:")
    print(df_final['target'].value_counts(normalize=True) * 100)
    
    # Save to CSV
    print(f"Saving to {OUTPUT_PATH}...")
    df_final.to_csv(OUTPUT_PATH, index=False)
    print("Saved successfully!")

if __name__ == "__main__":
    start_time = time.time()
    main()
    print(f"Done in {time.time() - start_time:.2f} seconds.")
