# Test Phases

> What each phase tests, how it works, and what we found.

---

## Phase 1: Rust API

**File:** `src/phase1_api.rs` | **Status:** PASS

### What it tests

The embedded Rust API from the website's first tab — the core push/subscribe loop.

### How it works

1. Creates a `LaminarDB` instance with `builder().buffer_size(65536).build()`
2. Registers a `trades` source with 4 columns (symbol, price, volume, ts)
3. Creates a stream that computes OHLC bars per symbol using a 5-second TUMBLE window
4. Subscribes to the stream and polls for results in a loop
5. A `MarketGenerator` produces 5 trades per cycle (one per symbol) with random-walk prices

### SQL (actual working version)

```sql
CREATE SOURCE trades (
    symbol VARCHAR NOT NULL, price DOUBLE NOT NULL,
    volume BIGINT NOT NULL, ts BIGINT NOT NULL
);

CREATE STREAM ohlc_1m AS
SELECT symbol,
       first_value(price) AS open, MAX(price) AS high,
       MIN(price) AS low, last_value(price) AS close
FROM trades
GROUP BY symbol, TUMBLE(ts, INTERVAL '5' SECOND);
```

### API calls exercised

| Call | Purpose |
|------|---------|
| `LaminarDB::builder().build()` | Create embedded DB instance |
| `db.execute(sql)` | Register sources and streams |
| `db.start()` | Start the 100ms micro-batch pipeline |
| `db.source::<Trade>("trades")` | Get a typed push handle |
| `db.subscribe::<OhlcBar>("ohlc_1m")` | Get a typed poll subscription |
| `source.push_batch(trades)` | Push a batch of records |
| `source.watermark(ts)` | Advance the watermark |
| `sub.poll()` | Poll for new results |

### Results

490 trades pushed, 440 OHLC bars received. Near 1:1 ratio due to micro-batch model (each 100ms cycle produces one bar per symbol if data exists).

### Gotchas discovered

- **`first_value()`/`last_value()`** required, not `FIRST()`/`LAST()` — DataFusion doesn't recognize the short forms
- **`#[derive(FromRow)]`** not `FromRecordBatch` — only FromRow implements the `FromBatch` trait
- Source columns need `NOT NULL`
- Watermark must advance past window boundary

---

## Phase 2: Streaming SQL

**File:** `src/phase2_sql.rs` | **Status:** PARTIAL PASS

### What it tests

The website's second tab — extended SQL features: window start extraction, SUM aggregate, and cascading materialized views (stream-from-stream).

### How it works

1. Same `trades` source as Phase 1
2. **Level 1:** Creates `ohlc_5s` stream with `tumble()` for bar_start, `SUM(volume)`, and full OHLC
3. **Level 2:** Attempts a cascading stream `ohlc_10s` that reads FROM `ohlc_5s` (stream-from-stream)

### SQL (actual working version)

```sql
-- Level 1: PASS
CREATE STREAM ohlc_5s AS
SELECT symbol,
       CAST(tumble(ts, INTERVAL '5' SECOND) AS BIGINT) AS bar_start,
       first_value(price) AS open, MAX(price) AS high,
       MIN(price) AS low, last_value(price) AS close,
       SUM(volume) AS volume
FROM trades
GROUP BY symbol, tumble(ts, INTERVAL '5' SECOND);

-- Level 2: FAIL (creates successfully but produces 0 output)
CREATE STREAM ohlc_10s AS
SELECT symbol,
       CAST(tumble(bar_start, INTERVAL '10' SECOND) AS BIGINT) AS bar_start,
       first_value(open) AS open, MAX(high) AS high,
       MIN(low) AS low, last_value(close) AS close,
       SUM(volume) AS volume
FROM ohlc_5s
GROUP BY symbol, tumble(bar_start, INTERVAL '10' SECOND);
```

### Results

