# Steering Document

> Last Updated: 2026-02-13

## Goal

Test every LaminarDB pipeline type from the laminardb.io website code examples, one phase at a time. Additionally test v0.12.0 feature fixes and new API surface.

## Current Focus

**All 9 phases implemented and passing.** v0.12.0 feature tests (Phase 8) and API surface tests (Phase 9) complete. Monitoring upstream fixes for embedded executor gaps (#85, #86).

## Phase Priority Order

| Priority | Phase | What It Tests | External Deps | Status |
|----------|-------|--------------|---------------|--------|
| 1 (done) | Rust API | builder, execute, source, subscribe, push_batch, watermark, poll | None | **PASS** |
| 2 (done) | Streaming SQL | tumble(), first_value/last_value, SUM, cascading MVs | None | **PASS** |
| 3 (done) | Kafka Pipeline | FROM KAFKA, INTO KAFKA, ${VAR} substitution | Redpanda (:19092) | **PASS** |
| 4 (done) | Stream Joins | ASOF JOIN, stream-stream INNER JOIN, time bounds | None | **PASS** |
| 5 (done) | CDC Pipeline | Postgres CDC polling, SQL aggregation on CDC events | Postgres | **PASS** (polling) |
| 6+ (done) | Bonus | HOP, SESSION, EMIT ON UPDATE (not on website) | None | **PASS** |
| 7 (done) | Stress Test | 6-stream fraud-detect pipeline, 7-level throughput ramp | None | **PASS** |
| 8 (done) | v0.12.0 Features | Cascading MVs #35, SESSION #55, EOWC #52, INTERVAL #69, late data #65 | None | **PASS** (5/5) |
| 9 (done) | API Surface | api::Connection #49, push_arrow #64, SourceHandle metadata, topology | None | **PASS** (7/7) |

## Test Matrix

| Phase | Feature | Source | Result |
|-------|---------|--------|--------|
| 1 | `LaminarDB::builder()` | Website: Rust API tab | **PASS** |
| 1 | `.execute()` / `.start()` | Website: Rust API tab | **PASS** |
| 1 | `.source::<T>()` / `push_batch()` | Website: Rust API tab | **PASS** |
| 1 | `.subscribe::<T>()` / `poll()` | Website: Rust API tab | **PASS** |
| 1 | `watermark()` | Website: Rust API tab | **PASS** |
| 1 | `#[derive(Record)]` | Website: Rust API tab | **PASS** |
| 1 | `#[derive(FromRow)]` | Website: Rust API tab | **PASS** (not FromRecordBatch) |
| 2 | TUMBLE window | Website: Streaming SQL tab | **PASS** |
| 2 | TUMBLE_START → `tumble()` | Website: Streaming SQL tab | **PASS** (UDF name differs) |
| 2 | FIRST/LAST → `first_value/last_value` | Website: Streaming SQL tab | **PASS** (function names differ) |
| 2 | EMIT ON WINDOW CLOSE | Website: Streaming SQL tab | parsed, **no effect** (filed [#85](https://github.com/laminardb/laminardb/issues/85)) |
| 2 | Cascading MVs | Website: Streaming SQL tab | **PASS** (fixed by [#35](https://github.com/laminardb/laminardb/issues/35)) |
| 3 | Kafka source (FROM KAFKA) | Website: Kafka Pipeline tab | **PASS** |
| 3 | Kafka sink (INTO KAFKA) | Website: Kafka Pipeline tab | **PASS** |
| 3 | `${VAR}` config substitution | Website: Kafka Pipeline tab | **PASS** |
| 3 | Two sinks from one stream | Implementation detail | **PASS** |
| 4 | ASOF JOIN + TOLERANCE | Website: Stream Joins tab | **PASS** (42K-56K rows in stress pipeline) |
| 4 | Stream-stream INNER JOIN | Website: Stream Joins tab | **PASS** |
| 4 | Time-bounded join (BETWEEN) | Website: Stream Joins tab | **PASS** (numeric, not INTERVAL) |
| 5 | Postgres CDC source (SQL parsing) | Website: CDC Pipeline tab | **PASS** (SQL accepted, connector registers) |
| 5 | Native CDC connector I/O | Website: CDC Pipeline tab | **FAIL** (laminardb bug [#58](https://github.com/laminardb/laminardb/issues/58) — tokio-postgres lacks replication) |
| 5 | CDC polling workaround | laminardb-test | **PASS** (175 events → 155 totals via `pg_logical_slot_get_changes`) |
| 5 | SQL aggregation on CDC events | Website: CDC Pipeline tab | **PASS** (GROUP BY, COUNT, SUM all correct) |
| 5 | EMIT CHANGES (changelog) | Website: CDC Pipeline tab | Not testable (native connector blocked) |
| 5 | Delta Lake sink | Website: CDC Pipeline tab | Not testable (native connector blocked) |
| 6+ | HOP window | Codebase | **PASS** (885 results) |
| 6+ | SESSION window | Codebase | **PASS** (885 results) |
| 6+ | EMIT ON UPDATE | Codebase | **PASS** (885 results) |
| 7 | HOP (vol_baseline) | laminardb-fraud-detect | **PASS** |
| 7 | TUMBLE OHLC (ohlc_vol) | laminardb-fraud-detect | **PASS** |
| 7 | SESSION (rapid_fire) | laminardb-fraud-detect | **PASS** (proper merge) |
| 7 | TUMBLE + CASE WHEN (wash_score) | laminardb-fraud-detect | **PASS** |
| 7 | INNER JOIN (suspicious_match) | laminardb-fraud-detect | **PASS** |
| 7 | ASOF JOIN (asof_match) | laminardb-fraud-detect | **PASS** (42K-56K rows) |
| 7 | 7-level throughput ramp | laminardb-fraud-detect | **PASS** (~25,554/s Ubuntu CI) |
| 8 | Cascading MVs (#35 fix) | v0.12.0 regression test | **PASS** (L2 produces output) |
| 8 | SESSION window fix (#55) | v0.12.0 regression test | **PASS** (proper merge) |
| 8 | EMIT ON WINDOW CLOSE (#52) | v0.12.0 regression test | **PASS** (accepted, output received) |
| 8 | INTERVAL arithmetic (#69) | v0.12.0 regression test | **PASS** (HOP with INTERVAL on BIGINT) |
| 8 | Late data filtering (#65) | v0.12.0 regression test | **PASS** (not filtered — known limitation, [#86](https://github.com/laminardb/laminardb/issues/86)) |
| 9 | api::Connection lifecycle | v0.12.0 API (PR #49) | **PASS** (open → DDL → list → close) |
| 9 | Catalog introspection | v0.12.0 API (PR #49) | **PASS** (source/stream/sink info, get_schema) |
| 9 | Pipeline state & metrics | v0.12.0 API (PR #49) | **PASS** (state, watermark, events, insert) |
| 9 | ArrowSubscription | v0.12.0 API (PR #49) | **PASS** (subscribe → try_next) |
| 9 | push_arrow | v0.12.0 API (PR #64) | **PASS** (raw RecordBatch ingestion) |
| 9 | SourceHandle metadata | v0.12.0 API (PR #64) | **PASS** (name, schema, pending, capacity, backpressure, watermark) |
| 9 | Pipeline topology | v0.12.0 API (PR #49) | **PASS** (nodes + edges graph) |

## Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Project location | `~/gitrepos/github/laminardb-test/` | Separate repo, sibling to laminardb |
| Dep strategy | Path deps to `../laminardb/crates/` | Test against local source |
| Phase structure | One module per phase | Self-contained, testable independently |
| TUI framework | Ratatui 0.29 + Crossterm 0.28 | Same as laminardb demo |
| Feature flags | `kafka`, `postgres-cdc`, `api` | Enable connectors and sync API |
| CI | GitHub Actions, release builds | Stress test + Criterion benchmarks + all phases |

## GitHub Issues Filed

| Issue | Title | Status |
|-------|-------|--------|
| [#35](https://github.com/laminardb/laminardb/issues/35) | Cascading MVs | **FIXED** in v0.12.0 |
| [#44](https://github.com/laminardb/laminardb/issues/44) | CDC stub | Filed |
| [#58](https://github.com/laminardb/laminardb/issues/58) | tokio-postgres replication | Open |
| [#85](https://github.com/laminardb/laminardb/issues/85) | EOWC not wired in embedded SQL executor | Open |
| [#86](https://github.com/laminardb/laminardb/issues/86) | Late data filtering not invoked in embedded executor | Open |
