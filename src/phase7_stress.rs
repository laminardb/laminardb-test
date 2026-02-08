//! Phase 7: Stress Test — 6-stream fraud-detect pipeline throughput benchmark.
//!
//! Adapted from laminardb-fraud-detect (published crate v0.1.1 baseline).
//! Uses path dependencies to compare against the published crate results:
//!   - Published crate peak: ~2,275 trades/sec
//!   - Saturation point: Level 3 (~1,000 target → 608 actual)
//!
//! Pipeline: 2 sources (trades + orders) → 6 concurrent streams:
//!   1. vol_baseline    — HOP(2s slide, 10s window) GROUP BY symbol
//!   2. ohlc_vol        — TUMBLE(5s) GROUP BY symbol, OHLC + volatility
//!   3. rapid_fire      — SESSION(2s gap) GROUP BY account_id
//!   4. wash_score      — TUMBLE(5s) GROUP BY account_id, symbol, CASE WHEN
//!   5. suspicious_match — INNER JOIN on symbol + 2s time window
//!   6. asof_match      — ASOF JOIN with MATCH_CONDITION (may produce 0 output)

use std::collections::HashMap;
use std::time::{Duration, Instant};

use laminar_db::LaminarDB;
use rand::Rng;

use crate::types::*;

// ── Constants ──

const SYMBOLS: &[(&str, f64)] = &[
    ("AAPL", 150.0),
    ("GOOGL", 2800.0),
    ("MSFT", 420.0),
    ("AMZN", 185.0),
    ("TSLA", 250.0),
];
const NORMAL_ACCOUNTS: &[&str] = &["ACCT-001", "ACCT-002", "ACCT-003", "ACCT-004", "ACCT-005"];

pub struct StressLevel {
    pub trades_per_cycle: usize,
    pub sleep_ms: u64,
    pub target_tps: u64,
}

pub const LEVELS: &[StressLevel] = &[
    StressLevel { trades_per_cycle: 10,   sleep_ms: 100, target_tps: 100 },
    StressLevel { trades_per_cycle: 25,   sleep_ms: 100, target_tps: 250 },
    StressLevel { trades_per_cycle: 50,   sleep_ms: 50,  target_tps: 1_000 },
    StressLevel { trades_per_cycle: 100,  sleep_ms: 50,  target_tps: 2_000 },
    StressLevel { trades_per_cycle: 200,  sleep_ms: 20,  target_tps: 10_000 },
    StressLevel { trades_per_cycle: 500,  sleep_ms: 10,  target_tps: 50_000 },
    StressLevel { trades_per_cycle: 1000, sleep_ms: 5,   target_tps: 200_000 },
];

pub const STREAM_NAMES: [&str; 6] = [
    "vol_baseline",
    "ohlc_vol",
    "rapid_fire",
    "wash_score",
    "suspicious_match",
    "asof_match",
];

// ── Data Generator ──

pub struct FraudGenerator {
    prices: HashMap<String, f64>,
    order_seq: u64,
    trade_seq: u64,
}

impl FraudGenerator {
    pub fn new() -> Self {
        let mut prices = HashMap::new();
        for (sym, base) in SYMBOLS {
            prices.insert(sym.to_string(), *base);
        }
        Self {
            prices,
            order_seq: 0,
            trade_seq: 0,
        }
    }

    pub fn now_ms() -> i64 {
        chrono::Utc::now().timestamp_millis()
    }

    /// Event-time span of a stress cycle (constant 50ms step).
    pub fn stress_cycle_span_ms(count: usize) -> i64 {
        count as i64 * 50
    }

    /// Generate one cycle of trades + orders with constant 50ms timestamp spacing.
    pub fn generate_stress_cycle(
        &mut self,
        base_ts: i64,
        count: usize,
    ) -> (Vec<StressTrade>, Vec<StressOrder>) {
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

            trades.push(StressTrade {
                account_id: account.to_string(),
                symbol: symbol.clone(),
                side: side.to_string(),
                price: *price,
                volume,
                order_ref: format!("T-{:06}", self.trade_seq),
                ts: trade_ts,
            });

            // ~30% of trades generate matching orders
            if rng.gen_bool(0.3) {
                self.order_seq += 1;
                let offset = *price * rng.gen_range(-0.002..0.002);
                orders.push(StressOrder {
                    order_id: format!("ORD-{:06}", self.order_seq),
                    account_id: account.to_string(),
                    symbol,
                    side: side.to_string(),
                    quantity: volume,
                    price: *price + offset,
                    ts: trade_ts,
                });
            }
        }
        (trades, orders)
    }
}

