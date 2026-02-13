//! LaminarDB Test App — tests each pipeline type from laminardb.io code examples.
//!
//! Usage:
//!   cargo run              # TUI dashboard (live streaming)
//!   cargo run -- phase1    # Rust API basics (CLI mode)
//!   cargo run -- phase2    # Streaming SQL + cascading MVs
//!   cargo run -- phase3    # Kafka pipeline (needs Redpanda)
//!   cargo run -- phase4    # Stream joins (ASOF + stream-stream)
//!   cargo run -- phase5    # CDC pipeline (needs Postgres)
//!   cargo run -- phase6    # Bonus: HOP, SESSION, EMIT ON UPDATE

mod generator;
mod phase1_api;
mod phase2_sql;
mod phase3_kafka;
mod phase4_joins;
mod phase5_cdc;
mod phase6_bonus;
mod phase7_stress;
mod phase8_v012;
mod tui;
mod types;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let phase = args.get(1).map(|s| s.as_str());

    match phase {
        None | Some("tui") => tui::run().await?,
        Some("phase1") => phase1_api::run().await?,
        Some("phase2") => phase2_sql::run().await?,
        Some("phase3") => phase3_kafka::run().await?,
        Some("phase4") => phase4_joins::run().await?,
        Some("phase5") => phase5_cdc::run().await?,
        Some("phase6") | Some("bonus") => phase6_bonus::run().await?,
        Some("phase7") | Some("stress") => phase7_stress::run().await?,
        Some("phase8") | Some("v012") => phase8_v012::run().await?,
        Some(other) => {
            eprintln!("Unknown phase: {}", other);
            eprintln!("Available: (no args for TUI), phase1..phase8, stress, bonus, v012");
            std::process::exit(1);
        }
    }

    Ok(())
}
