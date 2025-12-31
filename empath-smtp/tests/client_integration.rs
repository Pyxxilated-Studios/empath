//! Integration tests for the SMTP client.
//!
//! These tests verify that the client can interact with a real SMTP server.
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use empath_common::traits::protocol::{Protocol, SessionHandler};
use empath_ffi::modules::ModuleRegistryBuilder;
use empath_smtp::client::{MessageBuilder, QuitAfter, SmtpClientBuilder};
use empath_smtp::extensions::Extension;
use empath_smtp::{Smtp, SmtpArgs};
use empath_spool::MemoryBackingStore;
use tokio::net::TcpListener;
use tokio::time::timeout;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// Helper function to start a test SMTP server on a random port.
async fn start_test_server() -> TestResult<(u16, tokio::task::JoinHandle<()>)> {
    // Initialize modules (adds core module to registry)
    // This only needs to be done once, subsequent calls will just add to the store
    let registry = ModuleRegistryBuilder::new().build()?;
    registry.install()?;

    // Bind to port 0 to get a random available port
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let port = addr.port();

    // Create a simple SMTP server controller
    let (shutdown_tx, _) = tokio::sync::broadcast::channel(1);

    let handle = tokio::spawn(async move {
        let smtp = Smtp;
        let args = SmtpArgs::builder()
            .with_extensions(vec![Extension::Size(10_000_000)])
            .with_spool(Arc::new(MemoryBackingStore::new()));

        while let Ok((stream, peer)) = listener.accept().await {
            let shutdown_rx = shutdown_tx.subscribe();
            let session = smtp.handle(stream, peer, HashMap::default(), args.clone());

            tokio::spawn(async move {
                let _ = timeout(Duration::from_secs(30), async {
                    SessionHandler::run(session, shutdown_rx).await
                })
                .await;
            });
        }
    });

    // Give the server a moment to start
    tokio::time::sleep(Duration::from_millis(100)).await;

    Ok((port, handle))
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_basic_connection() -> TestResult {
    let (port, _handle) = start_test_server().await?;

    let result = SmtpClientBuilder::new(format!("127.0.0.1:{port}"), "localhost".to_string())
        .accept_invalid_certs(true)
        .quit_after(QuitAfter::Connect)
        .execute()
        .await;

    let responses = result?;
    assert_eq!(responses.len(), 1);
    assert_eq!(responses[0].code, 220);
    Ok(())
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_ehlo() -> TestResult {
    let (port, _handle) = start_test_server().await?;

    let result = SmtpClientBuilder::new(format!("127.0.0.1:{port}"), "localhost".to_string())
        .accept_invalid_certs(true)
        .ehlo("test.example.com")
        .quit_after(QuitAfter::Greeting)
        .execute()
        .await;

    let responses = result?;

    // Should have greeting + EHLO response
    assert!(responses.len() >= 2);
    assert_eq!(responses[0].code, 220); // Greeting
    assert_eq!(responses[1].code, 250); // EHLO response
    Ok(())
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_helo() -> TestResult {
    let (port, _handle) = start_test_server().await?;

    let result = SmtpClientBuilder::new(format!("127.0.0.1:{port}"), "localhost".to_string())
        .accept_invalid_certs(true)
        .helo("test.example.com")
        .quit_after(QuitAfter::Greeting)
        .execute()
        .await;

    let responses = result?;

    assert!(responses.len() >= 2);
    assert_eq!(responses[0].code, 220); // Greeting
    assert_eq!(responses[1].code, 250); // HELO response
    Ok(())
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_mail_from() -> TestResult {
    let (port, _handle) = start_test_server().await?;

    let result = SmtpClientBuilder::new(format!("127.0.0.1:{port}"), "localhost".to_string())
        .accept_invalid_certs(true)
        .ehlo("test.example.com")
        .mail_from("sender@example.com")
        .quit_after(QuitAfter::MailFrom)
        .execute()
        .await;

    let responses = result?;

    // Greeting + EHLO + MAIL FROM + QUIT
    assert!(responses.len() >= 4);
    let last = responses.last().ok_or("missing final response")?;
    assert_eq!(last.code, 221); // QUIT response
    // Check that MAIL FROM succeeded (second to last)
    assert_eq!(responses[responses.len() - 2].code, 250);
    Ok(())
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_mail_from_with_size() -> TestResult {
    let (port, _handle) = start_test_server().await?;

    let result = SmtpClientBuilder::new(format!("127.0.0.1:{port}"), "localhost".to_string())
        .accept_invalid_certs(true)
        .ehlo("test.example.com")
        .mail_from_with_size("sender@example.com", 1000)
        .quit_after(QuitAfter::MailFrom)
        .execute()
        .await;

    let responses = result?;
    assert!(responses.len() >= 4);
    let last = responses.last().ok_or("missing final response")?;
    assert_eq!(last.code, 221); // QUIT response
    assert_eq!(responses[responses.len() - 2].code, 250); // MAIL FROM response
    Ok(())
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_rcpt_to() -> TestResult {
    let (port, _handle) = start_test_server().await?;

    let result = SmtpClientBuilder::new(format!("127.0.0.1:{port}"), "localhost".to_string())
        .accept_invalid_certs(true)
        .ehlo("test.example.com")
        .mail_from("sender@example.com")
        .rcpt_to("recipient@example.com")
        .quit_after(QuitAfter::RcptTo)
        .execute()
        .await;

    let responses = result?;
    assert!(responses.len() >= 5);
    let last = responses.last().ok_or("missing final response")?;
    assert_eq!(last.code, 221); // QUIT response
    assert_eq!(responses[responses.len() - 2].code, 250); // RCPT TO response
    Ok(())
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_multiple_recipients() -> TestResult {
    let (port, _handle) = start_test_server().await?;

    let recipients = [
        "user1@example.com",
        "user2@example.com",
        "user3@example.com",
    ];

    let result = SmtpClientBuilder::new(format!("127.0.0.1:{port}"), "localhost".to_string())
        .accept_invalid_certs(true)
        .ehlo("test.example.com")
        .mail_from("sender@example.com")
        .rcpt_to_multiple(&recipients)
        .quit_after(QuitAfter::RcptTo)
        .execute()
        .await;

    let responses = result?;

    // Each RCPT TO should get a 250 response
    assert!(responses.iter().filter(|r| r.code == 250).count() >= 3);
    Ok(())
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_complete_transaction() -> TestResult {
    let (port, _handle) = start_test_server().await?;

    let message = "From: sender@example.com\r\n\
                   To: recipient@example.com\r\n\
                   Subject: Test Email\r\n\
                   \r\n\
                   This is a test message.\r\n";

    let result = SmtpClientBuilder::new(format!("127.0.0.1:{port}"), "localhost".to_string())
        .accept_invalid_certs(true)
        .ehlo("test.example.com")
        .mail_from("sender@example.com")
        .rcpt_to("recipient@example.com")
        .data_with_content(message)
        .quit_after(QuitAfter::DataEnd)
        .execute()
        .await;

    let responses = result?;

    // Find the final response after data
    let data_end_response = responses.iter().rev().find(|r| r.code == 250);
    assert!(data_end_response.is_some());
    Ok(())
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_full_session_with_quit() -> TestResult {
    let (port, _handle) = start_test_server().await?;

    let message = "Subject: Test\r\n\r\nHello World\r\n";

    let result = SmtpClientBuilder::new(format!("127.0.0.1:{port}"), "localhost".to_string())
        .accept_invalid_certs(true)
        .ehlo("test.example.com")
        .mail_from("sender@example.com")
        .rcpt_to("recipient@example.com")
        .data_with_content(message)
        .execute()
        .await;

    let responses = result?;

    // Last response should be QUIT (221)
    let last = responses.last().ok_or("missing final response")?;
    assert_eq!(last.code, 221);
    Ok(())
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_rset_command() -> TestResult {
    let (port, _handle) = start_test_server().await?;

    let result = SmtpClientBuilder::new(format!("127.0.0.1:{port}"), "localhost".to_string())
        .accept_invalid_certs(true)
        .ehlo("test.example.com")
        .mail_from("sender@example.com")
        .rcpt_to("recipient@example.com")
        .rset()
        .mail_from("newsender@example.com")
        .quit_after(QuitAfter::MailFrom)
        .execute()
        .await;

    let responses = result?;

    // Should have successful responses for both MAIL FROM commands
    assert!(responses.iter().filter(|r| r.code == 250).count() >= 2);
    Ok(())
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_raw_command() -> TestResult {
    let (port, _handle) = start_test_server().await?;

    let result = SmtpClientBuilder::new(format!("127.0.0.1:{port}"), "localhost".to_string())
        .accept_invalid_certs(true)
        .ehlo("test.example.com")
        .raw_command("MAIL FROM: test@gmail.com")
        .execute()
        .await;

    let responses = result?;

    // HELP command should get a response
    assert!(responses.len() >= 3);
    Ok(())
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_builder_build_method() -> TestResult {
    let (port, _handle) = start_test_server().await?;

    let mut client = SmtpClientBuilder::new(format!("127.0.0.1:{port}"), "localhost".to_string())
        .accept_invalid_certs(true)
        .ehlo("test.example.com")
        .build()
        .await?;

    // Use the client for additional commands
    let response = client.mail_from("sender@example.com", None).await;
    let response = response?;
    assert_eq!(response.code, 250);

    let response = client.rcpt_to("recipient@example.com").await;
    let response = response?;
    assert_eq!(response.code, 250);

    let response = client.quit().await;
    let response = response?;
    assert_eq!(response.code, 221);
    Ok(())
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_response_inspection() -> TestResult {
    let (port, _handle) = start_test_server().await?;

    let client = SmtpClientBuilder::new(format!("127.0.0.1:{port}"), "localhost".to_string())
        .accept_invalid_certs(true)
        .ehlo("test.example.com")
        .build()
        .await?;

    // All responses should be accessible
    let responses = client.responses();
    assert!(responses.len() >= 2); // Greeting + EHLO

    // Last response should be the EHLO response
    let last = client.last_response().ok_or("missing last response")?;
    assert_eq!(last.code, 250);
    assert!(last.is_success());
    assert!(!last.is_error());
    Ok(())
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_size_exceeded() -> TestResult {
    let (port, _handle) = start_test_server().await?;

    // Server has 10MB limit, declare larger size
    let result = SmtpClientBuilder::new(format!("127.0.0.1:{port}"), "localhost".to_string())
        .accept_invalid_certs(true)
        .ehlo("test.example.com")
        .mail_from_with_size("sender@example.com", 20_000_000)
        .quit_after(QuitAfter::MailFrom)
        .execute()
        .await;

    let responses = result?;

    // Should get error response (552) then QUIT (221)
    let last = responses.last().ok_or("missing final response")?;
    assert_eq!(last.code, 221); // QUIT after error
    let error_response = &responses[responses.len() - 2]; // SIZE error
    assert_eq!(error_response.code, 552);
    assert!(error_response.is_permanent_error());
    Ok(())
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_message_builder_simple() -> TestResult {
    let message = MessageBuilder::new()
        .from("sender@example.com")
        .to("recipient@example.com")
        .subject("Test Message")
        .body("This is a test message.")
        .build()?;

    assert!(message.contains("From: sender@example.com"));
    assert!(message.contains("To: recipient@example.com"));
    assert!(message.contains("Subject: Test Message"));
    assert!(message.contains("This is a test message."));
    assert!(message.contains("MIME-Version: 1.0"));
    assert!(message.contains("Content-Type: text/plain"));
    Ok(())
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_message_builder_with_attachment() -> TestResult {
    let message = MessageBuilder::new()
        .from("sender@example.com")
        .to("recipient@example.com")
        .subject("File Attached")
        .body("Please find the attachment.")
        .attach("test.txt", "text/plain", b"File contents here".to_vec())
        .build()?;

    assert!(message.contains("From: sender@example.com"));
    assert!(message.contains("multipart/mixed"));
    assert!(message.contains("test.txt"));
    assert!(message.contains("base64"));
    assert!(message.contains("Content-Disposition: attachment"));
    Ok(())
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_message_builder_auto_headers() -> TestResult {
    // Test that data_with_builder auto-populates FROM/TO headers
    let message = MessageBuilder::new()
        .from("sender@example.com")
        .to_multiple(&["recipient1@example.com", "recipient2@example.com"])
        .subject("Auto Headers Test")
        .body("Testing FROM/TO headers")
        .build()?;

    // Verify FROM and TO headers are properly set
    assert!(message.contains("From: sender@example.com"));
    assert!(message.contains("To: recipient1@example.com, recipient2@example.com"));
    assert!(message.contains("Subject: Auto Headers Test"));
    Ok(())
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_send_with_message_builder() -> TestResult {
    let (port, _handle) = start_test_server().await?;

    let message = MessageBuilder::new()
        .from("sender@example.com")
        .to("recipient@example.com")
        .subject("Integration Test")
        .body("Testing MessageBuilder integration with SMTP client")
        .build()?;

    let result = SmtpClientBuilder::new(format!("127.0.0.1:{port}"), "localhost".to_string())
        .accept_invalid_certs(true)
        .ehlo("test.example.com")
        .mail_from("sender@example.com")
        .rcpt_to("recipient@example.com")
        .data_with_message(message)
        .execute()
        .await;

    let responses = result?;

    // Should have successful delivery
    let last_response = responses.last().ok_or("missing final response")?;
    assert_eq!(last_response.code, 221); // QUIT
    Ok(())
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_data_with_builder_closure() -> TestResult {
    let (port, _handle) = start_test_server().await?;

    let builder = SmtpClientBuilder::new(format!("127.0.0.1:{port}"), "localhost".to_string())
        .accept_invalid_certs(true)
        .ehlo("test.example.com")
        .mail_from("sender@example.com")
        .rcpt_to("recipient@example.com")
        .data_with_builder(|msg| {
            msg.subject("Closure Test")
                .body("Testing the ergonomic closure API")
                .build()
        })?;
    let result = builder.execute().await;

    let responses = result?;

    // Should have successful delivery
    let last_response = responses.last().ok_or("missing final response")?;
    assert_eq!(last_response.code, 221); // QUIT
    Ok(())
}
