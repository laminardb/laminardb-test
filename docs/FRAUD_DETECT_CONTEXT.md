# Context from laminardb-fraud-detect Stress Testing

This document provides context from the `laminardb-fraud-detect` project's stress testing results.
The goal is to build a Phase 7 throughput test in laminardb-test that uses **path dependencies**
(local laminardb source) to compare against the published crate (v0.1.1) results below.

## Published Crate Baseline (v0.1.1) — Release Mode, 10s per level

```
==========================================================================================
                                   STRESS TEST RESULTS
==========================================================================================
 Level   Target/s   Actual/s   Push p50   Push p99   Proc p99   Alerts     Time
------------------------------------------------------------------------------------------
 1            100         98       28us      129us       83us      680    10.1s
 2            250        245       55us       87us       87us     2642    10.1s
 3           1000        608     54.3ms     57.1ms       96us     7569    10.0s
 4           2000        725     56.6ms    164.9ms      155us    14673    10.1s
 5          10000       1623     88.4ms    197.6ms      218us    34576    10.1s
 6          50000       2222    208.4ms    319.5ms      258us     7567    10.1s
 7         200000       2275    432.7ms    435.5ms      309us     2463    10.1s
==========================================================================================
Totals: 78765 trades, 23855 orders, 70170 alerts in 70.6s

Saturation point: Level 3 (~1000 trades/sec target)
  Actual throughput: 608/sec (61% of target)
  Push p99: 57.1ms
Peak sustained throughput: ~2275 trades/sec (Level 7)

Stream output totals:
  vol_baseline         12345
  ohlc_vol             6999
  rapid_fire           76533
  wash_score           27559
  suspicious_match     129002
  asof_match           0
```

## Key Findings

1. **~2,275 trades/sec is the engine ceiling** for a 6-stream pipeline with published crates v0.1.1.
   This was confirmed by reducing JOIN fan-out by 91% with no throughput change — the bottleneck
   is the micro-batch engine, not the SQL.

2. **push_batch() blocks at saturation** — Push p99 jumps from 87us (Level 2) to 57ms (Level 3).
   The engine's internal processing can't keep up, so push becomes the backpressure point.

3. **SESSION window emits per-batch, not per-session** — `rapid_fire` (SESSION with 2s gap) produces
   76,533 output rows from 78,765 input trades (nearly 1:1). A true session window should merge
   nearby events and emit far fewer rows.

4. **ASOF JOIN produces 0 output** — Known issue #57. SQL parses and stream creates, but no output
   rows. Works with local path deps per earlier laminardb-test findings.

## Pipeline Configuration (what to replicate)

The fraud-detect pipeline uses:
- **2 sources**: `trades` (7 columns) and `orders` (7 columns)
- **6 concurrent streams**:
  1. `vol_baseline` — `HOP(ts, INTERVAL '2' SECOND, INTERVAL '10' SECOND)` GROUP BY symbol
  2. `ohlc_vol` — `TUMBLE(ts, INTERVAL '5' SECOND)` GROUP BY symbol, with first_value/last_value
  3. `rapid_fire` — `SESSION(ts, INTERVAL '2' SECOND)` GROUP BY account_id
  4. `wash_score` — `TUMBLE(ts, INTERVAL '5' SECOND)` GROUP BY account_id, symbol, with CASE WHEN
  5. `suspicious_match` — `INNER JOIN` on symbol + `ts BETWEEN -2000 AND +2000`
  6. `asof_match` — `ASOF JOIN` with `MATCH_CONDITION(t.ts >= o.ts)` (0 output on published crate)
- **100ms micro-batch ticks** (default LaminarDB config)
- **buffer_size: 65536**

## Data Generation for Stress Test

Critical: the data generator must use **constant timestamp spacing** to keep JOIN fan-out
consistent across load levels. The fraud-detect project learned this the hard way:

- Use 50ms step between consecutive trades (constant, not dependent on batch size)
- Round-robin symbols across 5 tickers (AAPL, GOOGL, MSFT, AMZN, TSLA)
- ~30% of trades generate matching orders
- Sequential event timestamps between cycles (no overlap)
- Watermark = latest_event_ts + cycle_span + 10_000ms