| Test | Result | Notes |
|------|--------|-------|
| Level 1 (ohlc_5s FROM trades) | **PASS** | 440 bars from 490 trades |
| Level 2 (ohlc_10s FROM ohlc_5s) | **FAIL** | 0 output — cascading MVs not supported |

### Gotchas discovered

- **`tumble()`** not `TUMBLE_START()` — the registered UDF name is `tumble`
- **`tumble()` returns `Timestamp(Millisecond)`** not Int64 — must `CAST(... AS BIGINT)` for i64 fields
- **Cascading MVs don't work** in embedded mode — `start_embedded_pipeline()` only feeds `CREATE SOURCE` entries to the executor. Stream results go to subscribers but never loop back as input. (GitHub issue [#35](https://github.com/laminardb/laminardb/issues/35))
- **EMIT ON WINDOW CLOSE** is parsed but has no effect in micro-batch model

---

## Phase 3: Kafka Pipeline

**File:** `src/phase3_kafka.rs` | **Status:** PASS

### What it tests

The website's third tab — full end-to-end Kafka pipeline: external producer → Kafka source connector → SQL aggregation → Kafka sink connector → external consumer.

### Prerequisites

- Redpanda running on `localhost:19092` (use podman)
- Topics: `market-trades` (source), `trade-summaries` (sink)
- Create topics: `podman exec <container> rpk topic create market-trades trade-summaries`

### How it works

1. An `rdkafka::FutureProducer` sends JSON trade messages to the `market-trades` Kafka topic
2. LaminarDB's `FROM KAFKA` source connector consumes from the topic
3. SQL aggregation computes COUNT and SUM(price * volume) per symbol per 5s window
4. Results go to both a local subscriber (for TUI/CLI display) and a Kafka sink (`trade-summaries` topic)
5. `${KAFKA_BROKERS}` variable substitution via `builder.config_var()`

### SQL (actual working version)

```sql
-- Source from Kafka
CREATE SOURCE trades (
    symbol  VARCHAR NOT NULL,
    price   DOUBLE NOT NULL,
    volume  BIGINT NOT NULL,
    ts      BIGINT NOT NULL
) FROM KAFKA (
    brokers = '${KAFKA_BROKERS}',
    topic = 'market-trades',
    group_id = 'laminar-test-p3',
    format = 'json',
    offset_reset = 'earliest'
);

-- SQL aggregation
CREATE STREAM trade_summary AS
SELECT symbol,
       COUNT(*) AS trades,
       SUM(price * CAST(volume AS DOUBLE)) AS notional
FROM trades
GROUP BY symbol, TUMBLE(ts, INTERVAL '5' SECOND);

-- Local sink for subscriber
CREATE SINK summary_local FROM trade_summary;

-- Kafka sink
CREATE SINK summary_kafka FROM trade_summary
INTO KAFKA (
    brokers = '${KAFKA_BROKERS}',
    topic = 'trade-summaries',
    format = 'json'
);
```

### Results

| Test | Result | Notes |
|------|--------|-------|
| FROM KAFKA source | **PASS** | Reads JSON from market-trades topic |
| SQL aggregation | **PASS** | 315 trades → 285 summaries |
| INTO KAFKA sink | **PASS** | JSON summaries written to trade-summaries topic |
| ${VAR} substitution | **PASS** | KAFKA_BROKERS resolved to localhost:19092 |
| Local subscriber | **PASS** | poll() receives aggregated results |
| Two sinks from one stream | **PASS** | Both local and Kafka sinks work simultaneously |

### Gotchas discovered

