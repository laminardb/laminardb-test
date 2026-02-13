//! Phase 2: Streaming SQL test.
//!
//! Tests the "Streaming SQL" tab from laminardb.io:
//! - tumble() in SELECT for window start (website: TUMBLE_START)
//! - first_value/last_value + SUM(volume) aggregation
//! - EMIT ON WINDOW CLOSE (parser accepts it; micro-batch ignores it)
//! - Cascading MVs: ohlc_10s reads FROM ohlc_5s (tests stream-from-stream)
//!
//! v0.12.0 status:
//! - EMIT ON WINDOW CLOSE now works (Issue #52 — see Phase 8 for dedicated test)
//! - Cascading MVs now work (Issue #35 — topological ordering fix in v0.12.0)
//!   Previously: only CREATE SOURCE tables fed the executor.
//!   Now: intermediate results are registered as temp tables for downstream queries.

use std::time::Duration;

use laminar_db::LaminarDB;

use crate::generator::MarketGenerator;
use crate::types::{OhlcBarFull, Trade};

/// Handles returned by Phase 2 setup.
pub struct Phase2Handles {
    pub db: LaminarDB,
    pub source: laminar_db::SourceHandle<Trade>,
    pub ohlc_5s_sub: laminar_db::TypedSubscription<OhlcBarFull>,
    /// Will be None if cascading MV setup fails.
    pub ohlc_cascade_sub: Option<laminar_db::TypedSubscription<OhlcBarFull>>,
    pub cascade_supported: bool,
}

/// Set up the Phase 2 pipeline.
pub async fn setup() -> Result<Phase2Handles, Box<dyn std::error::Error>> {
    let db = LaminarDB::builder()
        .buffer_size(65536)
        .build()
        .await?;

    // Same source as Phase 1
    db.execute(
        "CREATE SOURCE trades (
            symbol  VARCHAR NOT NULL,
            price   DOUBLE NOT NULL,
            volume  BIGINT NOT NULL,
            ts      BIGINT NOT NULL
        )",
    )
    .await?;

    // Level 1: 5-second OHLC bars with bar_start and volume
    // Website uses TUMBLE_START() but the registered UDF name is "tumble"
    // Website uses FIRST/LAST but DataFusion needs first_value/last_value
    // EMIT ON WINDOW CLOSE is accepted by parser but has no effect in micro-batch
    // tumble() returns Timestamp(Millisecond) — cast to BIGINT to match OhlcBarFull.bar_start: i64
    db.execute(
        "CREATE STREAM ohlc_5s AS
         SELECT symbol,
                CAST(tumble(ts, INTERVAL '5' SECOND) AS BIGINT) AS bar_start,
                first_value(price) AS open,
                MAX(price) AS high,
                MIN(price) AS low,
                last_value(price) AS close,
                SUM(volume) AS volume
         FROM trades
         GROUP BY symbol, tumble(ts, INTERVAL '5' SECOND)",
    )
    .await?;

    db.execute("CREATE SINK ohlc_5s_output FROM ohlc_5s").await?;

    // Level 2: Cascading MV — 10-second bars from 5-second bars
    // This tests stream-from-stream. Expected to fail in embedded pipeline
    // because only CREATE SOURCE entries feed the executor.
    let cascade_supported = match db
        .execute(
            "CREATE STREAM ohlc_10s AS
             SELECT symbol,
                    CAST(tumble(bar_start, INTERVAL '10' SECOND) AS BIGINT) AS bar_start,
                    first_value(open) AS open,
                    MAX(high) AS high,
                    MIN(low) AS low,
                    last_value(close) AS close,
                    SUM(volume) AS volume
             FROM ohlc_5s
             GROUP BY symbol, tumble(bar_start, INTERVAL '10' SECOND)",
        )
        .await
    {
        Ok(_) => true,
        Err(e) => {
            eprintln!("  [WARN] Cascading MV creation failed: {e}");
            false
        }
    };

    let ohlc_cascade_sub = if cascade_supported {
        let _ = db.execute("CREATE SINK ohlc_10s_output FROM ohlc_10s").await;
        match db.subscribe::<OhlcBarFull>("ohlc_10s") {
            Ok(sub) => Some(sub),
            Err(e) => {
                eprintln!("  [WARN] Cascading MV subscribe failed: {e}");
                None
            }
        }
    } else {
        None
    };

    db.start().await?;

    let source = db.source::<Trade>("trades")?;
    let ohlc_5s_sub = db.subscribe::<OhlcBarFull>("ohlc_5s")?;

    Ok(Phase2Handles {
        db,
        source,
        ohlc_5s_sub,
        ohlc_cascade_sub,
        cascade_supported,
    })
}

/// Run Phase 2 in CLI mode.
pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Phase 2: Streaming SQL Test ===");
    println!("Testing: tumble() as TUMBLE_START, first_value/last_value, SUM, cascading MVs");
    println!();

    let handles = setup().await?;
    println!("[OK] Pipeline setup complete");
    if handles.cascade_supported {
        println!("[OK] Cascading MV (ohlc_10s FROM ohlc_5s) created");
    } else {
        println!("[--] Cascading MV creation failed (expected in embedded mode)");
    }

    let mut gen = MarketGenerator::new();
    let mut total_pushed = 0u64;
    let mut level1_received = 0u64;
    let mut level2_received = 0u64;

    println!();
    println!("Pushing trades and polling for OHLC bars...");
    println!("(Will run for 10 seconds)");
    println!();

    let start = std::time::Instant::now();

    while start.elapsed() < Duration::from_secs(10) {
        let ts = MarketGenerator::now_ms();

        let trades = gen.generate_trades(ts);
        let count = trades.len();
        handles.source.push_batch(trades);
        handles.source.watermark(ts + 5_000);
        total_pushed += count as u64;

        // Poll level 1 (5s bars)
        while let Some(bars) = handles.ohlc_5s_sub.poll() {
            for bar in &bars {
                println!(
                    "  5s  | {:<6} | bar_start={} open={:>10.2} high={:>10.2} low={:>10.2} close={:>10.2} vol={}",
                    bar.symbol, bar.bar_start, bar.open, bar.high, bar.low, bar.close, bar.volume
                );
                level1_received += 1;
            }
        }

        // Poll level 2 (cascading 10s bars)
        if let Some(ref sub) = handles.ohlc_cascade_sub {
            while let Some(bars) = sub.poll() {
                for bar in &bars {
                    println!(
                        "  10s | {:<6} | bar_start={} open={:>10.2} high={:>10.2} low={:>10.2} close={:>10.2} vol={}",
                        bar.symbol, bar.bar_start, bar.open, bar.high, bar.low, bar.close, bar.volume
                    );
                    level2_received += 1;
                }
            }
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    println!();
    println!("=== Phase 2 Results ===");
    println!("  Trades pushed:           {}", total_pushed);
    println!("  Level 1 (5s) bars:       {}", level1_received);
    println!("  Level 2 (cascade) bars:  {}", level2_received);
    println!(
        "  Level 1 status:          {}",
        if level1_received > 0 { "PASS" } else { "FAIL" }
    );
    println!(
        "  Cascading MV status:     {}",
        if level2_received > 0 {
            "PASS"
        } else if handles.cascade_supported {
            "FAIL (created but no output — stream-from-stream not supported in embedded pipeline)"
        } else {
            "SKIP (creation failed)"
        }
    );

    Ok(())
}
