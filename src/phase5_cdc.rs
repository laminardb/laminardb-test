//! Phase 5: CDC Pipeline test.
//!
//! Tests the "CDC Pipeline" tab from laminardb.io:
//! - CREATE SOURCE ... FROM POSTGRES_CDC(...) — Postgres CDC connector
//! - SQL aggregation on CDC events (with _op filter if supported)
//! - Changelog/retraction semantics
//!
//! Requires Postgres running on localhost:5432 with wal_level=logical.
//! Start with: docker compose up -d (see docker-compose.yml)

use std::time::Duration;

use laminar_db::LaminarDB;
use tokio_postgres::NoTls;

use crate::types::CustomerTotal;

/// Handles returned by Phase 5 setup, used by both CLI and TUI modes.
pub struct Phase5Handles {
    pub db: LaminarDB,
    pub pg_client: tokio_postgres::Client,
    pub totals_sub: Option<laminar_db::TypedSubscription<CustomerTotal>>,
    pub source_created: bool,
}

/// Set up the Phase 5 CDC pipeline.
///
/// 1. Connects to Postgres for inserting test data.
/// 2. Configures LaminarDB with FROM POSTGRES_CDC source + SQL aggregation.
/// 3. Returns handles for inserting data and polling results.
pub async fn setup() -> Result<Phase5Handles, Box<dyn std::error::Error>> {
    // Test Postgres connectivity (credentials via env vars, defaults for local dev)
    let pg_host = std::env::var("LAMINAR_PG_HOST").unwrap_or_else(|_| "localhost".to_string());
    let pg_user = std::env::var("LAMINAR_PG_USER").unwrap_or_else(|_| "laminar".to_string());
    let pg_password = std::env::var("LAMINAR_PG_PASSWORD").unwrap_or_else(|_| "laminar".to_string());
    let pg_conn = format!(
        "host={pg_host} port=5432 dbname=shop user={pg_user} password={pg_password}"
    );
    let (pg_client, connection) = tokio_postgres::connect(&pg_conn, NoTls)
        .await
        .map_err(|e| format!("Postgres not available on {pg_host}:5432: {e}"))?;

    // Spawn connection handler in background
    tokio::spawn(async move {
        if let Err(e) = connection.await {
            eprintln!("  [ERR] Postgres connection lost: {e}");
        }
    });

    // Ensure replication slot exists (connector expects it to be pre-created)
    match pg_client
        .batch_execute(
            "SELECT pg_create_logical_replication_slot('laminar_orders', 'pgoutput')",
        )
        .await
    {
        Ok(_) => eprintln!("  [OK] Created replication slot 'laminar_orders'"),
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("already exists") {
                eprintln!("  [OK] Replication slot 'laminar_orders' already exists");
            } else {
                eprintln!("  [WARN] Replication slot creation: {e}");
            }
        }
    }

    let db = LaminarDB::builder()
        .buffer_size(65536)
        .config_var("PG_HOST", &pg_host)
        .config_var("PG_USER", &pg_user)
        .config_var("PG_PASSWORD", &pg_password)
        .build()
        .await?;

    // CDC source from Postgres — replicates the `orders` table via logical replication
    let source_created = match db
        .execute(
            "CREATE SOURCE orders_cdc (
                id INT NOT NULL,
                customer_id INT NOT NULL,
                amount DOUBLE NOT NULL,
                status VARCHAR NOT NULL,
                ts BIGINT NOT NULL
            ) FROM \"postgres-cdc\" (
                'host' = '${PG_HOST}',
                'port' = '5432',
                'database' = 'shop',
                'username' = '${PG_USER}',
                'password' = '${PG_PASSWORD}',
                'slot.name' = 'laminar_orders',
                'publication' = 'laminar_pub',
                'snapshot.mode' = 'never'
            )",
        )
        .await
    {
        Ok(_) => true,
        Err(e) => {
            eprintln!("  [WARN] CDC source creation failed: {e}");
            false
        }
    };

    // Aggregation: per-customer order count + total spent
    // Website example uses WHERE _op IN ('I', 'U') — try that first
    let stream_created = if source_created {
        match db
            .execute(
                "CREATE STREAM customer_totals AS
                 SELECT customer_id,
                        COUNT(*) AS total_orders,
                        CAST(SUM(amount) AS DOUBLE) AS total_spent
                 FROM orders_cdc
                 WHERE _op IN ('I', 'U')
                 GROUP BY customer_id",
            )
            .await
        {
            Ok(_) => true,
            Err(e) => {
                eprintln!("  [WARN] Query with _op filter failed: {e}");
                // Fallback: without _op filter
                match db
                    .execute(
                        "CREATE STREAM customer_totals AS
                         SELECT customer_id,
                                COUNT(*) AS total_orders,
                                CAST(SUM(amount) AS DOUBLE) AS total_spent
                         FROM orders_cdc
                         GROUP BY customer_id",
                    )
                    .await
                {
                    Ok(_) => true,
                    Err(e2) => {
                        eprintln!("  [WARN] Fallback query also failed: {e2}");
                        false
                    }
                }
            }
        }
    } else {
        false
    };

    if stream_created {
        db.execute("CREATE SINK totals_local FROM customer_totals")
            .await?;
    }

    db.start().await?;

    let totals_sub = if stream_created {
        match db.subscribe::<CustomerTotal>("customer_totals") {
            Ok(sub) => Some(sub),
            Err(e) => {
                eprintln!("  [WARN] Subscribe to customer_totals failed: {e}");
                None
            }
        }
    } else {
        None
    };

    Ok(Phase5Handles {
        db,
        pg_client,
        totals_sub,
        source_created,
    })
}

