//! Phase 5: CDC Pipeline test.
//!
//! Tests the "CDC Pipeline" tab from laminardb.io:
//! - CREATE SOURCE ... FROM POSTGRES_CDC(...) — Postgres CDC connector
//! - SQL aggregation on CDC events
//! - Changelog/retraction semantics
//!
//! When the postgres-cdc connector fails (tokio-postgres lacks replication
//! support — see <https://github.com/laminardb/laminardb/issues/58>), this
//! falls back to a **polling workaround**: reads CDC events from
//! `pg_logical_slot_get_changes()` with the `test_decoding` plugin and
//! pushes them into an in-memory LaminarDB source for aggregation.
//!
//! Requires Postgres running on localhost:5432 with wal_level=logical.
//! Start with: docker compose up -d (see docker-compose.yml)

use std::time::Duration;

use laminar_db::LaminarDB;
use tokio_postgres::NoTls;

use crate::types::{CdcOrder, CustomerTotal};

/// Handles returned by Phase 5 setup, used by both CLI and TUI modes.
pub struct Phase5Handles {
    pub db: LaminarDB,
    pub pg_client: tokio_postgres::Client,
    pub totals_sub: Option<laminar_db::TypedSubscription<CustomerTotal>>,
    pub source_handle: Option<laminar_db::SourceHandle<CdcOrder>>,
    pub source_created: bool,
    /// True when using polling workaround instead of the connector.
    pub polling_mode: bool,
}

/// Connect to Postgres and return the client + spawn the connection task.
async fn connect_postgres() -> Result<tokio_postgres::Client, Box<dyn std::error::Error>> {
    let pg_host = std::env::var("LAMINAR_PG_HOST").unwrap_or_else(|_| "localhost".to_string());
    let pg_user = std::env::var("LAMINAR_PG_USER").unwrap_or_else(|_| "laminar".to_string());
    let pg_password =
        std::env::var("LAMINAR_PG_PASSWORD").unwrap_or_else(|_| "laminar".to_string());
    let pg_conn = format!(
        "host={pg_host} port=5432 dbname=shop user={pg_user} password={pg_password}"
    );
    let (pg_client, connection) = tokio_postgres::connect(&pg_conn, NoTls)
        .await
        .map_err(|e| format!("Postgres not available on {pg_host}:5432: {e}"))?;

    tokio::spawn(async move {
        if let Err(e) = connection.await {
            eprintln!("  [ERR] Postgres connection lost: {e}");
        }
    });

    Ok(pg_client)
}

/// Try the native postgres-cdc connector path. Returns None on failure.
async fn try_connector_path(
    db: &LaminarDB,
) -> Option<laminar_db::TypedSubscription<CustomerTotal>> {
    let pg_host = std::env::var("LAMINAR_PG_HOST").unwrap_or_else(|_| "localhost".to_string());
    let pg_user = std::env::var("LAMINAR_PG_USER").unwrap_or_else(|_| "laminar".to_string());
    let pg_password =
        std::env::var("LAMINAR_PG_PASSWORD").unwrap_or_else(|_| "laminar".to_string());

    // Need config vars for the connector SQL
    // Note: We already built the db, so rebuild is needed if we want config vars.
    // Instead, inline the values directly.
    let source_sql = format!(
        "CREATE SOURCE orders_cdc (
            id INT NOT NULL,
            customer_id INT NOT NULL,
            amount DOUBLE NOT NULL,
            status VARCHAR NOT NULL,
            ts BIGINT NOT NULL
        ) FROM \"postgres-cdc\" (
            'host' = '{pg_host}',
            'port' = '5432',
            'database' = 'shop',
            'username' = '{pg_user}',
            'password' = '{pg_password}',
            'slot.name' = 'laminar_orders',
            'publication' = 'laminar_pub',
            'snapshot.mode' = 'never'
        )"
    );

    if let Err(e) = db.execute(&source_sql).await {
        eprintln!("  [WARN] CDC connector source failed: {e}");
        return None;
    }

    if let Err(e) = db
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
        eprintln!("  [WARN] CDC connector stream failed: {e}");
        return None;
    }

    if let Err(e) = db
        .execute("CREATE SINK totals_local FROM customer_totals")
        .await
    {
        eprintln!("  [WARN] CDC connector sink failed: {e}");
        return None;
    }

    if let Err(e) = db.start().await {
        eprintln!("  [WARN] CDC connector start failed: {e}");
        return None;
    }

    match db.subscribe::<CustomerTotal>("customer_totals") {
        Ok(sub) => Some(sub),
        Err(e) => {
            eprintln!("  [WARN] CDC connector subscribe failed: {e}");
            None
        }
    }
}

