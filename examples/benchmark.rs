use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use stream_processing::{ProcessorConfig, StreamProcessor};
use tokio::time::sleep;

#[derive(Debug, Clone, Deserialize, Serialize)]
struct BenchEvent {
    id: u64,
    data: String,
}

struct BenchmarkRunner {
    processed_count: Arc<AtomicU64>,
}

impl BenchmarkRunner {
    fn new() -> Self {
        Self {
            processed_count: Arc::new(AtomicU64::new(0)),
        }
    }

    async fn run_benchmark(&self, total_events: usize, batch_size: usize) {
        println!("🚀 Running benchmark: {total_events} events, batch size: {batch_size}");

        let counter = Arc::clone(&self.processed_count);
        counter.store(0, Ordering::Relaxed);

        // Create processor with custom config
        let config = ProcessorConfig {
            buffer_size: 2000,
            batch_size,
            batch_timeout: Duration::from_millis(10),
            processing_timeout: Duration::from_secs(5),
            ..Default::default()
        };

        let processor = StreamProcessor::new(config, {
            let counter = Arc::clone(&counter);
            move |events: Vec<BenchEvent>| {
                let counter = Arc::clone(&counter);
                async move {
                    // Simulate some work
                    let work_time = if events.len() > 50 {
                        Duration::from_millis(5) // Heavier work for larger batches
                    } else {
                        Duration::from_micros(100) // Light work
                    };

                    sleep(work_time).await;

                    // Count processed events
                    counter.fetch_add(events.len() as u64, Ordering::Relaxed);

                    Ok(())
                }
            }
        });

        let start_time = Instant::now();

        // Send events as fast as possible
        let mut sent = 0;
        for i in 0..total_events {
            let event = BenchEvent {
                id: i as u64,
                data: format!("benchmark_data_{i}"),
            };

            match processor.try_send(event) {
                Ok(()) => sent += 1,
                Err(_) => {
                    // Channel full, wait a bit and retry
                    sleep(Duration::from_micros(10)).await;
                    if let Ok(()) = processor
                        .send(BenchEvent {
                            id: i as u64,
                            data: format!("benchmark_data_{i}"),
                        })
                        .await
                    {
                        sent += 1;
                    }
                }
            }

            // Print progress
            if sent % 10000 == 0 && sent > 0 {
                println!("Sent: {sent}/{total_events}");
            }
        }

        println!(
            "✅ Sent {} events in {:.2}ms",
            sent,
            start_time.elapsed().as_millis()
        );

        // Wait for processing to complete
        let mut last_count = 0;
        let mut stable_count = 0;

        while stable_count < 5 {
            // Wait for 5 stable readings
            sleep(Duration::from_millis(100)).await;
            let current_count = counter.load(Ordering::Relaxed);

            if current_count == last_count {
                stable_count += 1;
            } else {
                stable_count = 0;
                last_count = current_count;
            }

            if current_count >= total_events as u64 {
                break;
            }
        }

        let total_time = start_time.elapsed();
        let processed = counter.load(Ordering::Relaxed);
        let throughput = (processed as f64 / total_time.as_secs_f64()) as u64;

        println!("📊 RESULTS:");
        println!("   Events Sent: {sent}");
        println!("   Events Processed: {processed}");
        println!("   Total Time: {:.2}s", total_time.as_secs_f64());
        println!("   Throughput: {throughput} events/sec");
        println!("   Batch Size: {batch_size}");

        // Get processor metrics
        let (proc_processed, errors, timeouts, avg_time) = processor.metrics();
        println!("   Processor Metrics:");
        println!("     Processed: {proc_processed}, Errors: {errors}, Timeouts: {timeouts}");
        println!("     Avg Processing Time: {avg_time}ms");

        processor.shutdown().await;
        println!();
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("🎯 Stream Processing Benchmark");
    println!("==============================\n");

    let runner = BenchmarkRunner::new();

    // Test different batch sizes
    let test_cases = [
        (50_000, 1),   // No batching
        (50_000, 10),  // Small batches
        (50_000, 50),  // Medium batches
        (50_000, 100), // Large batches
        (50_000, 500), // Very large batches
    ];

    for (events, batch_size) in test_cases {
        runner.run_benchmark(events, batch_size).await;
        sleep(Duration::from_millis(500)).await; // Cool down between tests
    }

    // Stress test
    println!("🔥 STRESS TEST - High Volume");
    runner.run_benchmark(500_000, 100).await;

    println!("✅ Benchmark complete!");
    Ok(())
}
