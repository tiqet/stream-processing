use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use stream_processing::{ProcessorConfig, StreamProcessor};

#[derive(Debug, Clone, Deserialize, Serialize)]
struct RealEvent {
    user_id: String,
    action: String,
    timestamp: u64,
    metadata: HashMap<String, String>,
    payload: Vec<u8>,
}

impl RealEvent {
    fn new(id: u64, payload_size: usize) -> Self {
        let mut metadata = HashMap::new();
        metadata.insert("session_id".to_string(), format!("sess_{}", id % 1000));
        metadata.insert("ip_addr".to_string(), format!("192.168.1.{}", id % 255));
        metadata.insert("user_agent".to_string(), "benchmark/1.0".to_string());

        Self {
            user_id: format!("user_{}", id % 10000),
            action: ["login", "view", "click", "purchase", "logout"][id as usize % 5].to_string(),
            timestamp: id,
            metadata,
            payload: vec![42u8; payload_size],
        }
    }
}

struct RealisticBenchmark {
    processed_count: Arc<AtomicU64>,
    processing_time_total: Arc<AtomicU64>,
}

impl RealisticBenchmark {
    fn new() -> Self {
        Self {
            processed_count: Arc::new(AtomicU64::new(0)),
            processing_time_total: Arc::new(AtomicU64::new(0)),
        }
    }

    async fn run_cpu_bound_test(&self, events: usize, batch_size: usize) {
        println!(
            "🖥️  CPU-Bound Test: {} events, batch: {}",
            events, batch_size
        );
        self.run_test(events, batch_size, TestType::CpuBound).await;
    }

    async fn run_io_simulation_test(&self, events: usize, batch_size: usize) {
        println!(
            "💾 I/O Simulation Test: {} events, batch: {}",
            events, batch_size
        );
        self.run_test(events, batch_size, TestType::IoSimulation)
            .await;
    }

    async fn run_mixed_workload_test(&self, events: usize, batch_size: usize) {
        println!(
            "🔄 Mixed Workload Test: {} events, batch: {}",
            events, batch_size
        );
        self.run_test(events, batch_size, TestType::Mixed).await;
    }