// ── Pipeline Setup ──

pub struct DetectionPipeline {
    #[allow(dead_code)]
    pub db: LaminarDB,
    pub trade_source: laminar_db::SourceHandle<StressTrade>,
    pub order_source: laminar_db::SourceHandle<StressOrder>,
    pub vol_baseline_sub: Option<laminar_db::TypedSubscription<VolumeBaseline>>,
    pub ohlc_vol_sub: Option<laminar_db::TypedSubscription<OhlcVolatility>>,
    pub rapid_fire_sub: Option<laminar_db::TypedSubscription<RapidFireBurst>>,
    pub wash_score_sub: Option<laminar_db::TypedSubscription<WashScore>>,
    pub suspicious_match_sub: Option<laminar_db::TypedSubscription<SuspiciousMatch>>,
    pub asof_match_sub: Option<laminar_db::TypedSubscription<AsofMatch>>,
    pub streams_created: Vec<(String, bool)>,
}

pub async fn try_create(db: &LaminarDB, name: &str, sql: &str) -> bool {
    match db.execute(sql).await {
        Ok(_) => {
            eprintln!("  [OK] {} created", name);
            true
        }
        Err(e) => {
            eprintln!("  [WARN] {} failed: {e}", name);
            false
        }
    }
}

pub async fn setup_pipeline() -> Result<DetectionPipeline, Box<dyn std::error::Error>> {
    let db = LaminarDB::builder().buffer_size(65536).build().await?;

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
    )
    .await?;

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
    )
    .await?;

    let mut streams_created = Vec::new();

    // ── Stream 1: Volume Baseline (HOP window) ──
    let vol_ok = try_create(
        &db,
        "vol_baseline",
        "CREATE STREAM vol_baseline AS
         SELECT symbol,
                SUM(volume) AS total_volume,
                COUNT(*) AS trade_count,
                AVG(price) AS avg_price
         FROM trades
         GROUP BY symbol, HOP(ts, INTERVAL '2' SECOND, INTERVAL '10' SECOND)",
    )
    .await;
    streams_created.push(("vol_baseline".into(), vol_ok));

    // ── Stream 2: OHLC + Volatility (TUMBLE window) ──
    let ohlc_ok = try_create(
        &db,
        "ohlc_vol",
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
         GROUP BY symbol, tumble(ts, INTERVAL '5' SECOND)",
    )
    .await;
    streams_created.push(("ohlc_vol".into(), ohlc_ok));

    // ── Stream 3: Rapid-Fire Burst (SESSION window) ──
    let rapid_ok = try_create(
        &db,
        "rapid_fire",
        "CREATE STREAM rapid_fire AS
         SELECT account_id,
                COUNT(*) AS burst_trades,
                SUM(volume) AS burst_volume,
                MIN(price) AS low,
                MAX(price) AS high
         FROM trades
         GROUP BY account_id, SESSION(ts, INTERVAL '2' SECOND)",
    )
    .await;
    streams_created.push(("rapid_fire".into(), rapid_ok));

    // ── Stream 4: Wash Score (TUMBLE + CASE WHEN) ──
    let wash_ok = try_create(
        &db,
        "wash_score",
        "CREATE STREAM wash_score AS
         SELECT account_id,
                symbol,
                SUM(CASE WHEN side = 'buy' THEN volume ELSE CAST(0 AS BIGINT) END) AS buy_volume,
                SUM(CASE WHEN side = 'sell' THEN volume ELSE CAST(0 AS BIGINT) END) AS sell_volume,
                SUM(CASE WHEN side = 'buy' THEN 1 ELSE 0 END) AS buy_count,
                SUM(CASE WHEN side = 'sell' THEN 1 ELSE 0 END) AS sell_count
         FROM trades
         GROUP BY account_id, symbol, TUMBLE(ts, INTERVAL '5' SECOND)",
    )
    .await;
    streams_created.push(("wash_score".into(), wash_ok));

    // ── Stream 5: Suspicious Match (INNER JOIN, 2s window) ──
    let match_ok = try_create(
        &db,
        "suspicious_match",
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
         AND o.ts BETWEEN t.ts - 2000 AND t.ts + 2000",
    )
    .await;
    streams_created.push(("suspicious_match".into(), match_ok));

    // ── Stream 6: ASOF Match (front-running detection) ──
    let asof_ok = try_create(
        &db,
        "asof_match",
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
         ON t.symbol = o.symbol",
    )
    .await;
    streams_created.push(("asof_match".into(), asof_ok));

    // ── Create sinks + subscribe ──
    // IMPORTANT: CREATE SINK before db.start(), subscribe() after start
    macro_rules! setup_sub {
        ($db:expr, $name:expr, $ok:expr, $ty:ty) => {
            if $ok {
                let _ = $db
                    .execute(&format!("CREATE SINK {}_sink FROM {}", $name, $name))
                    .await;
                match $db.subscribe::<$ty>($name) {
                    Ok(sub) => Some(sub),
                    Err(e) => {
                        eprintln!("  [WARN] Subscribe to {} failed: {e}", $name);
                        None
                    }
                }
            } else {
                None
            }
        };
    }

    let vol_baseline_sub = setup_sub!(db, "vol_baseline", vol_ok, VolumeBaseline);
    let ohlc_vol_sub = setup_sub!(db, "ohlc_vol", ohlc_ok, OhlcVolatility);
    let rapid_fire_sub = setup_sub!(db, "rapid_fire", rapid_ok, RapidFireBurst);
    let wash_score_sub = setup_sub!(db, "wash_score", wash_ok, WashScore);
    let suspicious_match_sub = setup_sub!(db, "suspicious_match", match_ok, SuspiciousMatch);
    let asof_match_sub = setup_sub!(db, "asof_match", asof_ok, AsofMatch);

    db.start().await?;

    let trade_source = db.source::<StressTrade>("trades")?;
    let order_source = db.source::<StressOrder>("orders")?;

    Ok(DetectionPipeline {
        db,
        trade_source,
        order_source,
        vol_baseline_sub,
        ohlc_vol_sub,
        rapid_fire_sub,
        wash_score_sub,
        suspicious_match_sub,
        asof_match_sub,
        streams_created,
    })
}

