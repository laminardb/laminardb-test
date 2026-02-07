//! Market data generator for all phases.
//!
//! Generates realistic-ish trades, quotes, and orders across 5 symbols.

use rand::Rng;
use std::collections::HashMap;

use crate::types::{Order, Quote, Trade};

/// Symbols used across all phases.
pub const SYMBOLS: &[(&str, f64)] = &[
    ("AAPL", 150.0),
    ("GOOGL", 2800.0),
    ("MSFT", 420.0),
    ("AMZN", 185.0),
    ("TSLA", 250.0),
];

/// Tracks per-symbol price state for random walks.
pub struct MarketGenerator {
    prices: HashMap<String, f64>,
    order_seq: u64,
}

impl MarketGenerator {
    pub fn new() -> Self {
        let mut prices = HashMap::new();
        for (sym, base) in SYMBOLS {
            prices.insert(sym.to_string(), *base);
        }
        Self {
            prices,
            order_seq: 0,
        }
    }

    /// Generate a batch of trades (one per symbol).
    pub fn generate_trades(&mut self, ts: i64) -> Vec<Trade> {
        let mut rng = rand::thread_rng();
        let mut trades = Vec::with_capacity(SYMBOLS.len());

        for (sym, _) in SYMBOLS {
            let symbol = sym.to_string();
            let price = self.prices.get_mut(&symbol).unwrap();

            // Random walk: +/- 0.5%
            let change = *price * rng.gen_range(-0.005..0.005);
            *price += change;

            let volume = rng.gen_range(10..500);
            let _side = if rng.gen_bool(0.5) { "buy" } else { "sell" };

            trades.push(Trade {
                symbol,
                price: *price,
                volume,
                ts,
            });
        }

        trades
    }

    /// Generate a batch of quotes (one per symbol, bid/ask around current price).
    pub fn generate_quotes(&mut self, ts: i64) -> Vec<Quote> {
        let mut rng = rand::thread_rng();
        let mut quotes = Vec::with_capacity(SYMBOLS.len());

        for (sym, _) in SYMBOLS {
            let symbol = sym.to_string();
            let price = *self.prices.get(&symbol).unwrap();
            let half_spread = price * rng.gen_range(0.0001..0.001);

            quotes.push(Quote {
                symbol,
                bid: price - half_spread,
                ask: price + half_spread,
                ts,
            });
        }

        quotes
    }

    /// Generate a single random order.
    pub fn generate_order(&mut self, ts: i64) -> Order {
        let mut rng = rand::thread_rng();
        let idx = rng.gen_range(0..SYMBOLS.len());
        let (sym, _) = SYMBOLS[idx];
        let symbol = sym.to_string();
        let price = *self.prices.get(&symbol).unwrap();

        self.order_seq += 1;
        let side = if rng.gen_bool(0.5) { "buy" } else { "sell" };
        let offset = price * rng.gen_range(-0.002..0.002);

        Order {
            order_id: format!("ORD-{:06}", self.order_seq),
            symbol,
            side: side.to_string(),
            quantity: rng.gen_range(1..100),
            price: price + offset,
            ts,
        }
    }

    /// Current timestamp in milliseconds.
    pub fn now_ms() -> i64 {
        chrono::Utc::now().timestamp_millis()
    }
}
