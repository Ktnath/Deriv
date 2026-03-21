# Deriv Trading Bot

This refactor splits the project into purpose-built binaries so live execution, raw market-data capture, and offline research no longer share one monolithic entrypoint.

## Binary layout

- `executor` — live trading engine. Connects to Deriv WebSocket, runs the existing trading FSM, asks strategy/prior/risk engines for decisions, executes trades, and emits telemetry.
- `recorder` — market-data recorder. Connects to Deriv WebSocket, subscribes to one or more symbols, persists raw ticks plus selected metadata into SQLite, and never trades.
- `research` — offline research CLI. Reads SQLite historical data and provides simple `summarize`, `replay`, and `inspect-regimes` commands.
- `deriv-bot` — compatibility wrapper that now delegates to `executor`.

## Configuration

### Shared live connectivity

Required for `executor` and `recorder`:

- `DERIV_API_TOKEN` — Deriv API token.
- `DERIV_APP_ID` — Deriv application id.
- `DERIV_ENDPOINT` — optional WebSocket endpoint. Defaults to `wss://ws.binaryws.com/websockets/v3`.

### Executor settings

- `DERIV_SYMBOL` — symbol to trade. Default: `R_100`.
- `DERIV_ACCOUNT_TYPE` — informational account label. Default: `demo`.
- `DRY_RUN` — `1`/`true` to simulate order placement flow. Default: `1`.
- `DERIV_INITIAL_BALANCE` — starting balance used for local risk accounting. Default: `10000`.
- `DERIV_STRATEGY` — strategy selector. Default: `temporal`.
- `DERIV_CONTRACT_DURATION` — contract duration. Default: `300`.
- `DERIV_DURATION_UNIT` — duration unit. Default: `s`.
- `DERIV_STAKE` — informational base stake. Default: `1.0`.
- `DERIV_MIN_STAKE` — minimum live stake. Default: `0.35`.
- `DERIV_MODEL_PATH` — optional ONNX model path.
- `DERIV_ALLOW_MODEL_FALLBACK` — defaults to `true`.
- `DERIV_MARKET_PRIOR` — optional fixed prior in `[0,1]`.
- `DERIV_MAX_POSITIONS` — max open positions. Default: `1`.
- `DERIV_MAX_DAILY_LOSS` — daily loss limit. Default: `50.0`.
- `DERIV_COOLDOWN_MS` — cooldown after loss. Default: `30000`.
- `DERIV_MAX_CONSEC_LOSSES` — max losing streak. Default: `5`.
- `DERIV_STOP_LOSS_PCT` — early-exit threshold fraction of buy price. Default: `0.80`.
- `DERIV_TELEMETRY_DB_PATH` — SQLite path for executor telemetry. Default: `deriv_metrics.db`.
- `DERIV_TELEMETRY_BIND` — telemetry WebSocket bind address. Default: `127.0.0.1:3000`.

### Recorder settings

- `DERIV_RECORDER_SYMBOLS` — comma-separated symbols to capture. Falls back to `DERIV_SYMBOL`.
- `DERIV_RECORDER_DB_PATH` — SQLite output path. Default: `deriv_recorder.db`.
- `DERIV_RECORDER_BALANCE` — subscribe to balance updates. Default: `false`.
- `DERIV_RECORDER_TIME` — periodically request server time metadata. Default: `true`.
- `DERIV_RECORDER_RETENTION_DAYS` — optional retention window for pruning old raw ticks.

### Research settings

- `DERIV_RESEARCH_DB_PATH` — SQLite file used by research CLI. Falls back to `DERIV_RECORDER_DB_PATH`.

## Running the binaries

### Executor

```bash
cargo run --bin executor
```

Executor responsibilities:

- connect and authorize,
- subscribe to live data,
- build decision inputs,
- invoke strategy / prior / risk engines,
- execute trades through the current trader FSM,
- emit telemetry and persist execution-side metrics.

### Recorder

```bash
DERIV_RECORDER_SYMBOLS=R_100,R_50 cargo run --bin recorder
```

Recorder responsibilities:

- connect and authorize,
- subscribe to market-data streams,
- write raw ticks into `raw_ticks`,
- optionally persist balance / time metadata into `recorder_metadata`,
- reconnect safely and support long-running capture.

### Research

```bash
cargo run --bin research -- summarize
cargo run --bin research -- replay R_100 25
cargo run --bin research -- inspect-regimes R_100 200
```

Research responsibilities in this PR:

- inspect recorded history without a live WebSocket,
- provide a clean CLI scaffold for future replay/backtest work,
- expose simple diagnostics over recorder data.

## SQLite schema notes

The telemetry layer now includes recorder-oriented tables:

- `raw_ticks` — append-only tick history with event time, receive time, symbol, price, and source.
- `recorder_metadata` — raw balance/time snapshots for later audit or analysis.
- existing `ticks`, `trade_events`, and `alpha_signals` remain for execution telemetry compatibility.

## Migration notes

- `src/main.rs` is now a thin compatibility wrapper around the new executor app module.
- Live trading orchestration moved into reusable library code under `src/app/executor.rs`.
- Historical data workflows should now point at recorder output (`DERIV_RECORDER_DB_PATH`) instead of expecting ad hoc dataset side effects from the live engine.
- If you previously ran `cargo run`, the default binary is now `executor`.

## Breaking changes

- Research/data-capture responsibilities were removed from the live entrypoint; use `recorder` or `research` explicitly.
- Recorder data is written to `raw_ticks` instead of being inferred from execution telemetry side effects.
- New environment variables were introduced for recorder and research binaries.

## Validation

- `cargo fmt --check`
- `cargo metadata --no-deps`
- `cargo check --bins --tests` currently depends on `bot_core`'s `ort` download path and may fail in restricted environments if the ONNX Runtime binary cannot be fetched.
