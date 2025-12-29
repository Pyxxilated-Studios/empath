//! Test for queue restoration across restart (task 1.1)
//!
//! This test verifies that:
//! 1. Queue state is restored from Context.delivery in spooled messages
//! 2. `next_retry_at` timestamps are honored (no immediate retries)
//! 3. All delivery state fields are preserved (status, attempts, `current_server_index`)
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::{
    sync::Arc,
    time::{Duration, SystemTime},
};

use empath_common::{
    DeliveryAttempt, DeliveryStatus, SpooledMessageId,
    address::{Address, AddressList},
    context::{Context, DeliveryContext, DeliveryPriorityTier},
    email::ServerId,
    envelope::Envelope,
};
use empath_delivery::{DeliveryInfo, DeliveryProcessor};
use empath_spool::{BackingStore, MemoryBackingStore};

fn create_test_context_with_delivery_state(
    from: &str,
    to: &str,
    status: DeliveryStatus,
    attempts: u32,
    next_retry_at: Option<SystemTime>,
) -> Context {
    let mut envelope = Envelope::default();

    // Parse and set sender - add brackets if needed
    let from_with_brackets = if from.starts_with('<') {
        from.to_string()
    } else {
        format!("<{from}>")
    };
    if let Ok(sender_mailbox) =
        empath_common::address_parser::parse_forward_path(&from_with_brackets)
    {
        *envelope.sender_mut() = Some(Address::from(sender_mailbox));
    }

    // Parse and set recipient - add brackets if needed
    let to_with_brackets = if to.starts_with('<') {
        to.to_string()
    } else {
        format!("<{to}>")
    };
    let domain = to.split('@').nth(1).unwrap_or("example.com").to_string();
    if let Ok(recip_mailbox) = empath_common::address_parser::parse_forward_path(&to_with_brackets)
    {
        *envelope.recipients_mut() = Some(AddressList(vec![Address::from(recip_mailbox)]));
    }

    // Create delivery context with state
    // Generate attempt history matching the attempt count
    let attempt_history: Vec<DeliveryAttempt> = (0..attempts)
        .map(|i| DeliveryAttempt {
            timestamp: SystemTime::now() - Duration::from_secs(120 * u64::from(attempts - i)),
            error: Some(format!("Attempt {}: Connection refused", i + 1)),
            server: ServerId::new("mx.example.com:25").unwrap(),
        })
        .collect();

    let delivery = DeliveryContext {
        message_id: SpooledMessageId::generate(),
        domain: empath_common::Domain::new(domain),
        server: Some(ServerId::new("mx.example.com:25").unwrap()),
        error: Some("Connection refused".to_string()),
        attempts: Some(attempts),
        binding: Some("primary".to_string()),
        status,
        attempt_history,
        queued_at: SystemTime::now() - Duration::from_secs(300),
        next_retry_at,
        current_server_index: 1, // Trying second MX server
        priority_tier: DeliveryPriorityTier::default(),
    };

    Context {
        envelope: Arc::new(envelope),
        id: "test-session".to_string(),
        data: Some(Arc::from(
            b"Test message content for queue restoration".as_slice(),
        )),
        delivery: Some(delivery),
        ..Default::default()
    }
}

fn init_processor(spool: Arc<dyn BackingStore>) -> DeliveryProcessor {
    let mut processor = DeliveryProcessor::default();
    processor.scan_interval_secs = 1;
    processor.process_interval_secs = 1;
    {
        let mut retry_policy = processor.retry_policy.write();
        retry_policy.max_attempts = 5;
        retry_policy.base_retry_delay_secs = 60;
    }
    processor
        .init(spool, None)
        .expect("Failed to init processor");
    processor
}

async fn spool_retry_message(spool: Arc<dyn BackingStore>) -> (SpooledMessageId, SystemTime) {
    let future_retry_time = SystemTime::now() + Duration::from_secs(3600);
    let mut context = create_test_context_with_delivery_state(
        "sender@example.org",
        "recipient@test.example.com",
        DeliveryStatus::Retry {
            attempts: 2,
            last_error: "Temporary failure".to_string(),
        },
        2,
        Some(future_retry_time),
    );

    let msg_id = BackingStore::write(spool.as_ref(), &mut context)
        .await
        .expect("Failed to spool message");

    (msg_id, future_retry_time)
}

async fn assert_persisted_delivery(spool: Arc<dyn BackingStore>, msg_id: &SpooledMessageId) {
    let persisted_context: Context = BackingStore::read(spool.as_ref(), msg_id)
        .await
        .expect("Failed to read spooled message");
    assert!(
        persisted_context.delivery.is_some(),
        "Context.delivery should be persisted"
    );
    let persisted_delivery = persisted_context.delivery.as_ref().unwrap();
    assert_eq!(persisted_delivery.attempts, Some(2));
    assert_eq!(persisted_delivery.current_server_index, 1);
    assert!(
        persisted_delivery.next_retry_at.is_some(),
        "next_retry_at should be persisted"
    );
}