- **`FROM KAFKA` works in embedded mode** — the connector pipeline integrates with the embedded pipeline seamlessly
- **Two sinks from one stream** is supported — `CREATE SINK local FROM stream` + `CREATE SINK kafka FROM stream INTO KAFKA(...)` both work
- **`config_var()` substitution** — `LaminarDB::builder().config_var("KAFKA_BROKERS", "localhost:19092")` resolves `${KAFKA_BROKERS}` in all SQL statements
- **Kafka feature flag** — must enable `laminar-db = { features = ["kafka"] }` in Cargo.toml
- **rdkafka 0.39 cmake-build** — required as direct dependency for producer/consumer code
- **`CAST(volume AS DOUBLE)`** needed when multiplying BIGINT × DOUBLE in SUM()

---

## Phase 4: Stream Joins

**File:** `src/phase4_joins.rs` | **Status:** PARTIAL PASS

### What it tests

The website's fourth tab — two join types:
1. **ASOF JOIN:** Enrich each trade with the most recent quote (backward temporal match with 5s tolerance)
2. **Stream-stream INNER JOIN:** Match trades with orders on the same symbol within a time window

### How it works

1. Creates 3 sources: `trades`, `quotes`, `orders`
2. Each TUI/CLI cycle pushes 5 trades, 5 quotes, and 1 order
3. Attempts both join types as `CREATE STREAM` queries
4. Polls for results from both subscriptions

### SQL (actual working version)

```sql
-- ASOF JOIN: FAIL (DataFusion doesn't understand ASOF syntax)
CREATE STREAM asof_enriched AS
SELECT t.symbol, t.price AS trade_price, t.volume,
       q.bid, q.ask, t.price - q.bid AS spread
FROM trades t
ASOF JOIN quotes q
MATCH_CONDITION(t.ts >= q.ts AND t.ts - q.ts <= INTERVAL '5' SECOND)
ON t.symbol = q.symbol;

-- Stream-stream INNER JOIN: PASS
-- Note: numeric arithmetic, NOT INTERVAL, because ts is BIGINT
CREATE STREAM trade_order_match AS
SELECT t.symbol, t.price AS trade_price, t.volume,
       o.order_id, o.side, o.price AS order_price,
       t.price - o.price AS price_diff
FROM trades t
INNER JOIN orders o
ON t.symbol = o.symbol
AND o.ts BETWEEN t.ts - 60000 AND t.ts + 60000;
```

### Results

| Test | Result | Notes |
|------|--------|-------|
| ASOF JOIN | **FAIL** | Creates silently, 0 output — DataFusion can't execute ASOF syntax |
| Stream-stream INNER JOIN | **PASS** | 88 matches from 98 orders |

### Gotchas discovered