// ── Latency helpers ──

pub fn format_latency(us: u64) -> String {
    if us < 1_000 {
        format!("{}us", us)
    } else if us < 1_000_000 {
        format!("{:.1}ms", us as f64 / 1_000.0)
    } else {
        format!("{:.1}s", us as f64 / 1_000_000.0)
    }
}

// ── Stress Test Runner ──

pub struct LevelResult {
    pub level_num: usize,
    pub target_tps: u64,
    pub actual_tps: u64,
    pub push_p50: u64,
    pub push_p99: u64,
    pub total_trades: u64,
    pub total_orders: u64,
    pub stream_counts: [u64; 6],
    pub elapsed_secs: f64,
}

pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let level_duration: u64 = std::env::var("STRESS_DURATION")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(10);

    println!("=== PHASE 7: STRESS TEST (6-stream fraud-detect pipeline) ===");
    println!(
        "Levels: {}, Duration per level: {}s",
        LEVELS.len(),
        level_duration
    );
    println!("Baseline (published crate v0.1.1): ~2,275 trades/sec peak");
    println!();

    // Show which streams were created
    println!("Setting up pipeline...");
    let pipeline = setup_pipeline().await?;
    println!();
    println!("Streams created:");
    for (name, ok) in &pipeline.streams_created {
        println!(
            "  {} {}",
            if *ok { "[OK]  " } else { "[FAIL]" },
            name
        );
    }
    println!();

    let mut gen = FraudGenerator::new();
    let level_dur = Duration::from_secs(level_duration);
    let mut results: Vec<LevelResult> = Vec::new();
    let mut grand_trades = 0u64;
    let mut grand_orders = 0u64;
    let mut grand_stream_totals = [0u64; 6];
    let test_start = Instant::now();

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
            let (trades, orders) =
                gen.generate_stress_cycle(event_ts, level.trades_per_cycle);
            total_trades += trades.len() as u64;
            total_orders += orders.len() as u64;

            let push_start = Instant::now();
            pipeline.trade_source.push_batch(trades);
            if !orders.is_empty() {
                pipeline.order_source.push_batch(orders);
            }
            pipeline
                .trade_source
                .watermark(event_ts + cycle_span + 10_000);
            pipeline
                .order_source
                .watermark(event_ts + cycle_span + 10_000);
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
        let p99 = if n > 0 {
            push_latencies[(n * 99 / 100).min(n - 1)]
        } else {
            0
        };

        // Print level results inline
        println!(
            " {:>2}  {:>8}/s  {:>7}/s  {:>9}  {:>9}  {:>6}  {:>6}  {:>6}  {:>6}  {:>6}  {:>6}  {:.1}s",
            level_num,
            level.target_tps,
            actual_tps,
            format_latency(p50),
            format_latency(p99),
            stream_counts[0],
            stream_counts[1],
            stream_counts[2],
            stream_counts[3],
            stream_counts[4],
            stream_counts[5],
            elapsed,
        );

        grand_trades += total_trades;
        grand_orders += total_orders;
        for i in 0..6 {
            grand_stream_totals[i] += stream_counts[i];
        }

        results.push(LevelResult {
            level_num,
            target_tps: level.target_tps,
            actual_tps,
            push_p50: p50,
            push_p99: p99,
            total_trades,
            total_orders,
            stream_counts,
            elapsed_secs: elapsed,
        });
    }

    let total_elapsed = test_start.elapsed().as_secs_f64();

    // ── Summary ──
    println!();
    println!("==========================================================================================");
    println!("                                   STRESS TEST RESULTS");
    println!("==========================================================================================");
    println!(
        " {:>5}  {:>9}  {:>9}  {:>9}  {:>9}  {:>7}  {:>7}  {:>7}",
        "Level", "Target/s", "Actual/s", "Push p50", "Push p99", "Trades", "Orders", "Time"
    );
    println!("------------------------------------------------------------------------------------------");
    for r in &results {
        println!(
            " {:>5}  {:>9}  {:>9}  {:>9}  {:>9}  {:>7}  {:>7}  {:>5.1}s",
            r.level_num,
            r.target_tps,
            r.actual_tps,
            format_latency(r.push_p50),
            format_latency(r.push_p99),
            r.total_trades,
            r.total_orders,
            r.elapsed_secs,
        );
    }
    println!("==========================================================================================");
    println!(
        "Totals: {} trades, {} orders in {:.1}s",
        grand_trades, grand_orders, total_elapsed
    );

    // Saturation point: where actual < 90% of target
    if let Some(sat) = results
        .iter()
        .find(|r| (r.actual_tps as f64) < (r.target_tps as f64 * 0.9))
    {
        let pct = (sat.actual_tps as f64 / sat.target_tps as f64 * 100.0) as u64;
        println!();
        println!("Saturation point: Level {} (~{}/sec target)", sat.level_num, sat.target_tps);
        println!(
            "  Actual throughput: {}/sec ({}% of target)",
            sat.actual_tps, pct
        );
        println!("  Push p99: {}", format_latency(sat.push_p99));
    }

    // Peak sustained throughput
    if let Some(peak) = results.iter().max_by_key(|r| r.actual_tps) {
        println!(
            "Peak sustained throughput: ~{} trades/sec (Level {})",
            peak.actual_tps, peak.level_num
        );
    }

    // Stream output totals
    println!();
    println!("Stream output totals:");
    for (i, name) in STREAM_NAMES.iter().enumerate() {
        println!("  {:<25} {}", name, grand_stream_totals[i]);
    }

    // Compare against baseline
    let peak_tps = results.iter().map(|r| r.actual_tps).max().unwrap_or(0);
    println!();
    println!("--- Comparison vs published crate v0.1.1 ---");
    println!("  Published peak: ~2,275 trades/sec");
    println!("  Path deps peak: ~{} trades/sec", peak_tps);
    if peak_tps > 2275 {
        let improvement = ((peak_tps as f64 / 2275.0 - 1.0) * 100.0) as i64;
        println!("  Result: +{}% improvement", improvement);
    } else if peak_tps < 2275 {
        let regression = ((1.0 - peak_tps as f64 / 2275.0) * 100.0) as i64;
        println!("  Result: -{}% regression", regression);
    } else {
        println!("  Result: identical");
    }

    // ASOF JOIN status
    let asof_total = grand_stream_totals[5];
    println!();
    if asof_total > 0 {
        println!("ASOF JOIN: PASS ({} output rows — issue #57 fixed in source)", asof_total);
    } else {
        println!("ASOF JOIN: 0 output (same as published crate — issue #57 not fixed)");
    }

    // SESSION window check
    let rapid_total = grand_stream_totals[2];
    let ratio = if grand_trades > 0 {
        rapid_total as f64 / grand_trades as f64
    } else {
        0.0
    };
    if ratio > 0.9 {
        println!(
            "SESSION window: per-batch emit ({} outputs from {} trades, {:.1}:1 ratio — not fixed)",
            rapid_total, grand_trades, ratio
        );
    } else {
        println!(
            "SESSION window: proper session merge ({} outputs from {} trades, {:.2}:1 ratio)",
            rapid_total, grand_trades, ratio
        );
    }

    println!("\n--- Done ---");
    Ok(())
}
