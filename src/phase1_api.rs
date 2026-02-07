//! Phase 1: Rust API test.
//!
//! Tests the embedded API from the laminardb.io website "Rust API" tab:
//! - LaminarDB::builder()
//! - db.execute() / db.start()
//! - db.source::<T>() / push_batch() / watermark()
//! - db.subscribe::<T>() / poll()
//! - #[derive(Record)] / #[derive(FromRow)]

use std::time::Duration;

use laminar_db::LaminarDB;

use crate::generator::MarketGenerator;
use crate::types::{OhlcBar, Trade};

/// Handles returned by Phase 1 setup, used by both CLI and TUI modes.
pub struct Phase1Handles {
    pub db: LaminarDB,
    pub source: laminar_db::SourceHandle<Trade>,
    pub sub: laminar_db::TypedSubscription<OhlcBar>,
}

/// Set up the Phase 1 pipeline (builder, source, stream, sink, start).
/// Returns handles for pushing data and polling results.
pub async fn setup() -> Result<Phase1Handles, Box<dyn std::error::Error>> {
    let db = LaminarDB::builder()
        .buffer_size(65536)
        .build()
        .await?;

    db.execute(
        "CREATE SOURCE trades (
            symbol  VARCHAR NOT NULL,
            price   DOUBLE NOT NULL,
            volume  BIGINT NOT NULL,
            ts      BIGINT NOT NULL
        )",
    )
    .await?;

    // NOTE: Use first_value/last_value (DataFusion built-in names), not FIRST/LAST.
    db.execute(
        "CREATE STREAM ohlc_1m AS
         SELECT symbol, first_value(price) as open, MAX(price) as high,
                MIN(price) as low, last_value(price) as close
         FROM trades
         GROUP BY symbol, TUMBLE(ts, INTERVAL '5' SECOND)",
    )
    .await?;

    db.execute("CREATE SINK ohlc_output FROM ohlc_1m").await?;

    db.start().await?;

    let source = db.source::<Trade>("trades")?;
    let sub = db.subscribe::<OhlcBar>("ohlc_1m")?;

    Ok(Phase1Handles { db, source, sub })
}

/// Run Phase 1 in CLI mode (println output).
pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Phase 1: Rust API Test ===");
    println!("Testing: builder, execute, start, source, subscribe, push_batch, watermark, poll");
    println!();

    let handles = setup().await?;
    println!("[OK] Pipeline setup complete");

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

        let trades = gen.generate_trades(ts);
        let count = trades.len();
        handles.source.push_batch(trades);
        handles.source.watermark(ts + 5_000);
        total_pushed += count as u64;

        while let Some(bars) = handles.sub.poll() {
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
