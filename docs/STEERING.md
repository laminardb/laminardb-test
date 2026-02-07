# Steering Document

> Last Updated: 2026-02-07

## Goal

Test every LaminarDB pipeline type from the laminardb.io website code examples, one phase at a time.

## Current Focus

**All phases complete.** HOP, SESSION, EMIT ON UPDATE all PASS.

## Phase Priority Order

| Priority | Phase | What It Tests | External Deps | Status |
|----------|-------|--------------|---------------|--------|
| 1 (done) | Rust API | builder, execute, source, subscribe, push_batch, watermark, poll | None | **PASS** |
| 2 (done) | Streaming SQL | tumble(), first_value/last_value, SUM, cascading MVs | None | **PARTIAL** |
| 3 (done) | Kafka Pipeline | FROM KAFKA, INTO KAFKA, ${VAR} substitution | Redpanda (:19092) | **PASS** |
| 4 (done) | Stream Joins | ASOF JOIN, stream-stream INNER JOIN, time bounds | None | **PARTIAL** |
| 5 (done) | CDC Pipeline | Postgres CDC, EMIT CHANGES, Delta Lake sink | Postgres | **PARTIAL** |
| 6+ (done) | Bonus | HOP, SESSION, EMIT ON UPDATE (not on website) | None | **PASS** |

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
| 2 | EMIT ON WINDOW CLOSE | Website: Streaming SQL tab | parsed, **no effect** |
| 2 | Cascading MVs | Website: Streaming SQL tab | **FAIL** (architectural limit) |
| 3 | Kafka source (FROM KAFKA) | Website: Kafka Pipeline tab | **PASS** |
| 3 | Kafka sink (INTO KAFKA) | Website: Kafka Pipeline tab | **PASS** |
| 3 | `${VAR}` config substitution | Website: Kafka Pipeline tab | **PASS** |
| 3 | Two sinks from one stream | Implementation detail | **PASS** |
| 4 | ASOF JOIN + TOLERANCE | Website: Stream Joins tab | **FAIL** (DataFusion limitation) |
| 4 | Stream-stream INNER JOIN | Website: Stream Joins tab | **PASS** |
| 4 | Time-bounded join (BETWEEN) | Website: Stream Joins tab | **PASS** (numeric, not INTERVAL) |
| 5 | Postgres CDC source (SQL + connector) | Website: CDC Pipeline tab | **PARTIAL** (SQL parses, connector stub) |
| 5 | CDC replication data flow | Website: CDC Pipeline tab | **FAIL** (connector open() is stub — no actual replication I/O) |
| 5 | EMIT CHANGES (changelog) | Website: CDC Pipeline tab | Not testable (no data flow) |
| 5 | Delta Lake sink | Website: CDC Pipeline tab | Not testable (no data flow) |
| 6+ | HOP window | Codebase | **PASS** (885 results) |
| 6+ | SESSION window | Codebase | **PASS** (885 results) |
| 6+ | EMIT ON UPDATE | Codebase | **PASS** (885 results) |

## Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Project location | `~/gitrepos/github/laminardb-test/` | Separate repo, sibling to laminardb |
| Dep strategy | Path deps to `../laminardb/crates/` | Test against local source |
| Phase structure | One module per phase | Self-contained, testable independently |
| TUI framework | Ratatui 0.29 + Crossterm 0.28 | Same as laminardb demo |
