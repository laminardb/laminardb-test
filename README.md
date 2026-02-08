# laminardb-test

[![Test All Phases](https://github.com/laminardb/laminardb-test/actions/workflows/test.yml/badge.svg)](https://github.com/laminardb/laminardb-test/actions/workflows/test.yml)

Test app that exercises each [LaminarDB](https://laminardb.io) pipeline type from the website's code examples, one phase at a time. Includes a TUI dashboard that runs all phases simultaneously with live pipeline visualizations.

## Results

| Phase | Feature | Status |
|-------|---------|--------|
| 1: Rust API | builder, execute, source, subscribe, push_batch, watermark, poll | **PASS** |
| 2: Streaming SQL | tumble(), first_value/last_value, SUM, cascading MVs | **PASS** |
| 3: Kafka Pipeline | FROM KAFKA, INTO KAFKA, ${VAR} substitution | **PASS** |
| 4: Stream Joins | ASOF JOIN, stream-stream INNER JOIN, time bounds | **PARTIAL** (ASOF: DataFusion limitation) |
| 5: CDC Pipeline | Postgres CDC polling, SQL aggregation on CDC events | **PASS** (polling; native connector blocked by [#58](https://github.com/laminardb/laminardb/issues/58)) |
| 6+: Bonus | HOP window, SESSION window, EMIT ON UPDATE | **PASS** |
| 7: Stress Test | 6-stream fraud-detect pipeline, 7-level throughput ramp | **PENDING** (run locally) |

See [docs/PHASES.md](docs/PHASES.md) for detailed per-feature results and gotchas discovered.

## Prerequisites

- **Rust** (stable toolchain)
- **LaminarDB source** — clone as sibling directory:
  ```bash
  cd ~/gitrepos/github
  git clone <laminardb-repo-url> laminardb
  git clone <this-repo-url> laminardb-test
  ```
  The `Cargo.toml` uses path dependencies to `../laminardb/crates/`.

### Optional (for specific phases)

- **Redpanda** on port 19092 — for Phase 3 (Kafka Pipeline)
- **Postgres 16** with `wal_level=logical` — for Phase 5 (CDC Pipeline):
  ```bash
  docker compose up -d
  ```

## Build

```bash
cargo build
```

## Run

```bash
cargo run              # TUI dashboard (all phases, live streaming)
cargo run -- phase1    # Rust API basics
cargo run -- phase2    # Streaming SQL + cascading MVs
cargo run -- phase3    # Kafka pipeline (needs Redpanda on :19092)
cargo run -- phase4    # Stream joins (ASOF + stream-stream)
cargo run -- phase5    # CDC pipeline (needs Postgres on :5432)
cargo run -- phase6    # Bonus: HOP, SESSION, EMIT ON UPDATE
cargo run -- phase7    # Stress test: 6-stream pipeline throughput benchmark
```

For the stress test, use release mode for comparable numbers:

```bash
STRESS_DURATION=10 cargo run --release -- phase7
```

## Environment Variables

Phase 5 (CDC) reads credentials from env vars with local-dev defaults:

| Variable | Default | Used by |
|----------|---------|---------|
| `LAMINAR_PG_HOST` | `localhost` | Postgres connection + CDC connector |
| `LAMINAR_PG_USER` | `laminar` | Postgres connection + CDC connector |
| `LAMINAR_PG_PASSWORD` | `laminar` | Postgres connection + CDC connector |
| `STRESS_DURATION` | `10` | Seconds per stress level (Phase 7) |

## Project Structure

```
src/
  main.rs          # Entry point, phase selector
  types.rs         # Record + FromRow structs
  generator.rs     # Synthetic market data generator
  phase1_api.rs    # Rust API test
  phase2_sql.rs    # Streaming SQL + cascading MVs
  phase3_kafka.rs  # Kafka pipeline (source + sink)
  phase4_joins.rs  # ASOF JOIN + stream-stream INNER JOIN
  phase5_cdc.rs    # CDC pipeline (Postgres)
  phase6_bonus.rs  # HOP, SESSION, EMIT ON UPDATE
  phase7_stress.rs # Stress test: 6-stream fraud-detect throughput benchmark
  tui.rs           # Ratatui dashboard with pipeline visualization
docs/
  PHASES.md        # Detailed per-phase documentation and results
  CONTEXT.md       # Session context and learnings
  STEERING.md      # Test matrix and decisions
```

## License

[MIT](LICENSE)
