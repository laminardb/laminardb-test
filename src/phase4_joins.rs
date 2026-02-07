//! Phase 4: Stream Joins test.
//!
//! Tests the "Stream Joins" tab from laminardb.io:
//! - ASOF JOIN: enrich trades with the most recent quote (backward + tolerance)
//! - Stream-stream INNER JOIN: match trades with orders within a time window
//!
//! Key syntax differences from the website:
//! - Website uses `ON t.ts >= q.ts TOLERANCE INTERVAL '5' SECOND`
//! - Actual uses `MATCH_CONDITION(t.ts >= q.ts AND t.ts - q.ts <= INTERVAL '5' SECOND)`
//!
//! Both join types are implemented as Ring 0 operators in laminar-core and
//! should work in the embedded pipeline.

use std::time::Duration;

use laminar_db::LaminarDB;

use crate::generator::MarketGenerator;
use crate::types::{AsofEnriched, Order, Quote, Trade, TradeOrderMatch};

/// Handles returned by Phase 4 setup.
pub struct Phase4Handles {
    pub db: LaminarDB,
    pub trade_source: laminar_db::SourceHandle<Trade>,
    pub quote_source: laminar_db::SourceHandle<Quote>,
    pub order_source: laminar_db::SourceHandle<Order>,
    pub asof_sub: Option<laminar_db::TypedSubscription<AsofEnriched>>,
    pub join_sub: Option<laminar_db::TypedSubscription<TradeOrderMatch>>,
    pub asof_supported: bool,
    pub join_supported: bool,
}

/// Set up the Phase 4 pipeline.
pub async fn setup() -> Result<Phase4Handles, Box<dyn std::error::Error>> {
    let db = LaminarDB::builder()
        .buffer_size(65536)
        .build()
        .await?;

    // Source 1: trades
    db.execute(
        "CREATE SOURCE trades (
            symbol  VARCHAR NOT NULL,
            price   DOUBLE NOT NULL,
            volume  BIGINT NOT NULL,
            ts      BIGINT NOT NULL
        )",
    )
    .await?;

    // Source 2: quotes (for ASOF JOIN)
    db.execute(
        "CREATE SOURCE quotes (
            symbol  VARCHAR NOT NULL,
            bid     DOUBLE NOT NULL,
            ask     DOUBLE NOT NULL,
            ts      BIGINT NOT NULL
        )",
    )
    .await?;

    // Source 3: orders (for stream-stream JOIN)
    db.execute(
        "CREATE SOURCE orders (
            order_id VARCHAR NOT NULL,
            symbol   VARCHAR NOT NULL,
            side     VARCHAR NOT NULL,
            quantity BIGINT NOT NULL,
            price    DOUBLE NOT NULL,
            ts       BIGINT NOT NULL
        )",
    )
    .await?;

    // ── Test 1: ASOF JOIN ──────────────────────────────────────────────
    // Enrich each trade with the most recent quote (backward, 5s tolerance).
    // Website syntax:  ON t.symbol = q.symbol AND t.ts >= q.ts TOLERANCE INTERVAL '5' SECOND
    // Actual syntax:   MATCH_CONDITION(t.ts >= q.ts ...) ON t.symbol = q.symbol
    //
    // NOTE: ASOF JOIN only works through the connector pipeline (custom operators).
    // The embedded pipeline uses DataFusion's ctx.sql() which does NOT support
    // ASOF JOIN syntax. Expected to create but produce 0 output.
    let asof_supported = match db
        .execute(
            "CREATE STREAM asof_enriched AS
             SELECT t.symbol,
                    t.price AS trade_price,
                    t.volume,
                    q.bid,
                    q.ask,
                    t.price - q.bid AS spread
             FROM trades t
             ASOF JOIN quotes q
             MATCH_CONDITION(t.ts >= q.ts AND t.ts - q.ts <= INTERVAL '5' SECOND)
             ON t.symbol = q.symbol",
        )
        .await
    {
        Ok(_) => true,
        Err(e) => {
            eprintln!("  [WARN] ASOF JOIN creation failed: {e}");
            false
        }
    };

    let asof_sub = if asof_supported {
        let _ = db.execute("CREATE SINK asof_output FROM asof_enriched").await;
        match db.subscribe::<AsofEnriched>("asof_enriched") {
            Ok(sub) => Some(sub),
            Err(e) => {
                eprintln!("  [WARN] ASOF subscribe failed: {e}");
                None
            }
        }
    } else {
        None
    };

    // ── Test 2: Stream-stream INNER JOIN ───────────────────────────────
    // Match trades with orders on the same symbol within a 60-second window.
    // Website uses INTERVAL syntax but ts is BIGINT (ms), so DataFusion can't
    // do INTERVAL arithmetic on it. Use numeric arithmetic instead:
    // 60000ms = 60 seconds.
    let join_supported = match db
        .execute(
            "CREATE STREAM trade_order_match AS
             SELECT t.symbol,
                    t.price AS trade_price,
                    t.volume,
                    o.order_id,
                    o.side,
                    o.price AS order_price,
                    t.price - o.price AS price_diff
             FROM trades t
             INNER JOIN orders o
             ON t.symbol = o.symbol
             AND o.ts BETWEEN t.ts - 60000 AND t.ts + 60000",
        )
        .await
    {
        Ok(_) => true,
        Err(e) => {
            eprintln!("  [WARN] Stream-stream JOIN creation failed: {e}");
            false
        }
    };

    let join_sub = if join_supported {
        let _ = db
            .execute("CREATE SINK join_output FROM trade_order_match")
            .await;
        match db.subscribe::<TradeOrderMatch>("trade_order_match") {
            Ok(sub) => Some(sub),
            Err(e) => {
                eprintln!("  [WARN] Stream-stream JOIN subscribe failed: {e}");
                None
            }
        }
    } else {
        None
    };

    db.start().await?;

    let trade_source = db.source::<Trade>("trades")?;
    let quote_source = db.source::<Quote>("quotes")?;
    let order_source = db.source::<Order>("orders")?;

    Ok(Phase4Handles {
        db,
        trade_source,
        quote_source,
        order_source,
        asof_sub,
        join_sub,
        asof_supported,
        join_supported,
    })
}

