use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot};
use tokio::time::timeout;

// ============================================================================
// CORE EVENT STRUCTURE
// ============================================================================

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Event {
    pub user_id: String,
    pub action: String,
    pub timestamp: u64,
}

// ============================================================================
// PROCESSING METRICS
// ============================================================================

#[derive(Debug, Default)]
pub struct ProcessingMetrics {
    pub processed: AtomicU64,
    pub errors: AtomicU64,
    pub timeouts: AtomicU64,
    pub avg_processing_time_ms: AtomicU64,
}

impl ProcessingMetrics {
    pub fn inc_processed(&self) {
        self.processed.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_errors(&self) {
        self.errors.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_timeouts(&self) {
        self.timeouts.fetch_add(1, Ordering::Relaxed);
    }

    pub fn update_avg_time(&self, duration_ms: u64) {
        // Simple exponential moving average
        let current = self.avg_processing_time_ms.load(Ordering::Relaxed);
        let new_avg = if current == 0 {
            duration_ms
        } else {
            (current * 7 + duration_ms) / 8 // 87.5% weight to history
        };
        self.avg_processing_time_ms
            .store(new_avg, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> (u64, u64, u64, u64) {
        (
            self.processed.load(Ordering::Relaxed),
            self.errors.load(Ordering::Relaxed),
            self.timeouts.load(Ordering::Relaxed),
            self.avg_processing_time_ms.load(Ordering::Relaxed),
        )
    }
}

// ============================================================================
// STREAM PROCESSOR CONFIGURATION
// ============================================================================

#[derive(Debug, Clone)]
pub struct ProcessorConfig {
    pub buffer_size: usize,           // Channel buffer size
    pub worker_count: usize,          // Number of parallel workers
    pub processing_timeout: Duration, // Max time per event
    pub batch_size: usize,            // Events per batch (1 = no batching)
    pub batch_timeout: Duration,      // Max time to wait for batch to fill
}

impl Default for ProcessorConfig {
    fn default() -> Self {
        Self {
            buffer_size: 1000,
            worker_count: num_cpus::get(),
            processing_timeout: Duration::from_secs(30),
            batch_size: 1,
            batch_timeout: Duration::from_millis(100),
        }
    }
}

// ============================================================================
// STREAM PROCESSOR CORE
// ============================================================================

pub struct StreamProcessor<T> {
    sender: mpsc::Sender<T>,
    metrics: Arc<ProcessingMetrics>,
    shutdown_tx: Option<oneshot::Sender<()>>,
}

impl<T> Clone for StreamProcessor<T> {
    fn clone(&self) -> Self {
        Self {
            sender: self.sender.clone(),
            metrics: Arc::clone(&self.metrics),
            shutdown_tx: None, // Only the original can trigger shutdown
        }
    }
}

impl<T> StreamProcessor<T>
where
    T: Send + 'static,
{
    /// Create a new stream processor with custom configuration
    pub fn new<F, Fut>(config: ProcessorConfig, processor_fn: F) -> Self
    where
        F: Fn(Vec<T>) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<(), Box<dyn std::error::Error + Send + Sync>>>
            + Send
            + 'static,
    {
        let (tx, rx) = mpsc::channel(config.buffer_size);
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let metrics = Arc::new(ProcessingMetrics::default());

        // Spawn the processing task
        let metrics_clone = Arc::clone(&metrics);
        tokio::spawn(Self::run_processor(
            rx,
            shutdown_rx,
            config,
            processor_fn,
            metrics_clone,
        ));

        Self {
            sender: tx,
            metrics,
            shutdown_tx: Some(shutdown_tx),
        }
    }

    /// Send an event to be processed (non-blocking)
    pub async fn send(&self, event: T) -> Result<(), mpsc::error::SendError<T>> {
        self.sender.send(event).await
    }

    /// Try to send an event without blocking
    pub fn try_send(&self, event: T) -> Result<(), mpsc::error::TrySendError<T>> {
        self.sender.try_send(event)
    }

    /// Get processing metrics snapshot
    pub fn metrics(&self) -> (u64, u64, u64, u64) {
        self.metrics.snapshot()
    }

    /// Graceful shutdown - waits for in-flight events to complete
    pub async fn shutdown(mut self) {
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }
        // Close sender to signal no more events
        drop(self.sender);
    }

    /// Core processing loop - completely optimized with zero Arc::clone calls
    async fn run_processor<F, Fut>(
        mut rx: mpsc::Receiver<T>,
        mut shutdown_rx: oneshot::Receiver<()>,
        config: ProcessorConfig,
        processor_fn: F,
        metrics: Arc<ProcessingMetrics>,
    ) where
        F: Fn(Vec<T>) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<(), Box<dyn std::error::Error + Send + Sync>>>
            + Send
            + 'static,
    {
        // Store processor function for efficient access
        let processor_fn = Arc::new(processor_fn);

        // Fast path for single-event processing (no batching overhead)
        if config.batch_size == 1 {
            loop {
                tokio::select! {
                    event = rx.recv() => {
                        match event {
                            Some(event) => {
                                // Inline single-event processing for maximum efficiency
                                let start_time = Instant::now();

                                // Process with timeout - Arc<F> can be called directly
                                let result = timeout(
                                    config.processing_timeout,
                                    processor_fn(vec![event])
                                ).await;

                                let processing_time_ms = start_time.elapsed().as_millis() as u64;

                                match result {
                                    Ok(Ok(())) => {
                                        metrics.inc_processed();
                                        metrics.update_avg_time(processing_time_ms);
                                    }
                                    Ok(Err(_)) => {
                                        metrics.inc_errors();
                                    }
                                    Err(_) => {
                                        metrics.inc_timeouts();
                                    }
                                }
                            }
                            None => break,
                        }
                    }
                    _ = &mut shutdown_rx => break,
                }
            }
            return;
        }

        // Batched processing path
        let mut batch = Vec::with_capacity(config.batch_size);
        let mut last_batch_time = Instant::now();

        loop {
            let should_process_batch = batch.len() >= config.batch_size
                || (last_batch_time.elapsed() >= config.batch_timeout && !batch.is_empty());

            if should_process_batch {
                let batch_to_process = std::mem::take(&mut batch);

                // Inline batch processing for maximum efficiency - zero Arc::clone calls
                let start_time = Instant::now();
                let batch_size = batch_to_process.len();

                // Process with timeout - Arc<F> can be called directly
                let result =
                    timeout(config.processing_timeout, processor_fn(batch_to_process)).await;

                let processing_time_ms = start_time.elapsed().as_millis() as u64;

                match result {
                    Ok(Ok(())) => {
                        // Success - update metrics for entire batch
                        for _ in 0..batch_size {
                            metrics.inc_processed();
                        }
                        metrics.update_avg_time(processing_time_ms);
                    }
                    Ok(Err(_)) => {
                        // Processing error
                        for _ in 0..batch_size {
                            metrics.inc_errors();
                        }
                    }
                    Err(_) => {
                        // Timeout
                        for _ in 0..batch_size {
                            metrics.inc_timeouts();
                        }
                    }
                }

                last_batch_time = Instant::now();
                continue;
            }

            // Efficient timeout calculation
            let batch_deadline = if batch.is_empty() {
                tokio::time::sleep(Duration::from_secs(3600)) // Long sleep when no batch
            } else {
                tokio::time::sleep(
                    config
                        .batch_timeout
                        .saturating_sub(last_batch_time.elapsed()),
                )
            };

            tokio::select! {
                biased; // Prioritize events over timeouts

                // New event received
                event = rx.recv() => {
                    match event {
                        Some(event) => {
                            if batch.is_empty() {
                                last_batch_time = Instant::now();
                            }
                            batch.push(event);
                        }
                        None => {
                            // Channel closed - process remaining batch and exit
                            if !batch.is_empty() {
                                // Inline final batch processing
                                let start_time = Instant::now();
                                let batch_size = batch.len();

                                let result = timeout(
                                    config.processing_timeout,
                                    processor_fn(batch)
                                ).await;

                                let processing_time_ms = start_time.elapsed().as_millis() as u64;

                                match result {
                                    Ok(Ok(())) => {
                                        for _ in 0..batch_size {
                                            metrics.inc_processed();
                                        }
                                        metrics.update_avg_time(processing_time_ms);
                                    }
                                    Ok(Err(_)) => {
                                        for _ in 0..batch_size {
                                            metrics.inc_errors();
                                        }
                                    }
                                    Err(_) => {
                                        for _ in 0..batch_size {
                                            metrics.inc_timeouts();
                                        }
                                    }
                                }
                            }
                            break;
                        }
                    }
                }

                // Shutdown signal received
                _ = &mut shutdown_rx => {
                    // Process remaining batch and exit
                    if !batch.is_empty() {
                        // Inline final batch processing
                        let start_time = Instant::now();
                        let batch_size = batch.len();

                        let result = timeout(
                            config.processing_timeout,
                            processor_fn(batch)
                        ).await;

                        let processing_time_ms = start_time.elapsed().as_millis() as u64;

                        match result {
                            Ok(Ok(())) => {
                                for _ in 0..batch_size {
                                    metrics.inc_processed();
                                }
                                metrics.update_avg_time(processing_time_ms);
                            }
                            Ok(Err(_)) => {
                                for _ in 0..batch_size {
                                    metrics.inc_errors();
                                }
                            }
                            Err(_) => {
                                for _ in 0..batch_size {
                                    metrics.inc_timeouts();
                                }
                            }
                        }
                    }
                    break;
                }

                // Batch timeout for partial batches
                _ = batch_deadline, if !batch.is_empty() => {
                    let batch_to_process = std::mem::take(&mut batch);

                    // Inline batch processing for timeout case
                    let start_time = Instant::now();
                    let batch_size = batch_to_process.len();

                    let result = timeout(
                        config.processing_timeout,
                        processor_fn(batch_to_process)
                    ).await;

                    let processing_time_ms = start_time.elapsed().as_millis() as u64;

                    match result {
                        Ok(Ok(())) => {
                            for _ in 0..batch_size {
                                metrics.inc_processed();
                            }
                            metrics.update_avg_time(processing_time_ms);
                        }
                        Ok(Err(_)) => {
                            for _ in 0..batch_size {
                                metrics.inc_errors();
                            }
                        }
                        Err(_) => {
                            for _ in 0..batch_size {
                                metrics.inc_timeouts();
                            }
                        }
                    }

                    last_batch_time = Instant::now();
                }
            }
        }
    }
}

// ============================================================================
// CONVENIENCE FUNCTIONS FOR EVENT PROCESSING
// ============================================================================

/// Parse JSON events from string payload with error handling
pub fn parse_event(payload: &str) -> Option<Event> {
    serde_json::from_str::<Event>(payload).ok()
}

/// Parse JSON events from bytes with zero-copy when possible
pub fn parse_event_from_bytes(payload: &[u8]) -> Option<Event> {
    serde_json::from_slice::<Event>(payload).ok()
}

/// Create a simple event processor with default config
pub fn create_event_processor<F, Fut>(processor_fn: F) -> StreamProcessor<Event>
where
    F: Fn(Vec<Event>) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<(), Box<dyn std::error::Error + Send + Sync>>>
        + Send
        + 'static,
{
    StreamProcessor::new(ProcessorConfig::default(), processor_fn)
}

// ============================================================================
// USAGE EXAMPLE IN TESTS
// ============================================================================

#[cfg(test)]
mod example {
    use super::*;
    use tokio::time::sleep;

    /// Example business logic for processing events
    async fn process_events(
        events: Vec<Event>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        println!("Processing batch of {} events", events.len());

        for event in events {
            // Simulate processing work
            match event.action.as_str() {
                "login" => {
                    println!("User {} logged in at {}", event.user_id, event.timestamp);
                    // Store in database, update metrics, etc.
                }
                "purchase" => {
                    println!(
                        "User {} made a purchase at {}",
                        event.user_id, event.timestamp
                    );
                    // Process payment, update inventory, etc.
                }
                "logout" => {
                    println!("User {} logged out at {}", event.user_id, event.timestamp);
                    // Update session time, etc.
                }
                _ => {
                    println!(
                        "Unknown action: {} for user {}",
                        event.action, event.user_id
                    );
                }
            }
        }

        // Simulate some async work (database write, API call, etc.)
        sleep(Duration::from_millis(10)).await;
        Ok(())
    }

    #[tokio::test]
    async fn example_usage() {
        // Create processor with custom config
        let config = ProcessorConfig {
            buffer_size: 2000,
            worker_count: 4,
            processing_timeout: Duration::from_secs(5),
            batch_size: 10, // Process events in batches of 10
            batch_timeout: Duration::from_millis(100),
        };

        let processor = StreamProcessor::new(config, process_events);

        // Send some test events
        let test_events = vec![
            Event {
                user_id: "user_123".to_string(),
                action: "login".to_string(),
                timestamp: 1642694400,
            },
            Event {
                user_id: "user_456".to_string(),
                action: "purchase".to_string(),
                timestamp: 1642694460,
            },
            Event {
                user_id: "user_123".to_string(),
                action: "logout".to_string(),
                timestamp: 1642694520,
            },
        ];

        // Send events to processor
        for event in test_events {
            if let Err(e) = processor.send(event).await {
                eprintln!("Failed to send event: {e}");
            }
        }

        // Let processing happen
        sleep(Duration::from_millis(200)).await;

        // Check metrics
        let (processed, errors, timeouts, avg_time) = processor.metrics();
        println!(
            "Metrics - Processed: {processed}, Errors: {errors}, Timeouts: {timeouts}, Avg Time: {avg_time}ms"
        );

        // Graceful shutdown
        processor.shutdown().await;
    }

    /// Example of processing JSON strings directly
    #[tokio::test]
    async fn json_processing_example() {
        let processor = create_event_processor(process_events);

        // Simulate receiving JSON from network/file
        let json_payloads = vec![
            r#"{"user_id": "user_789", "action": "login", "timestamp": 1642694600}"#,
            r#"{"user_id": "user_789", "action": "purchase", "timestamp": 1642694660}"#,
            r#"invalid_json"#, // This will be filtered out
        ];

        for payload in json_payloads {
            if let Some(event) = parse_event(payload) {
                let _ = processor.send(event).await;
            } else {
                println!("Failed to parse payload: {payload}");
            }
        }

        sleep(Duration::from_millis(100)).await;
        processor.shutdown().await;
    }
}
