import pandas as pd
import numpy as np
import time
from sklearn.ensemble import RandomForestClassifier
from sklearn.metrics import accuracy_score, classification_report
try:
    from skl2onnx import convert_sklearn
    from skl2onnx.common.data_types import FloatTensorType
    with_onnx = True
except ImportError:
    with_onnx = False
    print("skl2onnx not found. Skipping ONNX export. (pip install skl2onnx)")

DATASET_PATH = "d:/Deriv/dataset_ml.csv"
MODEL_OUTPUT_PATH = "d:/Deriv/model_v2.onnx"

def main():
    print(f"Loading dataset from {DATASET_PATH}...")
    try:
        df = pd.read_csv(DATASET_PATH)
    except Exception as e:
        print(f"Error loading dataset: {e}")
        return

    print(f"Total samples loaded: {len(df)}")
    
    # Sort chronologically to prevent look-ahead bias
    df = df.sort_values('timestamp').reset_index(drop=True)

    # Features must match build_dataset.py + Rust exactly
    features = [
        'returns', 'dist_sma10', 'dist_sma30', 
        'rsi_14', 'bb_position', 'bb_width', 'volatility_20',
        'fractal_efficiency', 'mom_accel', 'vol_heat'
    ]
    
    X = df[features].values
    y = df['target'].values
    
    # Time-series split (Chronological)
    # 80% Train, 20% Test (representing the most recent market data)
    split_idx = int(len(df) * 0.8)
    
    X_train, X_test = X[:split_idx], X[split_idx:]
    y_train, y_test = y[:split_idx], y[split_idx:]
    
    print(f"\nTime-Series Split:")
    print(f"Training set: {len(X_train)} samples")
    print(f"Testing set: {len(X_test)} samples")
    
    # Calculate baseline
    print("\nBaseline accuracy (if we always guess UP):")
    print(f"Train baseline: {np.mean(y_train) * 100:.2f}%")
    print(f"Test baseline: {np.mean(y_test) * 100:.2f}%")

    print("\nTraining RandomForestClassifier...")
    start_time = time.time()
    
    # Parameters optimized for tabular noisy financial data
    # max_depth limits over-fitting
    # min_samples_leaf prevents learning noise on single ticks
    clf = RandomForestClassifier(
        n_estimators=100,
        max_depth=7,
        min_samples_leaf=20,
        random_state=42,
        n_jobs=-1
    )
    
    clf.fit(X_train, y_train)
    print(f"Training completed in {time.time() - start_time:.2f} seconds.")

    print("\nEvaluating on Test Set (Out-of-sample):")
    y_pred = clf.predict(X_test)
    acc = accuracy_score(y_test, y_pred)
    print(f"Accuracy: {acc * 100:.2f}%")
    
    print("\nClassification Report:")
    print(classification_report(y_test, y_pred))

    # Pattern Identification Insights: Feature Importance
    print("\nFeature Importances (Patterns discovered):")
    importances = clf.feature_importances_
    for feat, imp in sorted(zip(features, importances), key=lambda x: x[1], reverse=True):
        print(f" - {feat:15s}: {imp*100:.2f}%")

    # Virtual Trading Simulation
    # Check what happens if we only trade on high confidence signals
    y_prob = clf.predict_proba(X_test)
    # y_prob[:, 1] is the probability of class 1 (UP)
    
    confidence_threshold = 0.55
    print(f"\nSimulated Trading Strategy (Threshold > {confidence_threshold}):")
    
    # Go Long
    long_signals_idx = np.where(y_prob[:, 1] > confidence_threshold)[0]
    long_wins = sum(y_test[long_signals_idx] == 1)
    long_total = len(long_signals_idx)
    
    # Go Short
    short_signals_idx = np.where(y_prob[:, 0] > confidence_threshold)[0]
    short_wins = sum(y_test[short_signals_idx] == 0)
    short_total = len(short_signals_idx)
    
    print(f"Long Trades : {long_total} | Win Rate: {(long_wins/long_total)*100:.2f}%" if long_total > 0 else "Long Trades : 0")
    print(f"Short Trades: {short_total} | Win Rate: {(short_wins/short_total)*100:.2f}%" if short_total > 0 else "Short Trades: 0")
    print(f"Total Trades: {long_total + short_total}")

    MODEL_V3_PATH = "d:/Deriv/model_v3.onnx"
    # Export to ONNX
    if with_onnx:
        print(f"\nExporting model to ONNX: {MODEL_V3_PATH}...")
        try:
            # We have len(features) inputs of float32
            initial_type = [('float_input', FloatTensorType([None, len(features)]))]
            # Disable ZipMap to get a plain tensor for probabilities (easier for Rust)
            onx = convert_sklearn(clf, initial_types=initial_type, options={'zipmap': False})
            with open(MODEL_V3_PATH, "wb") as f:
                f.write(onx.SerializeToString())
            print("Export successful!")
        except Exception as e:
            print(f"Failed to export ONNX: {e}")

if __name__ == "__main__":
    main()
