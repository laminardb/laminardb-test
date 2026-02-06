//! LaminarDB Test App — tests each pipeline type from laminardb.io code examples.
//!
//! Usage:
//!   cargo run -- phase1    # Rust API basics
//!   cargo run -- phase2    # Streaming SQL + cascading MVs
//!   cargo run -- phase3    # Kafka pipeline (needs Redpanda)
//!   cargo run -- phase4    # Stream joins (ASOF + stream-stream)
//!   cargo run -- phase5    # CDC pipeline (needs Postgres)
//!   cargo run              # Run all phases sequentially

mod generator;
mod phase1_api;
mod types;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let phase = args.get(1).map(|s| s.as_str()).unwrap_or("phase1");

    match phase {
        "phase1" => phase1_api::run().await?,
        // "phase2" => phase2_sql::run().await?,
        // "phase3" => phase3_kafka::run().await?,
        // "phase4" => phase4_joins::run().await?,
        // "phase5" => phase5_cdc::run().await?,
        other => {
            eprintln!("Unknown phase: {}", other);
            eprintln!("Available: phase1, phase2, phase3, phase4, phase5");
            std::process::exit(1);
        }
    }

    Ok(())
}
