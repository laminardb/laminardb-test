//! Phase 3: Kafka Pipeline test.
//!
//! Tests the "Kafka Pipeline" tab from laminardb.io:
//! - CREATE SOURCE ... FROM KAFKA(...) — Kafka source connector
//! - CREATE STREAM ... — SQL aggregation on Kafka stream
//! - CREATE SINK ... INTO KAFKA(...) — Kafka sink connector
//! - ${VAR} config variable substitution via builder.config_var()
//!
//! Requires Redpanda running on localhost:19092.
//! Topics: `market-trades` (input), `trade-summaries` (output).

use std::time::Duration;

use rdkafka::config::ClientConfig;
use rdkafka::producer::{FutureProducer, FutureRecord};

use laminar_db::LaminarDB;

use crate::generator::MarketGenerator;
use crate::types::TradeSummary;

/// Handles returned by Phase 3 setup, used by both CLI and TUI modes.
pub struct Phase3Handles {
    pub db: LaminarDB,
    pub producer: FutureProducer,
    pub summary_sub: Option<laminar_db::TypedSubscription<TradeSummary>>,
    pub source_created: bool,
    pub sink_created: bool,
}

/// Set up the Phase 3 Kafka pipeline.
///
/// 1. Creates an rdkafka producer for pushing test trades.
/// 2. Configures LaminarDB with FROM KAFKA source, SQL aggregation, INTO KAFKA sink.
/// 3. Returns handles for producing data and polling results.
pub async fn setup() -> Result<Phase3Handles, Box<dyn std::error::Error>> {
    // Test Kafka connectivity first
    tokio::net::TcpStream::connect("localhost:19092")
        .await
        .map_err(|_| "Redpanda not running on localhost:19092")?;

    // Create rdkafka producer for pushing test data
    let producer: FutureProducer = ClientConfig::new()
        .set("bootstrap.servers", "localhost:19092")
        .set("message.timeout.ms", "5000")
        .create()?;

    let db = LaminarDB::builder()
        .config_var("KAFKA_BROKERS", "localhost:19092")
        .buffer_size(65536)
        .build()
        .await?;

    // Source from Kafka — uses ${KAFKA_BROKERS} variable substitution
    let source_created = db
        .execute(
            "CREATE SOURCE trades (
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
            )",
        )
        .await
        .is_ok();

    // SQL aggregation: count trades and compute notional per symbol per 5s window
    // NOTE: CAST(volume AS DOUBLE) needed because volume is BIGINT and price is DOUBLE
    db.execute(
        "CREATE STREAM trade_summary AS
         SELECT symbol,
                COUNT(*) AS trades,
                SUM(price * CAST(volume AS DOUBLE)) AS notional
         FROM trades
         GROUP BY symbol, TUMBLE(ts, INTERVAL '5' SECOND)",
    )
    .await?;

    // Local sink for subscriber (poll results in TUI/CLI)
    db.execute("CREATE SINK summary_local FROM trade_summary")
        .await?;

    // Kafka sink — writes summaries to trade-summaries topic
    let sink_created = match db
        .execute(
            "CREATE SINK summary_kafka FROM trade_summary
             INTO KAFKA (
                 brokers = '${KAFKA_BROKERS}',
                 topic = 'trade-summaries',
                 format = 'json'
             )",
        )
        .await
    {
        Ok(_) => true,
        Err(e) => {
            eprintln!("  [WARN] Kafka sink creation failed: {e}");
            false
        }
    };

    db.start().await?;

    // Subscribe locally for TUI/CLI polling
    let summary_sub = match db.subscribe::<TradeSummary>("trade_summary") {
        Ok(sub) => Some(sub),
        Err(e) => {
            eprintln!("  [WARN] Subscribe to trade_summary failed: {e}");
            None
        }
    };

    Ok(Phase3Handles {
        db,
        producer,
        summary_sub,
        source_created,
        sink_created,
    })
}

/// Produce a batch of trades as JSON to the market-trades Kafka topic.
/// Returns the number of trades produced.
pub async fn produce_trades(
    producer: &FutureProducer,
    gen: &mut MarketGenerator,
    ts: i64,
) -> usize {
    let trades = gen.generate_trades(ts);
    let count = trades.len();

    for trade in &trades {
        let payload = serde_json::json!({
            "symbol": trade.symbol,
            "price": trade.price,
            "volume": trade.volume,
            "ts": trade.ts,
        });
        let bytes = payload.to_string();

        // Send to Kafka — don't block on delivery confirmation
        let record = FutureRecord::to("market-trades")
            .key(trade.symbol.as_str())
            .payload(bytes.as_bytes());
        let _ = producer.send(record, Duration::from_millis(100)).await;
    }

    count
}

/// Run Phase 3 in CLI mode (println output).
pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Phase 3: Kafka Pipeline Test ===");
    println!("Testing: FROM KAFKA source, INTO KAFKA sink, ${{VAR}} substitution");
    println!("Requires: Redpanda on localhost:19092");
    println!();

    let handles = setup().await?;
    println!("[OK] Pipeline setup complete");
    println!(
        "  Kafka source: {}",
        if handles.source_created {
            "created"
        } else {
            "FAILED"
        }
    );
    println!(
        "  Kafka sink:   {}",
        if handles.sink_created {
            "created"
        } else {
            "FAILED"
        }
    );

    let mut gen = MarketGenerator::new();
    let mut total_produced = 0u64;
    let mut total_received = 0u64;

    println!();
    println!("Producing trades to Kafka and polling for summaries...");
    println!("(Will run for 15 seconds)");
    println!();

    let start = std::time::Instant::now();

    while start.elapsed() < Duration::from_secs(15) {
        let ts = MarketGenerator::now_ms();

        let count = produce_trades(&handles.producer, &mut gen, ts).await;
        total_produced += count as u64;

        // Poll local subscriber for aggregated summaries
        if let Some(ref sub) = handles.summary_sub {
            while let Some(rows) = sub.poll() {
                for row in &rows {
                    println!(
                        "  SUMMARY | {:<6} | trades={:<4} notional={:>12.2}",
                        row.symbol, row.trades, row.notional
                    );
                    total_received += 1;
                }
            }
        }

        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    println!();
    println!("=== Phase 3 Results ===");
    println!("  Trades produced:     {}", total_produced);
    println!("  Summaries received:  {}", total_received);
    println!(
        "  Kafka source:        {}",
        if handles.source_created {
            "PASS"
        } else {
            "FAIL (FROM KAFKA not supported)"
        }
    );
    println!(
        "  Pipeline status:     {}",
        if total_received > 0 {
            "PASS"
        } else {
            "FAIL (no output received)"
        }
    );
    println!(
        "  Kafka sink:          {}",
        if handles.sink_created {
            "created (check trade-summaries topic)"
        } else {
            "FAIL (INTO KAFKA not supported)"
        }
    );

    let _ = handles.db.shutdown().await;
    Ok(())
}
