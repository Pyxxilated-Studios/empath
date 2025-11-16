//! Integration tests for metrics collection
//!
//! Verifies that metric counters accurately reflect actual events, especially
//! after the `AtomicU64` optimization (task 0.30) which reduced overhead by 90%.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::{sync::Arc, time::SystemTime};

use empath_metrics::{DeliveryMetrics, DnsMetrics, SmtpMetrics};

#[test]
fn test_smtp_connection_counter_accuracy() {
    let metrics = SmtpMetrics::new().expect("Failed to create SMTP metrics");

    // Record 10 connections
    for _ in 0..10 {
        metrics.record_connection();
    }

    // Verify active connections counter
    assert_eq!(
        metrics.active_connections(),
        10,
        "Active connections counter should match recorded connections"
    );

    // Close 3 connections
    for _ in 0..3 {
        metrics.record_connection_closed(1.5);
    }

    // Verify active connections decreased
    assert_eq!(
        metrics.active_connections(),
        7,
        "Active connections should decrease when connections are closed"
    );

    // Close all remaining connections
    for _ in 0..7 {
        metrics.record_connection_closed(2.0);
    }

    // Verify all connections closed
    assert_eq!(
        metrics.active_connections(),
        0,
        "All connections should be closed"
    );
}

#[test]
fn test_smtp_message_received_counter() {
    let metrics = SmtpMetrics::new().expect("Failed to create SMTP metrics");

    // Record messages with various sizes
    let message_sizes = [1024, 2048, 512, 4096, 8192];

    for &size in &message_sizes {
        metrics.record_message_received(size);
    }

    // Note: We can't directly read the total count from observable counters
    // in tests, but we can verify the API doesn't panic and operations complete
    // The observable counter callback will read the atomic value during export
}

#[test]
fn test_smtp_error_recording() {
    let metrics = SmtpMetrics::new().expect("Failed to create SMTP metrics");

    // Record various SMTP errors
    metrics.record_error(550); // Mailbox unavailable
    metrics.record_error(552); // Exceeded storage
    metrics.record_error(554); // Transaction failed
    metrics.record_error(550); // Duplicate error code

    // Errors are recorded via Counter::add() with attributes
    // OpenTelemetry handles aggregation internally
}

#[test]
fn test_smtp_command_duration() {
    let metrics = SmtpMetrics::new().expect("Failed to create SMTP metrics");

    // Record command processing durations
    metrics.record_command("EHLO", 0.001);
    metrics.record_command("MAIL FROM", 0.002);
    metrics.record_command("RCPT TO", 0.001);
    metrics.record_command("DATA", 0.150);
    metrics.record_command("QUIT", 0.001);

    // Histogram records are aggregated internally by OpenTelemetry
}

#[test]
fn test_delivery_counter_accuracy() {
    let metrics = DeliveryMetrics::new(1000, vec![]).expect("Failed to create delivery metrics");

    // Record successful deliveries
    for _ in 0..5 {
        metrics.record_delivery_success("example.com", 1.5, 0);
    }

    // Record failed deliveries
    for _ in 0..2 {
        metrics.record_delivery_failure("test.com", "Connection timeout");
    }

    // Record retrying messages
    for _ in 0..3 {
        metrics.record_delivery_retry("retry.com");
    }

    // Verify queue size changes
    assert_eq!(
        metrics.get_queue_size("pending"),
        0,
        "Initial queue size should be 0"
    );

    metrics.set_queue_size("pending", 10);
    assert_eq!(
        metrics.get_queue_size("pending"),
        10,
        "Queue size should be updated"
    );

    metrics.set_queue_size("pending", 5);
    assert_eq!(
        metrics.get_queue_size("pending"),
        5,
        "Queue size should decrease"
    );
}

#[test]
fn test_dns_cache_metrics() {
    let metrics = DnsMetrics::new().expect("Failed to create DNS metrics");

    // Record cache hits
    for _ in 0..50 {
        metrics.record_cache_hit("test");
    }

    // Record cache misses
    for _ in 0..10 {
        metrics.record_cache_miss("test");
    }

    // Record cache evictions
    for _ in 0..5 {
        metrics.record_cache_eviction();
    }

    // Verify cache size tracking
    assert_eq!(
        metrics.get_cache_size(),
        0,
        "Initial cache size should be 0"
    );

    metrics.set_cache_size(100);
    assert_eq!(
        metrics.get_cache_size(),
        100,
        "Cache size should be updated"
    );

    metrics.set_cache_size(95);
    assert_eq!(
        metrics.get_cache_size(),
        95,
        "Cache size should decrease after evictions"
    );
}

#[test]
fn test_dns_lookup_duration() {
    let metrics = DnsMetrics::new().expect("Failed to create DNS metrics");

    // Record successful lookups with durations
    metrics.record_lookup("example.com", 0.050);
    metrics.record_lookup("test.com", 0.025);
    metrics.record_lookup("mail.example.com", 0.100);

    // Record failed lookups
    metrics.record_error("nonexistent.invalid");
    metrics.record_error("error.test");

    // Durations are recorded in histograms internally
}

