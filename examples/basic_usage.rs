// Basic usage example for the stream processing library

use serde::{Deserialize, Serialize};
use std::time::Duration;
use tokio::time::sleep;

// Import stream processing library
use stream_processing::*;

// Custom event type for advanced examples
#[derive(Debug, Clone, Deserialize, Serialize)]
struct UserEvent {
    user_id: String,
    action: String,
    timestamp: u64,
    data: Option<String>,
}

impl UserEvent {
    fn new(user_id: &str, action: &str) -> Self {
        Self {
            user_id: user_id.to_string(),
            action: action.to_string(),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            data: None,
        }
    }

    fn with_data(mut self, data: &str) -> Self {
        self.data = Some(data.to_string());
        self
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("🚀 Stream Processing - Basic Usage Example");
    println!("==========================================\n");

    // Example 1: Using built-in Event type (simplest)
    builtin_event_example().await?;

    // Example 2: Simple custom event processing
    simple_processing_example().await?;

    // Example 3: Batched processing
    batched_processing_example().await?;

    // Example 4: JSON string processing
    json_processing_example().await?;

    // Example 5: Error handling
    error_handling_example().await?;

    // Example 6: Metrics monitoring
    metrics_example().await?;

    println!("✅ All examples completed successfully!");
    Ok(())
}

// Example 1: Using the built-in Event type (simplest approach)
async fn builtin_event_example() -> Result<(), Box<dyn std::error::Error>> {
    println!("🎯 Example 1: Built-in Event Type (Simplest)");

    // Use the convenience function for built-in Event type
    let processor = create_event_processor(|events: Vec<Event>| async move {
        for event in events {
            println!(
                "  Processing: User {} performed '{}'",
                event.user_id, event.action
            );

            match event.action.as_str() {
                "login" => println!("    ✅ Login processed"),
                "purchase" => println!("    💰 Purchase processed"),
                "logout" => println!("    👋 Logout processed"),
                _ => println!("    📊 Event logged"),
            }
        }
        Ok(())
    });

    // Create and send built-in Event types
    let event1 = Event {
        user_id: "alice".to_string(),
        action: "login".to_string(),
        timestamp: 1642694400,
    };

    let event2 = Event {
        user_id: "bob".to_string(),
        action: "purchase".to_string(),
        timestamp: 1642694460,
    };

    processor.send(event1).await?;
    processor.send(event2).await?;

    // Wait a moment for processing
    sleep(Duration::from_millis(100)).await;

    processor.shutdown().await;
    println!("  ✅ Built-in event processing complete\n");
    Ok(())
}

// Example 2: Process custom events one by one
async fn simple_processing_example() -> Result<(), Box<dyn std::error::Error>> {
    println!("📝 Example 2: Custom Event Processing");

    // For custom types, use StreamProcessor::new with default config
    let processor = StreamProcessor::new(
        ProcessorConfig::default(),
        |events: Vec<UserEvent>| async move {
            for event in events {
                println!(
                    "  Processing: User {} performed '{}'",
                    event.user_id, event.action
                );

                // Simulate some work
                match event.action.as_str() {
                    "login" => println!("    ✅ Login processed"),
                    "purchase" => {
                        if let Some(data) = &event.data {
                            println!("    💰 Purchase processed: {data}");
                        } else {
                            println!("    💰 Purchase processed");
                        }
                    }
                    "logout" => println!("    👋 Logout processed"),
                    _ => println!("    📊 Event logged"),
                }
            }
            Ok(())
        },
    );

    // Send some custom events
    processor.send(UserEvent::new("alice", "login")).await?;
    processor
        .send(UserEvent::new("bob", "purchase").with_data("product_123"))
        .await?;
    processor.send(UserEvent::new("alice", "logout")).await?;

    // Wait a moment for processing
    sleep(Duration::from_millis(100)).await;

    // Graceful shutdown
    processor.shutdown().await;
    println!("  ✅ Custom event processing complete\n");
    Ok(())
}

// Example 3: Batch processing for efficiency
async fn batched_processing_example() -> Result<(), Box<dyn std::error::Error>> {
    println!("📦 Example 3: Batched Processing");

    // Configure for batch processing
    let config = ProcessorConfig {
        batch_size: 5,                             // Process 5 events at once
        batch_timeout: Duration::from_millis(100), // Or wait max 100ms
        buffer_size: 1000,
        ..Default::default()
    };

    let processor = StreamProcessor::new(config, |events: Vec<UserEvent>| async move {
        println!("  📦 Processing batch of {} events:", events.len());

        // Process entire batch
        for event in &events {
            println!("    - {} {}", event.user_id, event.action);
        }

        // Simulate batch database insert
        println!("    💾 Batch saved to database");

        Ok(())
    });

    // Send multiple events
    for i in 1..=12 {
        let event = UserEvent::new(&format!("user_{i}"), "page_view");
        processor.send(event).await?;
    }

    // Wait for processing
    sleep(Duration::from_millis(300)).await;

    processor.shutdown().await;
    println!("  ✅ Batched processing complete\n");
    Ok(())
}

// Example 4: Processing JSON strings (using built-in Event type)
async fn json_processing_example() -> Result<(), Box<dyn std::error::Error>> {
    println!("🔧 Example 4: JSON Processing");

    // Use the convenience function for JSON processing with built-in Event type
    let processor = create_event_processor(|events: Vec<Event>| async move {
        for event in events {
            // Convert back to JSON for storage/forwarding
            let json = serde_json::to_string(&event).unwrap();
            println!("  📄 JSON: {json}");
        }
        Ok(())
    });

    // Sample JSON payloads (like from webhook, Kafka, etc.)
    let json_payloads = vec![
        r#"{"user_id": "webhook_user", "action": "signup", "timestamp": 1642694400}"#,
        r#"{"user_id": "api_user", "action": "api_call", "timestamp": 1642694460}"#,
        r#"invalid_json_here"#, // This will be filtered out
    ];

    let mut processed_count = 0;
    for payload in json_payloads {
        if let Some(event) = parse_event(payload) {
            processor.send(event).await?;
            processed_count += 1;
        } else {
            println!("  ⚠️  Failed to parse: {payload}");
        }
    }

    sleep(Duration::from_millis(100)).await;

    processor.shutdown().await;
    println!("  ✅ Processed {processed_count} valid JSON events\n");
    Ok(())
}

// Example 5: Error handling
async fn error_handling_example() -> Result<(), Box<dyn std::error::Error>> {
    println!("⚠️  Example 5: Error Handling");

    let processor = StreamProcessor::new(
        ProcessorConfig::default(),
        |events: Vec<UserEvent>| async move {
            for event in events {
                // Simulate validation
                if event.user_id.is_empty() {
                    return Err("Empty user_id not allowed".into());
                }

                if event.action == "forbidden_action" {
                    return Err("Forbidden action attempted".into());
                }

                println!("  ✅ Valid event: {} {}", event.user_id, event.action);
            }
            Ok(())
        },
    );

    // Send mix of valid and invalid events
    processor
        .send(UserEvent::new("valid_user", "login"))
        .await?;
    processor.send(UserEvent::new("", "invalid")).await?; // Will cause error
    processor
        .send(UserEvent::new("another_user", "forbidden_action"))
        .await?; // Will cause error
    processor
        .send(UserEvent::new("good_user", "purchase"))
        .await?;

    sleep(Duration::from_millis(200)).await;

    // Check metrics to see errors
    let (processed, errors, timeouts, _avg_time) = processor.metrics();
    println!("  📊 Results: {processed} processed, {errors} errors, {timeouts} timeouts");

    processor.shutdown().await;
    println!("  ✅ Error handling example complete\n");
    Ok(())
}

// Example 6: Monitoring with metrics
async fn metrics_example() -> Result<(), Box<dyn std::error::Error>> {
    println!("📊 Example 6: Metrics Monitoring");

    let config = ProcessorConfig {
        batch_size: 10,
        processing_timeout: Duration::from_millis(500),
        ..Default::default()
    };

    let processor = StreamProcessor::new(config, |events: Vec<UserEvent>| async move {
        // Simulate variable processing time
        let delay = std::cmp::min(events.len() * 10, 100);
        sleep(Duration::from_millis(delay as u64)).await;

        println!("  ⚡ Processed {} events in {}ms", events.len(), delay);
        Ok(())
    });

    println!("  📈 Sending events and monitoring metrics...");

    // Send events in batches and monitor
    for batch in 0..5 {
        // Send a batch of events
        for i in 0..15 {
            let event = UserEvent::new(&format!("user_{}", batch * 15 + i), "action");
            processor.send(event).await?;
        }

        // Check metrics periodically
        sleep(Duration::from_millis(200)).await;
        let (processed, _errors, _timeouts, avg_time) = processor.metrics();
        println!(
            "  📊 Batch {}: {} processed, avg {}ms",
            batch + 1,
            processed,
            avg_time
        );
    }

    // Final metrics
    let (total_processed, total_errors, total_timeouts, final_avg) = processor.metrics();
    println!("  🏁 Final: {total_processed} events processed");
    println!("     Errors: {total_errors}, Timeouts: {total_timeouts}");
    println!("     Average processing time: {final_avg}ms");

    processor.shutdown().await;
    println!("  ✅ Metrics example complete\n");
    Ok(())
}

// Helper function: Create sample events for testing
#[allow(dead_code)]
fn create_sample_events(count: usize) -> Vec<UserEvent> {
    let actions = ["login", "logout", "purchase", "view", "click"];
    (0..count)
        .map(|i| UserEvent::new(&format!("user_{}", i % 100), actions[i % actions.len()]))
        .collect()
}

// Helper function: Simulate realistic processing work
#[allow(dead_code)]
async fn simulate_database_write(event: &UserEvent) {
    // Simulate database latency
    sleep(Duration::from_millis(1)).await;
    println!("    💾 Saved {} to database", event.user_id);
}

// Helper function: Simulate API call
#[allow(dead_code)]
async fn simulate_api_call(events: &[UserEvent]) -> Result<(), Box<dyn std::error::Error>> {
    sleep(Duration::from_millis(10)).await;
    println!("    🌐 Sent {} events to external API", events.len());
    Ok(())
}
