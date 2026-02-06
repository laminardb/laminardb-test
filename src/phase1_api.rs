//! Phase 1: Rust API test.
//!
//! Tests the embedded API from the laminardb.io website "Rust API" tab:
//! - LaminarDB::builder()
//! - db.execute() / db.start()
//! - db.source::<T>() / push_batch() / watermark()
//! - db.subscribe::<T>() / poll()
//! - #[derive(Record)] / #[derive(FromRecordBatch)]

use std::time::Duration;

use laminar_db::LaminarDB;

use crate::generator::MarketGenerator;
use crate::types::{OhlcBar, Trade};

pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Phase 1: Rust API Test ===");
    println!("Testing: builder, execute, start, source, subscribe, push_batch, watermark, poll");
    println!();

    // 1. Build the database
    let db = LaminarDB::builder()
        .buffer_size(65536)
        .build()
        .await?;
    println!("[OK] LaminarDB::builder().build()");

    // 2. Define source schema
    db.execute(
        "CREATE SOURCE trades (
            symbol VARCHAR, price DOUBLE, volume BIGINT, ts BIGINT
        )",
    )
    .await?;
    println!("[OK] db.execute(CREATE SOURCE)");

    // 3. Create materialized view (from website example)
    db.execute(
        "CREATE MATERIALIZED VIEW ohlc_1m AS
         SELECT symbol, FIRST(price) as open, MAX(price) as high,
                MIN(price) as low, LAST(price) as close
         FROM trades
         GROUP BY symbol, TUMBLE(ts, INTERVAL '1' MINUTE)",
    )
    .await?;
    println!("[OK] db.execute(CREATE MATERIALIZED VIEW)");

    // 4. Start processing
    db.start().await?;
    println!("[OK] db.start()");

    // 5. Get typed handles
    let source = db.source::<Trade>("trades")?;
    println!("[OK] db.source::<Trade>(\"trades\")");

    let sub = db.subscribe::<OhlcBar>("ohlc_1m")?;
    println!("[OK] db.subscribe::<OhlcBar>(\"ohlc_1m\")");

    // 6. Generate and push data
    let mut gen = MarketGenerator::new();
    let mut total_pushed = 0u64;
    let mut total_received = 0u64;

    println!();
    println!("Pushing trades and polling for OHLC bars...");
    println!("(Will run for 10 seconds)");
    println!();

    let start = std::time::Instant::now();

    while start.elapsed() < Duration::from_secs(10) {
        let ts = MarketGenerator::now_ms();

        // Push a batch of trades
        let trades = gen.generate_trades(ts);
        let count = trades.len();
        source.push_batch(trades);
        source.watermark(ts);
        total_pushed += count as u64;

        // Poll for results
        while let Some(bars) = sub.poll() {
            for bar in &bars {
                println!(
                    "  OHLC | {:<6} | open={:>10.2} high={:>10.2} low={:>10.2} close={:>10.2}",
                    bar.symbol, bar.open, bar.high, bar.low, bar.close
                );
                total_received += 1;
            }
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    println!();
    println!("=== Phase 1 Results ===");
    println!("  Trades pushed:     {}", total_pushed);
    println!("  OHLC bars received: {}", total_received);
    println!(
        "  Status:            {}",
        if total_received > 0 {
            "PASS"
        } else {
            "FAIL (no output received)"
        }
    );

    Ok(())
}
