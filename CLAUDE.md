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
cargo run -- phase6    # Bonus: HOP, SESSION, EMIT ON UPDATE
cargo run -- phase7    # Stress test (6-stream throughput benchmark)
cargo run -- phase8    # v0.12.0 feature tests (cascading MVs, SESSION, EOWC, INTERVAL, late data)
cargo run -- phase9    # v0.12.0 API surface tests (api::Connection, push_arrow, metadata, topology)
cargo run -- phase10   # SQL extensions (HAVING, LAG, LEAD)
cargo run -- phase11   # Subscription modes & backpressure (recv_timeout, poll_each, multi-sub)
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
├── phase6_bonus.rs  # Phase 6: HOP, SESSION, EMIT ON UPDATE
├── phase7_stress.rs # Phase 7: 6-stream fraud-detect throughput benchmark
├── phase8_v012.rs   # Phase 8: v0.12.0 feature tests (5 regression tests)
├── phase9_api.rs    # Phase 9: v0.12.0 API surface tests (7 API tests)
├── phase10_sql_ext.rs # Phase 10: SQL extensions (HAVING, LAG, LEAD)
├── phase11_subs.rs  # Phase 11: Subscription modes & backpressure (4 tests)
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

// push_arrow — raw Arrow RecordBatch ingestion (PR #64)
let source = db.source::<Trade>("trades")?;
source.push_arrow(record_batch)?;

// SourceHandle metadata (PR #64)
source.name();               // source name
source.schema();             // Arrow Schema
source.pending();            // buffered records count
source.capacity();           // buffer capacity
source.is_backpressured();   // true if buffer full
source.current_watermark();  // current watermark value

// api::Connection — sync FFI-friendly API (PR #49, requires `api` feature)
let conn = Connection::open()?;
conn.execute("CREATE SOURCE ...")?;
conn.execute("CREATE STREAM ...")?;
conn.start()?;
conn.insert("trades", record_batch)?;  // push Arrow RecordBatch
let mut sub = conn.subscribe("stream_name")?;  // ArrowSubscription
let batch = sub.try_next()?;  // non-blocking poll
conn.list_sources(); conn.list_streams(); conn.list_sinks();
conn.source_info(); conn.stream_info(); conn.sink_info();
conn.get_schema("name")?;
conn.pipeline_state(); conn.pipeline_watermark();
conn.metrics(); conn.total_events_processed();
conn.pipeline_topology();  // PipelineTopology { nodes, edges }
conn.shutdown()?;
conn.close()?;
```

## Derive Macros

- `#[derive(Record)]` — input structs pushed into sources. Mark event time with `#[event_time]`
- `#[derive(FromRow)]` — output structs read from subscriptions. Implements `laminar_db::FromBatch` trait needed by `db.subscribe()`. (Website says `FromRecordBatch` but that only generates inherent methods, not the trait impl)

## Dependencies

Path deps to local laminardb (must be at `../laminardb/`):
- `laminar-db` — main database crate (features: `kafka`, `postgres-cdc`, `api`)
- `laminar-derive` — Record/FromRow macros
- `laminar-core` — required by Record derive macro at compile time

Feature flags: `kafka` (Kafka connectors), `postgres-cdc` (CDC connector), `api` (sync Connection API).

## Phase Status

| Phase | Type | Key Features | Status |
|-------|------|-------------|--------|
| 1 | Rust API | TUMBLE + first_value/last_value, push_batch/poll | **PASS** |
| 2 | Streaming SQL | tumble() as TUMBLE_START, SUM, cascading MVs | **PASS** (cascade fixed by [#35](https://github.com/laminardb/laminardb/issues/35)) |
| 3 | Kafka Pipeline | FROM KAFKA, INTO KAFKA, ${VAR} substitution | **PASS** |
| 4 | Stream Joins | ASOF JOIN, stream-stream INNER JOIN | **PASS** (ASOF 42K-56K rows, INNER JOIN works) |
| 5 | CDC Pipeline | postgres-cdc SQL, polling workaround | **PASS** (polling; native connector blocked by [#58](https://github.com/laminardb/laminardb/issues/58)) |
| 6+ | Bonus | HOP, SESSION, EMIT ON UPDATE | **PASS** |
| 7 | Stress Test | 6-stream fraud-detect pipeline, 7-level ramp | **PASS** (~25,554/s Ubuntu CI, ~2,330/s macOS) |
| 8 | v0.12.0 Features | Cascading MVs #35, SESSION #55, EOWC #52, INTERVAL #69, late data #65 | **PASS** (5/5) |
| 9 | API Surface | api::Connection #49, push_arrow #64, SourceHandle metadata, topology | **PASS** (7/7) |
| 10 | SQL Extensions | HAVING clause, LAG window function, LEAD window function | **PASS** (3/3) |
| 11 | Subscriptions | recv_timeout, poll_each, backpressure detection, multi-subscriber | **PASS** (4/4) |