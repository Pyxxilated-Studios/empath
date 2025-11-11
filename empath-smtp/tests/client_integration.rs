//! Integration tests for the SMTP client.
//!
//! These tests verify that the client can interact with a real SMTP server.

use std::{collections::HashMap, sync::Arc, time::Duration};

use empath_common::traits::protocol::Protocol;
use empath_smtp::{
    Smtp, SmtpArgs,
    client::{MessageBuilder, QuitAfter, SmtpClientBuilder},
    extensions::Extension,
};
use tokio::{net::TcpListener, time::timeout};

/// Helper function to start a test SMTP server on a random port.
async fn start_test_server() -> (u16, tokio::task::JoinHandle<()>) {
    // Initialize modules (adds core module to MODULE_STORE)
    // This only needs to be done once, subsequent calls will just add to the store
    let _ = empath_ffi::modules::init(vec![]);

    // Bind to port 0 to get a random available port
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let port = addr.port();

    // Create a simple SMTP server controller
    let (shutdown_tx, _) = tokio::sync::broadcast::channel(1);

    let handle = tokio::spawn(async move {
        let smtp = Smtp;
        let args = SmtpArgs::builder()
            .with_extensions(vec![Extension::Size(10_000_000)])
            .with_spool(Arc::new(empath_spool::MemoryBackingStore::new()));

        while let Ok((stream, peer)) = listener.accept().await {
            let shutdown_rx = shutdown_tx.subscribe();
            let session = smtp.handle(stream, peer, HashMap::default(), args.clone());

            tokio::spawn(async move {
                let _ = timeout(Duration::from_secs(30), async {
                    empath_common::traits::protocol::SessionHandler::run(session, shutdown_rx).await
                })
                .await;
            });
        }
    });

    // Give the server a moment to start
    tokio::time::sleep(Duration::from_millis(100)).await;

    (port, handle)
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_basic_connection() {
    let (port, _handle) = start_test_server().await;

    let result = SmtpClientBuilder::new(format!("127.0.0.1:{port}"), "localhost".to_string())
        .accept_invalid_certs(true)
        .quit_after(QuitAfter::Connect)
        .execute()
        .await;

    assert!(result.is_ok());
    let responses = result.unwrap();
    assert_eq!(responses.len(), 1);
    assert_eq!(responses[0].code, 220);
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_ehlo() {
    let (port, _handle) = start_test_server().await;

    let result = SmtpClientBuilder::new(format!("127.0.0.1:{port}"), "localhost".to_string())
        .accept_invalid_certs(true)
        .ehlo("test.example.com")
        .quit_after(QuitAfter::Greeting)
        .execute()
        .await;

    assert!(result.is_ok());
    let responses = result.unwrap();
    eprintln!("{responses:?}");

    // Should have greeting + EHLO response
    assert!(responses.len() >= 2);
    assert_eq!(responses[0].code, 220); // Greeting
    assert_eq!(responses[1].code, 250); // EHLO response
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_helo() {
    let (port, _handle) = start_test_server().await;

    let result = SmtpClientBuilder::new(format!("127.0.0.1:{port}"), "localhost".to_string())
        .accept_invalid_certs(true)
        .helo("test.example.com")
        .quit_after(QuitAfter::Greeting)
        .execute()
        .await;

    assert!(result.is_ok());
    let responses = result.unwrap();

    eprintln!("{responses:?}");
    assert!(responses.len() >= 2);
    assert_eq!(responses[0].code, 220); // Greeting
    assert_eq!(responses[1].code, 250); // HELO response
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_mail_from() {
    let (port, _handle) = start_test_server().await;

    let result = SmtpClientBuilder::new(format!("127.0.0.1:{port}"), "localhost".to_string())
        .accept_invalid_certs(true)
        .ehlo("test.example.com")
        .mail_from("sender@example.com")
        .quit_after(QuitAfter::MailFrom)
        .execute()
        .await;

    assert!(result.is_ok());
    let responses = result.unwrap();

    // Greeting + EHLO + MAIL FROM + QUIT
    assert!(responses.len() >= 4);
    assert_eq!(responses.last().unwrap().code, 221); // QUIT response
    // Check that MAIL FROM succeeded (second to last)
    assert_eq!(responses[responses.len() - 2].code, 250);
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_mail_from_with_size() {
    let (port, _handle) = start_test_server().await;

    let result = SmtpClientBuilder::new(format!("127.0.0.1:{port}"), "localhost".to_string())
        .accept_invalid_certs(true)
        .ehlo("test.example.com")
        .mail_from_with_size("sender@example.com", 1000)
        .quit_after(QuitAfter::MailFrom)
        .execute()
        .await;

    assert!(result.is_ok());
    let responses = result.unwrap();
    assert!(responses.len() >= 4);
    assert_eq!(responses.last().unwrap().code, 221); // QUIT response
    assert_eq!(responses[responses.len() - 2].code, 250); // MAIL FROM response
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_rcpt_to() {
    let (port, _handle) = start_test_server().await;

    let result = SmtpClientBuilder::new(format!("127.0.0.1:{port}"), "localhost".to_string())
        .accept_invalid_certs(true)
        .ehlo("test.example.com")
        .mail_from("sender@example.com")
        .rcpt_to("recipient@example.com")
        .quit_after(QuitAfter::RcptTo)
        .execute()
        .await;

    assert!(result.is_ok());
    let responses = result.unwrap();
    assert!(responses.len() >= 5);
    assert_eq!(responses.last().unwrap().code, 221); // QUIT response
    assert_eq!(responses[responses.len() - 2].code, 250); // RCPT TO response
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_multiple_recipients() {
    let (port, _handle) = start_test_server().await;

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

    assert!(result.is_ok());
    let responses = result.unwrap();

    // Each RCPT TO should get a 250 response
    assert!(responses.iter().filter(|r| r.code == 250).count() >= 3);
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_complete_transaction() {
    let (port, _handle) = start_test_server().await;

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

    assert!(result.is_ok());
    let responses = result.unwrap();

    // Find the final response after data
    let data_end_response = responses.iter().rev().find(|r| r.code == 250);
    assert!(data_end_response.is_some());
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_full_session_with_quit() {
    let (port, _handle) = start_test_server().await;

    let message = "Subject: Test\r\n\r\nHello World\r\n";

    let result = SmtpClientBuilder::new(format!("127.0.0.1:{port}"), "localhost".to_string())
        .accept_invalid_certs(true)
        .ehlo("test.example.com")
        .mail_from("sender@example.com")
        .rcpt_to("recipient@example.com")
        .data_with_content(message)
        .execute()
        .await;

    assert!(result.is_ok());
    let responses = result.unwrap();

    // Last response should be QUIT (221)
    assert_eq!(responses.last().unwrap().code, 221);
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_rset_command() {
    let (port, _handle) = start_test_server().await;

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

    assert!(result.is_ok());
    let responses = result.unwrap();

    // Should have successful responses for both MAIL FROM commands
    assert!(responses.iter().filter(|r| r.code == 250).count() >= 2);
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_raw_command() {
    let (port, _handle) = start_test_server().await;

    let result = SmtpClientBuilder::new(format!("127.0.0.1:{port}"), "localhost".to_string())
        .accept_invalid_certs(true)
        .ehlo("test.example.com")
        .raw_command("MAIL FROM: test@gmail.com")
        .execute()
        .await;

    assert!(result.is_ok());
    let responses = result.unwrap();

    // HELP command should get a response
    assert!(responses.len() >= 3);
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_builder_build_method() {
    let (port, _handle) = start_test_server().await;

    let mut client = SmtpClientBuilder::new(format!("127.0.0.1:{port}"), "localhost".to_string())
        .accept_invalid_certs(true)
        .ehlo("test.example.com")
        .build()
        .await
        .unwrap();

    // Use the client for additional commands
    let response = client.mail_from("sender@example.com", None).await;
    assert!(response.is_ok());
    assert_eq!(response.unwrap().code, 250);

    let response = client.rcpt_to("recipient@example.com").await;
    assert!(response.is_ok());
    assert_eq!(response.unwrap().code, 250);

    let response = client.quit().await;
    assert!(response.is_ok());
    assert_eq!(response.unwrap().code, 221);
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_response_inspection() {
    let (port, _handle) = start_test_server().await;

    let client = SmtpClientBuilder::new(format!("127.0.0.1:{port}"), "localhost".to_string())
        .accept_invalid_certs(true)
        .ehlo("test.example.com")
        .build()
        .await
        .unwrap();

    // All responses should be accessible
    let responses = client.responses();
    assert!(responses.len() >= 2); // Greeting + EHLO

    // Last response should be the EHLO response
    let last = client.last_response().unwrap();
    assert_eq!(last.code, 250);
    assert!(last.is_success());
    assert!(!last.is_error());
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_size_exceeded() {
    let (port, _handle) = start_test_server().await;

    // Server has 10MB limit, declare larger size
    let result = SmtpClientBuilder::new(format!("127.0.0.1:{port}"), "localhost".to_string())
        .accept_invalid_certs(true)
        .ehlo("test.example.com")
        .mail_from_with_size("sender@example.com", 20_000_000)
        .quit_after(QuitAfter::MailFrom)
        .execute()
        .await;

    assert!(result.is_ok());
    let responses = result.unwrap();

    // Should get error response (552) then QUIT (221)
    assert_eq!(responses.last().unwrap().code, 221); // QUIT after error
    let error_response = &responses[responses.len() - 2]; // SIZE error
    assert_eq!(error_response.code, 552);
    assert!(error_response.is_permanent_error());
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_message_builder_simple() {
    let message = MessageBuilder::new()
        .from("sender@example.com")
        .to("recipient@example.com")
        .subject("Test Message")
        .body("This is a test message.")
        .build()
        .unwrap();

    assert!(message.contains("From: sender@example.com"));
    assert!(message.contains("To: recipient@example.com"));
    assert!(message.contains("Subject: Test Message"));
    assert!(message.contains("This is a test message."));
    assert!(message.contains("MIME-Version: 1.0"));
    assert!(message.contains("Content-Type: text/plain"));
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_message_builder_with_attachment() {
    let message = MessageBuilder::new()
        .from("sender@example.com")
        .to("recipient@example.com")
        .subject("File Attached")
        .body("Please find the attachment.")
        .attach("test.txt", "text/plain", b"File contents here".to_vec())
        .build()
        .unwrap();

    assert!(message.contains("From: sender@example.com"));
    assert!(message.contains("multipart/mixed"));
    assert!(message.contains("test.txt"));
    assert!(message.contains("base64"));
    assert!(message.contains("Content-Disposition: attachment"));
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_message_builder_auto_headers() {
    // Test that data_with_builder auto-populates FROM/TO headers
    let message = MessageBuilder::new()
        .from("sender@example.com")
        .to_multiple(&["recipient1@example.com", "recipient2@example.com"])
        .subject("Auto Headers Test")
        .body("Testing FROM/TO headers")
        .build()
        .unwrap();

    // Verify FROM and TO headers are properly set
    assert!(message.contains("From: sender@example.com"));
    assert!(message.contains("To: recipient1@example.com, recipient2@example.com"));
    assert!(message.contains("Subject: Auto Headers Test"));
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_send_with_message_builder() {
    let (port, _handle) = start_test_server().await;

    let message = MessageBuilder::new()
        .from("sender@example.com")
        .to("recipient@example.com")
        .subject("Integration Test")
        .body("Testing MessageBuilder integration with SMTP client")
        .build()
        .unwrap();

    let result = SmtpClientBuilder::new(format!("127.0.0.1:{port}"), "localhost".to_string())
        .accept_invalid_certs(true)
        .ehlo("test.example.com")
        .mail_from("sender@example.com")
        .rcpt_to("recipient@example.com")
        .data_with_message(message)
        .execute()
        .await;

    assert!(result.is_ok());
    let responses = result.unwrap();

    // Should have successful delivery
    let last_response = responses.last().unwrap();
    assert_eq!(last_response.code, 221); // QUIT
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_data_with_builder_closure() {
    let (port, _handle) = start_test_server().await;

    let result = SmtpClientBuilder::new(format!("127.0.0.1:{port}"), "localhost".to_string())
        .accept_invalid_certs(true)
        .ehlo("test.example.com")
        .mail_from("sender@example.com")
        .rcpt_to("recipient@example.com")
        .data_with_builder(|msg| {
            msg.subject("Closure Test")
                .body("Testing the ergonomic closure API")
                .build()
        })
        .unwrap()
        .execute()
        .await;

    assert!(result.is_ok());
    let responses = result.unwrap();

    // Should have successful delivery
    let last_response = responses.last().unwrap();
    assert_eq!(last_response.code, 221); // QUIT
}
