//! Ratatui TUI dashboard for live LaminarDB test phases.
//!
//! Shows a tabbed view with live streaming data, throughput sparklines,
//! latency stats, and per-symbol price bar charts.
//! Press Tab to switch phases, q/Esc to quit.

use std::collections::{HashMap, VecDeque};
use std::io;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Row, Table, Tabs};
use ratatui::Terminal;

use crate::generator::MarketGenerator;
use crate::types::{AsofEnriched, CustomerTotal, HopVolume, OhlcBar, OhlcBarFull, SessionBurst, TradeSummary, TradeOrderMatch};

const MAX_RECENT_BARS: usize = 50;
const SPARKLINE_HISTORY: usize = 60; // ~60 samples at 5 FPS = 12 seconds

/// Per-phase state tracked by the TUI.
pub struct PhaseState {
    pub name: &'static str,
    pub description: &'static str,
    pub status: PhaseStatus,
    pub trades_pushed: u64,
    pub bars_received: u64,
    pub recent_bars: VecDeque<OhlcBar>,
    // Throughput tracking
    pub trades_per_sec: f64,
    pub bars_per_sec: f64,
    trades_this_sec: u64,
    bars_this_sec: u64,
    last_throughput_tick: Instant,
    pub trades_sparkline: VecDeque<u64>,
    pub bars_sparkline: VecDeque<u64>,
    // Latency tracking
    pub last_push_time: Option<Instant>,
    pub latency_us: VecDeque<u64>, // microseconds
    pub avg_latency_us: u64,
    pub min_latency_us: u64,
    pub max_latency_us: u64,
    // Per-symbol latest close price
    pub symbol_prices: HashMap<String, f64>,
    // Pipeline flow counters (for live visualization)
    pub pipeline_counters: HashMap<String, u64>,
}

#[derive(Clone, Copy, PartialEq)]
pub enum PhaseStatus {
    Setting,
    Running,
    Pass,
    Fail(&'static str),
}

impl PhaseStatus {
    fn label(self) -> &'static str {
        match self {
            Self::Setting => "SETUP",
            Self::Running => "RUNNING",
            Self::Pass => "PASS",
            Self::Fail(_) => "FAIL",
        }
    }

    fn color(self) -> Color {
        match self {
            Self::Setting => Color::Yellow,
            Self::Running => Color::Cyan,
            Self::Pass => Color::Green,
            Self::Fail(_) => Color::Red,
        }
    }
}

/// App-level TUI state.
pub struct App {
    pub phases: Vec<PhaseState>,
    pub selected_tab: usize,
    pub should_quit: bool,
    pub cycle: u64,
    pub uptime: Instant,
}

impl App {
    pub fn new() -> Self {
        Self {
            phases: Vec::new(),
            selected_tab: 0,
            should_quit: false,
            cycle: 0,
            uptime: Instant::now(),
        }
    }

    pub fn add_phase(&mut self, name: &'static str, description: &'static str) -> usize {
        let idx = self.phases.len();
        self.phases.push(PhaseState {
            name,
            description,
            status: PhaseStatus::Setting,
            trades_pushed: 0,
            bars_received: 0,
            recent_bars: VecDeque::with_capacity(MAX_RECENT_BARS),
            trades_per_sec: 0.0,
            bars_per_sec: 0.0,
            trades_this_sec: 0,
            bars_this_sec: 0,
            last_throughput_tick: Instant::now(),
            trades_sparkline: VecDeque::with_capacity(SPARKLINE_HISTORY),
            bars_sparkline: VecDeque::with_capacity(SPARKLINE_HISTORY),
            last_push_time: None,
            latency_us: VecDeque::with_capacity(SPARKLINE_HISTORY),
            avg_latency_us: 0,
            min_latency_us: 0,
            max_latency_us: 0,
            symbol_prices: HashMap::new(),
            pipeline_counters: HashMap::new(),
        });
        idx
    }

    /// Record that trades were pushed at this instant.
    fn record_push(&mut self, phase_idx: usize, count: u64) {
        let phase = &mut self.phases[phase_idx];
        phase.trades_pushed += count;
        phase.trades_this_sec += count;
        phase.last_push_time = Some(Instant::now());
    }

    /// Record polled bars and measure latency from last push.
    fn record_bars(&mut self, phase_idx: usize, bars: Vec<OhlcBar>) {
        let phase = &mut self.phases[phase_idx];
        let poll_time = Instant::now();

        // Measure latency from last push
        if let Some(push_time) = phase.last_push_time {
            let lat = poll_time.duration_since(push_time).as_micros() as u64;
            phase.latency_us.push_back(lat);
            if phase.latency_us.len() > SPARKLINE_HISTORY {
                phase.latency_us.pop_front();
            }
            // Update stats
            if !phase.latency_us.is_empty() {
                let sum: u64 = phase.latency_us.iter().sum();
                phase.avg_latency_us = sum / phase.latency_us.len() as u64;
                phase.min_latency_us = *phase.latency_us.iter().min().unwrap_or(&0);
                phase.max_latency_us = *phase.latency_us.iter().max().unwrap_or(&0);
            }
        }

        let bar_count = bars.len() as u64;
        phase.bars_received += bar_count;
        phase.bars_this_sec += bar_count;

        for bar in bars {
            phase
                .symbol_prices
                .insert(bar.symbol.clone(), bar.close);
            phase.recent_bars.push_front(bar);
            if phase.recent_bars.len() > MAX_RECENT_BARS {
                phase.recent_bars.pop_back();
            }
        }
    }

    /// Record polled OhlcBarFull (Phase 2) — converts to OhlcBar for display.
    fn record_bars_full(&mut self, phase_idx: usize, bars: Vec<OhlcBarFull>) {
        let converted: Vec<OhlcBar> = bars
            .into_iter()
            .map(|b| OhlcBar {
                symbol: b.symbol,
                open: b.open,
                high: b.high,
                low: b.low,
                close: b.close,
            })
            .collect();
        self.record_bars(phase_idx, converted);
    }

