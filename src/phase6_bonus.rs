//! Phase 6+: Bonus window types and emit modes.
//!
//! Tests features supported by laminardb but not shown on the website:
//! - HOP (sliding) window: overlapping windows with slide + size
//! - SESSION window: gap-based dynamic windows
//! - EMIT ON UPDATE: emit intermediate results on every state change
//!
//! All embedded, no external dependencies.

use std::time::Duration;

use laminar_db::LaminarDB;

use crate::generator::MarketGenerator;
use crate::types::{HopVolume, OhlcBar, SessionBurst, Trade};

/// Handles returned by Phase 6 setup.
pub struct Phase6Handles {
    pub db: LaminarDB,
    pub source: laminar_db::SourceHandle<Trade>,
    pub hop_sub: Option<laminar_db::TypedSubscription<HopVolume>>,
    pub session_sub: Option<laminar_db::TypedSubscription<SessionBurst>>,
    pub emit_sub: Option<laminar_db::TypedSubscription<OhlcBar>>,
    pub hop_created: bool,
    pub session_created: bool,
    pub emit_created: bool,
}

/// Set up the Phase 6 bonus pipeline.
///
/// Creates one source with three streams:
/// 1. HOP window (2s slide, 10s size) → hop_volume
/// 2. SESSION window (3s gap) → session_burst
/// 3. TUMBLE + EMIT ON UPDATE → ohlc_update
pub async fn setup() -> Result<Phase6Handles, Box<dyn std::error::Error>> {
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

    // Bonus 1: HOP (sliding) window — 2s slide, 10s window size
    let hop_created = match db
        .execute(
            "CREATE STREAM hop_volume AS
             SELECT symbol,
                    SUM(volume) AS total_volume,
                    COUNT(*) AS trade_count,
                    AVG(price) AS avg_price
             FROM trades
             GROUP BY symbol, HOP(ts, INTERVAL '2' SECOND, INTERVAL '10' SECOND)",
        )
        .await
    {
        Ok(_) => true,
        Err(e) => {
            eprintln!("  [WARN] HOP window failed: {e}");
            false
        }
    };

    // Bonus 2: SESSION window — 3s gap
    let session_created = match db
        .execute(
            "CREATE STREAM session_burst AS
             SELECT symbol,
                    COUNT(*) AS burst_trades,
                    SUM(volume) AS burst_volume,
                    MIN(price) AS low,
                    MAX(price) AS high
             FROM trades
             GROUP BY symbol, SESSION(ts, INTERVAL '3' SECOND)",
        )
        .await
    {
        Ok(_) => true,
        Err(e) => {
            eprintln!("  [WARN] SESSION window failed: {e}");
            false
        }
    };

    // Bonus 3: EMIT ON UPDATE — same as Phase 1 OHLC but with EMIT ON UPDATE
    let emit_created = match db
        .execute(
            "CREATE STREAM ohlc_update AS
             SELECT symbol,
                    first_value(price) AS open,
                    MAX(price) AS high,
                    MIN(price) AS low,
                    last_value(price) AS close
             FROM trades
             GROUP BY symbol, TUMBLE(ts, INTERVAL '5' SECOND)
             EMIT ON UPDATE",
        )
        .await
    {
        Ok(_) => true,
        Err(e) => {
            eprintln!("  [WARN] EMIT ON UPDATE failed: {e}");
            false
        }
    };

    // Create sinks for each stream that was created
    if hop_created {
        db.execute("CREATE SINK hop_local FROM hop_volume").await?;
    }
    if session_created {
        db.execute("CREATE SINK session_local FROM session_burst")
            .await?;
    }
    if emit_created {
        db.execute("CREATE SINK emit_local FROM ohlc_update")
            .await?;
    }

    db.start().await?;

    let source = db.source::<Trade>("trades")?;

    let hop_sub = if hop_created {
        match db.subscribe::<HopVolume>("hop_volume") {
            Ok(sub) => Some(sub),
            Err(e) => {
                eprintln!("  [WARN] Subscribe to hop_volume failed: {e}");
                None
            }
        }
    } else {
        None
    };

    let session_sub = if session_created {
        match db.subscribe::<SessionBurst>("session_burst") {
            Ok(sub) => Some(sub),
            Err(e) => {
                eprintln!("  [WARN] Subscribe to session_burst failed: {e}");
                None
            }
        }
    } else {
        None
    };

    let emit_sub = if emit_created {
        match db.subscribe::<OhlcBar>("ohlc_update") {
            Ok(sub) => Some(sub),
            Err(e) => {
                eprintln!("  [WARN] Subscribe to ohlc_update failed: {e}");
                None
            }
        }
    } else {
        None
    };

    Ok(Phase6Handles {
        db,
        source,
        hop_sub,
        session_sub,
        emit_sub,
        hop_created,
        session_created,
        emit_created,
    })
}

