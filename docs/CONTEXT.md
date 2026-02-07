# Session Context

> Read this first. Update at the end of each session.

## Last Session

**Date**: 2026-02-07

### What Was Accomplished
- Phase 1 (Rust API) **PASS**: 490 trades → 440 OHLC bars
- Phase 2 (Streaming SQL) **PARTIAL PASS**:
  - Level 1 (5s OHLC with tumble/first_value/last_value/SUM): PASS (440 bars)
  - Level 2 (cascading MV ohlc_10s FROM ohlc_5s): FAIL — stream-from-stream not supported in embedded pipeline
- Phase 3 (Kafka Pipeline) **PASS**: Full end-to-end Kafka pipeline working
  - FROM KAFKA source: PASS (reads JSON from market-trades topic)
  - SQL aggregation: PASS (315 trades → 285 summaries via COUNT + SUM)
  - INTO KAFKA sink: PASS (JSON summaries written to trade-summaries topic)
  - ${VAR} config substitution: PASS (KAFKA_BROKERS resolved correctly)
  - rdkafka FutureProducer → Redpanda:19092 → LaminarDB → Redpanda:19092
- Phase 4 (Stream Joins) **PARTIAL PASS**:
  - Stream-stream INNER JOIN (trades ⋈ orders, numeric BETWEEN): PASS (88 matches from 98 orders)
  - ASOF JOIN (trades ⋈ quotes, MATCH_CONDITION): FAIL — DataFusion doesn't support ASOF JOIN syntax
- TUI dashboard with pipeline flow visualization, latency stats
- GitHub issue #35 filed for cascading MV failure

### Where We Left Off
- **Phase 1: PASS**, **Phase 2: PARTIAL**, **Phase 3: PASS**, **Phase 4: PARTIAL**
- Next: Phase 5 (CDC Pipeline) or Bonus phases

### Current Phase Status

| Phase | Status | Notes |
|-------|--------|-------|
| 1: Rust API | **PASS** | 490 trades → 440 OHLC bars |
| 2: Streaming SQL | **PARTIAL** | Level 1 PASS, cascading MV FAIL (architectural limit) |
| 3: Kafka Pipeline | **PASS** | 315 trades → 285 summaries, source + sink + ${VAR} all working |
| 4: Stream Joins | **PARTIAL** | INNER JOIN PASS (88 matches), ASOF JOIN FAIL (connector-only) |
| 5: CDC Pipeline | Not Started | Needs Postgres |
| 6+: Bonus | Not Started | HOP, SESSION, EMIT ON UPDATE |

### Immediate Next Steps
1. Phase 5 (CDC Pipeline) — needs Postgres running
2. Bonus phases (HOP, SESSION, EMIT ON UPDATE) — embedded, no external deps

### Key Learnings
- `laminar-core` required as direct dep (Record derive macro references it)
- `#[derive(FromRow)]` NOT `FromRecordBatch` — only FromRow implements `FromBatch` trait
- Demo pattern: `CREATE SOURCE` → `CREATE STREAM` → `CREATE SINK` → `db.start()`
- Source columns need `NOT NULL`
- **CRITICAL**: Use `first_value()`/`last_value()` NOT `FIRST()`/`LAST()` — DataFusion only recognizes the full names
- **CRITICAL**: `tumble()` returns `Timestamp(Millisecond)` not `Int64` — use `CAST(tumble(...) AS BIGINT)` when mapping to i64 fields
- **CRITICAL**: Website uses `TUMBLE_START()` but the registered UDF name is `tumble` — use `tumble()` in both SELECT and GROUP BY
- **CRITICAL**: ASOF JOIN only works through connector pipeline (custom operators) — embedded pipeline uses DataFusion's `ctx.sql()` which doesn't understand `ASOF JOIN ... MATCH_CONDITION()`
- **CRITICAL**: INTERVAL arithmetic on BIGINT columns fails in DataFusion — use numeric arithmetic (`o.ts BETWEEN t.ts - 60000 AND t.ts + 60000`) instead
- Stream-stream INNER JOIN works in embedded mode — standard SQL JOINs execute through `ctx.sql()` when both sources have data in the same cycle
- Watermark: `watermark(ts + 5_000)` for 5s windows
- Embedded pipeline: 100ms tick cycle, stateless micro-batch, no cross-cycle window state
- **Cascading MVs not supported**: embedded pipeline only feeds `CREATE SOURCE` entries to executor; stream results push to subscribers but don't loop back as executor input
- EMIT ON WINDOW CLOSE: parser accepts it but micro-batch model ignores it
- Redpanda external Kafka API on port **19092** (not 9092)
- **Phase 3 Kafka**: `FROM KAFKA(brokers, topic, group_id, format, offset_reset)` works in embedded mode
- **Phase 3 Kafka**: `INTO KAFKA(brokers, topic, format)` sink creates and writes JSON to output topic
- **Phase 3 Kafka**: `.config_var("KAFKA_BROKERS", "localhost:19092")` resolves `${KAFKA_BROKERS}` in SQL
- **Phase 3 Kafka**: Two sinks from one stream works: `CREATE SINK local FROM stream` + `CREATE SINK kafka FROM stream INTO KAFKA(...)`
- **Phase 3 Kafka**: rdkafka 0.39 cmake-build feature required; `FutureProducer::send()` for async produce
- **Phase 3 Kafka**: Kafka feature flag: `laminar-db = { features = ["kafka"] }` → enables laminar-connectors/kafka
