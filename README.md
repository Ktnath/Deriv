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
- run the same shared `DecisionEngine` probability / prior / regime / stake proposal flow used by live execution,
- separate **decision** from **execution** by allowing signal-only mode or simulated execution mode,
- persist experiment metadata plus proposed / entered / settled replay lifecycle telemetry for later comparison,
- print baseline offline summaries for quick validation.

### 3. Executor

Use the executor for live runs after replay validation.

```bash
DERIV_API_TOKEN=... DERIV_APP_ID=... cargo run --bin executor
```

Executor responsibilities:

- connect to Deriv live services,
- evaluate decisions through the shared `DecisionEngine`,
- place trades through the existing trader FSM,
- persist decision, intent, and executed-trade lifecycle telemetry alongside execution outcomes.

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
- `trade_intents` — one execution-attempt record derived from a decision, including signal-only, rejection, submission, failure, or execution state.
- `executed_trades` — realized or simulated execution records for trades that actually opened, plus exit reason and PnL.

### Compatibility views

- `alpha_signals` view — exposes decision probabilities in the old shape.
- `decision_snapshots` view — exposes decision records in the old shape.

This design keeps raw ticks and experiment metadata normalized instead of forcing every concept into one sparse table.


## Decision / intent / trade semantics

The project now treats decision generation and execution telemetry as separate layers, with a single lifecycle per logical opportunity:

- one evaluated opportunity should produce one primary `decision_events` row,
- that decision may produce one `trade_intents` row describing the execution attempt semantics,
- and only an actual open should create an `executed_trades` row.

- `decision_events.decision`
  - `hold` — no actionable entry was produced or the entry was blocked.
  - `signal` — the shared decision engine produced an actionable entry intent.
- `trade_intents.intent_status`
  - `signal_only` — replay or audit-only signal with no execution attempt.
  - `rejected` — an intent existed, but risk / timing / lifecycle checks blocked it.
  - `submitted` — live execution attempted to route the intent through the trader FSM.
  - `execution_failed` — a live execution attempt was made but no trade opened.
  - `executed` — a trade was actually opened.
  - `dry_run_executed` — executor-owned dry-run opened a synthetic simulated trade lifecycle without routing to Deriv.
- `executed_trades.status`
  - `open` — trade is currently open.
  - `settled` — live trade settled naturally.
  - `closed_early` — live trade was sold before expiry.
  - `aborted` — live execution attempt was interrupted before a clean close.
  - `simulated_settled` — replay execution completed through the simulated lifecycle.
  - `dry_run_open` — executor-owned dry-run opened a simulated trade row.
  - `dry_run_settled` — executor-owned dry-run closed that simulated trade row.

### Lifecycle examples

- **Replay, signal-only:** one `decision_events` row with `decision=signal`, one `trade_intents` row with `intent_status=signal_only`, and no `executed_trades` row.
- **Live, execution failed:** one `decision_events` row with `decision=signal`, one `trade_intents` row with `intent_status=execution_failed`, and no `executed_trades` row because nothing opened.
- **Live, trade opened successfully:** one `decision_events` row with `decision=signal`, one `trade_intents` row that moves `submitted -> executed`, and one `executed_trades` row that moves `open -> settled|closed_early|aborted`.
- **Replay with simulated execution:** one `decision_events` row with `decision=signal`, one `trade_intents` row with `intent_status=executed`, and one `executed_trades` row that finishes as `simulated_settled`.
- **Executor dry-run:** one `decision_events` row with `decision=signal`, one `trade_intents` row with `intent_status=dry_run_executed`, and one `executed_trades` row that moves `dry_run_open -> dry_run_settled` using a synthetic `contract_id` prefixed with `dry_run:`.

### Benchmark semantics

`benchmark_signal` is now a normalized comparator derived from the same shared decision contract in both replay and live paths:

- `CALL` when the shared decision logic points long,
- `PUT` when it points short,
- `HOLD` when the shared decision logic rejects entry.

It is no longer allowed to drift between a legacy live-only strategy output and a replay-only placeholder.

### Report semantics

Reports distinguish between:

- **decisions** — rows in `decision_events`,
- **signal intents** — `trade_intents` rows with `intent_status = signal_only`,
- **trades** — rows in `executed_trades` only.

So a `signal` does **not** count as a trade in reports.

Win/loss reporting is intentionally conservative:

- only rows with realized non-`NULL` `pnl` count toward wins or losses,
- `open` trades are reported separately,
- non-open rows with `NULL pnl` are reported as unresolved,
- `aborted` rows with `NULL pnl` are also broken out explicitly as `aborted_without_pnl`.

### Replay versus live versus dry-run

- Replay uses the shared `DecisionEngine` in **replay-owned lifecycle mode**. In execution-enabled replay, the engine itself opens simulated trades, closes them at contract expiry, updates its internal `RiskGate`, and records `simulated_settled` outcomes.
- Live execution uses the same decision generation path in **live-synchronized mode**. In this mode the executor and trader FSM remain the source of truth for order lifecycle, while the engine only keeps a synchronized risk view.
- `DRY_RUN=1` now also uses **live-synchronized mode**, but the executor owns the simulation lifecycle instead of Deriv. The trader still opens a realistic in-memory contract shape, the executor persists an `executed_trades` row with `dry_run_open`, then immediately settles it as `dry_run_settled` with `exit_reason=dry_run_simulated_expiry`.
- The executor now calls explicit synchronization hooks on the engine:
  - `notify_live_balance(balance)` whenever Deriv sends an updated account balance,
  - `notify_live_trade_opened(...)` after the trader FSM has a real open contract,
  - `notify_live_trade_closed(...)` after settlement or early close with realized PnL,
  - `notify_live_trade_aborted(...)` when a disconnect or interrupted execution invalidates the open trade.
- The same open/close synchronization hooks are also used for executor dry-run, so Kelly sizing and open-position gating stay coherent across consecutive dry-run trades.
- This prevents live drift: Kelly sizing reads the latest synchronized live balance, open-position gating matches the trader FSM, and realized PnL is only applied once when the live trade truly closes.
- On reconnect, the executor clears aborted live state through the same synchronization path so the engine does not keep a phantom open position.

### How to audit `DRY_RUN=1`

- Expect one `decision_events` row per actionable dry-run opportunity.
- Expect one `trade_intents` row with `intent_status = dry_run_executed`.
- Expect one `executed_trades` row whose lifecycle ends at `status = dry_run_settled`.
- Dry-run rows are distinct from replay rows because replay uses `status = simulated_settled`.
- Dry-run rows are distinct from real executor rows because they use `dry_run_*` statuses and synthetic `contract_id` values prefixed with `dry_run:`.

## Offline reports

`research report` prints practical baseline metrics including:

- decision count,
- signal-intent count,
- executed trade count,
- average edge,
- PnL summary,
- win/loss summary,
- regime distribution,
- rejection-reason counts.

## Tests and fixtures

- schema round-trip tests cover run metadata, decisions, intents, and executions,
- replay/report tests cover command parsing and report aggregation,
- lifecycle tests reject impossible combinations such as `signal_only` intents with `executed_trades`,
- replay fixtures can be created by recording a short `raw_ticks` sequence and replaying it through `research`.

## Current limitations

- replay currently uses a practical simulated execution outcome rather than a full contract lifecycle model,
- the live executor still uses the existing trader FSM and is not redesigned in this PR,
- no dashboard or notebook ecosystem is added,
- ONNX-backed builds can still be blocked in restricted environments if `ort` cannot download runtime binaries.

## Validation commands

- `cargo fmt`
- `cargo check --bins --tests`