    async fn run_test(&self, total_events: usize, batch_size: usize, test_type: TestType) {
        self.processed_count.store(0, Ordering::Relaxed);
        self.processing_time_total.store(0, Ordering::Relaxed);

        let config = ProcessorConfig {
            buffer_size: 5000,
            batch_size,
            batch_timeout: Duration::from_millis(1), // Very fast batching
            processing_timeout: Duration::from_secs(10),
            worker_count: num_cpus::get(),
            ..Default::default()
        };

        let processor = StreamProcessor::new(config, {
            let counter = Arc::clone(&self.processed_count);
            let time_counter = Arc::clone(&self.processing_time_total);
            move |events: Vec<RealEvent>| {
                let counter = Arc::clone(&counter);
                let time_counter = Arc::clone(&time_counter);
                let test_type = test_type.clone();
                async move {
                    let start = Instant::now();

                    match test_type {
                        TestType::CpuBound => {
                            // Realistic CPU work: JSON serialization, hashing, validation
                            for event in &events {
                                // Serialize to JSON (realistic workload)
                                let _json = serde_json::to_string(event).unwrap();

                                // Simulate validation logic
                                let _is_valid = !event.user_id.is_empty()
                                    && !event.action.is_empty()
                                    && event.timestamp > 0;

                                // Simulate some computation
                                let _hash = event.user_id.len()
                                    + event.action.len()
                                    + event.metadata.len()
                                    + event.payload.len();
                            }
                        }
                        TestType::IoSimulation => {
                            // Simulate database batch insert (realistic async I/O)
                            tokio::time::sleep(Duration::from_micros(100)).await;

                            // Simulate network call latency variation
                            if events.len() > 100 {
                                tokio::time::sleep(Duration::from_micros(50)).await;
                            }
                        }
                        TestType::Mixed => {
                            // Mix of CPU and I/O work
                            for (i, event) in events.iter().enumerate() {
                                if i % 10 == 0 {
                                    // Occasional I/O work
                                    tokio::time::sleep(Duration::from_micros(10)).await;
                                }

                                // CPU work
                                let _json = serde_json::to_string(event).unwrap();
                            }
                        }
                    }

                    let elapsed = start.elapsed();
                    counter.fetch_add(events.len() as u64, Ordering::Relaxed);
                    time_counter.fetch_add(elapsed.as_millis() as u64, Ordering::Relaxed);

                    Ok(())
                }
            }
        });

        let start_time = Instant::now();

        // Send events with realistic producer patterns
        let mut sent = 0;
        let events_per_burst = 1000;
        let burst_delay = Duration::from_millis(1);

        for burst in 0..(total_events / events_per_burst + 1) {
            let burst_start = burst * events_per_burst;
            let burst_end = std::cmp::min(burst_start + events_per_burst, total_events);

            // Send burst of events
            for i in burst_start..burst_end {
                let event = RealEvent::new(i as u64, 256);

                match processor.try_send(event) {
                    Ok(()) => sent += 1,
                    Err(_) => {
                        // Backpressure - use blocking send
                        let event = RealEvent::new(i as u64, 256);
                        if let Ok(()) = processor.send(event).await {
                            sent += 1;
                        }
                    }
                }
            }

            // Small delay between bursts (realistic pattern)
            if burst_end < total_events {
                tokio::time::sleep(burst_delay).await;
            }

            if sent % 50000 == 0 && sent > 0 {
                println!("  Sent: {sent}/{total_events}");
            }
        }

        let send_time = start_time.elapsed();
        println!(
            "  ✅ Sent {} events in {:.2}ms",
            sent,
            send_time.as_millis()
        );

        // Wait for processing completion
        let mut last_count = 0;
        let mut stable_readings = 0;

        while stable_readings < 3 {
            tokio::time::sleep(Duration::from_millis(50)).await;
            let current_count = self.processed_count.load(Ordering::Relaxed);

            if current_count == last_count && current_count >= total_events as u64 {
                stable_readings += 1;
            } else {
                stable_readings = 0;
                last_count = current_count;
            }
        }

        let total_time = start_time.elapsed();
        let processed = self.processed_count.load(Ordering::Relaxed);
        let total_proc_time = self.processing_time_total.load(Ordering::Relaxed);

        let throughput = (processed as f64 / total_time.as_secs_f64()) as u64;
        let avg_proc_time = if processed > 0 {
            total_proc_time / processed
        } else {
            0
        };

        // Get processor metrics
        let (proc_processed, errors, timeouts, proc_avg_time) = processor.metrics();

        println!("  📈 RESULTS:");
        println!("     Throughput: {:} events/sec", throughput);
        println!("     Total Time: {:.2}s", total_time.as_secs_f64());
        println!("     Avg Processing: {}μs per event", avg_proc_time * 1000);
        println!(
            "     Processor Metrics: {}/{}/{} (proc/err/timeout)",
            proc_processed, errors, timeouts
        );
        println!("     Library Overhead: {}ms", proc_avg_time);

        processor.shutdown().await;
        println!();
    }
}

#[derive(Clone)]
enum TestType {
    CpuBound,
    IoSimulation,
    Mixed,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("🚀 Realistic Stream Processing Benchmark");
    println!("=========================================\n");
    println!("CPU Cores: {}", num_cpus::get());
    println!();

    let benchmark = RealisticBenchmark::new();

    // Test optimal batch sizes for different workload types
    let test_events = 100_000;

    // CPU-bound workloads (JSON processing, validation, etc.)
    for batch_size in [1, 10, 50, 100, 500] {
        benchmark.run_cpu_bound_test(test_events, batch_size).await;
    }

    // I/O simulation (database writes, API calls)
    for batch_size in [10, 50, 100, 500] {
        benchmark
            .run_io_simulation_test(test_events, batch_size)
            .await;
    }

    // Mixed workloads
    for batch_size in [50, 100, 200] {
        benchmark
            .run_mixed_workload_test(test_events, batch_size)
            .await;
    }

    // High-volume stress test
    println!("🔥 STRESS TEST - 1M Events");
    benchmark.run_cpu_bound_test(1_000_000, 100).await;

    println!("✅ Realistic benchmark complete!");
    Ok(())
}
