import os
import sqlite3
import pandas as pd
import numpy as np
import torch
import torch.nn as nn
import gymnasium as gym
from gymnasium import spaces
import time

try:
    from stable_baselines3 import PPO
    from stable_baselines3.common.vec_env import DummyVecEnv
    has_sb3 = True
except ImportError:
    has_sb3 = False
    print("WARNING: stable-baselines3 or gymnasium not installed. Please pip install it.")

DB_PATH = "data/telemetry_demo.sqlite"
ONNX_OUTPUT_PATH = "model_v3_rl.onnx"

def calculate_rsi(prices, period=14):
    delta = prices.diff()
    gain = (delta.where(delta > 0, 0)).rolling(window=period).mean()
    loss = (-delta.where(delta < 0, 0)).rolling(window=period).mean()
    rs = gain / loss
    return 100 - (100 / (1 + rs))

def load_and_prepare_data():
    print("Connecting to DB to load ticks...")
    if not os.path.exists(DB_PATH):
        raise FileNotFoundError(f"{DB_PATH} not found.")
    
    conn = sqlite3.connect(DB_PATH)
    
    # We load from raw_ticks since ticks isn't used in telemetry
    try:
        query = "SELECT timestamp_ms as timestamp, price FROM raw_ticks ORDER BY timestamp_ms ASC"
        df = pd.read_sql_query(query, conn)
    except pd.errors.DatabaseError:
        query = "SELECT timestamp, price FROM ticks ORDER BY timestamp ASC"
        df = pd.read_sql_query(query, conn)
        
    conn.close()
    
    print(f"Loaded {len(df)} ticks.")
    df = df.drop_duplicates(subset=['timestamp'])
    
    # Target time + 300s (300,000 ms)
    df_future = df[['timestamp', 'price']].copy()
    df_future = df_future.rename(columns={'timestamp': 'future_timestamp', 'price': 'future_price'})
    
    # if timestamp in ms
    if df['timestamp'].max() > 1e11:
        df['target_time'] = df['timestamp'] + 300_000
        tolerance = 5000
    else:
        df['target_time'] = df['timestamp'] + 300
        tolerance = 5
        
    df = df.sort_values('target_time')
    df_future = df_future.sort_values('future_timestamp')
    df = pd.merge_asof(df, df_future, left_on='target_time', right_on='future_timestamp', direction='forward', tolerance=tolerance)
    df = df.dropna(subset=['future_price'])
    df = df.sort_values('timestamp').reset_index(drop=True)
    
    print("Computing Technical Features...")
    df['returns'] = df['price'].pct_change()
    df['sma_10'] = df['price'].rolling(window=10).mean()
    df['sma_30'] = df['price'].rolling(window=30).mean()
    df['dist_sma10'] = (df['price'] - df['sma_10']) / df['sma_10']
    df['dist_sma30'] = (df['price'] - df['sma_30']) / df['sma_30']
    df['rsi_14'] = calculate_rsi(df['price'], period=14)
    
    sma_20 = df['price'].rolling(window=20).mean()
    std_20 = df['price'].rolling(window=20).std()
    bb_upper = sma_20 + (2 * std_20)
    bb_lower = sma_20 - (2 * std_20)
    
    df['bb_position'] = (df['price'] - bb_lower) / (bb_upper - bb_lower).replace(0, 1e-9)
    df['bb_width'] = (bb_upper - bb_lower) / sma_20.replace(0, 1e-9)
    df['volatility_20'] = df['returns'].rolling(window=20).std()
    
    n_period = 10
    df['net_change'] = (df['price'] - df['price'].shift(n_period)).abs()
    df['sum_abs_changes'] = df['price'].diff().abs().rolling(window=n_period).sum()
    df['fractal_efficiency'] = df['net_change'] / df['sum_abs_changes'].replace(0, 1e-9)
    
    df['mom_accel'] = df['price'].diff(5) - df['price'].diff(20)
    df['vol_heat'] = df['price'].rolling(window=10).std() / df['price'].rolling(window=50).std().replace(0, 1e-9)
    
    df = df.dropna()
    features = [
        'returns', 'dist_sma10', 'dist_sma30', 
        'rsi_14', 'bb_position', 'bb_width', 'volatility_20',
        'fractal_efficiency', 'mom_accel', 'vol_heat'
    ]
    
    # Normalize features roughly to avoid massive scale issues in RL
    X = df[features].values.astype(np.float32)
    y_current = df['price'].values
    y_future = df['future_price'].values
    
    return X, y_current, y_future

