# Deriv Trading Bot

## Live-trading environment variables

- `DERIV_MODEL_PATH` — optional ONNX model path. If unset, the bot runs in quant-only fallback mode.
- `DERIV_ALLOW_MODEL_FALLBACK` — optional boolean, defaults to `true`. When `false`, startup fails if `DERIV_MODEL_PATH` is set but the model cannot be loaded.
- `DERIV_MARKET_PRIOR` — optional fixed prior in `[0,1]`. If unset, live probability remains model-only until a real prior engine is provided.
- `DERIV_MIN_STAKE` — optional minimum live stake, defaults to `0.35`.
- `DERIV_STOP_LOSS_PCT` — early-exit loss threshold for an open contract, expressed as a fraction of buy price.

## Live safety notes

- Live stake sizing now uses the Kelly risk engine output as the execution source of truth.
- Trades whose computed Kelly size is below `DERIV_MIN_STAKE` are skipped.
- Daily loss protection resets on UTC day boundaries.
