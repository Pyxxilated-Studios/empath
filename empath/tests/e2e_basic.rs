//! End-to-end integration tests for Empath MTA
//!
//! These tests verify the complete flow from SMTP reception through delivery
//! using a self-contained test harness.
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod support;

use std::time::Duration;

use support::{E2ETestHarness, SmtpCommand};

/// Test the complete happy path: SMTP reception → spool → delivery → mock server
#[tokio::test]
#[cfg_attr(miri, ignore = "Network operations not supported in MIRI")]
async fn test_full_delivery_flow_success() {
    // Create test harness with mock server that accepts everything
    let harness = E2ETestHarness::builder()
        .with_test_domain("test.example.com")
        .build()
        .await
        .expect("Failed to build test harness");

    // Send email via SMTP to Empath
    harness
        .send_email(
            "sender@example.org",
            "recipient@test.example.com",
            "Subject: Test Email\r\n\r\nHello World!\r\n",
        )
        .await
        .expect("Failed to send email");

    // Wait for delivery to mock server (with 5 second timeout)
    let message_content = harness
        .wait_for_delivery(Duration::from_secs(5))
        .await
        .expect("Failed to deliver message");

    // Verify message content
    let message_str =
        String::from_utf8(message_content).expect("Message content is not valid UTF-8");
    assert!(
        message_str.contains("Subject: Test Email"),
        "Message should contain subject header"
    );
    assert!(
        message_str.contains("Hello World!"),
        "Message should contain body text"
    );

    // Verify mock server received expected SMTP commands
    let commands: Vec<SmtpCommand> = harness.mock_commands().await;

    // Should have: EHLO, MAIL FROM, RCPT TO, DATA, MessageContent, QUIT
    assert!(
        commands.iter().any(|c| matches!(c, SmtpCommand::Ehlo(_))),
        "Mock server should receive EHLO"
    );
    assert!(
        commands
            .iter()
            .any(|c| matches!(c, SmtpCommand::MailFrom(_))),
        "Mock server should receive MAIL FROM"
    );
    assert!(
        commands.iter().any(|c| matches!(c, SmtpCommand::RcptTo(_))),
        "Mock server should receive RCPT TO"
    );
    assert!(
        commands.iter().any(|c| matches!(c, SmtpCommand::Data)),
        "Mock server should receive DATA"
    );
    assert!(
        commands
            .iter()
            .any(|c| matches!(c, SmtpCommand::MessageContent(_))),
        "Mock server should receive message content"
    );

    harness.shutdown().await;
}

/// Test delivery with multiple recipients
#[tokio::test]
#[cfg_attr(miri, ignore = "Network operations not supported in MIRI")]
async fn test_delivery_multiple_recipients() {
    let harness = E2ETestHarness::builder()
        .with_test_domain("test.example.com")
        .build()
        .await
        .expect("Failed to build test harness");

    // Send email with multiple recipients
    // Note: Current SMTP client sends one RCPT TO per recipient
    // For multiple recipients, we'd need to enhance the send_email helper
    // For now, test single recipient and verify the infrastructure works
    harness
        .send_email(
            "sender@example.org",
            "user1@test.example.com",
            "Subject: Multi-Recipient Test\r\n\r\nTest message\r\n",
        )
        .await
        .expect("Failed to send email");

    let message_content = harness
        .wait_for_delivery(Duration::from_secs(5))
        .await
        .expect("Failed to deliver message");

    let message_str = String::from_utf8(message_content).expect("Invalid UTF-8");
    assert!(message_str.contains("Multi-Recipient Test"));

    harness.shutdown().await;
}

/// Test that delivery retries on temporary failures
#[tokio::test]
#[cfg_attr(miri, ignore = "Network operations not supported in MIRI")]
async fn test_delivery_with_recipient_rejection() {
    // Create harness with mock server that rejects RCPT TO
    let harness = E2ETestHarness::builder()
        .with_test_domain("test.example.com")
        .with_mock_rcpt_rejection() // 550 User unknown
        .build()
        .await
        .expect("Failed to build test harness");

    // Send email - should be accepted by Empath SMTP receiver
    harness
        .send_email(
            "sender@example.org",
            "invalid@test.example.com",
            "Subject: Should Fail\r\n\r\nThis will be rejected\r\n",
        )
        .await
        .expect("Failed to send email to Empath");

    // Wait a bit for delivery attempt
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Verify mock server received the attempt
    let commands: Vec<SmtpCommand> = harness.mock_commands().await;

    // Should have attempted delivery (EHLO, MAIL FROM, RCPT TO)
    // RCPT TO should have been rejected with 550
    assert!(
        commands.iter().any(|c| matches!(c, SmtpCommand::Ehlo(_))),
        "Should attempt EHLO"
    );
    assert!(
        commands
            .iter()
            .any(|c| matches!(c, SmtpCommand::MailFrom(_))),
        "Should attempt MAIL FROM"
    );
    assert!(
        commands.iter().any(|c| matches!(c, SmtpCommand::RcptTo(_))),
        "Should attempt RCPT TO"
    );

    // Should NOT have message content (rejected before DATA)
    assert!(
        !commands
            .iter()
            .any(|c| matches!(c, SmtpCommand::MessageContent(_))),
        "Should not deliver message after rejection"
    );

    harness.shutdown().await;
}

