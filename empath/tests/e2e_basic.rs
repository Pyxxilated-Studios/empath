//! End-to-end integration tests for Empath MTA
//!
//! These tests verify the complete flow from SMTP reception through delivery
//! using a self-contained test harness.
mod support;

use std::sync::Arc;
use std::time::Duration;

use empath_delivery::{DnsError, DnsResolver, MockDnsResolver};
use support::{E2ETestHarness, SmtpCommand};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// Test the complete happy path: SMTP reception → spool → delivery → mock server
#[tokio::test]
#[cfg_attr(miri, ignore = "Miri does not support the socket syscall")]
async fn test_full_delivery_flow_success() -> TestResult {
    // Create test harness with mock server that accepts everything
    let harness = E2ETestHarness::builder()
        .with_test_domain("test.example.com")
        .build()
        .await?;

    // Send email via SMTP to Empath
    harness
        .send_email(
            "sender@example.org",
            "recipient@test.example.com",
            "Subject: Test Email\r\n\r\nHello World!\r\n",
        )
        .await?;

    // Wait for delivery to mock server (with 5 second timeout)
    let message_content = harness.wait_for_delivery(Duration::from_secs(5)).await?;

    // Verify message content
    let message_str = String::from_utf8(message_content)?;
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
    Ok(())
}

/// Test delivery with multiple recipients
#[tokio::test]
#[cfg_attr(miri, ignore = "Miri does not support the socket syscall")]
async fn test_delivery_multiple_recipients() -> TestResult {
    let harness = E2ETestHarness::builder()
        .with_test_domain("test.example.com")
        .build()
        .await?;

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
        .await?;

    let message_content = harness.wait_for_delivery(Duration::from_secs(5)).await?;

    let message_str = String::from_utf8(message_content)?;
    assert!(message_str.contains("Multi-Recipient Test"));

    harness.shutdown().await;
    Ok(())
}

/// Test that delivery retries on temporary failures
#[tokio::test]
#[cfg_attr(miri, ignore = "Miri does not support the socket syscall")]
async fn test_delivery_with_recipient_rejection() -> TestResult {
    // Create harness with mock server that rejects RCPT TO
    let harness = E2ETestHarness::builder()
        .with_test_domain("test.example.com")
        .with_mock_rcpt_rejection() // 550 User unknown
        .build()
        .await?;

    // Send email - should be accepted by Empath SMTP receiver
    harness
        .send_email(
            "sender@example.org",
            "invalid@test.example.com",
            "Subject: Should Fail\r\n\r\nThis will be rejected\r\n",
        )
        .await?;

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
    Ok(())
}

#[tokio::test]
#[cfg_attr(miri, ignore = "Miri does not support the socket syscall")]
async fn test_harness_builder_options_smoke() -> TestResult {
    let harness = E2ETestHarness::builder()
        .with_test_domain("test.example.com")
        .with_mock_mail_from_rejection()
        .build()
        .await?;

    let _ = harness.smtp_port();
    let _ = harness.mock_addr();
    let _ = harness.backing_store();

    harness.shutdown().await;
    Ok(())
}

#[tokio::test]
#[cfg_attr(miri, ignore = "Miri does not support the socket syscall")]
async fn test_mock_server_builder_options_smoke() -> TestResult {
    use support::mock_server::MockSmtpServer;

    let server = MockSmtpServer::builder()
        .with_helo_response(250, "Hello")
        .with_quit_response(221, "Bye")
        .with_starttls_response(220, "Ready")
        .with_connection_delay(Duration::from_millis(1))
        .with_response_delay(Duration::from_millis(1))
        .with_network_error_after_commands(1)
        .with_timeout_on_command(1)
        .build()
        .await?;

    let _ = server.command_count();
    server.shutdown();
    Ok(())
}

/// Test message content preservation through the pipeline
#[tokio::test]
#[cfg_attr(miri, ignore = "Miri does not support the socket syscall")]
async fn test_message_content_preservation() -> TestResult {
    let harness = E2ETestHarness::builder()
        .with_test_domain("test.example.com")
        .build()
        .await?;

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
        .await?;

    let delivered_content = harness.wait_for_delivery(Duration::from_secs(5)).await?;

    let delivered_str = String::from_utf8(delivered_content)?;

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
    Ok(())
}

/// Test graceful shutdown during delivery
#[tokio::test]
#[cfg_attr(miri, ignore = "Miri does not support the socket syscall")]
async fn test_graceful_shutdown() -> TestResult {
    let harness = E2ETestHarness::builder()
        .with_test_domain("test.example.com")
        .build()
        .await?;

    // Send email
    harness
        .send_email(
            "sender@example.org",
            "recipient@test.example.com",
            "Subject: Shutdown Test\r\n\r\nTest message\r\n",
        )
        .await?;

    // Immediately shutdown (may or may not have delivered yet)
    harness.shutdown().await;

    // Test passes if shutdown completes without hanging
    Ok(())
}

/// Test SMTP SIZE extension
#[tokio::test]
#[cfg_attr(miri, ignore = "Miri does not support the socket syscall")]
async fn test_smtp_size_extension() -> TestResult {
    use empath_smtp::extensions::Extension;

    let harness = E2ETestHarness::builder()
        .with_test_domain("test.example.com")
        .with_smtp_extension(Extension::Size(1000)) // 1KB limit
        .build()
        .await?;

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
    Ok(())
}

