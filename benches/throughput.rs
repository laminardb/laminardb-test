//! Criterion benchmark for LaminarDB 6-stream pipeline throughput.
//!
//! Measures push_batch + poll throughput at various batch sizes,
//! comparable to Phase 7 stress test results and published crate baseline (~2,275/sec).
//!
//! Run:
//!   cargo bench                    # all benchmarks
//!   cargo bench -- push_cycle      # just the push+poll cycle
//!   cargo bench -- pipeline_setup  # just the setup cost

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use std::time::Duration;

use laminardb_test::phase7_stress::{FraudGenerator, setup_pipeline};

/// Benchmark: pipeline setup latency (how long to create 2 sources + 6 streams + start).
fn bench_pipeline_setup(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    c.bench_function("pipeline_setup", |b| {
        b.to_async(&rt).iter(|| async {
            let _pipeline = setup_pipeline().await.unwrap();
        });
    });
}

/// Benchmark: push_batch + poll cycle at various batch sizes.
/// Measures end-to-end throughput of generating trades, pushing into the pipeline,
/// and polling all 6 output streams.
fn bench_push_cycle(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    let mut group = c.benchmark_group("push_cycle");
    group.measurement_time(Duration::from_secs(10));
    group.sample_size(20);

    for batch_size in [10, 50, 100, 500] {
        group.throughput(Throughput::Elements(batch_size as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(batch_size),
            &batch_size,
            |b, &size| {
                // Setup pipeline once per benchmark parameter
                let pipeline = rt.block_on(async { setup_pipeline().await.unwrap() });
                let mut gen = FraudGenerator::new();
                let mut event_ts = FraudGenerator::now_ms();

                b.to_async(&rt).iter(|| {
                    let cycle_span = FraudGenerator::stress_cycle_span_ms(size);
                    let (trades, orders) = gen.generate_stress_cycle(event_ts, size);

                    pipeline.trade_source.push_batch(trades);
                    if !orders.is_empty() {
                        pipeline.order_source.push_batch(orders);
                    }
                    pipeline
                        .trade_source
                        .watermark(event_ts + cycle_span + 10_000);
                    pipeline
                        .order_source
                        .watermark(event_ts + cycle_span + 10_000);

                    event_ts += cycle_span;

                    // Poll all 6 streams to drain output
                    async {
                        tokio::time::sleep(Duration::from_millis(1)).await;

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
                    }
                });
            },
        );
    }
    group.finish();
}

criterion_group!(benches, bench_pipeline_setup, bench_push_cycle);
criterion_main!(benches);