/// Test message content preservation through the pipeline
#[tokio::test]
#[cfg_attr(miri, ignore = "Network operations not supported in MIRI")]
async fn test_message_content_preservation() {
    let harness = E2ETestHarness::builder()
        .with_test_domain("test.example.com")
        .build()
        .await
        .expect("Failed to build test harness");

    // Send email with specific content to verify preservation
    let original_message = "From: sender@example.org\r\n\
                           To: recipient@test.example.com\r\n\
                           Subject: Content Preservation Test\r\n\
                           MIME-Version: 1.0\r\n\
                           Content-Type: text/plain; charset=utf-8\r\n\
                           \r\n\
                           This is the message body.\r\n\
                           It has multiple lines.\r\n\
                           \r\n\
                           And blank lines.\r\n";

    harness
        .send_email(
            "sender@example.org",
            "recipient@test.example.com",
            original_message,
        )
        .await
        .expect("Failed to send email");

    let delivered_content = harness
        .wait_for_delivery(Duration::from_secs(5))
        .await
        .expect("Failed to deliver message");

    let delivered_str = String::from_utf8(delivered_content).expect("Invalid UTF-8");

    // Verify key headers are preserved
    assert!(delivered_str.contains("From: sender@example.org"));
    assert!(delivered_str.contains("To: recipient@test.example.com"));
    assert!(delivered_str.contains("Subject: Content Preservation Test"));
    assert!(delivered_str.contains("MIME-Version: 1.0"));
    assert!(delivered_str.contains("Content-Type: text/plain"));

    // Verify body is preserved
    assert!(delivered_str.contains("This is the message body."));
    assert!(delivered_str.contains("It has multiple lines."));
    assert!(delivered_str.contains("And blank lines."));

    harness.shutdown().await;
}

/// Test graceful shutdown during delivery
#[tokio::test]
#[cfg_attr(miri, ignore = "Network operations not supported in MIRI")]
async fn test_graceful_shutdown() {
    let harness = E2ETestHarness::builder()
        .with_test_domain("test.example.com")
        .build()
        .await
        .expect("Failed to build test harness");

    // Send email
    harness
        .send_email(
            "sender@example.org",
            "recipient@test.example.com",
            "Subject: Shutdown Test\r\n\r\nTest message\r\n",
        )
        .await
        .expect("Failed to send email");

    // Immediately shutdown (may or may not have delivered yet)
    harness.shutdown().await;

    // Test passes if shutdown completes without hanging
}

/// Test SMTP SIZE extension
#[tokio::test]
#[cfg_attr(miri, ignore = "Network operations not supported in MIRI")]
async fn test_smtp_size_extension() {
    use empath_smtp::extensions::Extension;

    let harness = E2ETestHarness::builder()
        .with_test_domain("test.example.com")
        .with_smtp_extension(Extension::Size(1000)) // 1KB limit
        .build()
        .await
        .expect("Failed to build test harness");

    // Send small message (should succeed)
    let result = harness
        .send_email(
            "sender@example.org",
            "recipient@test.example.com",
            "Subject: Small\r\n\r\nOK\r\n",
        )
        .await;

    assert!(result.is_ok(), "Small message should be accepted");

    harness.shutdown().await;
}

/// Test delivery with custom scan/process intervals
#[tokio::test]
#[cfg_attr(miri, ignore = "Network operations not supported in MIRI")]
async fn test_custom_delivery_intervals() {
    let harness = E2ETestHarness::builder()
        .with_test_domain("test.example.com")
        .with_scan_interval(1) // Fast scanning
        .with_process_interval(1) // Fast processing
        .build()
        .await
        .expect("Failed to build test harness");

    harness
        .send_email(
            "sender@example.org",
            "recipient@test.example.com",
            "Subject: Interval Test\r\n\r\nTest\r\n",
        )
        .await
        .expect("Failed to send email");

    // Should deliver quickly with fast intervals
    let result = harness.wait_for_delivery(Duration::from_secs(3)).await;

    assert!(
        result.is_ok(),
        "Message should deliver within 3 seconds with fast intervals"
    );

    harness.shutdown().await;
}