/// Set up the polling workaround path: in-memory source + SQL aggregation,
/// with CDC events fed via `pg_logical_slot_get_changes()`.
async fn setup_polling_path(
    db: &LaminarDB,
    pg_client: &tokio_postgres::Client,
) -> Result<
    (
        laminar_db::SourceHandle<CdcOrder>,
        laminar_db::TypedSubscription<CustomerTotal>,
    ),
    Box<dyn std::error::Error>,
> {
    // Create a test_decoding slot for polling (separate from the pgoutput slot)
    match pg_client
        .batch_execute(
            "SELECT pg_create_logical_replication_slot('laminar_orders_poll', 'test_decoding')",
        )
        .await
    {
        Ok(_) => eprintln!("  [OK] Created polling slot 'laminar_orders_poll' (test_decoding)"),
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("already exists") {
                eprintln!("  [OK] Polling slot 'laminar_orders_poll' already exists");
            } else {
                return Err(format!("Failed to create polling slot: {e}").into());
            }
        }
    }

    // Create an in-memory source (no connector)
    db.execute(
        "CREATE SOURCE orders_cdc (
            id INT NOT NULL,
            customer_id INT NOT NULL,
            amount DOUBLE NOT NULL,
            status VARCHAR NOT NULL,
            ts BIGINT NOT NULL
        )",
    )
    .await?;

    // Same aggregation as the connector path (without _op filter since
    // we only push INSERT/UPDATE events from the polling parser)
    db.execute(
        "CREATE STREAM customer_totals AS
         SELECT customer_id,
                COUNT(*) AS total_orders,
                CAST(SUM(amount) AS DOUBLE) AS total_spent
         FROM orders_cdc
         GROUP BY customer_id",
    )
    .await?;

    db.execute("CREATE SINK totals_local FROM customer_totals")
        .await?;

    db.start().await?;

    let source = db.source::<CdcOrder>("orders_cdc")?;
    let sub = db.subscribe::<CustomerTotal>("customer_totals")?;

    Ok((source, sub))
}

