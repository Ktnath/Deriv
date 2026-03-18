import sqlite3
import pandas as pd
import numpy as np
from sklearn.model_selection import train_test_split
from sklearn.ensemble import RandomForestClassifier
from sklearn.metrics import accuracy_score
import os

DB_PATH = "d:/Deriv/deriv_metrics.db"

def load_data():
    if not os.path.exists(DB_PATH):
        print(f"Database {DB_PATH} not found. Please run fetch_history.py first.")
        return None
        
    conn = sqlite3.connect(DB_PATH)
    df = pd.read_sql_query("SELECT timestamp, price FROM ticks ORDER BY timestamp ASC", conn)
    conn.close()
    return df

def create_features(df):
    if df is None or len(df) < 50:
        return None
        
    df['return'] = df['price'].pct_change()
    df['sma_10'] = df['price'].rolling(window=10).mean()
    df['sma_30'] = df['price'].rolling(window=30).mean()
    
    # Target: 1 if next 5-tick return is positive, 0 otherwise
    df['target'] = (df['price'].shift(-5) > df['price']).astype(int)
    
    df = df.dropna()
    return df

def train_model():
    df = load_data()
    df_feat = create_features(df)
    
    if df_feat is None:
        print("Not enough data to train.")
        return
        
    features = ['return', 'sma_10', 'sma_30']
    X = df_feat[features]
    y = df_feat['target']
    
    # Time-series split
    X_train, X_test, y_train, y_test = train_test_split(X, y, test_size=0.2, shuffle=False)
    
    model = RandomForestClassifier(n_estimators=50, max_depth=5, random_state=42)
    model.fit(X_train, y_train)
    
    preds = model.predict(X_test)
    acc = accuracy_score(y_test, preds)
    
    print(f"Model trained successfully! Test Accuracy predicting next 5 ticks: {acc:.2%}")
    
    # Export to ONNX
    from skl2onnx import convert_sklearn
    from skl2onnx.common.data_types import FloatTensorType
    
    # Define input type (3 features)
    initial_type = [('float_input', FloatTensorType([None, 3]))]
    onx = convert_sklearn(model, initial_types=initial_type, target_opset=12)
    
    with open("d:/Deriv/model_v1.onnx", "wb") as f:
        f.write(onx.SerializeToString())
        
    print("Model exported to model_v1.onnx successfully!")

if __name__ == "__main__":
    train_model()