### Stress Levels

```rust
const LEVELS: &[StressLevel] = &[
    StressLevel { trades_per_cycle: 10,   sleep_ms: 100, target_tps: 100 },
    StressLevel { trades_per_cycle: 25,   sleep_ms: 100, target_tps: 250 },
    StressLevel { trades_per_cycle: 50,   sleep_ms: 50,  target_tps: 1_000 },
    StressLevel { trades_per_cycle: 100,  sleep_ms: 50,  target_tps: 2_000 },
    StressLevel { trades_per_cycle: 200,  sleep_ms: 20,  target_tps: 10_000 },
    StressLevel { trades_per_cycle: 500,  sleep_ms: 10,  target_tps: 50_000 },
    StressLevel { trades_per_cycle: 1000, sleep_ms: 5,   target_tps: 200_000 },
];
```

## What Phase 7 Should Test

1. **Same 6-stream pipeline** using local path deps instead of published crates
2. **Same ramp test** (7 levels) to produce directly comparable throughput numbers
3. **Compare**: published crate (~2,275/sec peak) vs latest source — has throughput improved?
4. **Also test ASOF JOIN**: with path deps, ASOF JOIN should produce output (issue #57 is fixed in source)
5. **SESSION window behavior**: does latest source still emit per-batch or has it been fixed to emit per-session?

## LaminarDB SQL Gotchas (from laminardb-test learnings)

- Use `tumble()` not `TUMBLE_START()` — that's the registered UDF name
- Use `first_value()`/`last_value()` not `FIRST()`/`LAST()`
- `CAST(tumble(...) AS BIGINT)` for i64 fields — tumble returns Timestamp(Millisecond)
- Numeric time arithmetic only: `ts + 60000`, NOT `ts + INTERVAL '1' MINUTE`
- Source columns need `NOT NULL`
- `#[derive(Record)]` for inputs, `#[derive(FromRow)]` for outputs
- `laminar-core` must be a direct dependency (derive macro references it at compile time)
- FromRow field order must match SQL SELECT column order exactly
- `CREATE SINK` before `db.start()`, then `db.subscribe()` after start

## Hardware Context

All results from the same MacOS machine (Darwin 25.2.0). Debug mode peaks at ~1,736/sec,
release at ~2,275/sec (~31% improvement from release optimizations).

---

## Implementation Code (from laminardb-fraud-detect)

Adapt this for laminardb-test (path deps instead of crates.io). Do NOT rewrite from scratch.

### `src/types.rs` — Input/Output structs

```rust
use laminar_derive::{FromRow, Record};

// ── Input Types (pushed into sources) ──

#[derive(Debug, Clone, Record)]
pub struct Trade {
    pub account_id: String,
    pub symbol: String,
    pub side: String,
    pub price: f64,
    pub volume: i64,
    pub order_ref: String,
    #[event_time]
    pub ts: i64,
}

#[derive(Debug, Clone, Record)]
pub struct Order {
    pub order_id: String,
    pub account_id: String,
    pub symbol: String,
    pub side: String,
    pub quantity: i64,
    pub price: f64,
    #[event_time]
    pub ts: i64,
}

// ── Output Types (polled from subscriptions) ──
// IMPORTANT: field order must match SQL SELECT column order exactly

#[derive(Debug, Clone, FromRow)]
pub struct VolumeBaseline {
    pub symbol: String,
    pub total_volume: i64,
    pub trade_count: i64,
    pub avg_price: f64,
}

#[derive(Debug, Clone, FromRow)]
pub struct OhlcVolatility {
    pub symbol: String,
    pub bar_start: i64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: i64,
    pub price_range: f64,
}

#[derive(Debug, Clone, FromRow)]
pub struct RapidFireBurst {
    pub account_id: String,
    pub burst_trades: i64,
    pub burst_volume: i64,
    pub low: f64,
    pub high: f64,
}

#[derive(Debug, Clone, FromRow)]
pub struct WashScore {
    pub account_id: String,
    pub symbol: String,
    pub buy_volume: i64,
    pub sell_volume: i64,
    pub buy_count: i64,
    pub sell_count: i64,
}

#[derive(Debug, Clone, FromRow)]
pub struct SuspiciousMatch {
    pub symbol: String,
    pub trade_price: f64,
    pub volume: i64,
    pub order_id: String,
    pub account_id: String,
    pub side: String,
    pub order_price: f64,
    pub price_diff: f64,
}

#[derive(Debug, Clone, FromRow)]
pub struct AsofMatch {
    pub symbol: String,
    pub trade_price: f64,
    pub volume: i64,
    pub trade_account: String,
    pub order_id: String,
    pub order_account: String,
    pub order_price: f64,
    pub price_spread: f64,
}
```

### `src/detection.rs` — Pipeline setup (all 6 SQL streams)

```rust
use laminar_db::LaminarDB;
use crate::types::*;

pub struct DetectionPipeline {
    pub db: LaminarDB,
    pub trade_source: laminar_db::SourceHandle<Trade>,
    pub order_source: laminar_db::SourceHandle<Order>,
    pub vol_baseline_sub: Option<laminar_db::TypedSubscription<VolumeBaseline>>,
    pub ohlc_vol_sub: Option<laminar_db::TypedSubscription<OhlcVolatility>>,
    pub rapid_fire_sub: Option<laminar_db::TypedSubscription<RapidFireBurst>>,
    pub wash_score_sub: Option<laminar_db::TypedSubscription<WashScore>>,
    pub suspicious_match_sub: Option<laminar_db::TypedSubscription<SuspiciousMatch>>,
    pub asof_match_sub: Option<laminar_db::TypedSubscription<AsofMatch>>,
    pub streams_created: Vec<(String, bool)>,
}

pub async fn setup() -> Result<DetectionPipeline, Box<dyn std::error::Error>> {
    let db = LaminarDB::builder()
        .buffer_size(65536)
        .build()
        .await?;

    // ── Sources ──
    db.execute(
        "CREATE SOURCE trades (
            account_id VARCHAR NOT NULL,
            symbol     VARCHAR NOT NULL,
            side       VARCHAR NOT NULL,
            price      DOUBLE NOT NULL,
            volume     BIGINT NOT NULL,
            order_ref  VARCHAR NOT NULL,
            ts         BIGINT NOT NULL
        )",
    ).await?;

    db.execute(
        "CREATE SOURCE orders (
            order_id   VARCHAR NOT NULL,
            account_id VARCHAR NOT NULL,
            symbol     VARCHAR NOT NULL,
            side       VARCHAR NOT NULL,
            quantity   BIGINT NOT NULL,
            price      DOUBLE NOT NULL,
            ts         BIGINT NOT NULL
        )",
    ).await?;

    let mut streams_created = Vec::new();

    // ── Stream 1: Volume Baseline (HOP window) ──
    let vol_ok = try_create(&db, "vol_baseline",
        "CREATE STREAM vol_baseline AS
         SELECT symbol,
                SUM(volume) AS total_volume,
                COUNT(*) AS trade_count,
                AVG(price) AS avg_price
         FROM trades
         GROUP BY symbol, HOP(ts, INTERVAL '2' SECOND, INTERVAL '10' SECOND)"
    ).await;
    streams_created.push(("vol_baseline".into(), vol_ok));

    // ── Stream 2: OHLC + Volatility (TUMBLE window) ──
    let ohlc_ok = try_create(&db, "ohlc_vol",
        "CREATE STREAM ohlc_vol AS
         SELECT symbol,
                CAST(tumble(ts, INTERVAL '5' SECOND) AS BIGINT) AS bar_start,
                first_value(price) AS open,
                MAX(price) AS high,
                MIN(price) AS low,
                last_value(price) AS close,
                SUM(volume) AS volume,
                MAX(price) - MIN(price) AS price_range
         FROM trades
         GROUP BY symbol, tumble(ts, INTERVAL '5' SECOND)"
    ).await;
    streams_created.push(("ohlc_vol".into(), ohlc_ok));

    // ── Stream 3: Rapid-Fire Burst (SESSION window) ──
    let rapid_ok = try_create(&db, "rapid_fire",
        "CREATE STREAM rapid_fire AS
         SELECT account_id,
                COUNT(*) AS burst_trades,
                SUM(volume) AS burst_volume,
                MIN(price) AS low,
                MAX(price) AS high
         FROM trades
         GROUP BY account_id, SESSION(ts, INTERVAL '2' SECOND)"
    ).await;
    streams_created.push(("rapid_fire".into(), rapid_ok));

    // ── Stream 4: Wash Score (TUMBLE + CASE WHEN) ──
    let wash_ok = try_create(&db, "wash_score",
        "CREATE STREAM wash_score AS
         SELECT account_id,
                symbol,
                SUM(CASE WHEN side = 'buy' THEN volume ELSE CAST(0 AS BIGINT) END) AS buy_volume,
                SUM(CASE WHEN side = 'sell' THEN volume ELSE CAST(0 AS BIGINT) END) AS sell_volume,
                SUM(CASE WHEN side = 'buy' THEN 1 ELSE 0 END) AS buy_count,
                SUM(CASE WHEN side = 'sell' THEN 1 ELSE 0 END) AS sell_count
         FROM trades
         GROUP BY account_id, symbol, TUMBLE(ts, INTERVAL '5' SECOND)"
    ).await;
    streams_created.push(("wash_score".into(), wash_ok));

    // ── Stream 5: Suspicious Match (INNER JOIN, 2s window) ──
    let match_ok = try_create(&db, "suspicious_match",
        "CREATE STREAM suspicious_match AS
         SELECT t.symbol,
                t.price AS trade_price,
                t.volume,
                o.order_id,
                o.account_id,
                o.side,
                o.price AS order_price,
                t.price - o.price AS price_diff
         FROM trades t
         INNER JOIN orders o
         ON t.symbol = o.symbol
         AND o.ts BETWEEN t.ts - 2000 AND t.ts + 2000"
    ).await;
    streams_created.push(("suspicious_match".into(), match_ok));

    // ── Stream 6: ASOF Match (front-running detection) ──
    let asof_ok = try_create(&db, "asof_match",
        "CREATE STREAM asof_match AS
         SELECT t.symbol,
                t.price AS trade_price,
                t.volume,
                t.account_id AS trade_account,
                o.order_id,
                o.account_id AS order_account,
                o.price AS order_price,
                t.price - o.price AS price_spread
         FROM trades t
         ASOF JOIN orders o
         MATCH_CONDITION(t.ts >= o.ts)
         ON t.symbol = o.symbol"
    ).await;
    streams_created.push(("asof_match".into(), asof_ok));

    // ── Create sinks + subscribe ──
    // IMPORTANT: CREATE SINK before db.start(), subscribe() after start
    macro_rules! setup_sub {
        ($db:expr, $name:expr, $ok:expr, $ty:ty) => {
            if $ok {
                let _ = $db.execute(&format!("CREATE SINK {}_sink FROM {}", $name, $name)).await;
                match $db.subscribe::<$ty>($name) {
                    Ok(sub) => Some(sub),
                    Err(e) => {
                        eprintln!("  [WARN] Subscribe to {} failed: {e}", $name);
                        None
                    }
                }
            } else { None }
        };
    }

    let vol_baseline_sub = setup_sub!(db, "vol_baseline", vol_ok, VolumeBaseline);
    let ohlc_vol_sub = setup_sub!(db, "ohlc_vol", ohlc_ok, OhlcVolatility);
    let rapid_fire_sub = setup_sub!(db, "rapid_fire", rapid_ok, RapidFireBurst);
    let wash_score_sub = setup_sub!(db, "wash_score", wash_ok, WashScore);
    let suspicious_match_sub = setup_sub!(db, "suspicious_match", match_ok, SuspiciousMatch);
    let asof_match_sub = setup_sub!(db, "asof_match", asof_ok, AsofMatch);

    db.start().await?;

    let trade_source = db.source::<Trade>("trades")?;
    let order_source = db.source::<Order>("orders")?;

    Ok(DetectionPipeline {
        db, trade_source, order_source,
        vol_baseline_sub, ohlc_vol_sub, rapid_fire_sub,
        wash_score_sub, suspicious_match_sub, asof_match_sub,
        streams_created,
    })
}

async fn try_create(db: &LaminarDB, name: &str, sql: &str) -> bool {
    match db.execute(sql).await {
        Ok(_) => { eprintln!("  [OK] {} created", name); true }
        Err(e) => { eprintln!("  [WARN] {} failed: {e}", name); false }
    }
}
```

### `src/generator.rs` — Data generation (stress cycle)

```rust
use rand::Rng;
use std::collections::HashMap;
use crate::types::{Order, Trade};

pub const SYMBOLS: &[(&str, f64)] = &[
    ("AAPL", 150.0), ("GOOGL", 2800.0), ("MSFT", 420.0),
    ("AMZN", 185.0), ("TSLA", 250.0),
];
const NORMAL_ACCOUNTS: &[&str] = &["ACCT-001", "ACCT-002", "ACCT-003", "ACCT-004", "ACCT-005"];

pub struct FraudGenerator {
    prices: HashMap<String, f64>,
    order_seq: u64,
    trade_seq: u64,
}

impl FraudGenerator {
    pub fn new() -> Self {
        let mut prices = HashMap::new();
        for (sym, base) in SYMBOLS { prices.insert(sym.to_string(), *base); }
        Self { prices, order_seq: 0, trade_seq: 0 }
    }

    pub fn now_ms() -> i64 { chrono::Utc::now().timestamp_millis() }

    /// Event-time span of a stress cycle. Used to advance event_ts between cycles.
    pub fn stress_cycle_span_ms(count: usize) -> i64 { count as i64 * 50 }

    /// Constant 50ms step. Caller must advance base_ts between cycles.
    pub fn generate_stress_cycle(&mut self, base_ts: i64, count: usize) -> (Vec<Trade>, Vec<Order>) {
        let mut rng = rand::thread_rng();
        let mut trades = Vec::with_capacity(count);
        let mut orders = Vec::new();
        let step_ms: i64 = 50;

        for i in 0..count {
            let trade_ts = base_ts + (i as i64 * step_ms);
            let (sym, _) = SYMBOLS[i % SYMBOLS.len()];
            let symbol = sym.to_string();
            let price = self.prices.get_mut(&symbol).unwrap();
            *price += *price * rng.gen_range(-0.005..0.005);

            let account = NORMAL_ACCOUNTS[rng.gen_range(0..NORMAL_ACCOUNTS.len())];
            let side = if rng.gen_bool(0.5) { "buy" } else { "sell" };
            let volume = rng.gen_range(10..500);
            self.trade_seq += 1;

            trades.push(Trade {
                account_id: account.to_string(), symbol: symbol.clone(),
                side: side.to_string(), price: *price, volume,
                order_ref: format!("T-{:06}", self.trade_seq), ts: trade_ts,
            });

            if rng.gen_bool(0.3) {
                self.order_seq += 1;
                let offset = *price * rng.gen_range(-0.002..0.002);
                orders.push(Order {
                    order_id: format!("ORD-{:06}", self.order_seq),
                    account_id: account.to_string(), symbol,
                    side: side.to_string(), quantity: volume,
                    price: *price + offset, ts: trade_ts,
                });
            }
        }
        (trades, orders)
    }
}
```

### `src/stress.rs` — Stress test runner (complete working version)

```rust
use std::time::{Duration, Instant};
use crate::detection;
use crate::generator::FraudGenerator;

struct StressLevel { trades_per_cycle: usize, sleep_ms: u64, target_tps: u64 }

const LEVELS: &[StressLevel] = &[
    StressLevel { trades_per_cycle: 10,   sleep_ms: 100, target_tps: 100 },
    StressLevel { trades_per_cycle: 25,   sleep_ms: 100, target_tps: 250 },
    StressLevel { trades_per_cycle: 50,   sleep_ms: 50,  target_tps: 1_000 },
    StressLevel { trades_per_cycle: 100,  sleep_ms: 50,  target_tps: 2_000 },
    StressLevel { trades_per_cycle: 200,  sleep_ms: 20,  target_tps: 10_000 },
    StressLevel { trades_per_cycle: 500,  sleep_ms: 10,  target_tps: 50_000 },
    StressLevel { trades_per_cycle: 1000, sleep_ms: 5,   target_tps: 200_000 },
];

pub async fn run(level_duration: u64) -> Result<(), Box<dyn std::error::Error>> {
    println!("=== STRESS TEST ===");
    println!("Levels: {}, Duration per level: {}s", LEVELS.len(), level_duration);

    let pipeline = detection::setup().await?;
    let mut gen = FraudGenerator::new();
    let level_dur = Duration::from_secs(level_duration);
    let stream_names = ["vol_baseline", "ohlc_vol", "rapid_fire",
                        "wash_score", "suspicious_match", "asof_match"];

    for (idx, level) in LEVELS.iter().enumerate() {
        let level_num = idx + 1;
        let mut total_trades = 0u64;
        let mut total_orders = 0u64;
        let mut stream_counts: [u64; 6] = [0; 6];
        let mut push_latencies: Vec<u64> = Vec::new();

        // Sequential event timestamps — no overlap between cycles
        let mut event_ts: i64 = FraudGenerator::now_ms();
        let cycle_span = FraudGenerator::stress_cycle_span_ms(level.trades_per_cycle);
        let level_start = Instant::now();

        while level_start.elapsed() < level_dur {
            let (trades, orders) = gen.generate_stress_cycle(event_ts, level.trades_per_cycle);
            total_trades += trades.len() as u64;
            total_orders += orders.len() as u64;

            let push_start = Instant::now();
            pipeline.trade_source.push_batch(trades);
            if !orders.is_empty() { pipeline.order_source.push_batch(orders); }
            pipeline.trade_source.watermark(event_ts + cycle_span + 10_000);
            pipeline.order_source.watermark(event_ts + cycle_span + 10_000);
            push_latencies.push(push_start.elapsed().as_micros() as u64);

            event_ts += cycle_span; // advance past this cycle

            // Poll all 6 streams
            macro_rules! poll_stream {
                ($sub:expr, $idx:expr) => {
                    if let Some(ref sub) = $sub {
                        while let Some(rows) = sub.poll() {
                            stream_counts[$idx] += rows.len() as u64;
                        }
                    }
                };
            }
            poll_stream!(pipeline.vol_baseline_sub, 0);
            poll_stream!(pipeline.ohlc_vol_sub, 1);
            poll_stream!(pipeline.rapid_fire_sub, 2);
            poll_stream!(pipeline.wash_score_sub, 3);
            poll_stream!(pipeline.suspicious_match_sub, 4);
            poll_stream!(pipeline.asof_match_sub, 5);

            tokio::time::sleep(Duration::from_millis(level.sleep_ms)).await;
        }

        let elapsed = level_start.elapsed().as_secs_f64();
        let actual_tps = (total_trades as f64 / elapsed) as u64;
        push_latencies.sort();
        let n = push_latencies.len();
        let p50 = if n > 0 { push_latencies[n / 2] } else { 0 };
        let p99 = if n > 0 { push_latencies[(n * 99 / 100).min(n - 1)] } else { 0 };

        println!("Level {}/{}: target ~{}/s, actual {}/s, push p50={}us p99={}us, trades={} orders={}",
            level_num, LEVELS.len(), level.target_tps, actual_tps, p50, p99,
            total_trades, total_orders);

        // Print per-stream counts
        for (i, name) in stream_names.iter().enumerate() {
            if stream_counts[i] > 0 {
                print!("  {}={}", name, stream_counts[i]);
            }
        }
        println!();
    }

    // Saturation: where actual < 90% of target
    println!("\n--- Done ---");
    let _ = pipeline.db.shutdown().await;
    Ok(())
}
```

### Cargo.toml dependencies (for laminardb-test with path deps)

```toml
[dependencies]
# Path deps for testing latest laminardb source:
laminar-db = { path = "../laminardb/laminar-db" }
laminar-derive = { path = "../laminardb/laminar-derive" }
laminar-core = { path = "../laminardb/laminar-core" }
tokio = { version = "1", features = ["full"] }
chrono = "0.4"
rand = "0.8"

[dev-dependencies]
criterion = { version = "0.5", features = ["async_tokio"] }

[[bench]]
name = "throughput"
harness = false
```