    /// Record ASOF JOIN results (Phase 4) — maps to OhlcBar for display.
    fn record_asof(&mut self, phase_idx: usize, rows: Vec<AsofEnriched>) {
        let converted: Vec<OhlcBar> = rows
            .into_iter()
            .map(|r| OhlcBar {
                symbol: r.symbol,
                open: r.trade_price,
                high: r.bid,
                low: r.ask,
                close: r.spread,
            })
            .collect();
        self.record_bars(phase_idx, converted);
    }

    /// Record Kafka trade summaries (Phase 3) — maps to OhlcBar for display.
    fn record_summary(&mut self, phase_idx: usize, rows: Vec<TradeSummary>) {
        let converted: Vec<OhlcBar> = rows
            .into_iter()
            .map(|r| OhlcBar {
                symbol: r.symbol,
                open: r.trades as f64,
                high: r.notional,
                low: 0.0,
                close: r.notional,
            })
            .collect();
        self.record_bars(phase_idx, converted);
    }

    /// Record HOP volume results (Phase 6) — maps to OhlcBar for display.
    fn record_hop(&mut self, phase_idx: usize, rows: Vec<HopVolume>) {
        let converted: Vec<OhlcBar> = rows
            .into_iter()
            .map(|r| OhlcBar {
                symbol: r.symbol,
                open: r.trade_count as f64,
                high: r.avg_price,
                low: r.total_volume as f64,
                close: r.avg_price,
            })
            .collect();
        self.record_bars(phase_idx, converted);
    }

    /// Record SESSION burst results (Phase 6) — maps to OhlcBar for display.
    fn record_session(&mut self, phase_idx: usize, rows: Vec<SessionBurst>) {
        let converted: Vec<OhlcBar> = rows
            .into_iter()
            .map(|r| OhlcBar {
                symbol: r.symbol,
                open: r.burst_trades as f64,
                high: r.high,
                low: r.low,
                close: r.burst_volume as f64,
            })
            .collect();
        self.record_bars(phase_idx, converted);
    }

    /// Record CDC customer totals (Phase 5) — maps to OhlcBar for display.
    fn record_cdc_totals(&mut self, phase_idx: usize, rows: Vec<CustomerTotal>) {
        let converted: Vec<OhlcBar> = rows
            .into_iter()
            .map(|r| OhlcBar {
                symbol: format!("C{}", r.customer_id),
                open: r.total_orders as f64,
                high: r.total_spent,
                low: 0.0,
                close: r.total_spent,
            })
            .collect();
        self.record_bars(phase_idx, converted);
    }

    /// Record stream-stream JOIN results (Phase 4) — maps to OhlcBar for display.
    fn record_join_match(&mut self, phase_idx: usize, rows: Vec<TradeOrderMatch>) {
        let converted: Vec<OhlcBar> = rows
            .into_iter()
            .map(|r| OhlcBar {
                symbol: r.symbol,
                open: r.trade_price,
                high: r.order_price,
                low: r.price_diff,
                close: r.volume as f64,
            })
            .collect();
        self.record_bars(phase_idx, converted);
    }

    /// Update throughput counters every second.
    fn tick_throughput(&mut self, phase_idx: usize) {
        let phase = &mut self.phases[phase_idx];
        let elapsed = phase.last_throughput_tick.elapsed();
        if elapsed >= Duration::from_secs(1) {
            let secs = elapsed.as_secs_f64();
            phase.trades_per_sec = phase.trades_this_sec as f64 / secs;
            phase.bars_per_sec = phase.bars_this_sec as f64 / secs;

            phase.trades_sparkline.push_back(phase.trades_this_sec);
            if phase.trades_sparkline.len() > SPARKLINE_HISTORY {
                phase.trades_sparkline.pop_front();
            }
            phase.bars_sparkline.push_back(phase.bars_this_sec);
            if phase.bars_sparkline.len() > SPARKLINE_HISTORY {
                phase.bars_sparkline.pop_front();
            }

            phase.trades_this_sec = 0;
            phase.bars_this_sec = 0;
            phase.last_throughput_tick = Instant::now();
        }
    }

    /// Record a pipeline flow counter for live visualization.
    fn record_pipeline_count(&mut self, phase_idx: usize, key: &str, count: u64) {
        *self.phases[phase_idx]
            .pipeline_counters
            .entry(key.to_string())
            .or_insert(0) += count;
    }
}