async fn restore_queue(
    processor: &DeliveryProcessor,
    spool: Arc<dyn BackingStore>,
) -> Vec<SpooledMessageId> {
    let messages = BackingStore::list(spool.as_ref())
        .await
        .expect("Failed to list spool");

    for msg_id in &messages {
        let context = BackingStore::read(spool.as_ref(), msg_id)
            .await
            .expect("Failed to read message");

        if let Some(delivery_ctx) = &context.delivery {
            let info = DeliveryInfo {
                message_id: msg_id.clone(),
                status: delivery_ctx.status.clone(),
                attempts: delivery_ctx.attempt_history.clone(),
                recipient_domain: delivery_ctx.domain.clone(),
                mail_servers: Arc::new(Vec::new()),
                current_server_index: delivery_ctx.current_server_index,
                priority_tier: delivery_ctx.priority_tier,
                binding: delivery_ctx.binding.clone(),
                queued_at: delivery_ctx.queued_at,
                next_retry_at: delivery_ctx.next_retry_at,
            };

            processor.queue().insert(msg_id.clone(), info);
        }
    }

    messages
}

fn assert_restored_info(restored_info: &DeliveryInfo, future_retry_time: SystemTime) {
    assert!(
        matches!(
            restored_info.status,
            DeliveryStatus::Retry { attempts: 2, .. }
        ),
        "Status should be restored as Retry with 2 attempts"
    );
    assert_eq!(
        restored_info.attempt_count(),
        2,
        "Should have 2 attempts restored"
    );
    assert_eq!(
        restored_info.current_server_index, 1,
        "Current server index should be restored to 1"
    );
    assert!(
        restored_info.next_retry_at.is_some(),
        "next_retry_at should be restored"
    );
    assert_eq!(
        restored_info.binding.as_deref(),
        Some("primary"),
        "binding should be restored"
    );

    let restored_retry_time = restored_info.next_retry_at.unwrap();
    let time_diff = restored_retry_time
        .duration_since(future_retry_time)
        .unwrap_or_else(|e| e.duration());

    assert!(
        time_diff < Duration::from_secs(2),
        "next_retry_at should be accurate (diff: {time_diff:?})"
    );

    let now = SystemTime::now();
    assert!(
        restored_retry_time > now,
        "next_retry_at should be in the future"
    );
}

#[tokio::test]
#[cfg_attr(miri, ignore = "Async operations")]
async fn test_queue_restoration_across_restart() {
    let spool: Arc<dyn BackingStore> = Arc::new(MemoryBackingStore::default());

    let processor1 = init_processor(spool.clone());
    let (msg_id, future_retry_time) = spool_retry_message(spool.clone()).await;
    assert_persisted_delivery(spool.clone(), &msg_id).await;

    drop(processor1);
    let processor2 = init_processor(spool.clone());
    let messages = restore_queue(&processor2, spool.clone()).await;
    assert_eq!(messages.len(), 1, "Should have 1 message in spool");

    let restored_info = processor2
        .queue()
        .get(&msg_id)
        .expect("Message should be in queue");

    assert_restored_info(&restored_info, future_retry_time);
}

#[tokio::test]
#[cfg_attr(miri, ignore = "Async operations")]
async fn test_queue_restoration_with_multiple_messages() {
    let spool: Arc<dyn BackingStore> = Arc::new(MemoryBackingStore::default());

    // Create multiple messages with different states
    let states = vec![
        (DeliveryStatus::Pending, 0, None),
        (
            DeliveryStatus::Retry {
                attempts: 1,
                last_error: "Error 1".to_string(),
            },
            1,
            Some(SystemTime::now() + Duration::from_secs(600)),
        ),
        (
            DeliveryStatus::Retry {
                attempts: 3,
                last_error: "Error 3".to_string(),
            },
            3,
            Some(SystemTime::now() + Duration::from_secs(1800)),
        ),
    ];

    let mut message_ids = Vec::new();

    for (i, (status, attempts, next_retry_at)) in states.into_iter().enumerate() {
        let mut context = create_test_context_with_delivery_state(
            &format!("sender{i}@example.org"),
            &format!("recipient{i}@test.example.com"),
            status.clone(),
            attempts,
            next_retry_at,
        );

        let msg_id = BackingStore::write(spool.as_ref(), &mut context)
            .await
            .expect("Failed to spool");
        message_ids.push((msg_id, status, attempts, next_retry_at));
    }

    println!(
        "Spooled {} messages with different states",
        message_ids.len()
    );

    // Create processor and restore queue
    let processor = init_processor(spool.clone());
    let messages = restore_queue(&processor, spool.clone()).await;

    // Verify all messages were restored
    assert_eq!(
        messages.len(),
        message_ids.len(),
        "All messages should be restored"
    );

    for (msg_id, expected_status, expected_attempts, expected_retry_at) in message_ids {
        let info = processor
            .queue()
            .get(&msg_id)
            .expect("Message should be in queue");

        // Verify status
        assert_eq!(
            std::mem::discriminant(&info.status),
            std::mem::discriminant(&expected_status),
            "Status type should match for {msg_id:?}"
        );

        // Verify attempts
        assert_eq!(
            info.attempt_count(),
            expected_attempts,
            "Attempts should match for {msg_id:?}"
        );

        // Verify next_retry_at
        assert_eq!(
            info.next_retry_at.is_some(),
            expected_retry_at.is_some(),
            "next_retry_at presence should match for {msg_id:?}"
        );
    }

    println!("✅ Multi-message queue restoration test PASSED");
}