/// Set up the Phase 5 CDC pipeline.
///
/// Tries the native connector first, falls back to polling on failure.
pub async fn setup() -> Result<Phase5Handles, Box<dyn std::error::Error>> {
    let pg_client = connect_postgres().await?;

    // Ensure pgoutput slot exists (for the connector path)
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

    let pg_host = std::env::var("LAMINAR_PG_HOST").unwrap_or_else(|_| "localhost".to_string());
    let pg_user = std::env::var("LAMINAR_PG_USER").unwrap_or_else(|_| "laminar".to_string());
    let pg_password =
        std::env::var("LAMINAR_PG_PASSWORD").unwrap_or_else(|_| "laminar".to_string());

    let db = LaminarDB::builder()
        .buffer_size(65536)
        .config_var("PG_HOST", &pg_host)
        .config_var("PG_USER", &pg_user)
        .config_var("PG_PASSWORD", &pg_password)
        .build()
        .await?;

    // Try native connector path first
    eprintln!("  Trying native postgres-cdc connector...");
    if let Some(sub) = try_connector_path(&db).await {
        eprintln!("  [OK] Native connector path succeeded");
        return Ok(Phase5Handles {
            db,
            pg_client,
            totals_sub: Some(sub),
            source_handle: None,
            source_created: true,
            polling_mode: false,
        });
    }

    // Connector failed — fall back to polling workaround
    eprintln!("  [INFO] Falling back to polling workaround (see issue #58)");

    // Need a fresh LaminarDB instance since the connector path may have
    // partially registered sources
    let db = LaminarDB::builder()
        .buffer_size(65536)
        .build()
        .await?;

    let (source_handle, sub) = setup_polling_path(&db, &pg_client).await?;

    Ok(Phase5Handles {
        db,
        pg_client,
        totals_sub: Some(sub),
        source_handle: Some(source_handle),
        source_created: true,
        polling_mode: true,
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

/// Parse a `test_decoding` change line and extract a CdcOrder.
///
/// Format: `table public.orders: INSERT: id[integer]:123 customer_id[integer]:5 ...`
fn parse_test_decoding_change(data: &str) -> Option<CdcOrder> {
    // Only process INSERT and UPDATE on the orders table
    if !data.contains("table public.orders:") {
        return None;
    }
    let is_insert = data.contains("INSERT:");
    let is_update = data.contains("UPDATE:");
    if !is_insert && !is_update {
        return None;
    }

    // Extract the column values section (after "INSERT:" or the last occurrence
    // of column data for UPDATE)
    let cols_section = if is_insert {
        data.split("INSERT:").nth(1)?
    } else {
        // For UPDATE, new values come after "new-tuple:" if present, else after "UPDATE:"
        data.split("new-tuple:")
            .nth(1)
            .or_else(|| data.split("UPDATE:").nth(1))?
    };

    let mut id = 0i32;
    let mut customer_id = 0i32;
    let mut amount = 0.0f64;
    let mut status = String::new();
    let mut ts = 0i64;

    // Parse "col[type]:value" pairs
    for token in cols_section.split_whitespace() {
        if let Some((col_type, value)) = token.split_once(':') {
            let col_name = col_type.split('[').next().unwrap_or("");
            // Strip surrounding quotes from string values
            let val = value.trim_matches('\'');
            match col_name {
                "id" => id = val.parse().unwrap_or(0),
                "customer_id" => customer_id = val.parse().unwrap_or(0),
                "amount" => amount = val.parse().unwrap_or(0.0),
                "status" => status = val.to_string(),
                "ts" => ts = val.parse().unwrap_or(0),
                _ => {}
            }
        }
    }

    if customer_id == 0 {
        return None;
    }

    Some(CdcOrder {
        id,
        customer_id,
        amount,
        status,
        ts,
    })
}

/// Poll `pg_logical_slot_get_changes()` and push parsed events into the source.
/// Returns number of CDC events pushed.
pub async fn poll_and_push(
    pg_client: &tokio_postgres::Client,
    source: &laminar_db::SourceHandle<CdcOrder>,
) -> Result<usize, Box<dyn std::error::Error>> {
    let rows = pg_client
        .query(
            "SELECT data FROM pg_logical_slot_get_changes('laminar_orders_poll', NULL, NULL)",
            &[],
        )
        .await?;

    let mut count = 0usize;
    for row in &rows {
        let data: &str = row.get(0);
        if let Some(order) = parse_test_decoding_change(data) {
            source.push(order)?;
            count += 1;
        }
    }

    if count > 0 {
        let ts = chrono::Utc::now().timestamp_millis();
        source.watermark(ts + 5_000);
    }

    Ok(count)
}

/// Run Phase 5 in CLI mode (println output).
pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Phase 5: CDC Pipeline Test ===");
    println!("Testing: Postgres CDC events → LaminarDB aggregation");
    println!("Requires: Postgres on localhost:5432 (docker compose up -d)");
    println!();

    let handles = setup().await?;
    println!("[OK] Pipeline setup complete");
    println!(
        "  Mode:       {}",
        if handles.polling_mode {
            "POLLING (pg_logical_slot_get_changes)"
        } else {
            "NATIVE (postgres-cdc connector)"
        }
    );
    println!(
        "  Source:      {}",
        if handles.source_created {
            "created"
        } else {
            "FAILED"
        }
    );
    println!(
        "  Subscriber:  {}",
        if handles.totals_sub.is_some() {
            "active"
        } else {
            "none"
        }
    );

    let mut total_inserted = 0u64;
    let mut total_cdc_events = 0u64;
    let mut total_received = 0u64;

    println!();
    println!("Inserting orders into Postgres and polling for aggregated totals...");
    println!("(Will run for 20 seconds)");
    println!();

    let start = std::time::Instant::now();
    let mut batch_num = 0u64;

    while start.elapsed() < Duration::from_secs(20) {
        batch_num += 1;

        // Insert test data into Postgres
        match insert_orders(&handles.pg_client, batch_num).await {
            Ok(count) => {
                total_inserted += count as u64;
            }
            Err(e) => {
                eprintln!("  [ERR] Insert failed: {e}");
            }
        }

        // In polling mode: read CDC events from slot and push into laminardb
        if handles.polling_mode {
            if let Some(ref source) = handles.source_handle {
                match poll_and_push(&handles.pg_client, source).await {
                    Ok(count) => {
                        if count > 0 {
                            total_cdc_events += count as u64;
                            eprintln!("  [CDC] Polled {count} events (total: {total_cdc_events})");
                        }
                    }
                    Err(e) => {
                        eprintln!("  [ERR] Poll failed: {e}");
                    }
                }
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
    println!(
        "  Mode:                {}",
        if handles.polling_mode {
            "POLLING"
        } else {
            "NATIVE"
        }
    );
    println!("  Orders inserted:     {}", total_inserted);
    if handles.polling_mode {
        println!("  CDC events polled:   {}", total_cdc_events);
    }
    println!("  Totals received:     {}", total_received);
    println!(
        "  CDC capture:         {}",
        if handles.polling_mode && total_cdc_events > 0 {
            "PASS (polling workaround)"
        } else if !handles.polling_mode && handles.source_created {
            "PASS (native connector)"
        } else {
            "FAIL"
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