/// Run the TUI with all implemented phases.
pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut app = App::new();
    let p1_idx = app.add_phase(
        "Phase 1: Rust API",
        "TUMBLE window, first_value/last_value, push_batch/poll",
    );
    let p2_idx = app.add_phase(
        "Phase 2: Streaming SQL",
        "tumble() as TUMBLE_START, SUM(volume), cascading MVs",
    );
    let p3_idx = app.add_phase(
        "Phase 3: Kafka Pipeline",
        "FROM KAFKA source, INTO KAFKA sink, ${VAR} substitution",
    );
    let p4_idx = app.add_phase(
        "Phase 4: Stream Joins",
        "ASOF JOIN (backward+tolerance), stream-stream INNER JOIN",
    );
    let p5_idx = app.add_phase(
        "Phase 5: CDC Pipeline",
        "FROM POSTGRES_CDC source, CDC events, customer aggregation",
    );
    let p6_idx = app.add_phase(
        "Phase 6+: Bonus",
        "HOP window, SESSION window, EMIT ON UPDATE",
    );

    // Setup Phase 1
    let p1_handles = match crate::phase1_api::setup().await {
        Ok(h) => {
            app.phases[p1_idx].status = PhaseStatus::Running;
            Some(h)
        }
        Err(e) => {
            app.phases[p1_idx].status = PhaseStatus::Fail("setup failed");
            eprintln!("Phase 1 setup error: {e}");
            None
        }
    };

    // Setup Phase 2
    let p2_handles = match crate::phase2_sql::setup().await {
        Ok(h) => {
            app.phases[p2_idx].status = PhaseStatus::Running;
            Some(h)
        }
        Err(e) => {
            app.phases[p2_idx].status = PhaseStatus::Fail("setup failed");
            eprintln!("Phase 2 setup error: {e}");
            None
        }
    };

    // Setup Phase 3 (Kafka — may fail if Redpanda is down)
    let p3_handles = match crate::phase3_kafka::setup().await {
        Ok(h) => {
            app.phases[p3_idx].status = PhaseStatus::Running;
            Some(h)
        }
        Err(e) => {
            app.phases[p3_idx].status = PhaseStatus::Fail("setup failed");
            eprintln!("Phase 3 setup error: {e}");
            None
        }
    };

    // Setup Phase 4
    let p4_handles = match crate::phase4_joins::setup().await {
        Ok(h) => {
            app.phases[p4_idx].status = PhaseStatus::Running;
            Some(h)
        }
        Err(e) => {
            app.phases[p4_idx].status = PhaseStatus::Fail("setup failed");
            eprintln!("Phase 4 setup error: {e}");
            None
        }
    };

    // Setup Phase 5 (CDC — may fail if Postgres is down)
    let p5_handles = match crate::phase5_cdc::setup().await {
        Ok(h) => {
            app.phases[p5_idx].status = PhaseStatus::Running;
            Some(h)
        }
        Err(e) => {
            app.phases[p5_idx].status = PhaseStatus::Fail("setup failed");
            eprintln!("Phase 5 setup error: {e}");
            None
        }
    };

    // Setup Phase 6 (Bonus — embedded, no external deps)
    let p6_handles = match crate::phase6_bonus::setup().await {
        Ok(h) => {
            app.phases[p6_idx].status = PhaseStatus::Running;
            Some(h)
        }
        Err(e) => {
            app.phases[p6_idx].status = PhaseStatus::Fail("setup failed");
            eprintln!("Phase 6 setup error: {e}");
            None
        }
    };

    let mut gen1 = MarketGenerator::new();
    let mut gen2 = MarketGenerator::new();
    let mut gen3 = MarketGenerator::new();
    let mut gen4 = MarketGenerator::new();
    let mut gen6 = MarketGenerator::new();

    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Panic hook to restore terminal
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        original_hook(info);
    }));

    // Event loop (~5 FPS)
    loop {
        terminal.draw(|f| draw(f, &app))?;

        if event::poll(Duration::from_millis(200))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => {
                            app.should_quit = true;
                        }
                        KeyCode::Tab | KeyCode::Right => {
                            if !app.phases.is_empty() {
                                app.selected_tab = (app.selected_tab + 1) % app.phases.len();
                            }
                        }
                        KeyCode::BackTab | KeyCode::Left => {
                            if !app.phases.is_empty() {
                                app.selected_tab = app
                                    .selected_tab
                                    .checked_sub(1)
                                    .unwrap_or(app.phases.len() - 1);
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        if app.should_quit {
            break;
        }

        app.cycle += 1;
        let ts = MarketGenerator::now_ms();

        // Phase 1: push data + poll results
        if let Some(ref h) = p1_handles {
            let trades = gen1.generate_trades(ts);
            let count = trades.len() as u64;
            h.source.push_batch(trades);
            h.source.watermark(ts + 5_000);
            app.record_push(p1_idx, count);
            app.record_pipeline_count(p1_idx, "trades_in", count);

            for _ in 0..64 {
                match h.sub.poll() {
                    Some(bars) => {
                        app.record_pipeline_count(p1_idx, "ohlc_out", bars.len() as u64);
                        app.record_bars(p1_idx, bars);
                    }
                    None => break,
                }
            }

            app.tick_throughput(p1_idx);

            if app.phases[p1_idx].bars_received > 0 {
                app.phases[p1_idx].status = PhaseStatus::Pass;
            }
        }

        // Phase 2: push data + poll results
        if let Some(ref h) = p2_handles {
            let trades = gen2.generate_trades(ts);
            let count = trades.len() as u64;
            h.source.push_batch(trades);
            h.source.watermark(ts + 5_000);
            app.record_push(p2_idx, count);
            app.record_pipeline_count(p2_idx, "trades_in", count);

            // Level 1: 5s bars
            for _ in 0..64 {
                match h.ohlc_5s_sub.poll() {
                    Some(bars) => {
                        app.record_pipeline_count(p2_idx, "ohlc_5s_out", bars.len() as u64);
                        app.record_bars_full(p2_idx, bars);
                    }
                    None => break,
                }
            }
            // Level 2: cascading 10s bars
            if let Some(ref sub) = h.ohlc_cascade_sub {
                for _ in 0..64 {
                    match sub.poll() {
                        Some(bars) => {
                            app.record_pipeline_count(p2_idx, "ohlc_10s_out", bars.len() as u64);
                            app.record_bars_full(p2_idx, bars);
                        }
                        None => break,
                    }
                }
            }

            app.tick_throughput(p2_idx);

            if app.phases[p2_idx].bars_received > 0 {
                app.phases[p2_idx].status = PhaseStatus::Pass;
            }
        }

        // Phase 3: produce trades to Kafka + poll subscriber
        if let Some(ref h) = p3_handles {
            let count =
                crate::phase3_kafka::produce_trades(&h.producer, &mut gen3, ts).await;
            app.record_push(p3_idx, count as u64);
            app.record_pipeline_count(p3_idx, "produced", count as u64);

            if let Some(ref sub) = h.summary_sub {
                for _ in 0..64 {
                    match sub.poll() {
                        Some(rows) => {
                            app.record_pipeline_count(
                                p3_idx,
                                "summary_out",
                                rows.len() as u64,
                            );
                            app.record_summary(p3_idx, rows);
                        }
                        None => break,
                    }
                }
            }

            app.tick_throughput(p3_idx);

            if app.phases[p3_idx].bars_received > 0 {
                app.phases[p3_idx].status = PhaseStatus::Pass;
            }
        }

        // Phase 4: push trades/quotes/orders + poll join results
        if let Some(ref h) = p4_handles {
            let trades = gen4.generate_trades(ts);
            let trade_count = trades.len() as u64;
            h.trade_source.push_batch(trades);
            h.trade_source.watermark(ts + 5_000);

            // Push quotes slightly behind trades for ASOF backward match
            let quotes = gen4.generate_quotes(ts - 100);
            let quote_count = quotes.len() as u64;
            h.quote_source.push_batch(quotes);
            h.quote_source.watermark(ts + 5_000);

            // Push 1 order per cycle
            let order = gen4.generate_order(ts);
            h.order_source.push_batch(vec![order]);
            h.order_source.watermark(ts + 60_000);

            app.record_push(p4_idx, trade_count);
            app.record_pipeline_count(p4_idx, "trades_in", trade_count);
            app.record_pipeline_count(p4_idx, "quotes_in", quote_count);
            app.record_pipeline_count(p4_idx, "orders_in", 1);

            // Poll ASOF results
            if let Some(ref sub) = h.asof_sub {
                for _ in 0..64 {
                    match sub.poll() {
                        Some(rows) => {
                            app.record_pipeline_count(p4_idx, "asof_out", rows.len() as u64);
                            app.record_asof(p4_idx, rows);
                        }
                        None => break,
                    }
                }
            }

            // Poll stream-stream JOIN results
            if let Some(ref sub) = h.join_sub {
                for _ in 0..64 {
                    match sub.poll() {
                        Some(rows) => {
                            app.record_pipeline_count(p4_idx, "join_out", rows.len() as u64);
                            app.record_join_match(p4_idx, rows);
                        }
                        None => break,
                    }
                }
            }

            app.tick_throughput(p4_idx);

            if app.phases[p4_idx].bars_received > 0 {
                app.phases[p4_idx].status = PhaseStatus::Pass;
            }
        }

        // Phase 5: insert orders into Postgres + poll CDC subscriber
        if let Some(ref h) = p5_handles {
            // Insert batch of orders every other cycle (slower rate for CDC)
            if app.cycle % 3 == 0 {
                match crate::phase5_cdc::insert_orders(&h.pg_client, app.cycle).await {
                    Ok(count) => {
                        app.record_push(p5_idx, count as u64);
                        app.record_pipeline_count(p5_idx, "inserted", count as u64);
                    }
                    Err(e) => {
                        eprintln!("Phase 5 insert error: {e}");
                    }
                }
            }

            // In polling mode: read CDC events from slot and push into laminardb
            if h.polling_mode {
                if let Some(ref source) = h.source_handle {
                    if let Ok(count) =
                        crate::phase5_cdc::poll_and_push(&h.pg_client, source).await
                    {
                        if count > 0 {
                            app.record_pipeline_count(p5_idx, "cdc_polled", count as u64);
                        }
                    }
                }
            }

            // Poll CDC subscriber for aggregated totals
            if let Some(ref sub) = h.totals_sub {
                for _ in 0..64 {
                    match sub.poll() {
                        Some(rows) => {
                            app.record_pipeline_count(
                                p5_idx,
                                "totals_out",
                                rows.len() as u64,
                            );
                            app.record_cdc_totals(p5_idx, rows);
                        }
                        None => break,
                    }
                }
            }

            app.tick_throughput(p5_idx);

            if app.phases[p5_idx].bars_received > 0 {
                app.phases[p5_idx].status = PhaseStatus::Pass;
            }
        }

        // Phase 6: push trades + poll HOP/SESSION/EMIT subscribers
        if let Some(ref h) = p6_handles {
            let trades = gen6.generate_trades(ts);
            let count = trades.len() as u64;
            h.source.push_batch(trades);
            h.source.watermark(ts + 10_000);
            app.record_push(p6_idx, count);
            app.record_pipeline_count(p6_idx, "trades_in", count);

            // Poll HOP results
            if let Some(ref sub) = h.hop_sub {
                for _ in 0..64 {
                    match sub.poll() {
                        Some(rows) => {
                            app.record_pipeline_count(p6_idx, "hop_out", rows.len() as u64);
                            app.record_hop(p6_idx, rows);
                        }
                        None => break,
                    }
                }
            }

            // Poll SESSION results
            if let Some(ref sub) = h.session_sub {
                for _ in 0..64 {
                    match sub.poll() {
                        Some(rows) => {
                            app.record_pipeline_count(
                                p6_idx,
                                "session_out",
                                rows.len() as u64,
                            );
                            app.record_session(p6_idx, rows);
                        }
                        None => break,
                    }
                }
            }

            // Poll EMIT ON UPDATE results
            if let Some(ref sub) = h.emit_sub {
                for _ in 0..64 {
                    match sub.poll() {
                        Some(rows) => {
                            app.record_pipeline_count(p6_idx, "emit_out", rows.len() as u64);
                            app.record_bars(p6_idx, rows);
                        }
                        None => break,
                    }
                }
            }

            app.tick_throughput(p6_idx);

            if app.phases[p6_idx].bars_received > 0 {
                app.phases[p6_idx].status = PhaseStatus::Pass;
            }
        }
    }

    // Restore terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    if let Some(h) = p1_handles {
        let _ = h.db.shutdown().await;
    }
    if let Some(h) = p2_handles {
        let _ = h.db.shutdown().await;
    }
    if let Some(h) = p3_handles {
        let _ = h.db.shutdown().await;
    }
    if let Some(h) = p4_handles {
        let _ = h.db.shutdown().await;
    }
    if let Some(h) = p5_handles {
        let _ = h.db.shutdown().await;
    }
    if let Some(h) = p6_handles {
        let _ = h.db.shutdown().await;
    }

    Ok(())
}

