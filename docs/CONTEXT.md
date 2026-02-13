# Session Context

> Read this first. Update at the end of each session.

## Last Session

**Date**: 2026-02-13

### What Was Accomplished

- **Phase 10 (SQL Extensions)**: 3/3 PASS
  - HAVING clause: filters correctly (405 baseline rows, 0 with impossible threshold)
  - LAG window function: 1350 rows, 900 with prev_price, 450 with default (works!)
  - LEAD window function: 1335 rows, 890 with next_price, 445 with default (works!)
- **Phase 11 (Subscription Modes & Backpressure)**: 4/4 PASS
  - recv_timeout(): times out when empty, receives data when available
  - poll_each(): callback-based batch processing (135 rows across 135 batches)
  - Backpressure: is_backpressured(), pending(), capacity() all functional
  - Multi-subscriber: single-consumer model confirmed (first subscriber drains channel)
- **Feature audit**: Analyzed laminardb feature INDEX.md (160 features) to identify untested Done features
  - ROW_NUMBER/RANK/DENSE_RANK: NOT in AnalyticFunctionType enum — incomplete, skipped
  - Lookup Joins: Internal operator only, not exposed in SQL — skipped
  - EMIT CHANGES/FINAL: No EmitStrategy enum in public API — skipped
  - Changelog/Retraction: Internal Ring 0 only — skipped
  - subscribe_push/fn/stream: Don't exist — only subscribe() with TypedSubscription<T>
- **CI updated**: test.yml now runs Phase 10 and Phase 11

### Where We Left Off

- All 11 phases implemented and passing
- Governance docs updated to reflect current state

### Current Phase Status

| Phase | Status | Notes |
|-------|--------|-------|
| 1: Rust API | **PASS** | 490 trades → 440 OHLC bars |
| 2: Streaming SQL | **PASS** | L1 PASS, cascade PASS (fixed by #35) |
| 3: Kafka Pipeline | **PASS** | 315 trades → 285 summaries, full Kafka end-to-end |
| 4: Stream Joins | **PASS** | INNER JOIN PASS, ASOF JOIN PASS (42K-56K rows) |
| 5: CDC Pipeline | **PASS** | Polling workaround: 175 events → 155 totals. Native connector blocked by #58 |
| 6+: Bonus | **PASS** | HOP (885), SESSION (885), EMIT ON UPDATE (885) — all from 890 trades |
| 7: Stress Test | **PASS** | ~25,554/s Ubuntu CI, ~2,330/s macOS. ASOF works, SESSION merges properly |
| 8: v0.12.0 Features | **PASS** | 5/5: Cascade #35, SESSION #55, EOWC #52, INTERVAL #69, late data #65 |
| 9: API Surface | **PASS** | 7/7: Connection, catalog, metrics, subscribe, push_arrow, metadata, topology |
| 10: SQL Extensions | **PASS** | 3/3: HAVING, LAG, LEAD |
| 11: Subscriptions | **PASS** | 4/4: recv_timeout, poll_each, backpressure, multi-subscriber |

### Immediate Next Steps

1. Monitor upstream fixes for issues #85, #86 (embedded executor gaps)
2. When ROW_NUMBER/RANK added to AnalyticFunctionType enum, add tests
3. When #58 fixed, add Kafka CDC end-to-end test

### Key Learnings
- `laminar-core` required as direct dep (Record derive macro references it)
- `#[derive(FromRow)]` NOT `FromRecordBatch` — only FromRow implements `FromBatch` trait
- Demo pattern: `CREATE SOURCE` → `CREATE STREAM` → `CREATE SINK` → `db.start()`
- Source columns need `NOT NULL`
- **CRITICAL**: Use `first_value()`/`last_value()` NOT `FIRST()`/`LAST()` — DataFusion only recognizes the full names
- **CRITICAL**: `tumble()` returns `Timestamp(Millisecond)` not `Int64` — use `CAST(tumble(...) AS BIGINT)` when mapping to i64 fields
- **CRITICAL**: Website uses `TUMBLE_START()` but the registered UDF name is `tumble` — use `tumble()` in both SELECT and GROUP BY
- **v0.12.0 FIXES**: Cascading MVs (#35), ASOF JOIN output, SESSION merge (#55), INTERVAL on BIGINT (#69) — all now working
- **v0.12.0 KNOWN GAPS**: EMIT ON WINDOW CLOSE not wired in embedded SQL executor (#85), late data filtering not invoked in embedded path (#86)
- **api::Connection**: Sync FFI-friendly wrapper, requires `api` feature flag. `Connection::open()` creates embedded DB. `conn.execute()` uses `std::thread::scope` to bridge sync→async.
- **push_arrow**: Raw Arrow RecordBatch ingestion. Also available through `conn.insert("source", batch)` in the Connection API.
- **ArrowSubscription**: Untyped subscription returning RecordBatch via `try_next()` (non-blocking) or `next()` (blocking). Connection API doesn't expose `watermark()` directly.
- Stream-stream INNER JOIN works in embedded mode — standard SQL JOINs execute through `ctx.sql()` when both sources have data in the same cycle
- Watermark: `watermark(ts + 5_000)` for 5s windows
- Embedded pipeline: 100ms tick cycle, stateless micro-batch, no cross-cycle window state
- EMIT ON WINDOW CLOSE: parser accepts it but micro-batch model ignores it (filed #85)
- Redpanda external Kafka API on port **19092** (not 9092)
- **Phase 3 Kafka**: `FROM KAFKA(brokers, topic, group_id, format, offset_reset)` works in embedded mode
- **Phase 5 CDC**: Native connector blocked by laminardb bug [#58](https://github.com/laminardb/laminardb/issues/58) — tokio-postgres 0.7 lacks replication support
- **Phase 7 Stress**: Ubuntu CI 11x throughput anomaly — possible causes: shared macOS env (92% RAM), OS scheduler, x86_64 optimizations
- **Phase 7 Stress**: Criterion bench confirms: 111ms cycle (1 tick) vs 214-238ms (2 ticks) on macOS
- **HAVING**: Works in embedded SQL — `GROUP BY ... HAVING SUM(volume) > N` correctly filters post-aggregation
- **LAG/LEAD**: Available via `AnalyticFunctionType::Lag/Lead`. Work in embedded mode. Micro-batch resets partition state per cycle, so LAG returns default for first row in each cycle's partition.
- **poll_each()**: Callback receives individual `T` items (not `Vec<T>`), must return `bool` (true = continue)
- **Multi-subscriber**: LaminarDB uses single-consumer model — first `subscribe()` drains the channel, second gets nothing
- **Backpressure**: `is_backpressured()` triggers at >80% utilization. Default capacity ~2048 even with `buffer_size(64)`
- **Not testable yet**: ROW_NUMBER/RANK (not in enum), Lookup Joins (internal operator), EMIT CHANGES/FINAL (no public API), Changelog/Retraction (Ring 0 only)
