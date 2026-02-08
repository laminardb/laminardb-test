//! Typed structs for input events and output results.
//!
//! Input types use `#[derive(Record)]` for pushing into sources.
//! Output types use `#[derive(FromRow)]` for reading from subscriptions.

use laminar_derive::{FromRow, Record};

// -- Phase 1 & 2: Trade source + OHLC output --

/// A market trade event (pushed into the `trades` source).
#[derive(Debug, Clone, Record, FromRow)]
pub struct Trade {
    pub symbol: String,
    pub price: f64,
    pub volume: i64,
    #[event_time]
    pub ts: i64,
}

/// OHLC bar output from Phase 1 (simple, no TUMBLE_START).
#[derive(Debug, Clone, FromRow)]
pub struct OhlcBar {
    pub symbol: String,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
}

/// OHLC bar with bar_start from Phase 2 (TUMBLE_START + SUM volume).
#[derive(Debug, Clone, FromRow)]
pub struct OhlcBarFull {
    pub symbol: String,
    pub bar_start: i64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: i64,
}

// -- Phase 4: Join sources + outputs --

/// A quote event (pushed into the `quotes` source for ASOF join).
#[derive(Debug, Clone, Record)]
pub struct Quote {
    pub symbol: String,
    pub bid: f64,
    pub ask: f64,
    #[event_time]
    pub ts: i64,
}

/// An order event (pushed into the `orders` source for stream-stream join).
#[derive(Debug, Clone, Record)]
pub struct Order {
    pub order_id: String,
    pub symbol: String,
    pub side: String,
    pub quantity: i64,
    pub price: f64,
    #[event_time]
    pub ts: i64,
}

/// ASOF join output: trade enriched with latest quote.
#[derive(Debug, Clone, FromRow)]
pub struct AsofEnriched {
    pub symbol: String,
    pub trade_price: f64,
    pub volume: i64,
    pub bid: f64,
    pub ask: f64,
    pub spread: f64,
}

/// Stream-stream join output: trade matched with order.
#[derive(Debug, Clone, FromRow)]
pub struct TradeOrderMatch {
    pub symbol: String,
    pub trade_price: f64,
    pub volume: i64,
    pub order_id: String,
    pub side: String,
    pub order_price: f64,
    pub price_diff: f64,
}

// -- Phase 3: Kafka pipeline output --

/// Trade summary output from Kafka pipeline.
#[derive(Debug, Clone, FromRow)]
pub struct TradeSummary {
    pub symbol: String,
    pub trades: i64,
    pub notional: f64,
}

// -- Phase 5: CDC pipeline input + output --

/// An order captured from Postgres CDC (pushed into the in-memory source
/// when using the polling workaround via `pg_logical_slot_get_changes`).
#[derive(Debug, Clone, Record)]
pub struct CdcOrder {
    pub id: i32,
    pub customer_id: i32,
    pub amount: f64,
    pub status: String,
    #[event_time]
    pub ts: i64,
}

/// Customer aggregated totals from CDC pipeline.
#[derive(Debug, Clone, FromRow)]
pub struct CustomerTotal {
    pub customer_id: i32,
    pub total_orders: i64,
    pub total_spent: f64,
}

// -- Phase 7: Stress test (fraud-detect pipeline) --
// Prefixed with "Stress" to avoid conflicts with Phase 1-6 types.

/// A market trade event for the 6-stream fraud-detect pipeline.
#[derive(Debug, Clone, Record)]
pub struct StressTrade {
    pub account_id: String,
    pub symbol: String,
    pub side: String,
    pub price: f64,
    pub volume: i64,
    pub order_ref: String,
    #[event_time]
    pub ts: i64,
}

/// An order event for the 6-stream fraud-detect pipeline.
#[derive(Debug, Clone, Record)]
pub struct StressOrder {
    pub order_id: String,
    pub account_id: String,
    pub symbol: String,
    pub side: String,
    pub quantity: i64,
    pub price: f64,
    #[event_time]
    pub ts: i64,
}

/// HOP window: per-symbol volume baseline.
#[derive(Debug, Clone, FromRow)]
pub struct VolumeBaseline {
    pub symbol: String,
    pub total_volume: i64,
    pub trade_count: i64,
    pub avg_price: f64,
}

/// TUMBLE window: OHLC + volatility per symbol.
#[derive(Debug, Clone, FromRow)]
pub struct OhlcVolatility {
    pub symbol: String,
    pub bar_start: i64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: i64,
    pub price_range: f64,
}

/// SESSION window: rapid-fire burst detection per account.
#[derive(Debug, Clone, FromRow)]
pub struct RapidFireBurst {
    pub account_id: String,
    pub burst_trades: i64,
    pub burst_volume: i64,
    pub low: f64,
    pub high: f64,
}

/// TUMBLE + CASE WHEN: wash trading score per account+symbol.
#[derive(Debug, Clone, FromRow)]
pub struct WashScore {
    pub account_id: String,
    pub symbol: String,
    pub buy_volume: i64,
    pub sell_volume: i64,
    pub buy_count: i64,
    pub sell_count: i64,
}

/// INNER JOIN: suspicious trade-order matches within 2s window.
#[derive(Debug, Clone, FromRow)]
pub struct SuspiciousMatch {
    pub symbol: String,
    pub trade_price: f64,
    pub volume: i64,
    pub order_id: String,
    pub account_id: String,
    pub side: String,
    pub order_price: f64,
    pub price_diff: f64,
}

/// ASOF JOIN: front-running detection (trade matched to nearest prior order).
#[derive(Debug, Clone, FromRow)]
pub struct AsofMatch {
    pub symbol: String,
    pub trade_price: f64,
    pub volume: i64,
    pub trade_account: String,
    pub order_id: String,
    pub order_account: String,
    pub order_price: f64,
    pub price_spread: f64,
}

// -- Bonus: HOP / SESSION outputs --

/// HOP window volume output.
#[derive(Debug, Clone, FromRow)]
pub struct HopVolume {
    pub symbol: String,
    pub total_volume: i64,
    pub trade_count: i64,
    pub avg_price: f64,
}

/// SESSION window burst output.
#[derive(Debug, Clone, FromRow)]
pub struct SessionBurst {
    pub symbol: String,
    pub burst_trades: i64,
    pub burst_volume: i64,
    pub low: f64,
    pub high: f64,
}