- **ASOF JOIN only works through the connector pipeline** — the embedded pipeline routes all SQL through DataFusion's `ctx.sql()`, which doesn't understand `ASOF JOIN ... MATCH_CONDITION()`. The custom `AsofJoinOperator` in laminar-core is never invoked. (GitHub issue [#37](https://github.com/laminardb/laminardb/issues/37))
- **INTERVAL arithmetic on BIGINT fails** — `t.ts + INTERVAL '1' MINUTE` doesn't work when `ts` is BIGINT. Use numeric values: `t.ts + 60000` (milliseconds)
- **Standard INNER JOIN works** — DataFusion handles regular SQL JOINs fine through `ctx.sql()` as long as both source MemTables have data in the same cycle
- Silent failure pattern: both ASOF and cascading MVs accept the query, subscribe successfully, start without error, then produce nothing

---

## Phase 5: CDC Pipeline

**File:** not yet implemented | **Status:** Not Started

### What it will test

The website's fifth tab — Postgres CDC source with `_op` column handling and `EMIT CHANGES`.

### Prerequisites

- Postgres with logical replication enabled
- Publication and replication slot configured

---

## TUI Dashboard

**File:** `src/tui.rs`

### What it shows

A tabbed Ratatui dashboard that runs all implemented phases simultaneously (Phase 1-4). Each tab displays:

- **Stats panel:** trades pushed, bars/results received, cycle count, uptime, throughput, latency
- **Pipeline flow:** animated visualization of data flowing through the embedded pipeline, showing MemTable registration, `ctx.sql()` execution, subscriber buffers, and Kafka connectors
- **Data table:** most recent results (OHLC bars, summaries, or join results)

### Pipeline visualization features

- Animated `◆` dot flowing along `─` dashes when data is actively flowing
- Red `╳` for inactive/failed pipeline paths
- Pulsing vertical arrows (`│ ▼`) in cyan/blue showing live data flow
- Per-phase architecture: single source (P1), branching queries (P2), Kafka end-to-end (P3), multi-source multi-query (P4)
- Live counters showing records at each pipeline stage

### Controls

| Key | Action |
|-----|--------|
| Tab / Right | Next phase tab |
| Shift+Tab / Left | Previous phase tab |
| q / Esc | Quit |

### Running

```bash
cargo run              # TUI with all phases (1-4)
cargo run -- phase1    # Phase 1 CLI only
cargo run -- phase2    # Phase 2 CLI only
cargo run -- phase3    # Phase 3 CLI only (needs Redpanda on :19092)
cargo run -- phase4    # Phase 4 CLI only
```

---

## Common Types

**File:** `src/types.rs`

| Type | Derive | Used by |
|------|--------|---------|
| `Trade` | `Record, FromRow` | Phase 1, 2, 3, 4 (input) |
| `OhlcBar` | `FromRow` | Phase 1 (output) |
| `OhlcBarFull` | `FromRow` | Phase 2 (output, includes bar_start + volume) |
| `TradeSummary` | `FromRow` | Phase 3 Kafka pipeline (output) |
| `Quote` | `Record` | Phase 4 ASOF JOIN (input) |
| `Order` | `Record` | Phase 4 stream-stream JOIN (input) |
| `AsofEnriched` | `FromRow` | Phase 4 ASOF JOIN (output) |
| `TradeOrderMatch` | `FromRow` | Phase 4 stream-stream JOIN (output) |

## Data Generator

**File:** `src/generator.rs`

`MarketGenerator` produces synthetic market data across 5 symbols (AAPL, GOOGL, MSFT, AMZN, TSLA):
- `generate_trades(ts)` — one trade per symbol with random-walk prices
- `generate_quotes(ts)` — one quote per symbol with bid/ask around current price
- `generate_order(ts)` — one order for a random symbol

---

## Embedded Pipeline Architecture

All phases run through the same execution model:

```
Your app                          LaminarDB embedded pipeline
─────────                         ──────────────────────────────
push_batch(data) ──────────►  Source buffer
watermark(ts)    ──────────►  │
                              ▼  (every 100ms tick)
                         ┌─ Drain ALL source buffers
                         ├─ Register each as MemTable
                         ├─ Run ALL stream queries via ctx.sql()
                         ├─ Push results to subscriber buffers
                         └─ Clean up MemTables
                              │
poll()           ◄────────── Subscriber buffer
```

**Key limitation:** Only `CREATE SOURCE` tables feed data into the executor. Stream results exit through subscribers but never re-enter as input — which is why cascading MVs and ASOF JOINs (which need the custom operator path) don't work.

---

## Pipeline Flow Diagrams

### Phase 1: Single source, single stream

```
 MarketGenerator
      │
      │ generate_trades(ts)
      │ 5 trades per cycle
      ▼
┌──────────┐    push_batch     ┌──────────────────────────────────────┐
│ Your App │ ─────────────────►│  SOURCE: trades                      │
│          │    watermark       │  (symbol, price, volume, ts)         │
│          │ ─────────────────►│                                      │
│          │                    │         ┌────────────────────┐       │
│          │                    │         │ DataFusion ctx.sql │       │
│          │                    │         │                    │       │
│          │                    │         │ SELECT symbol,     │       │
│          │                    │         │  first_value(price)│       │
│          │                    │         │  MAX(price)        │       │
│          │                    │         │  MIN(price)        │       │
│          │                    │         │  last_value(price) │       │
│          │                    │         │ FROM trades        │       │
│          │                    │         │ GROUP BY symbol,   │       │
│          │                    │         │  TUMBLE(ts, 5s)    │       │
│          │                    │         └────────┬───────────┘       │
│          │                    │                  │                   │
│          │                    │                  ▼                   │
│          │      poll()        │  STREAM: ohlc_1m                    │
│          │ ◄─────────────────│  (symbol, open, high, low, close)   │
└──────────┘                    └──────────────────────────────────────┘

Result: 490 trades ──► 440 OHLC bars (PASS)
```

### Phase 2: Cascading streams (level 2 fails)

```
 MarketGenerator
      │
      ▼
┌──────────┐     push_batch    ┌──────────────────────────────────────┐
│ Your App │ ─────────────────►│  SOURCE: trades                      │
│          │                    │                                      │
│          │                    │         ┌────────────────────┐       │
│          │                    │         │ ctx.sql: ohlc_5s   │       │
│          │                    │         │ tumble + first_value│       │
│          │                    │         │ + SUM(volume)       │       │
│          │                    │         └────────┬───────────┘       │
│          │                    │                  │                   │
│          │      poll()        │                  ▼                   │
│          │ ◄─────────────────│  STREAM: ohlc_5s ── PASS (440 bars) │
│          │                    │         │                            │
│          │                    │         │ ✗ NOT fed back as input    │
│          │                    │         ▼                            │
│          │                    │  ┌────────────────────┐              │
│          │                    │  │ ctx.sql: ohlc_10s  │              │
│          │                    │  │ FROM ohlc_5s       │◄── empty!   │
│          │                    │  └────────┬───────────┘              │
│          │                    │           │                          │
│          │      poll()        │           ▼                          │
│          │ ◄ ─ ─ ─ ─ ─ ─ ─ ─│  STREAM: ohlc_10s ── FAIL (0 bars) │
└──────────┘   (nothing)        └──────────────────────────────────────┘

Problem: ohlc_5s output goes to subscribers, never loops back
         as a MemTable for ohlc_10s to read from.
```

### Phase 4: Multi-source joins

```
 MarketGenerator
      │
      ├─ generate_trades(ts)    5 per cycle
      ├─ generate_quotes(ts)    5 per cycle
      └─ generate_order(ts)     1 per cycle
      │
      ▼
┌──────────┐                    ┌──────────────────────────────────────┐
│ Your App │ ──push_batch──────►│  SOURCE: trades                      │
│          │ ──push_batch──────►│  SOURCE: quotes                      │
│          │ ──push_batch──────►│  SOURCE: orders                      │
│          │                    │                                      │
│          │                    │  All 3 registered as MemTables       │
│          │                    │  before any query runs               │
│          │                    │                                      │
│          │                    │  ┌─────────────────────────────┐     │
│          │                    │  │ ctx.sql: asof_enriched      │     │
│          │                    │  │ trades ASOF JOIN quotes     │     │
│          │                    │  │ MATCH_CONDITION(t.ts >= ..) │     │
│          │                    │  └─────────────┬───────────────┘     │
│          │                    │                │                     │
│          │                    │        ✗ DataFusion error            │
│          │                    │          (ASOF not understood)       │
│          │      poll()        │                ▼                     │
│          │ ◄ ─ ─ ─ ─ ─ ─ ─ ─│  STREAM: asof_enriched ── FAIL (0)  │
│          │   (nothing)        │                                      │
│          │                    │  ┌─────────────────────────────┐     │
│          │                    │  │ ctx.sql: trade_order_match  │     │
│          │                    │  │ trades INNER JOIN orders    │     │
│          │                    │  │ ON symbol = symbol          │     │
│          │                    │  │ AND o.ts BETWEEN            │     │
│          │                    │  │   t.ts-60000, t.ts+60000   │     │
│          │                    │  └─────────────┬───────────────┘     │
│          │                    │                │                     │
│          │                    │        ✓ Standard SQL works          │
│          │      poll()        │                ▼                     │
│          │ ◄─────────────────│  STREAM: trade_order_match ── PASS  │
└──────────┘                    └──────────────────────────────────────┘

ASOF:  DataFusion can't parse ASOF JOIN ──► silent 0 output
JOIN:  Standard SQL INNER JOIN works ──► 88 matches from 98 orders
```

### Phase 3: Kafka end-to-end pipeline

```
 MarketGenerator
      │
      │ generate_trades(ts)
      │ 5 trades per cycle
      ▼
┌──────────┐    produce()     ┌──────────────────────────────────────┐
│ rdkafka  │ ───── JSON ─────►│  Kafka: market-trades topic          │
│ Producer │                   │  (Redpanda localhost:19092)          │
└──────────┘                   └─────────────┬────────────────────────┘
                                             │
                                             ▼
                               ┌──────────────────────────────────────┐
                               │  LaminarDB                           │
                               │                                      │
                               │  FROM KAFKA ──► Arrow RecordBatch    │
                               │         │                            │
                               │         ▼                            │
                               │  ┌────────────────────────────────┐  │
                               │  │ ctx.sql:                       │  │
                               │  │ SELECT COUNT(*),               │  │
                               │  │   SUM(price * volume)          │  │
                               │  │ GROUP BY symbol, TUMBLE(5s)    │  │
                               │  └────────────────┬───────────────┘  │
                               │                   │                  │
                               │       ┌───────────┴──────────┐      │
                               │       ▼                      ▼      │
                               │  subscriber              INTO KAFKA  │
                               │  trade_summary       trade-summaries │
                               └───────┬──────────────────────┬───────┘
                                       │                      │
                              poll()   ▼                      ▼
                          ┌──────────┐              ┌──────────────────┐
                          │ Your App │              │  Kafka: trade-   │
                          │ TUI/CLI  │              │  summaries topic │
                          └──────────┘              └──────────────────┘

Result: 315 trades → 285 summaries (PASS)
        Kafka source ✓  SQL ✓  Kafka sink ✓  ${VAR} ✓
```

### What the connector pipeline would look like (not yet tested)

```
                              ┌──────────────────────────────────────┐
                              │     Connector Pipeline (Ring 0)      │
                              │                                      │
  Kafka/CDC ─────────────────►│  SOURCE ──► JoinParser.analyze()     │
                              │                    │                  │
                              │         ┌──────────┴──────────┐      │
                              │         │                     │      │
                              │    AsofJoinOperator    StreamJoinOp  │
                              │    (BTreeMap match)    (hash join)   │
                              │         │                     │      │
                              │         ▼                     ▼      │
  Kafka/Subscribers ◄────────│  SINK / Subscriber buffers           │
                              └──────────────────────────────────────┘

This path uses the CUSTOM OPERATORS (not DataFusion ctx.sql).
ASOF JOIN, cascading MVs, and EMIT ON WINDOW CLOSE would work here.
```

### Summary: What works where

```
                          Embedded Pipeline     Connector Pipeline
                          (ctx.sql path)        (operator DAG path)
                          ─────────────────     ───────────────────
Single-source TUMBLE          ✓ PASS                ✓ (expected)
Cascading MVs                 ✗ FAIL                ✓ (expected)
FROM KAFKA source             ✓ PASS                ✓ (expected)
INTO KAFKA sink               ✓ PASS                ✓ (expected)
${VAR} substitution           ✓ PASS                ✓ (expected)
ASOF JOIN                     ✗ FAIL                ✓ (expected)
Stream-stream INNER JOIN      ✓ PASS                ✓ (expected)
EMIT ON WINDOW CLOSE          ✗ ignored             ✓ (expected)
INTERVAL on BIGINT            ✗ FAIL                ? (untested)
```