// ── Drawing ──────────────────────────────────────────────────────────────

fn draw(f: &mut ratatui::Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),  // tabs
            Constraint::Length(7),  // stats
            Constraint::Length(14), // pipeline flow (expanded)
            Constraint::Min(6),    // ohlc table
            Constraint::Length(1), // footer
        ])
        .split(f.area());

    draw_tabs(f, app, chunks[0]);
    draw_stats(f, app, chunks[1]);
    draw_pipeline_flow(f, app, chunks[2]);
    draw_ohlc_table(f, app, chunks[3]);
    draw_footer(f, app, chunks[4]);
}

fn draw_tabs(f: &mut ratatui::Frame, app: &App, area: Rect) {
    let titles: Vec<Line> = app
        .phases
        .iter()
        .map(|p| {
            let status_color = p.status.color();
            Line::from(vec![
                Span::styled(p.name, Style::default().fg(Color::White)),
                Span::raw(" ["),
                Span::styled(
                    p.status.label(),
                    Style::default()
                        .fg(status_color)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("]"),
            ])
        })
        .collect();

    let tabs = Tabs::new(titles)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" LaminarDB Test "),
        )
        .select(app.selected_tab)
        .style(Style::default().fg(Color::DarkGray))
        .highlight_style(
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        );

    f.render_widget(tabs, area);
}

