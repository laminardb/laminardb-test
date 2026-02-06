# Implementation Plan

> Full plan with SQL and Rust code for each phase. Reference this when implementing.

## Phase 1: Rust API (website tab 1)

### SQL
```sql
CREATE SOURCE trades (
    symbol VARCHAR, price DOUBLE, volume BIGINT, ts BIGINT
)

CREATE MATERIALIZED VIEW ohlc_1m AS
SELECT symbol, FIRST(price) as open, MAX(price) as high,
       MIN(price) as low, LAST(price) as close
FROM trades
GROUP BY symbol, TUMBLE(ts, INTERVAL '1' MINUTE)
```

### Rust API calls exercised
- `LaminarDB::builder().buffer_size(65536).build().await?`
- `db.execute(SQL).await?`
- `db.start().await?`
- `db.source::<Trade>("trades")?`
- `db.subscribe::<OhlcBar>("ohlc_1m")?`
- `source.push_batch(vec![...])`
- `source.watermark(now())`
- `sub.poll()`

### Types
```rust
#[derive(Record, FromRecordBatch, Clone)]
struct Trade { symbol: String, price: f64, volume: i64, #[event_time] ts: i64 }

#[derive(FromRecordBatch, Clone)]
struct OhlcBar { symbol: String, open: f64, high: f64, low: f64, close: f64 }
```

---

## Phase 2: Streaming SQL (website tab 2)

### SQL
```sql
CREATE MATERIALIZED VIEW ohlc_1m AS
SELECT symbol, TUMBLE_START(ts, INTERVAL '1' MINUTE) AS bar_start,
       FIRST(price) AS open, MAX(price) AS high,
       MIN(price) AS low, LAST(price) AS close, SUM(volume) AS volume
FROM trades
GROUP BY symbol, TUMBLE(ts, INTERVAL '1' MINUTE)
EMIT ON WINDOW CLOSE;

CREATE MATERIALIZED VIEW ohlc_1h AS
SELECT symbol, TUMBLE_START(bar_start, INTERVAL '1' HOUR) AS hour_start,
       FIRST(open) AS open, MAX(high) AS high,
       MIN(low) AS low, LAST(close) AS close
FROM ohlc_1m
GROUP BY symbol, TUMBLE(bar_start, INTERVAL '1' HOUR);
```

### What's new vs Phase 1
- `TUMBLE_START()` function
- `EMIT ON WINDOW CLOSE`
- Cascading MVs (ohlc_1h reads from ohlc_1m)
- `SUM(volume)` aggregate

---

## Phase 3: Kafka Pipeline (website tab 3)

### SQL
```sql
CREATE SOURCE trades (
  symbol VARCHAR, price DOUBLE, ts BIGINT
) FROM KAFKA (
  brokers = '${KAFKA_BROKERS}',
  topic = 'market-trades',
  group_id = 'laminar-analytics',
  format = 'json',
  offset_reset = 'earliest'
);

CREATE MATERIALIZED VIEW trade_summary AS
SELECT symbol, COUNT(*) as trades, SUM(price * volume) as notional
FROM trades
GROUP BY symbol, TUMBLE(ts, INTERVAL '1' MINUTE);

CREATE SINK output TO KAFKA (
  brokers = '${KAFKA_BROKERS}',
  topic = 'trade-summaries',
  format = 'json'
) SELECT * FROM trade_summary;
```

### Config
- `KAFKA_BROKERS=localhost:19092` (Redpanda external port)
- Format: json (avro needs schema registry)

---

## Phase 4: Stream Joins (website tab 4)

### SQL — ASOF JOIN
```sql
CREATE SOURCE quotes (
    symbol VARCHAR, bid DOUBLE, ask DOUBLE, ts BIGINT
)

CREATE MATERIALIZED VIEW asof_enriched AS
SELECT t.symbol, t.price AS trade_price, t.volume,
       q.bid, q.ask, t.price - q.bid AS spread
FROM trades t
ASOF JOIN quotes q
  ON t.symbol = q.symbol
  AND t.ts >= q.ts
  TOLERANCE INTERVAL '5' SECOND;
```

### SQL — Stream-Stream Join
```sql
CREATE SOURCE orders (
    order_id VARCHAR, symbol VARCHAR, side VARCHAR,
    quantity BIGINT, price DOUBLE, ts BIGINT
)

CREATE MATERIALIZED VIEW trade_order_match AS
SELECT t.symbol, t.price AS trade_price, t.volume,
       o.order_id, o.side, o.price AS order_price,
       t.price - o.price AS price_diff
FROM trades t
INNER JOIN orders o
  ON t.symbol = o.symbol
  AND t.ts BETWEEN o.ts AND o.ts + INTERVAL '1' MINUTE
WHERE t.price >= o.price;
```

---

## Phase 5: CDC Pipeline (website tab 5)

### SQL
```sql
CREATE SOURCE orders_cdc (
  id INT, customer_id INT, amount DECIMAL, status VARCHAR, ts BIGINT
) FROM POSTGRES_CDC (
  hostname = 'localhost', port = '5432',
  database = 'shop', schema = 'public', table = 'orders',
  slot.name = 'laminar_orders', publication = 'laminar_pub'
);

CREATE MATERIALIZED VIEW customer_totals AS
SELECT customer_id, COUNT(*) AS total_orders, SUM(amount) AS total_spent
FROM orders_cdc
WHERE _op IN ('I', 'U')
GROUP BY customer_id
EMIT CHANGES;
```

---

## Bonus Phases (not on website)

### HOP window
```sql
CREATE MATERIALIZED VIEW hop_volume AS
SELECT symbol, SUM(volume) AS total_volume, COUNT(*) AS trade_count,
       AVG(price) AS avg_price
FROM trades
GROUP BY symbol, HOP(ts, INTERVAL '2' SECOND, INTERVAL '10' SECOND);
```

### SESSION window
```sql
CREATE MATERIALIZED VIEW session_bursts AS
SELECT symbol, COUNT(*) AS burst_trades, SUM(volume) AS burst_volume,
       MIN(price) AS low, MAX(price) AS high
FROM trades
GROUP BY symbol, SESSION(ts, INTERVAL '3' SECOND);
```

### EMIT ON UPDATE
```sql
CREATE MATERIALIZED VIEW emit_update_test AS
SELECT symbol, COUNT(*) AS cnt, SUM(volume) AS vol
FROM trades
GROUP BY symbol, TUMBLE(ts, INTERVAL '5' SECOND)
EMIT ON UPDATE;
```
