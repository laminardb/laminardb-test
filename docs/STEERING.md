# Steering Document

> Last Updated: 2026-02-06

## Goal

Test every LaminarDB pipeline type from the laminardb.io website code examples, one phase at a time.

## Current Focus

**Phase 1: Rust API** — Get the basic embedded API working end-to-end.

## Phase Priority Order

| Priority | Phase | What It Tests | External Deps |
|----------|-------|--------------|---------------|
| 1 (now) | Rust API | builder, execute, source, subscribe, push_batch, watermark, poll | None |
| 2 | Streaming SQL | TUMBLE_START, FIRST/LAST, EMIT ON WINDOW CLOSE, cascading MVs | None |
| 3 | Kafka Pipeline | Kafka source/sink, ${VAR} substitution, exactly-once | Redpanda (:19092) |
| 4 | Stream Joins | ASOF JOIN + TOLERANCE, stream-stream INNER JOIN, time bounds | None |
| 5 | CDC Pipeline | Postgres CDC, EMIT CHANGES, Delta Lake sink | Postgres |
| 6+ | Bonus | HOP, SESSION, EMIT ON UPDATE (not on website) | None |

## Test Matrix

| Phase | Feature | Source |
|-------|---------|--------|
| 1 | `LaminarDB::builder()` | Website: Rust API tab |
| 1 | `.execute()` / `.start()` | Website: Rust API tab |
| 1 | `.source::<T>()` / `push_batch()` | Website: Rust API tab |
| 1 | `.subscribe::<T>()` / `poll()` | Website: Rust API tab |
| 1 | `watermark()` | Website: Rust API tab |
| 1 | `#[derive(Record)]` | Website: Rust API tab |
| 1 | `#[derive(FromRecordBatch)]` | Website: Rust API tab |
| 2 | TUMBLE window | Website: Streaming SQL tab |
| 2 | TUMBLE_START | Website: Streaming SQL tab |
| 2 | FIRST/LAST aggregates | Website: Streaming SQL tab |
| 2 | EMIT ON WINDOW CLOSE | Website: Streaming SQL tab |
| 2 | Cascading MVs | Website: Streaming SQL tab |
| 3 | Kafka source (FROM KAFKA) | Website: Kafka Pipeline tab |
| 3 | Kafka sink (TO KAFKA) | Website: Kafka Pipeline tab |
| 3 | `${VAR}` config substitution | Website: Kafka Pipeline tab |
| 3 | exactly-once delivery | Website: Kafka Pipeline tab |
| 4 | ASOF JOIN + TOLERANCE | Website: Stream Joins tab |
| 4 | Stream-stream INNER JOIN | Website: Stream Joins tab |
| 4 | Time-bounded join (BETWEEN) | Website: Stream Joins tab |
| 5 | Postgres CDC source | Website: CDC Pipeline tab |
| 5 | EMIT CHANGES (changelog) | Website: CDC Pipeline tab |
| 5 | Delta Lake sink | Website: CDC Pipeline tab |
| 6+ | HOP window | Codebase |
| 6+ | SESSION window | Codebase |
| 6+ | EMIT ON UPDATE | Codebase |

## Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Project location | `~/gitrepos/github/laminardb-test/` | Separate repo, sibling to laminardb |
| Dep strategy | Path deps to `../laminardb/crates/` | Test against local source |
| Phase structure | One module per phase | Self-contained, testable independently |
| TUI framework | Ratatui 0.29 + Crossterm 0.28 | Same as laminardb demo |