class DerivTradingEnv(gym.Env):
    def __init__(self, X, y_current, y_future):
        super(DerivTradingEnv, self).__init__()
        self.X = X
        self.y_current = y_current
        self.y_future = y_future
        self.current_step = 0
        self.max_steps = len(X) - 1
        
        # Action space: [-1, 1] mapped to [0, 1] probability
        self.action_space = spaces.Box(low=-1.0, high=1.0, shape=(1,), dtype=np.float32)
        # Observation space: 10 quantitative features
        self.observation_space = spaces.Box(low=-np.inf, high=np.inf, shape=(10,), dtype=np.float32)
        
    def reset(self, seed=None, options=None):
        super().reset(seed=seed)
        self.current_step = 0
        return self.X[self.current_step], {}
        
    def step(self, action):
        # Action mapped from [-1, 1] to [0, 1] (Probability of CALL)
        q_model = (action[0] + 1.0) / 2.0
        
        current_price = self.y_current[self.current_step]
        future_price = self.y_future[self.current_step]
        
        # Threshold logic aligned with decision_engine
        # edge = q_model - 0.5. We simplify by hard directional bets: 
        # If Q > 0.55 -> CALL. If Q < 0.45 -> PUT. If close to 0.5 -> HOLD.
        
        reward = 0.0
        # Simulated derivation: Payout = 95% on win, Loss = -100% on loss.
        # If the bot holds, small opportunity cost / flat reward.
        trade_pnl = 0.0
        if q_model > 0.55: # CALL
            if future_price > current_price:
                trade_pnl = 0.95
            else:
                trade_pnl = -1.0
        elif q_model < 0.45: # PUT
            if future_price < current_price:
                trade_pnl = 0.95
            else:
                trade_pnl = -1.0
                
        reward = trade_pnl
        
        self.current_step += 1
        terminated = self.current_step >= self.max_steps
        truncated = False
        
        obs = self.X[self.current_step] if not terminated else self.X[-1]
        return obs, float(reward), terminated, truncated, {}

class OnnxWrapper(nn.Module):
    def __init__(self, policy):
        super(OnnxWrapper, self).__init__()
        self.policy = policy

    def forward(self, obs):
        features = self.policy.features_extractor(obs)
        # SB3 uses an intermediate MLP before the final action scaler
        latent_pi = self.policy.mlp_extractor.policy_net(features)
        action_mean = self.policy.action_net(latent_pi)
        
        clamped = torch.clamp(action_mean, -1.0, 1.0)
        q_prob = (clamped + 1.0) / 2.0
        return q_prob

def train_and_export():
    if not has_sb3:
        print("Installation missing. Exiting.")
        return
        
    print("Preparing dataset...")
    X, y_current, y_future = load_and_prepare_data()
    print(f"Data ready. {len(X)} usable samples.")
    
    # Chronological Split
    split_idx = int(len(X) * 0.8)
    X_train, y_c_train, y_f_train = X[:split_idx], y_current[:split_idx], y_future[:split_idx]
    X_test, y_c_test, y_f_test = X[split_idx:], y_current[split_idx:], y_future[split_idx:]
    
    env_train = DummyVecEnv([lambda: DerivTradingEnv(X_train, y_c_train, y_f_train)])
    
    print("Training PPO RL Agent...")
    model = PPO("MlpPolicy", env_train, verbose=1, n_steps=2048, learning_rate=3e-4, device="cpu")
    model.learn(total_timesteps=10000)
    print("Training completed.")
    
    print("Exporting Policy to ONNX...")
    policy = model.policy
    onnx_model = OnnxWrapper(policy)
    onnx_model.eval()
    
    dummy_input = torch.randn(1, 10)
    try:
        torch.onnx.export(
            onnx_model,
            dummy_input,
            ONNX_OUTPUT_PATH,
            export_params=True,
            opset_version=14,
            do_constant_folding=True,
            input_names=["input"],
            output_names=["output"],
            dynamic_axes={"input": {0: "batch_size"}, "output": {0: "batch_size"}}
        )
        print(f"SUCCESS: ONNX model successfully saved to {ONNX_OUTPUT_PATH}")
    except Exception as e:
        print("FAILED TO EXPORT ONNX:")
        print(str(e))

if __name__ == "__main__":
    train_and_export()