fn draw_stats(f: &mut ratatui::Frame, app: &App, area: Rect) {
    if app.phases.is_empty() {
        return;
    }
    let phase = &app.phases[app.selected_tab];
    let uptime = app.uptime.elapsed().as_secs();

    let text = vec![
        Line::from(vec![
            Span::styled("Description: ", Style::default().fg(Color::DarkGray)),
            Span::raw(phase.description),
        ]),
        Line::from(vec![
            Span::styled("Trades: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{}", phase.trades_pushed),
                Style::default().fg(Color::Yellow),
            ),
            Span::raw("  "),
            Span::styled("Bars: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{}", phase.bars_received),
                Style::default().fg(Color::Green),
            ),
            Span::raw("  "),
            Span::styled("Cycle: ", Style::default().fg(Color::DarkGray)),
            Span::styled(format!("{}", app.cycle), Style::default().fg(Color::Cyan)),
            Span::raw("  "),
            Span::styled("Uptime: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{}s", uptime),
                Style::default().fg(Color::White),
            ),
        ]),
        Line::from(vec![
            Span::styled("Throughput: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{:.0}", phase.trades_per_sec),
                Style::default().fg(Color::Yellow),
            ),
            Span::styled(" trades/s", Style::default().fg(Color::DarkGray)),
            Span::raw("  "),
            Span::styled(
                format!("{:.0}", phase.bars_per_sec),
                Style::default().fg(Color::Green),
            ),
            Span::styled(" bars/s", Style::default().fg(Color::DarkGray)),
        ]),
        Line::from(vec![
            Span::styled("Latency:    ", Style::default().fg(Color::DarkGray)),
            Span::styled("avg ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format_latency(phase.avg_latency_us),
                Style::default().fg(latency_color(phase.avg_latency_us)),
            ),
            Span::raw("  "),
            Span::styled("min ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format_latency(phase.min_latency_us),
                Style::default().fg(Color::Green),
            ),
            Span::raw("  "),
            Span::styled("max ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format_latency(phase.max_latency_us),
                Style::default().fg(latency_color(phase.max_latency_us)),
            ),
        ]),
    ];

    let block = Block::default().borders(Borders::ALL).title(" Stats ");
    let paragraph = Paragraph::new(text).block(block);
    f.render_widget(paragraph, area);
}

