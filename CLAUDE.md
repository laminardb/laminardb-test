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
├── types.rs         # Record + FromRow structs (input + output)
├── generator.rs     # Trade/order/quote data generator
├── phase1_api.rs    # Phase 1: Rust API test (website tab 1)
├── phase2_sql.rs    # Phase 2: Streaming SQL + cascading MVs (website tab 2)
├── phase3_kafka.rs  # Phase 3: Kafka source/sink pipeline (website tab 3)
├── phase4_joins.rs  # Phase 4: ASOF + stream-stream joins (website tab 4)
├── phase5_cdc.rs    # Phase 5: CDC pipeline (website tab 5, needs Postgres)
└── tui.rs           # Ratatui TUI with animated pipeline flow visualization
docs/
├── CONTEXT.md       # Session continuity (where we left off)
├── PHASES.md        # Detailed per-phase documentation with results
├── STEERING.md      # Phase priorities and test matrix
└── PLAN.md          # Original implementation plan with SQL + code
```

## Key Documents

- @docs/CONTEXT.md - Where we left off (read this first)
- @docs/STEERING.md - Phase priorities and test matrix
- @docs/PLAN.md - Full implementation plan with all SQL + code

## LaminarDB API Reference (quick)

```rust
// Builder (with optional Kafka config vars)
let db = LaminarDB::builder()
    .config_var("KAFKA_BROKERS", "localhost:19092")
    .buffer_size(65536)
    .build().await?;

// Execute SQL
db.execute("CREATE SOURCE trades (symbol VARCHAR NOT NULL, price DOUBLE NOT NULL, ts BIGINT NOT NULL)").await?;
db.execute("CREATE STREAM ohlc AS ... FROM trades GROUP BY symbol, TUMBLE(...)").await?;
db.execute("CREATE SINK output FROM ohlc").await?;

// Kafka connectors (Phase 3)
db.execute("CREATE SOURCE trades (...) FROM KAFKA (brokers = '${KAFKA_BROKERS}', topic = '...', format = 'json')").await?;
db.execute("CREATE SINK output FROM stream INTO KAFKA (brokers = '${KAFKA_BROKERS}', topic = '...', format = 'json')").await?;

// Start processing
db.start().await?;

// Push data (input) — embedded sources only
let source = db.source::<Trade>("trades")?;
source.push_batch(vec![Trade { ... }]);
source.watermark(now());

// Read results (output)
let sub = db.subscribe::<OhlcBar>("ohlc")?;
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

## Phase Status

| Phase | Type | Key Features | Status |
|-------|------|-------------|--------|
| 1 | Rust API | TUMBLE + first_value/last_value, push_batch/poll | **PASS** |
| 2 | Streaming SQL | tumble() as TUMBLE_START, SUM, cascading MVs | **PARTIAL** (L1 pass, cascade fail) |
| 3 | Kafka Pipeline | FROM KAFKA, INTO KAFKA, ${VAR} substitution | **PASS** |
| 4 | Stream Joins | ASOF JOIN, stream-stream INNER JOIN | **PARTIAL** (INNER pass, ASOF fail) |
| 5 | CDC Pipeline | postgres-cdc SQL + connector registration | **PARTIAL** (SQL pass, replication stub) |
| 6+ | Bonus | HOP, SESSION, EMIT ON UPDATE | **PASS** |