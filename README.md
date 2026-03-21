# Deriv Trading Bot

This repository now has a clearer **recorder → research/replay → executor** flow so live trading decisions can be validated offline before they are trusted in production.

## Binary layout

- `recorder` — connects to Deriv, stores raw ticks plus selected metadata, and never trades.
- `research` — replays recorded ticks deterministically through the decision pipeline with no live network requirement.
- `executor` — runs the live trading loop and current trader FSM.
- `deriv-bot` — compatibility wrapper that delegates to `executor`.

## Workflow

### 1. Recorder

Use the recorder to collect raw historical market data and audit metadata.

```bash
DERIV_API_TOKEN=... DERIV_APP_ID=... DERIV_RECORDER_SYMBOLS=R_100,R_50 cargo run --bin recorder
```

Recorder responsibilities:

- subscribe to live market data,
- write append-only ticks into `raw_ticks`,
- persist selected non-price metadata into `recorder_metadata`,
- maintain an offline dataset suitable for replay.

### 2. Research / replay

Use the research binary to replay recorded ticks through the decision engine without a live WebSocket.

```bash
cargo run --bin research -- summarize
cargo run --bin research -- replay R_100 500
cargo run --bin research -- replay R_100 500 --with-execution
cargo run --bin research -- report
cargo run --bin research -- report <run_id>
```

Replay responsibilities in this PR:

- load historical ticks deterministically from SQLite,
- run the same decision-stage probability / prior / regime / stake proposal flow offline,
- separate **decision** from **execution** by allowing signal-only mode,
- persist experiment metadata and replay outputs for later comparison,
- print baseline offline summaries for quick validation.

### 3. Executor

Use the executor for live runs after replay validation.

```bash
DERIV_API_TOKEN=... DERIV_APP_ID=... cargo run --bin executor
```

Executor responsibilities:

- connect to Deriv live services,
- evaluate the decision pipeline,
- place trades through the existing trader FSM,
- persist decision and intent telemetry alongside execution outcomes.

## Key environment variables

### Shared live connectivity

Required for `executor` and `recorder`:

- `DERIV_API_TOKEN`
- `DERIV_APP_ID`
- `DERIV_ENDPOINT` — defaults to `wss://ws.binaryws.com/websockets/v3`

### Recorder

- `DERIV_RECORDER_SYMBOLS`
- `DERIV_RECORDER_DB_PATH`
- `DERIV_RECORDER_BALANCE`
- `DERIV_RECORDER_TIME`
- `DERIV_RECORDER_RETENTION_DAYS`

### Research

- `DERIV_RESEARCH_DB_PATH`
- `DERIV_CONTRACT_DURATION`
- `DERIV_MIN_STAKE`
- `DERIV_INITIAL_BALANCE`
- `DERIV_MAX_POSITIONS`
- `DERIV_MAX_DAILY_LOSS`
- `DERIV_COOLDOWN_MS`
- `DERIV_MAX_CONSEC_LOSSES`
- `DERIV_MODEL_PATH`
- `DERIV_ALLOW_MODEL_FALLBACK`
- `DERIV_RESEARCH_STRATEGY_VERSION`
- `DERIV_RESEARCH_PRIOR_VERSION`

### Executor

- `DERIV_SYMBOL`
- `DERIV_ACCOUNT_TYPE`
- `DRY_RUN`
- `DERIV_INITIAL_BALANCE`
- `DERIV_STRATEGY`
- `DERIV_CONTRACT_DURATION`
- `DERIV_DURATION_UNIT`
- `DERIV_STAKE`
- `DERIV_MIN_STAKE`
- `DERIV_MODEL_PATH`
- `DERIV_ALLOW_MODEL_FALLBACK`
- `DERIV_MAX_POSITIONS`
- `DERIV_MAX_DAILY_LOSS`
- `DERIV_COOLDOWN_MS`
- `DERIV_MAX_CONSEC_LOSSES`
- `DERIV_STOP_LOSS_PCT`
- `DERIV_TELEMETRY_DB_PATH`
- `DERIV_TELEMETRY_BIND`

## Telemetry schema changes

The SQLite layer now keeps experiment-oriented data in a more normalized layout.

### Core tables

- `raw_ticks` — append-only recorded tick history.
- `recorder_metadata` — balance/time or other recorder-side payloads.
- `experiment_runs` — run metadata including `run_id`, binary type, model version, strategy version, prior version, config fingerprint, and run timestamp.
- `decision_events` — one row per decision snapshot with regime, model metadata, probabilities, proposed/executed stake, and rejection reason.
- `trade_intents` — intended trades derived from decisions, including signal-only versus executed outcomes.
- `executed_trades` — realized or simulated execution results plus exit reason and PnL.

### Compatibility views

- `alpha_signals` view — exposes decision probabilities in the old shape.
- `decision_snapshots` view — exposes decision records in the old shape.

This design keeps raw ticks and experiment metadata normalized instead of forcing every concept into one sparse table.

## Offline reports

`research report` prints practical baseline metrics including:

- decision count,
- trade count,
- average edge,
- PnL summary,
- win/loss summary,
- regime distribution,
- rejection-reason counts.

## Tests and fixtures

- schema round-trip tests cover run metadata, decisions, intents, and executions,
- replay/report tests cover command parsing and report aggregation,
- replay fixtures can be created by recording a short `raw_ticks` sequence and replaying it through `research`.

## Current limitations

- replay currently uses a practical simulated execution outcome rather than a full contract lifecycle model,
- the live executor still uses the existing trader FSM and is not redesigned in this PR,
- no dashboard or notebook ecosystem is added,
- ONNX-backed builds can still be blocked in restricted environments if `ort` cannot download runtime binaries.

## Validation commands

- `cargo fmt`
- `cargo check --bins --tests`