/// Run Phase 6 in CLI mode (println output).
pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Phase 6+: Bonus Window Types & Emit Modes ===");
    println!("Testing: HOP window, SESSION window, EMIT ON UPDATE");
    println!("All embedded, no external deps.");
    println!();

    let handles = setup().await?;
    println!("[OK] Pipeline setup complete");
    println!(
        "  HOP window:      {}",
        if handles.hop_created {
            "created"
        } else {
            "FAILED"
        }
    );
    println!(
        "  SESSION window:  {}",
        if handles.session_created {
            "created"
        } else {
            "FAILED"
        }
    );
    println!(
        "  EMIT ON UPDATE:  {}",
        if handles.emit_created {
            "created"
        } else {
            "FAILED"
        }
    );

    let mut gen = MarketGenerator::new();
    let mut total_pushed = 0u64;
    let mut hop_received = 0u64;
    let mut session_received = 0u64;
    let mut emit_received = 0u64;

    println!();
    println!("Pushing trades and polling all three subscribers...");
    println!("(Will run for 15 seconds)");
    println!();

    let start = std::time::Instant::now();

    while start.elapsed() < Duration::from_secs(15) {
        let ts = MarketGenerator::now_ms();
        let trades = gen.generate_trades(ts);
        let count = trades.len() as u64;
        handles.source.push_batch(trades);
        handles.source.watermark(ts + 10_000);
        total_pushed += count;

        // Poll HOP results
        if let Some(ref sub) = handles.hop_sub {
            while let Some(rows) = sub.poll() {
                for row in &rows {
                    println!(
                        "  HOP     | {:<6} | vol={:<8} trades={:<4} avg={:.2}",
                        row.symbol, row.total_volume, row.trade_count, row.avg_price
                    );
                    hop_received += 1;
                }
            }
        }

        // Poll SESSION results
        if let Some(ref sub) = handles.session_sub {
            while let Some(rows) = sub.poll() {
                for row in &rows {
                    println!(
                        "  SESSION | {:<6} | trades={:<4} vol={:<8} low={:.2} high={:.2}",
                        row.symbol, row.burst_trades, row.burst_volume, row.low, row.high
                    );
                    session_received += 1;
                }
            }
        }

        // Poll EMIT ON UPDATE results
        if let Some(ref sub) = handles.emit_sub {
            while let Some(rows) = sub.poll() {
                for row in &rows {
                    println!(
                        "  EMIT    | {:<6} | O={:.2} H={:.2} L={:.2} C={:.2}",
                        row.symbol, row.open, row.high, row.low, row.close
                    );
                    emit_received += 1;
                }
            }
        }

        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    println!();
    println!("=== Phase 6+ Results ===");
    println!("  Trades pushed:       {}", total_pushed);
    println!(
        "  HOP window:          {} (received {})",
        if hop_received > 0 {
            "PASS"
        } else if handles.hop_created {
            "FAIL (no output)"
        } else {
            "FAIL (not created)"
        },
        hop_received
    );
    println!(
        "  SESSION window:      {} (received {})",
        if session_received > 0 {
            "PASS"
        } else if handles.session_created {
            "FAIL (no output)"
        } else {
            "FAIL (not created)"
        },
        session_received
    );
    println!(
        "  EMIT ON UPDATE:      {} (received {})",
        if emit_received > 0 {
            "PASS"
        } else if handles.emit_created {
            "FAIL (no output)"
        } else {
            "FAIL (not created)"
        },
        emit_received
    );

    let _ = handles.db.shutdown().await;
    Ok(())
}