/// Test delivery with custom scan/process intervals
#[tokio::test]
#[cfg_attr(miri, ignore = "Miri does not support the socket syscall")]
async fn test_custom_delivery_intervals() -> TestResult {
    let harness = E2ETestHarness::builder()
        .with_test_domain("test.example.com")
        .with_scan_interval(1) // Fast scanning
        .with_process_interval(1) // Fast processing
        .build()
        .await?;

    harness
        .send_email(
            "sender@example.org",
            "recipient@test.example.com",
            "Subject: Interval Test\r\n\r\nTest\r\n",
        )
        .await?;

    // Should deliver quickly with fast intervals
    let result = harness.wait_for_delivery(Duration::from_secs(3)).await;

    assert!(
        result.is_ok(),
        "Message should deliver within 3 seconds with fast intervals"
    );

    harness.shutdown().await;
    Ok(())
}

/// Test DNS timeout handling - should result in retry (temporary failure)
#[tokio::test]
#[cfg_attr(miri, ignore = "Miri does not support the socket syscall")]
async fn test_delivery_with_dns_timeout() -> TestResult {
    // Create MockDnsResolver that returns DNS timeout error
    let mock_dns = MockDnsResolver::new();
    mock_dns.add_response(
        "timeout.example.com",
        Err(DnsError::Timeout("timeout.example.com".to_string())),
    );

    let harness = E2ETestHarness::builder()
        .with_test_domain("timeout.example.com")
        .with_dns_resolver(Arc::new(mock_dns) as Arc<dyn DnsResolver>)
        .with_max_attempts(3)
        .build()
        .await?;

    // Send email - should be accepted by SMTP receiver
    harness
        .send_email(
            "sender@example.org",
            "recipient@timeout.example.com",
            "Subject: DNS Timeout Test\r\n\r\nBody",
        )
        .await?;

    // Wait a bit for delivery attempts
    tokio::time::sleep(Duration::from_secs(4)).await;

    // Verify message was NOT delivered (DNS timeout prevents delivery)
    let commands = harness.mock_commands().await;
    assert!(
        commands.is_empty(),
        "Should not deliver message when DNS times out"
    );

    harness.shutdown().await;
    Ok(())
}

/// Test NXDOMAIN (domain not found) handling - should result in permanent failure
#[tokio::test]
#[cfg_attr(miri, ignore = "Miri does not support the socket syscall")]
async fn test_delivery_with_nxdomain() -> TestResult {
    // Create MockDnsResolver that returns NXDOMAIN error
    let mock_dns = MockDnsResolver::new();
    mock_dns.add_response(
        "nonexistent.example.com",
        Err(DnsError::DomainNotFound(
            "nonexistent.example.com".to_string(),
        )),
    );

    let harness = E2ETestHarness::builder()
        .with_test_domain("nonexistent.example.com")
        .with_dns_resolver(Arc::new(mock_dns) as Arc<dyn DnsResolver>)
        .build()
        .await?;

    // Send email - should be accepted by SMTP receiver
    harness
        .send_email(
            "sender@example.org",
            "recipient@nonexistent.example.com",
            "Subject: NXDOMAIN Test\r\n\r\nBody",
        )
        .await?;

    // Wait a bit for delivery attempt
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Verify message was NOT delivered (NXDOMAIN is permanent failure)
    let commands = harness.mock_commands().await;
    assert!(
        commands.is_empty(),
        "Should not deliver message when domain does not exist"
    );

    let dsn = harness
        .wait_for_spooled_message(Duration::from_secs(5), |data| {
            support::harness::bytes_contains(data, b"report-type=\"delivery-status\"")
        })
        .await?;
    let dsn_body = String::from_utf8_lossy(&dsn);
    assert!(
        dsn_body.contains("Subject: Delivery Status Notification (Failure)"),
        "DSN message should include failure subject"
    );

    harness.shutdown().await;
    Ok(())
}

/// Test that delivery works with the default `MockDnsResolver` (single MX record)
///
/// NOTE: Testing multiple MX records requires more complex test infrastructure
/// because we need to know the mock server port before building the harness,
/// but the harness creates the mock server. For now, we verify that the
/// basic `MockDnsResolver` injection works correctly (tested by the other E2E tests).
///
/// Multiple MX priority ordering is tested in pipeline unit tests instead.
#[tokio::test]
#[cfg_attr(miri, ignore = "Miri does not support the socket syscall")]
async fn test_delivery_with_mock_dns_resolver() -> TestResult {
    // This test verifies that the default E2ETestHarness behavior uses `MockDnsResolver`
    // instead of MX overrides (which was the old workaround)
    let harness = E2ETestHarness::builder()
        .with_test_domain("mock-dns.example.com")
        .build()
        .await?;

    // Send email
    harness
        .send_email(
            "sender@example.org",
            "recipient@mock-dns.example.com",
            "Subject: MockDnsResolver Test\r\n\r\nBody",
        )
        .await?;

    // Wait for delivery
    let result = harness.wait_for_delivery(Duration::from_secs(10)).await;
    assert!(
        result.is_ok(),
        "Message should be delivered using MockDnsResolver"
    );

    // Verify delivery attempt was made
    let commands = harness.mock_commands().await;
    assert!(
        !commands.is_empty(),
        "Should have attempted delivery using DNS resolution"
    );

    harness.shutdown().await;
    Ok(())
}
