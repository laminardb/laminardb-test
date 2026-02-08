//! Criterion benchmark for LaminarDB 6-stream pipeline throughput.
//!
//! Matches laminardb-fraud-detect benchmark structure for direct comparison:
//!   - push_throughput: raw ingestion speed (push only, no poll)
//!   - end_to_end: push + poll all 6 streams
//!   - pipeline_setup: time to create 2 sources + 6 streams + start
//!
//! Run:
//!   cargo bench                       # all benchmarks
//!   cargo bench -- push_throughput    # just push (no poll)
//!   cargo bench -- end_to_end        # push + poll
//!   cargo bench -- pipeline_setup    # just setup cost

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use tokio::runtime::Runtime;

use laminardb_test::phase7_stress::{FraudGenerator, setup_pipeline};

fn push_throughput(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let pipeline = rt.block_on(async { setup_pipeline().await.unwrap() });
    let mut gen = FraudGenerator::new();

    let mut group = c.benchmark_group("push_throughput");
    for size in [100, 500, 1000, 5000] {
        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, &size| {
            b.iter(|| {
                let ts = FraudGenerator::now_ms();
                let cycle_span = FraudGenerator::stress_cycle_span_ms(size);
                let (trades, orders) = gen.generate_stress_cycle(ts, size);
                pipeline.trade_source.push_batch(trades);
                if !orders.is_empty() {
                    pipeline.order_source.push_batch(orders);
                }
                pipeline.trade_source.watermark(ts + cycle_span + 10_000);
                pipeline.order_source.watermark(ts + cycle_span + 10_000);
            });
        });
    }
    group.finish();
}

fn end_to_end(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let pipeline = rt.block_on(async { setup_pipeline().await.unwrap() });
    let mut gen = FraudGenerator::new();

    let mut group = c.benchmark_group("end_to_end");
    for size in [100, 500, 1000, 5000] {
        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, &size| {
            b.iter(|| {
                let ts = FraudGenerator::now_ms();
                let cycle_span = FraudGenerator::stress_cycle_span_ms(size);
                let (trades, orders) = gen.generate_stress_cycle(ts, size);
                pipeline.trade_source.push_batch(trades);
                if !orders.is_empty() {
                    pipeline.order_source.push_batch(orders);
                }
                pipeline.trade_source.watermark(ts + cycle_span + 10_000);
                pipeline.order_source.watermark(ts + cycle_span + 10_000);

                // Poll all 6 streams to drain output
                macro_rules! drain {
                    ($sub:expr) => {
                        if let Some(ref sub) = $sub {
                            while let Some(_rows) = sub.poll() {}
                        }
                    };
                }
                drain!(pipeline.vol_baseline_sub);
                drain!(pipeline.ohlc_vol_sub);
                drain!(pipeline.rapid_fire_sub);
                drain!(pipeline.wash_score_sub);
                drain!(pipeline.suspicious_match_sub);
                drain!(pipeline.asof_match_sub);
            });
        });
    }
    group.finish();
}

fn pipeline_setup(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();

    c.bench_function("pipeline_setup", |b| {
        b.iter(|| {
            let _pipeline = rt.block_on(async { setup_pipeline().await.unwrap() });
        });
    });
}

criterion_group!(benches, push_throughput, end_to_end, pipeline_setup);
criterion_main!(benches);
