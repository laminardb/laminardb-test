# LaminarDB Test App

Standalone Rust app to test every LaminarDB pipeline type from https://laminardb.io code examples.
Depends on LaminarDB via path deps (`../laminardb/crates/`).

## Quick Start

```bash
cargo run -- phase1    # Rust API basics
cargo run -- phase2    # Streaming SQL + cascading MVs
cargo run -- phase3    # Kafka pipeline (needs Redpanda on localhost:19092)
cargo run -- phase4    # Stream joins (ASOF + stream-stream)
cargo run -- phase5    # CDC pipeline (needs Postgres)
cargo run              # TUI with all phases as tabs
```

## Project Structure

```
src/
├── main.rs          # Entry point, phase selector
├── types.rs         # Record + FromRecordBatch structs (input + output)
├── generator.rs     # Trade/order/quote data generator
├── phase1_api.rs    # Phase 1: Rust API test (website tab 1)
├── phase2_sql.rs    # Phase 2: Streaming SQL + cascading MVs (website tab 2)
├── phase3_kafka.rs  # Phase 3: Kafka source/sink pipeline (website tab 3)
├── phase4_joins.rs  # Phase 4: ASOF + stream-stream joins (website tab 4)
├── phase5_cdc.rs    # Phase 5: CDC pipeline (website tab 5)
└── tui.rs           # Ratatui TUI for visual output
sql/                 # SQL files for each phase (loaded at runtime)
docs/
├── CONTEXT.md       # Session continuity (where we left off)
├── STEERING.md      # Phase priorities and test matrix
└── PLAN.md          # Full implementation plan
```

## Key Documents

- @docs/CONTEXT.md - Where we left off (read this first)
- @docs/STEERING.md - Phase priorities and test matrix
- @docs/PLAN.md - Full implementation plan with all SQL + code

## LaminarDB API Reference (quick)

```rust
// Builder
let db = LaminarDB::builder().buffer_size(65536).build().await?;

// Execute SQL
db.execute("CREATE SOURCE trades (symbol VARCHAR, price DOUBLE, ts BIGINT)").await?;
db.execute("CREATE MATERIALIZED VIEW ... FROM trades GROUP BY symbol, TUMBLE(...)").await?;

// Start processing
db.start().await?;

// Push data (input)
let source = db.source::<Trade>("trades")?;
source.push_batch(vec![Trade { ... }]);
source.watermark(now());

// Read results (output)
let sub = db.subscribe::<OhlcBar>("ohlc_1m")?;
while let Some(bars) = sub.poll() { ... }
```

## Derive Macros

- `#[derive(Record)]` — input structs pushed into sources. Mark event time with `#[event_time]`
- `#[derive(FromRow)]` — output structs read from subscriptions. Implements `laminar_db::FromBatch` trait needed by `db.subscribe()`. (Website says `FromRecordBatch` but that only generates inherent methods, not the trait impl)

## Dependencies

Path deps to local laminardb (must be at `../laminardb/`):
- `laminar-db` — main database crate
- `laminar-derive` — Record/FromRow macros
- `laminar-core` — required by Record derive macro at compile time

## Supported Stream Types Being Tested

| Phase | Type | SQL | Status |
|-------|------|-----|--------|
| 1 | Rust API | TUMBLE + FIRST/LAST | Compiles |
| 2 | Streaming SQL | TUMBLE_START, cascading MVs, EMIT ON WINDOW CLOSE | Pending |
| 3 | Kafka Pipeline | FROM KAFKA / TO KAFKA, exactly-once | Pending |
| 4 | Stream Joins | ASOF JOIN + TOLERANCE, INNER JOIN + BETWEEN | Pending |
| 5 | CDC Pipeline | POSTGRES_CDC, EMIT CHANGES, DELTA_LAKE sink | Pending |
| 6+ | Bonus | HOP, SESSION, EMIT ON UPDATE | Pending |