/// Insert a batch of test orders into Postgres (triggers CDC events).
/// Returns the number of rows affected.
pub async fn insert_orders(
    client: &tokio_postgres::Client,
    batch_num: u64,
) -> Result<usize, Box<dyn std::error::Error>> {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let ts = chrono::Utc::now().timestamp_millis();

    let mut count = 0usize;
    let batch_size = rng.gen_range(3..=5);

    for _ in 0..batch_size {
        let customer_id = rng.gen_range(1..=20i32);
        let amount: f64 = rng.gen_range(10.0..500.0);
        let status = if rng.gen_bool(0.8) {
            "pending"
        } else {
            "completed"
        };

        // Use simple_query with formatted SQL to avoid ToSql serialization issues
        let sql = format!(
            "INSERT INTO orders (customer_id, amount, status, ts) VALUES ({}, {:.2}, '{}', {})",
            customer_id, amount, status, ts
        );
        client.batch_execute(&sql).await?;
        count += 1;
    }

    // Occasionally update a random pending order to test CDC UPDATE events
    if batch_num > 2 && rng.gen_bool(0.3) {
        let sql = format!(
            "UPDATE orders SET status = 'completed', ts = {} \
             WHERE id IN (SELECT id FROM orders WHERE status = 'pending' LIMIT 1)",
            ts
        );
        let _ = client.batch_execute(&sql).await;
        count += 1;
    }

    Ok(count)
}

/// Run Phase 5 in CLI mode (println output).
pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Phase 5: CDC Pipeline Test ===");
    println!("Testing: FROM POSTGRES_CDC source, CDC events, aggregation on changelog");
    println!("Requires: Postgres on localhost:5432 (docker compose up -d)");
    println!();

    let handles = setup().await?;
    println!("[OK] Pipeline setup complete");
    println!(
        "  CDC source: {}",
        if handles.source_created {
            "created"
        } else {
            "FAILED"
        }
    );
    println!(
        "  Subscriber: {}",
        if handles.totals_sub.is_some() {
            "active"
        } else {
            "none"
        }
    );

    let mut total_inserted = 0u64;
    let mut total_received = 0u64;

    println!();
    println!("Inserting orders into Postgres and polling for aggregated totals...");
    println!("(Will run for 20 seconds)");
    println!();

    let start = std::time::Instant::now();
    let mut batch_num = 0u64;

    while start.elapsed() < Duration::from_secs(20) {
        batch_num += 1;

        match insert_orders(&handles.pg_client, batch_num).await {
            Ok(count) => {
                total_inserted += count as u64;
            }
            Err(e) => {
                eprintln!("  [ERR] Insert failed: {e}");
            }
        }

        // Poll subscriber for aggregated results
        if let Some(ref sub) = handles.totals_sub {
            while let Some(rows) = sub.poll() {
                for row in &rows {
                    println!(
                        "  TOTAL | customer_id={:<4} | orders={:<4} spent={:>10.2}",
                        row.customer_id, row.total_orders, row.total_spent
                    );
                    total_received += 1;
                }
            }
        }

        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    println!();
    println!("=== Phase 5 Results ===");
    println!("  Orders inserted:     {}", total_inserted);
    println!("  Totals received:     {}", total_received);
    println!(
        "  CDC source:          {}",
        if handles.source_created {
            "PASS"
        } else {
            "FAIL (FROM POSTGRES_CDC not supported)"
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

    let _ = handles.db.shutdown().await;
    Ok(())
}