/// Run Phase 4 in CLI mode.
pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Phase 4: Stream Joins Test ===");
    println!("Testing: ASOF JOIN (backward + tolerance), stream-stream INNER JOIN");
    println!();

    let handles = setup().await?;
    println!("[OK] Pipeline setup complete");
    println!(
        "  ASOF JOIN:           {}",
        if handles.asof_supported {
            "created"
        } else {
            "FAILED"
        }
    );
    println!(
        "  Stream-stream JOIN:  {}",
        if handles.join_supported {
            "created"
        } else {
            "FAILED"
        }
    );

    let mut gen = MarketGenerator::new();
    let mut total_trades = 0u64;
    let mut total_quotes = 0u64;
    let mut total_orders = 0u64;
    let mut asof_received = 0u64;
    let mut join_received = 0u64;

    println!();
    println!("Pushing trades/quotes/orders and polling for join results...");
    println!("(Will run for 10 seconds)");
    println!();

    let start = std::time::Instant::now();

    while start.elapsed() < Duration::from_secs(10) {
        let ts = MarketGenerator::now_ms();

        // Push trades
        let trades = gen.generate_trades(ts);
        let trade_count = trades.len();
        handles.trade_source.push_batch(trades);
        handles.trade_source.watermark(ts + 5_000);
        total_trades += trade_count as u64;

        // Push quotes (slightly ahead so ASOF backward can match)
        let quotes = gen.generate_quotes(ts - 100);
        let quote_count = quotes.len();
        handles.quote_source.push_batch(quotes);
        handles.quote_source.watermark(ts + 5_000);
        total_quotes += quote_count as u64;

        // Push 1-2 orders per cycle
        let order = gen.generate_order(ts);
        handles.order_source.push_batch(vec![order]);
        handles.order_source.watermark(ts + 60_000);
        total_orders += 1;

        // Poll ASOF results
        if let Some(ref sub) = handles.asof_sub {
            while let Some(rows) = sub.poll() {
                for row in &rows {
                    println!(
                        "  ASOF  | {:<6} | trade={:>10.2} bid={:>10.2} ask={:>10.2} spread={:>8.4}",
                        row.symbol, row.trade_price, row.bid, row.ask, row.spread
                    );
                    asof_received += 1;
                }
            }
        }

        // Poll stream-stream JOIN results
        if let Some(ref sub) = handles.join_sub {
            while let Some(rows) = sub.poll() {
                for row in &rows {
                    println!(
                        "  JOIN  | {:<6} | trade={:>10.2} order={} side={:<4} order_px={:>10.2} diff={:>8.4}",
                        row.symbol, row.trade_price, row.order_id, row.side, row.order_price, row.price_diff
                    );
                    join_received += 1;
                }
            }
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    println!();
    println!("=== Phase 4 Results ===");
    println!("  Trades pushed:       {}", total_trades);
    println!("  Quotes pushed:       {}", total_quotes);
    println!("  Orders pushed:       {}", total_orders);
    println!("  ASOF results:        {}", asof_received);
    println!("  JOIN results:        {}", join_received);
    println!(
        "  ASOF status:         {}",
        if asof_received > 0 {
            "PASS"
        } else if handles.asof_supported {
            "FAIL (created but no output)"
        } else {
            "SKIP (creation failed)"
        }
    );
    println!(
        "  Stream-stream JOIN:  {}",
        if join_received > 0 {
            "PASS"
        } else if handles.join_supported {
            "FAIL (created but no output)"
        } else {
            "SKIP (creation failed)"
        }
    );

    Ok(())
}