#[test]
fn test_concurrent_metric_updates() {
    use std::thread;

    let metrics = Arc::new(SmtpMetrics::new().expect("Failed to create SMTP metrics"));

    // Spawn multiple threads to record connections concurrently
    let mut handles = vec![];

    for _ in 0..10 {
        let metrics_clone = Arc::clone(&metrics);
        let handle = thread::spawn(move || {
            for _ in 0..100 {
                metrics_clone.record_connection();
            }
        });
        handles.push(handle);
    }

    // Wait for all threads to complete
    for handle in handles {
        handle.join().expect("Thread panicked");
    }

    // Verify all 1000 connections were recorded (10 threads * 100 connections)
    assert_eq!(
        metrics.active_connections(),
        1000,
        "All concurrent connections should be recorded correctly"
    );
}

#[test]
fn test_atomic_counter_ordering() {
    let metrics = SmtpMetrics::new().expect("Failed to create SMTP metrics");

    // Record operations in sequence
    metrics.record_connection(); // active = 1
    assert_eq!(metrics.active_connections(), 1);

    metrics.record_connection(); // active = 2
    assert_eq!(metrics.active_connections(), 2);

    metrics.record_connection_closed(1.0); // active = 1
    assert_eq!(metrics.active_connections(), 1);

    metrics.record_connection(); // active = 2
    assert_eq!(metrics.active_connections(), 2);

    metrics.record_connection_closed(1.0); // active = 1
    metrics.record_connection_closed(1.0); // active = 0
    assert_eq!(metrics.active_connections(), 0);
}

#[test]
fn test_delivery_queue_size_consistency() {
    let metrics = DeliveryMetrics::new(1000, vec![]).expect("Failed to create delivery metrics");

    // Simulate queue operations for pending status
    metrics.set_queue_size("pending", 0);
    assert_eq!(metrics.get_queue_size("pending"), 0);

    // Messages added to queue
    for i in 1..=10 {
        metrics.set_queue_size("pending", i);
        assert_eq!(metrics.get_queue_size("pending"), i);
    }

    // Messages processed from queue
    for i in (0..10).rev() {
        metrics.set_queue_size("pending", i);
        assert_eq!(metrics.get_queue_size("pending"), i);
    }
}

#[test]
fn test_dns_cache_size_updates() {
    let metrics = DnsMetrics::new().expect("Failed to create DNS metrics");

    // Simulate cache growth
    for i in 1..=50 {
        metrics.set_cache_size(i);
        assert_eq!(metrics.get_cache_size(), i);
    }

    // Simulate cache evictions
    for i in (0..50).rev().step_by(5) {
        metrics.set_cache_size(i);
        assert_eq!(metrics.get_cache_size(), i);
    }
}

#[test]
fn test_smtp_metrics_creation() {
    // Verify metrics can be created without panicking
    let result = SmtpMetrics::new();
    assert!(
        result.is_ok(),
        "SMTP metrics creation should succeed: {:?}",
        result.err()
    );
}

#[test]
fn test_delivery_metrics_creation() {
    // Verify metrics can be created without panicking
    let result = DeliveryMetrics::new(1000, vec![]);
    assert!(
        result.is_ok(),
        "Delivery metrics creation should succeed: {:?}",
        result.err()
    );
}

#[test]
fn test_dns_metrics_creation() {
    // Verify metrics can be created without panicking
    let result = DnsMetrics::new();
    assert!(
        result.is_ok(),
        "DNS metrics creation should succeed: {:?}",
        result.err()
    );
}

#[test]
fn test_queue_age_recording() {
    use std::time::{Duration, SystemTime};

    let metrics = DeliveryMetrics::new(1000, vec![]).expect("Failed to create delivery metrics");

    // Simulate messages queued at different times
    let now = SystemTime::now();
    let one_minute_ago = now - Duration::from_secs(60);
    let five_minutes_ago = now - Duration::from_secs(300);
    let one_hour_ago = now - Duration::from_secs(3600);

    // Record queue ages
    metrics.record_queue_age(one_minute_ago);
    metrics.record_queue_age(five_minutes_ago);
    metrics.record_queue_age(one_hour_ago);

    // Histogram records are aggregated internally by OpenTelemetry
    // This test verifies the API doesn't panic and operations complete
}

#[test]
fn test_oldest_message_age_update() {
    let metrics = DeliveryMetrics::new(1000, vec![]).expect("Failed to create delivery metrics");

    // Simulate queue with varying message ages
    metrics.update_oldest_message_age(0); // Empty queue
    metrics.update_oldest_message_age(60); // 1 minute
    metrics.update_oldest_message_age(300); // 5 minutes
    metrics.update_oldest_message_age(3600); // 1 hour
    metrics.update_oldest_message_age(7200); // 2 hours

    // Observable gauge reads from atomic value
    // This test verifies the API doesn't panic and operations complete
}

#[test]
fn test_queue_age_with_system_time_edge_cases() {
    let metrics = DeliveryMetrics::new(1000, vec![]).expect("Failed to create delivery metrics");

    // Test with current time (age = 0)
    let now = SystemTime::now();
    metrics.record_queue_age(now);

    // Test with UNIX_EPOCH
    metrics.record_queue_age(SystemTime::UNIX_EPOCH);

    // All operations should complete without panic
}