fn draw_pipeline_flow(f: &mut ratatui::Frame, app: &App, area: Rect) {
    if app.phases.is_empty() {
        return;
    }
    let phase = &app.phases[app.selected_tab];
    let c = &phase.pipeline_counters;
    let cy = app.cycle;

    // Pulsing color for vertical flow arrows (shows data is flowing)
    let dim = Style::default().fg(Color::DarkGray);
    let bold_w = Style::default()
        .fg(Color::White)
        .add_modifier(Modifier::BOLD);

    let lines: Vec<Line> = match app.selected_tab {
        // Phase 1: Rust API — full vertical flow
        0 => {
            let t_in = c.get("trades_in").copied().unwrap_or(0);
            let ohlc = c.get("ohlc_out").copied().unwrap_or(0);
            let active = ohlc > 0;
            let pulse = vpulse(cy, active);
            vec![
                Line::from(vec![
                    Span::styled("  Source: ", dim),
                    Span::styled(
                        "trades",
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(format!(" [{}]", t_in), Style::default().fg(Color::Yellow)),
                    Span::styled("    push_batch() + watermark(ts + 5000)", dim),
                ]),
                Line::from(Span::styled("    │", pulse)),
                Line::from(Span::styled("    ▼", pulse)),
                Line::from(vec![
                    Span::styled("  MemTable: ", dim),
                    Span::styled(
                        "trades",
                        Style::default()
                            .fg(Color::Magenta)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(" (registered in DataFusion ctx each 100ms tick)", dim),
                ]),
                Line::from(vec![
                    Span::styled("    │", pulse),
                    Span::styled("  ctx.sql()", bold_w),
                ]),
                Line::from(vec![
                    Span::styled("    ▼", pulse),
                    Span::styled(
                        "  SELECT first_value(price), MAX, MIN, last_value(price)",
                        Style::default().fg(Color::White),
                    ),
                ]),
                Line::from(vec![
                    Span::styled("    │", pulse),
                    Span::styled(
                        "  GROUP BY symbol, TUMBLE(ts, INTERVAL '5' SECOND)",
                        Style::default().fg(Color::White),
                    ),
                ]),
                Line::from(vec![
                    Span::styled("    ▼", pulse),
                    Span::styled("  results ", dim),
                    flow_arrow(cy, 4, active),
                    Span::styled(" subscriber buffer ", dim),
                    flow_arrow(cy + 2, 4, active),
                    Span::styled(" poll()", dim),
                ]),
                Line::from(""),
                Line::from(vec![
                    Span::styled("  Output: ", dim),
                    Span::styled(
                        "ohlc_bars",
                        Style::default()
                            .fg(Color::Green)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(format!(" [{}]", ohlc), Style::default().fg(Color::Green)),
                    pipeline_status(ohlc),
                ]),
            ]
        }
        // Phase 2: Streaming SQL — two query branches from one MemTable
        1 => {
            let t_in = c.get("trades_in").copied().unwrap_or(0);
            let l1 = c.get("ohlc_5s_out").copied().unwrap_or(0);
            let l2 = c.get("ohlc_10s_out").copied().unwrap_or(0);
            let l1_act = l1 > 0;
            let pulse = vpulse(cy, l1_act);
            vec![
                Line::from(vec![
                    Span::styled("  Source: ", dim),
                    Span::styled(
                        "trades",
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(format!(" [{}]", t_in), Style::default().fg(Color::Yellow)),
                    Span::styled("  push_batch() + watermark(ts + 5000)", dim),
                ]),
                Line::from(Span::styled("    │", pulse)),
                Line::from(vec![
                    Span::styled("    ▼", pulse),
                    Span::styled("  MemTable: ", dim),
                    Span::styled(
                        "trades",
                        Style::default()
                            .fg(Color::Magenta)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(" (registered each 100ms tick)", dim),
                ]),
                Line::from(vec![
                    Span::styled("    ├", pulse),
                    flow_arrow(cy, 3, l1_act),
                    Span::styled(" ctx.sql(ohlc_5s): ", bold_w),
                    Span::styled(
                        "TUMBLE(5s) + first_value/last_value/SUM",
                        Style::default().fg(Color::White),
                    ),
                ]),
                Line::from(vec![
                    Span::styled("    │     └──▶ ", dim),
                    Span::styled(
                        "ohlc_5s",
                        Style::default()
                            .fg(Color::Green)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(format!(" [{}]", l1), Style::default().fg(Color::Green)),
                    Span::styled("  subscriber ", dim),
                    flow_arrow(cy, 3, l1_act),
                    Span::styled(" poll()", dim),
                    pipeline_status(l1),
                ]),
                Line::from(Span::styled("    │", dim)),
                Line::from(vec![
                    Span::styled("    └", Style::default().fg(Color::Red)),
                    flow_arrow(cy, 3, false),
                    Span::styled(" ctx.sql(ohlc_10s): ", bold_w),
                    Span::styled("FROM ohlc_5s ", Style::default().fg(Color::Red)),
                    Span::styled("◀── empty MemTable!", Style::default().fg(Color::Red)),
                ]),
                Line::from(vec![
                    Span::styled("          └──▶ ", dim),
                    Span::styled(
                        "ohlc_10s",
                        Style::default()
                            .fg(Color::Red)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(format!(" [{}]", l2), Style::default().fg(Color::Red)),
                    pipeline_status(l2),
                ]),
                Line::from(vec![Span::styled(
                    "          ohlc_5s output goes to subscribers, never loops back as MemTable",
                    dim,
                )]),
            ]
        }
        // Phase 3: Kafka Pipeline — producer → FROM KAFKA → SQL → INTO KAFKA
        2 => {
            let produced = c.get("produced").copied().unwrap_or(0);
            let summary = c.get("summary_out").copied().unwrap_or(0);
            let src_ok = produced > 0;
            let out_ok = summary > 0;
            let pulse = vpulse(cy, src_ok);
            vec![
                Line::from(vec![
                    Span::styled("  Producer: ", dim),
                    Span::styled(
                        "rdkafka FutureProducer",
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!(" [{}]", produced),
                        Style::default().fg(Color::Yellow),
                    ),
                ]),
                Line::from(vec![
                    Span::styled("    │", pulse),
                    Span::styled(
                        "  produce() → JSON → topic: market-trades",
                        dim,
                    ),
                ]),
                Line::from(Span::styled("    ▼", pulse)),
                Line::from(vec![
                    Span::styled("  FROM KAFKA: ", dim),
                    Span::styled(
                        "brokers=localhost:19092",
                        Style::default()
                            .fg(Color::Magenta)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        ", topic=market-trades, format=json",
                        dim,
                    ),
                ]),
                Line::from(vec![
                    Span::styled("    │", vpulse(cy, out_ok)),
                    Span::styled(
                        "  Kafka Source Connector → Arrow RecordBatch → ctx.sql()",
                        dim,
                    ),
                ]),
                Line::from(vec![
                    Span::styled("    ▼", vpulse(cy, out_ok)),
                    Span::styled(
                        "  SELECT COUNT(*), SUM(price * volume) GROUP BY TUMBLE(5s)",
                        Style::default().fg(Color::White),
                    ),
                ]),
                Line::from(vec![
                    Span::styled("    ├", vpulse(cy, out_ok)),
                    flow_arrow(cy, 3, out_ok),
                    Span::styled(" subscriber: ", dim),
                    Span::styled(
                        "trade_summary",
                        Style::default()
                            .fg(Color::Green)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!(" [{}]", summary),
                        Style::default().fg(Color::Green),
                    ),
                    Span::styled(" → poll()", dim),
                    pipeline_status(summary),
                ]),
                Line::from(vec![
                    Span::styled("    │", dim),
                ]),
                Line::from(vec![
                    Span::styled("    └", vpulse(cy, out_ok)),
                    flow_arrow(cy + 1, 3, out_ok),
                    Span::styled(" INTO KAFKA: ", dim),
                    Span::styled(
                        "topic=trade-summaries",
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(", format=json", dim),
                ]),
                Line::from(""),
            ]
        }
        // Phase 4: Stream Joins — 3 sources, 2 query branches
        3 => {
            let t_in = c.get("trades_in").copied().unwrap_or(0);
            let q_in = c.get("quotes_in").copied().unwrap_or(0);
            let o_in = c.get("orders_in").copied().unwrap_or(0);
            let asof = c.get("asof_out").copied().unwrap_or(0);
            let join = c.get("join_out").copied().unwrap_or(0);
            let join_act = join > 0;
            let pulse = vpulse(cy, join_act);
            vec![
                Line::from(vec![
                    Span::styled("  Sources: ", dim),
                    Span::styled("trades", Style::default().fg(Color::Yellow)),
                    Span::styled(
                        format!("[{}]", t_in),
                        Style::default().fg(Color::Yellow),
                    ),
                    Span::raw("  "),
                    Span::styled("quotes", Style::default().fg(Color::Magenta)),
                    Span::styled(
                        format!("[{}]", q_in),
                        Style::default().fg(Color::Magenta),
                    ),
                    Span::raw("  "),
                    Span::styled("orders", Style::default().fg(Color::Blue)),
                    Span::styled(format!("[{}]", o_in), Style::default().fg(Color::Blue)),
                ]),
                Line::from(vec![
                    Span::styled("    │ ", pulse),
                    Span::styled("push_batch() x3   watermark(ts + 5000 / +60000)", dim),
                ]),
                Line::from(vec![
                    Span::styled("    ▼ ", pulse),
                    Span::styled("MemTables: ", dim),
                    Span::styled(
                        "trades, quotes, orders",
                        Style::default()
                            .fg(Color::Magenta)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(" (all registered before queries)", dim),
                ]),
                Line::from(vec![
                    Span::styled("    ├", Style::default().fg(Color::Red)),
                    flow_arrow(cy, 3, false),
                    Span::styled(" ctx.sql: ", bold_w),
                    Span::styled(
                        "ASOF JOIN",
                        Style::default()
                            .fg(Color::Red)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(" (trades x quotes)", dim),
                ]),
                Line::from(vec![
                    Span::styled("    │     ", dim),
                    Span::styled(
                        "MATCH_CONDITION() — DataFusion can't parse ASOF syntax",
                        Style::default().fg(Color::Red),
                    ),
                ]),
                Line::from(vec![
                    Span::styled("    │     └──▶ ", dim),
                    Span::styled(
                        "asof_enriched",
                        Style::default()
                            .fg(Color::Red)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(format!(" [{}]", asof), Style::default().fg(Color::Red)),
                    pipeline_status(asof),
                ]),
                Line::from(Span::styled("    │", dim)),
                Line::from(vec![
                    Span::styled(
                        "    └",
                        Style::default().fg(if join_act { Color::Cyan } else { Color::Red }),
                    ),
                    flow_arrow(cy, 3, join_act),
                    Span::styled(" ctx.sql: ", bold_w),
                    Span::styled(
                        "INNER JOIN",
                        Style::default()
                            .fg(Color::Green)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(" (trades x orders)", dim),
                ]),
                Line::from(vec![
                    Span::styled(
                        "          ON symbol AND o.ts BETWEEN t.ts-60000, t.ts+60000",
                        dim,
                    ),
                ]),
                Line::from(vec![
                    Span::styled("          └──▶ ", dim),
                    Span::styled(
                        "trade_order_match",
                        Style::default()
                            .fg(Color::Green)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(format!(" [{}]", join), Style::default().fg(Color::Green)),
                    Span::styled("  subscriber ", dim),
                    flow_arrow(cy, 3, join_act),
                    Span::styled(" poll()", dim),
                    pipeline_status(join),
                ]),
            ]
        }
        // Phase 5: CDC Pipeline — Postgres → CDC → SQL → subscriber
        4 => {
            let inserted = c.get("inserted").copied().unwrap_or(0);
            let totals = c.get("totals_out").copied().unwrap_or(0);
            let src_ok = inserted > 0;
            let out_ok = totals > 0;
            let pulse = vpulse(cy, src_ok);
            vec![
                Line::from(vec![
                    Span::styled("  Postgres: ", dim),
                    Span::styled(
                        "orders table",
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!(" [{}]", inserted),
                        Style::default().fg(Color::Yellow),
                    ),
                    Span::styled("  INSERT/UPDATE via tokio-postgres", dim),
                ]),
                Line::from(vec![
                    Span::styled("    │", pulse),
                    Span::styled(
                        "  WAL → pgoutput → logical replication",
                        dim,
                    ),
                ]),
                Line::from(Span::styled("    ▼", pulse)),
                Line::from(vec![
                    Span::styled("  FROM POSTGRES_CDC: ", dim),
                    Span::styled(
                        "slot=laminar_orders",
                        Style::default()
                            .fg(Color::Magenta)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        ", pub=laminar_pub",
                        dim,
                    ),
                ]),
                Line::from(vec![
                    Span::styled("    │", vpulse(cy, out_ok)),
                    Span::styled(
                        "  CDC Connector → envelope(_op, _after) → ctx.sql()",
                        dim,
                    ),
                ]),
                Line::from(vec![
                    Span::styled("    ▼", vpulse(cy, out_ok)),
                    Span::styled(
                        "  SELECT COUNT(*), SUM(amount) GROUP BY customer_id",
                        Style::default().fg(Color::White),
                    ),
                ]),
                Line::from(vec![
                    Span::styled("    │", vpulse(cy, out_ok)),
                    flow_arrow(cy, 3, out_ok),
                    Span::styled(" subscriber: ", dim),
                    Span::styled(
                        "customer_totals",
                        Style::default()
                            .fg(Color::Green)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!(" [{}]", totals),
                        Style::default().fg(Color::Green),
                    ),
                    Span::styled(" → poll()", dim),
                    pipeline_status(totals),
                ]),
                Line::from(""),
                Line::from(vec![
                    Span::styled(
                        "  wal_level=logical, max_replication_slots=4, snapshot.mode=never",
                        dim,
                    ),
                ]),
            ]
        }
        // Phase 6: Bonus — HOP, SESSION, EMIT ON UPDATE from one source
        5 => {
            let t_in = c.get("trades_in").copied().unwrap_or(0);
            let hop = c.get("hop_out").copied().unwrap_or(0);
            let sess = c.get("session_out").copied().unwrap_or(0);
            let emit = c.get("emit_out").copied().unwrap_or(0);
            let _active = hop > 0 || sess > 0 || emit > 0;
            let pulse = vpulse(cy, t_in > 0);
            vec![
                Line::from(vec![
                    Span::styled("  Source: ", dim),
                    Span::styled(
                        "trades",
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(format!(" [{}]", t_in), Style::default().fg(Color::Yellow)),
                    Span::styled("  push_batch() + watermark(ts + 10000)", dim),
                ]),
                Line::from(Span::styled("    │", pulse)),
                Line::from(vec![
                    Span::styled("    ├", vpulse(cy, hop > 0)),
                    flow_arrow(cy, 3, hop > 0),
                    Span::styled(" HOP(2s slide, 10s size): ", bold_w),
                    Span::styled(
                        "hop_volume",
                        Style::default()
                            .fg(if hop > 0 { Color::Green } else { Color::Red })
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(format!(" [{}]", hop), Style::default().fg(Color::Green)),
                    pipeline_status(hop),
                ]),
                Line::from(vec![
                    Span::styled("    │", dim),
                ]),
                Line::from(vec![
                    Span::styled("    ├", vpulse(cy, sess > 0)),
                    flow_arrow(cy + 1, 3, sess > 0),
                    Span::styled(" SESSION(3s gap): ", bold_w),
                    Span::styled(
                        "session_burst",
                        Style::default()
                            .fg(if sess > 0 { Color::Green } else { Color::Red })
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(format!(" [{}]", sess), Style::default().fg(Color::Green)),
                    pipeline_status(sess),
                ]),
                Line::from(vec![
                    Span::styled("    │", dim),
                ]),
                Line::from(vec![
                    Span::styled("    └", vpulse(cy, emit > 0)),
                    flow_arrow(cy + 2, 3, emit > 0),
                    Span::styled(" TUMBLE(5s) EMIT ON UPDATE: ", bold_w),
                    Span::styled(
                        "ohlc_update",
                        Style::default()
                            .fg(if emit > 0 { Color::Green } else { Color::Red })
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(format!(" [{}]", emit), Style::default().fg(Color::Green)),
                    pipeline_status(emit),
                ]),
                Line::from(""),
                Line::from(vec![
                    Span::styled(
                        "  3 streams from 1 source — testing window types + emit modes",
                        dim,
                    ),
                ]),
            ]
        }
        _ => vec![Line::from(Span::styled(
            " No pipeline data",
            Style::default().fg(Color::DarkGray),
        ))],
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Embedded Pipeline (100ms tick cycle) ");
    let paragraph = Paragraph::new(lines).block(block);
    f.render_widget(paragraph, area);
}

/// Pulsing style for vertical flow arrows (│ ▼ ├ └).
fn vpulse(cycle: u64, active: bool) -> Style {
    if !active {
        return Style::default().fg(Color::Red);
    }
    if cycle % 4 < 2 {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::Blue)
    }
}

/// Animated flow arrow: a dot `◆` flows along `─` dashes when active, red `╳` when inactive.
fn flow_arrow(cycle: u64, width: usize, active: bool) -> Span<'static> {
    if !active {
        let s: String = "─".repeat(width) + "╳";
        return Span::styled(s, Style::default().fg(Color::Red));
    }
    let pos = (cycle as usize / 2) % width;
    let mut s = String::with_capacity(width * 3 + 3);
    for i in 0..width {
        if i == pos {
            s.push('◆');
        } else {
            s.push('─');
        }
    }
    s.push('▶');
    Span::styled(s, Style::default().fg(Color::Cyan))
}

/// PASS/FAIL indicator based on output count.
fn pipeline_status(count: u64) -> Span<'static> {
    if count > 0 {
        Span::styled(
            " PASS",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled(
            " FAIL",
            Style::default()
                .fg(Color::Red)
                .add_modifier(Modifier::BOLD),
        )
    }
}

fn draw_ohlc_table(f: &mut ratatui::Frame, app: &App, area: Rect) {
    if app.phases.is_empty() {
        return;
    }
    let phase = &app.phases[app.selected_tab];

    let header = Row::new(vec!["Symbol", "Open", "High", "Low", "Close"])
        .style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
        .bottom_margin(1);

    let rows: Vec<Row> = phase
        .recent_bars
        .iter()
        .map(|bar| {
            let change_color = if bar.close >= bar.open {
                Color::Green
            } else {
                Color::Red
            };
            Row::new(vec![
                bar.symbol.clone(),
                format!("{:.2}", bar.open),
                format!("{:.2}", bar.high),
                format!("{:.2}", bar.low),
                format!("{:.2}", bar.close),
            ])
            .style(Style::default().fg(change_color))
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(8),
            Constraint::Length(12),
            Constraint::Length(12),
            Constraint::Length(12),
            Constraint::Length(12),
        ],
    )
    .header(header)
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(" OHLC Bars (most recent) "),
    );

    f.render_widget(table, area);
}

fn draw_footer(f: &mut ratatui::Frame, _app: &App, area: Rect) {
    let footer = Paragraph::new(Line::from(vec![
        Span::styled(
            " Tab",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" switch phase  "),
        Span::styled(
            "q",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" quit"),
    ]))
    .style(Style::default().fg(Color::DarkGray));

    f.render_widget(footer, area);
}

// ── Helpers ──────────────────────────────────────────────────────────────

fn format_latency(us: u64) -> String {
    if us == 0 {
        "--".to_string()
    } else if us < 1_000 {
        format!("{}us", us)
    } else if us < 1_000_000 {
        format!("{:.1}ms", us as f64 / 1_000.0)
    } else {
        format!("{:.2}s", us as f64 / 1_000_000.0)
    }
}

fn latency_color(us: u64) -> Color {
    if us < 1_000 {
        Color::Green // < 1ms
    } else if us < 10_000 {
        Color::Yellow // < 10ms
    } else if us < 100_000 {
        Color::Rgb(255, 165, 0) // < 100ms (orange)
    } else {
        Color::Red // >= 100ms
    }
}
