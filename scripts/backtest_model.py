import pandas as pd
import numpy as np
import onnxruntime as ort
import os
from sklearn.metrics import accuracy_score, classification_report

MODEL_PATH = "d:/Deriv/model_v3.onnx"
DATASET_PATH = "d:/Deriv/dataset_ml.csv"

def main():
    if not os.path.exists(MODEL_PATH):
        print(f"Error: {MODEL_PATH} not found.")
        return
    if not os.path.exists(DATASET_PATH):
        print(f"Error: {DATASET_PATH} not found.")
        return

    print(f"Loading model from {MODEL_PATH}...")
    sess = ort.InferenceSession(MODEL_PATH)
    
    input_name = sess.get_inputs()[0].name
    output_names = [o.name for o in sess.get_outputs()]
    print(f"Inputs: {input_name}")
    print(f"Outputs: {output_names}")
    
    print(f"Loading dataset from {DATASET_PATH}...")
    df = pd.read_csv(DATASET_PATH)
    
    # Features must match build_dataset.py + Rust exactly
    features = [
        'returns', 'dist_sma10', 'dist_sma30', 
        'rsi_14', 'bb_position', 'bb_width', 'volatility_20',
        'fractal_efficiency', 'mom_accel', 'vol_heat'
    ]
    
    X = df[features].values.astype(np.float32)
    y_true = df['target'].values
    
    print(f"Running inference on {len(X)} samples...")
    res = sess.run(output_names, {input_name: X})
    y_pred = res[0]
    y_prob = res[1] 
    
    # Handle skl2onnx probability format (often list of dicts)
    if isinstance(y_prob, list) and len(y_prob) > 0 and isinstance(y_prob[0], dict):
        y_prob_arr = np.array([[d[0], d[1]] for d in y_prob])
    else:
        y_prob_arr = np.array(y_prob)

    acc = accuracy_score(y_true, y_pred)
    print(f"\nOverall Accuracy: {acc * 100:.2f}%")
    
    # Simulation
    # Thresholding to avoid low-confidence noise
    for threshold in [0.50, 0.55, 0.60]:
        long_mask = y_prob_arr[:, 1] > threshold
        short_mask = y_prob_arr[:, 0] > threshold
        
        long_trades = df[long_mask]
        short_trades = df[short_mask]
        
        long_wins = (long_trades['target'] == 1).sum()
        short_wins = (short_trades['target'] == 0).sum()
        
        total_trades = len(long_trades) + len(short_trades)
        if total_trades == 0:
            print(f"\nThreshold > {threshold}: No trades.")
            continue
            
        total_wins = long_wins + short_wins
        win_rate = (total_wins / total_trades) * 100
        
        # PnL Estimate (Assume 95% payout for options, -100% for loss)
        # Expected value = (win_rate * 0.95) - ((100-win_rate) * 1.0)
        expected_val = (win_rate/100 * 0.95) - ((1-win_rate/100) * 1.0)
        
        print(f"\n--- Results for Threshold > {threshold} ---")
        print(f"Total Trades: {total_trades}")
        print(f"Win Rate    : {win_rate:.2f}%")
        print(f"Expected Val: {expected_val:.4f} USD per 1 USD stake")
        
        if len(long_trades) > 0:
            print(f"  Long WR : {(long_wins/len(long_trades))*100:.2f}% ({len(long_trades)} trades)")
        if len(short_trades) > 0:
            print(f"  Short WR: {(short_wins/len(short_trades))*100:.2f}% ({len(short_trades)} trades)")

if __name__ == "__main__":
    main()