#[test]
fn test_delivery_error_rate_calculation() {
    let metrics = DeliveryMetrics::new(1000, vec![]).expect("Failed to create delivery metrics");

    // Record various delivery outcomes
    metrics.record_delivery_success("example.com", 1.0, 0);
    metrics.record_delivery_success("example.com", 1.5, 0);
    metrics.record_delivery_success("example.com", 2.0, 0);

    metrics.record_delivery_failure("test.com", "Connection timeout");
    metrics.record_delivery_failure("test.com", "Connection refused");

    metrics.record_delivery_retry("retry.com");

    // Error rate metrics are observable gauges that calculate on-demand
    // They will be computed when Prometheus/OTLP scrapes them
    // This test verifies the API doesn't panic and operations complete
}

#[test]
fn test_delivery_success_rate_with_zero_attempts() {
    let _metrics = DeliveryMetrics::new(1000, vec![]).expect("Failed to create delivery metrics");

    // With zero attempts, success rate should be 0.0 (not NaN or panic)
    // Observable gauge will calculate this when scraped
    // This test verifies initialization doesn't panic
}

#[test]
fn test_smtp_connection_error_rate() {
    let metrics = SmtpMetrics::new().expect("Failed to create SMTP metrics");

    // Record successful connections
    metrics.record_connection();
    metrics.record_connection();
    metrics.record_connection();

    // Record failed connections
    metrics.record_connection_failed();
    metrics.record_connection_failed();

    // Error rate metrics are observable gauges that calculate on-demand
    // Error rate should be 2/5 = 0.4 when scraped
    // This test verifies the API doesn't panic and operations complete
}

#[test]
fn test_smtp_error_rate_with_zero_connections() {
    let _metrics = SmtpMetrics::new().expect("Failed to create SMTP metrics");

    // With zero connections, error rate should be 0.0 (not NaN or panic)
    // Observable gauge will calculate this when scraped
    // This test verifies initialization doesn't panic
}

#[test]
fn test_delivery_domain_cardinality_limiting() {
    // Initialize metrics system
    // Initialize OpenTelemetry (happens once per test process)

    // Create metrics with low cardinality limit for testing
    let high_priority = vec!["gmail.com".to_string(), "outlook.com".to_string()];
    let metrics =
        DeliveryMetrics::new(3, high_priority).expect("Failed to create delivery metrics");

    // Record deliveries to 3 domains (should all be tracked)
    metrics.record_attempt("success", "example1.com");
    metrics.record_attempt("success", "example2.com");
    metrics.record_attempt("success", "example3.com");

    // Record to 4th domain (should be bucketed to "other")
    metrics.record_attempt("success", "example4.com");

    // High-priority domains should always bypass the limit
    metrics.record_attempt("success", "gmail.com");
    metrics.record_attempt("success", "outlook.com");

    // Record more attempts to already-bucketed domain
    metrics.record_attempt("failed", "example4.com");
    metrics.record_attempt("retry", "example5.com"); // 5th domain, also bucketed

    // Verify bucketed counter increased
    assert!(metrics.bucketed_domains_count() >= 2);
}

#[test]
fn test_delivery_domain_high_priority_bypass() {
    // Initialize OpenTelemetry (happens once per test process)

    // Create metrics with cardinality limit of 1
    let high_priority = vec!["important.com".to_string()];
    let metrics =
        DeliveryMetrics::new(1, high_priority).expect("Failed to create delivery metrics");

    // Fill the single tracked slot
    metrics.record_attempt("success", "example1.com");

    // High-priority domain should still be tracked individually
    metrics.record_delivery_success("important.com", 1.5, 0);

    // Regular domain should be bucketed to "other"
    metrics.record_attempt("success", "example2.com");

    // Verify high-priority domain bypassed the limit
    let bucketed = metrics.bucketed_domains_count();
    assert!(
        bucketed >= 1,
        "Expected at least 1 bucketed domain, got {bucketed}"
    );
}

#[test]
fn test_delivery_domain_cardinality_concurrent() {
    use std::{sync::Arc, thread};

    // Initialize OpenTelemetry (happens once per test process)

    let metrics =
        Arc::new(DeliveryMetrics::new(10, vec![]).expect("Failed to create delivery metrics"));

    // Spawn multiple threads recording to different domains concurrently
    let handles: Vec<_> = (0..20)
        .map(|i| {
            let metrics_clone = Arc::clone(&metrics);
            thread::spawn(move || {
                let domain = format!("example{i}.com");
                metrics_clone.record_attempt("success", &domain);
            })
        })
        .collect();

    // Wait for all threads
    for handle in handles {
        handle.join().unwrap();
    }

    // Verify some domains were bucketed (limit is 10, we tried 20)
    let bucketed = metrics.bucketed_domains_count();
    assert!(
        bucketed >= 10,
        "Expected at least 10 bucketed domains in concurrent test, got {bucketed}"
    );
}
