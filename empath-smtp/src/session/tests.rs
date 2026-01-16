use std::sync::{Arc, RwLock};

use empath_common::address::{Address, AddressList};
use empath_common::address_parser;
use empath_common::context::{Context, SPAN_ID_CONTEXT_KEY, TRACE_ID_CONTEXT_KEY};
use empath_common::email::BannerHostname;
use empath_common::status::Status;
use empath_ffi::modules::test::TestModule;
use empath_ffi::modules::validate::Event;
use empath_ffi::modules::{ModuleRegistry, ModuleRegistryBuilder};
use empath_spool::{BackingStore, TestBackingStore};
use opentelemetry::Context as OtelContext;
use opentelemetry::trace::{
    SpanContext, SpanId, TraceContextExt, TraceFlags, TraceId, TraceState, TracerProvider as _,
};
use opentelemetry_sdk::trace::SdkTracerProvider;
use tracing::Instrument;
use tracing_opentelemetry::{OpenTelemetryLayer, OpenTelemetrySpanExt as _};
use tracing_subscriber::prelude::*;

use crate::session::{Session, SessionConfig};
use crate::transaction::SmtpTransactionHandler;
use crate::{State, state};

type TestResult = std::result::Result<(), String>;

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn session() -> TestResult {
    // Initialize modules to add core module
    ModuleRegistryBuilder::new()
        .build()
        .map_err(|err| err.to_string())?
        .install()
        .map_err(|err| err.to_string())?;

    let banner = BannerHostname::new("testing").map_err(|err| err.to_string())?;
    let mut context = Context {
        banner: banner.as_str().into(),
        max_message_size: 0,
        ..Default::default()
    };

    let (mut session, _handler, _tls) = Session::create(
        "[::]:25"
            .parse::<std::net::SocketAddr>()
            .map_err(|err| err.to_string())?,
        SessionConfig::builder().banner(banner.clone()).build(),
    );

    let response = session.response(&mut context).await;
    let line = response
        .0
        .as_ref()
        .and_then(|lines| lines.first())
        .ok_or_else(|| "missing response".to_string())?;
    assert_eq!(line, &format!("{} {}", Status::ServiceReady, banner));
    Ok(())
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn helo() -> TestResult {
    // Initialize modules to add core module
    ModuleRegistryBuilder::new()
        .build()
        .map_err(|err| err.to_string())?
        .install()
        .map_err(|err| err.to_string())?;

    let banner = BannerHostname::new("testing").map_err(|err| err.to_string())?;
    let mut context = Context {
        banner: banner.as_str().into(),
        max_message_size: 0,
        ..Default::default()
    };

    let host = "Test";

    let (mut session, _handler, _tls) = Session::create(
        "[::]:25"
            .parse::<std::net::SocketAddr>()
            .map_err(|err| err.to_string())?,
        SessionConfig::builder().banner(banner.clone()).build(),
    );

    let _ = session.response(&mut context).await;
    context.response = None; // Clear response like run_inner does

    // Handle HELO command
    session.handle_command_reception(format!("HELO {host}").as_bytes(), &mut context);

    // HELO doesn't need emit(), it's not validated
    let response = session.response(&mut context).await;
    let line = response
        .0
        .as_ref()
        .and_then(|lines| lines.first())
        .ok_or_else(|| "missing response".to_string())?;
    assert_eq!(
        line,
        &format!("{} {banner} says hello to {host}", Status::Ok)
    );
    Ok(())
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn supports_extended_commands() -> TestResult {
    ModuleRegistryBuilder::new()
        .build()
        .map_err(|err| err.to_string())?
        .install()
        .map_err(|err| err.to_string())?;

    let banner = BannerHostname::new("testing").map_err(|err| err.to_string())?;
    let mut context = Context {
        banner: banner.as_str().into(),
        max_message_size: 0,
        ..Default::default()
    };

    let (mut session, _handler, _tls) = Session::create(
        "[::]:25"
            .parse::<std::net::SocketAddr>()
            .map_err(|err| err.to_string())?,
        SessionConfig::builder().banner(banner).build(),
    );

    let _ = session.response(&mut context).await;
    context.response = None;

    session.handle_command_reception(b"NOOP", &mut context);
    let response = session.response(&mut context).await;
    let line = response
        .0
        .as_ref()
        .and_then(|lines| lines.first())
        .ok_or_else(|| "missing response".to_string())?;
    assert_eq!(line, &format!("{} Ok", Status::Ok));
    context.response = None;

    session.handle_command_reception(b"VRFY user@example.com", &mut context);
    let response = session.response(&mut context).await;
    let line = response
        .0
        .as_ref()
        .and_then(|lines| lines.first())
        .ok_or_else(|| "missing response".to_string())?;
    assert_eq!(line, &format!("{} Cannot VRFY user", Status::Unknown(252)));
    context.response = None;

    session.handle_command_reception(b"EXPN staff", &mut context);
    let response = session.response(&mut context).await;
    let line = response
        .0
        .as_ref()
        .and_then(|lines| lines.first())
        .ok_or_else(|| "missing response".to_string())?;
    assert_eq!(
        line,
        &format!("{} Cannot expand list", Status::Unknown(252))
    );
    context.response = None;

    session.handle_command_reception(b"ETRN example.com", &mut context);
    let response = session.response(&mut context).await;
    let line = response
        .0
        .as_ref()
        .and_then(|lines| lines.first())
        .ok_or_else(|| "missing response".to_string())?;
    assert_eq!(
        line,
        &format!("{} Command not implemented", Status::Unknown(502))
    );
    context.response = None;

    session.handle_command_reception(b"TURN", &mut context);
    let response = session.response(&mut context).await;
    let line = response
        .0
        .as_ref()
        .and_then(|lines| lines.first())
        .ok_or_else(|| "missing response".to_string())?;
    assert_eq!(
        line,
        &format!("{} Command not implemented", Status::Unknown(502))
    );
    Ok(())
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn spool_integration() -> TestResult {
    // Initialize modules to add core module
    ModuleRegistryBuilder::new()
        .build()
        .map_err(|err| err.to_string())?
        .install()
        .map_err(|err| err.to_string())?;

    let banner = BannerHostname::new("testing").map_err(|err| err.to_string())?;
    let mut context = Context {
        banner: banner.as_str().into(),
        max_message_size: 0,
        ..Default::default()
    };

    let test_data = b"Subject: Test\r\n\r\nHello World\r\n.\r\n";

    // Create a mock spool controller
    let mock_spool = Arc::new(TestBackingStore::default());

    let (mut session, mut transaction_handler, _tls) = Session::create(
        "[::]:25"
            .parse::<std::net::SocketAddr>()
            .map_err(|err| err.to_string())?,
        SessionConfig::builder()
            .spool(Some(mock_spool.clone()))
            .banner(banner.clone())
            .build(),
    );

    // Simulate HELO state and receiving DATA
    session.context.state = State::RcptTo(state::RcptTo {
        sender: None,
        params: crate::MailParameters::new(),
    });
    let sender_mailbox = address_parser::parse_forward_path("<test@example.com>")
        .map_err(|err| format!("{err:?}"))?;
    Arc::make_mut(&mut context.envelope)
        .sender_mut()
        .replace(Address::from(sender_mailbox));
    let recipient_mailbox = address_parser::parse_forward_path("<recipient@example.com>")
        .map_err(|err| format!("{err:?}"))?;
    Arc::make_mut(&mut context.envelope)
        .recipients_mut()
        .replace(AddressList::from(vec![Address::from(recipient_mailbox)]));

    let _ = session.response(&mut context).await;

    // Handle DATA command to receive message
    session.handle_data_reception(test_data, &mut context);

    // Simulate PostDot state (handle_data_reception transitions here when it sees \r\n.\r\n)
    assert!(matches!(session.context.state, State::PostDot(_)));

    let tracer = SdkTracerProvider::builder().build().tracer("empath-test");
    let _ = tracing_subscriber::registry()
        .with(OpenTelemetryLayer::new(tracer))
        .try_init();

    let trace_id =
        TraceId::from_hex("4bf92f3577b34da6a3ce929d0e0e4736").map_err(|err| err.to_string())?;
    let span_id = SpanId::from_hex("00f067aa0ba902b7").map_err(|err| err.to_string())?;
    let span_context = SpanContext::new(
        trace_id,
        span_id,
        TraceFlags::default(),
        true,
        TraceState::default(),
    );
    let otel_ctx = OtelContext::new().with_remote_span_context(span_context);
    let span = tracing::info_span!("smtp_receive");
    let _ = span.set_parent(otel_ctx);
    let span_context = span.context().span().span_context().clone();
    assert_eq!(span_context.trace_id(), trace_id);

    // Handle message spooling within trace context
    transaction_handler
        .handle_message(&mut context)
        .instrument(span)
        .await;

    let response = session.response(&mut context).await;
    assert!(response.0.is_some());

    // Wait for the spool operation to complete with a timeout
    mock_spool
        .wait_for_count(1, std::time::Duration::from_secs(5))
        .await
        .map_err(|err| err.to_string())?;

    // Verify message was spooled
    assert_eq!(mock_spool.message_count(), 1);
    let ids = mock_spool.list().await.map_err(|err| err.to_string())?;
    let spooled_msg_id = ids
        .first()
        .ok_or_else(|| "missing spooled id".to_string())?;
    let mut spooled_msg = mock_spool
        .read(spooled_msg_id)
        .await
        .map_err(|err| err.to_string())?;
    assert!(spooled_msg.body_view().is_ok_and(|body| {
        body.is_some_and(|body| body.as_str_lossy() == "Hello World\r\n.\r\n")
    }));
    assert_eq!(
        spooled_msg.transaction.get(TRACE_ID_CONTEXT_KEY),
        Some(&span_context.trace_id().to_string())
    );
    assert_eq!(
        spooled_msg.transaction.get(SPAN_ID_CONTEXT_KEY),
        Some(&span_context.span_id().to_string())
    );
    Ok(())
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn modules() -> TestResult {
    let banner = BannerHostname::new("testing").map_err(|err| err.to_string())?;
    let mut context = Context {
        banner: banner.as_str().into(),
        max_message_size: 0,
        ..Default::default()
    };

    let module = TestModule(RwLock::default());
    ModuleRegistryBuilder::new()
        .with_modules(vec![Box::new(module)])
        .build()
        .map_err(|err| err.to_string())?
        .install()
        .map_err(|err| err.to_string())?;

    let (mut session, mut transaction_handler, _tls) = Session::create(
        "[::]:25"
            .parse::<std::net::SocketAddr>()
            .map_err(|err| err.to_string())?,
        SessionConfig::builder().banner(banner.clone()).build(),
    );

    session.context.state = State::Helo(state::Helo {
        id: "test".to_string(),
    });

    let _ = session.response(&mut context).await;

    // Handle MAIL FROM command
    session.handle_command_reception(b"MAIL FROM: test@gmail.com", &mut context);

    // After handle_command_reception, state should be MailFrom
    // Validate the command (this is where modules are dispatched)
    transaction_handler.validate_mail_from(&mut context).await;

    let response = session.response(&mut context).await;
    let line = response
        .0
        .as_ref()
        .and_then(|lines| lines.first())
        .ok_or_else(|| "missing response".to_string())?;
    assert_eq!(line, &format!("{} Ok", Status::Ok));

    let registry = ModuleRegistry::global().ok_or_else(|| "registry missing".to_string())?;
    for module in registry.modules() {
        if let Some(TestModule(mute)) = module.as_any().downcast_ref() {
            assert!(
                mute.read()
                    .map_err(|_| "module lock poisoned".to_string())?
                    .validators_called
                    .contains(&Event::MailFrom)
            );

            return Ok(());
        }
    }

    Err("Expected TestModule to exist".to_string())
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_injectable_transaction_handler() -> TestResult {
    use std::borrow::Cow;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;

    // Mock transaction handler that rejects all connections
    struct RejectAllHandler {
        call_count: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl SmtpTransactionHandler for RejectAllHandler {
        async fn validate_connect(&mut self, ctx: &mut Context) -> bool {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            ctx.response = Some((Status::Unavailable, Cow::Borrowed("Connection rejected")));
            false
        }

        async fn validate_ehlo(&mut self, _ctx: &mut Context) -> bool {
            false
        }

        async fn validate_mail_from(&mut self, _ctx: &mut Context) -> bool {
            false
        }

        async fn validate_rcpt_to(&mut self, _ctx: &mut Context) -> bool {
            false
        }

        async fn validate_auth(&mut self, _ctx: &mut Context) -> bool {
            false
        }

        async fn handle_message(&mut self, _ctx: &mut Context) -> bool {
            false
        }
    }

    let banner = BannerHostname::new("testing").map_err(|err| err.to_string())?;
    let mut context = Context {
        banner: banner.as_str().into(),
        max_message_size: 0,
        ..Default::default()
    };

    let call_count = Arc::new(AtomicUsize::new(0));
    let handler = RejectAllHandler {
        call_count: Arc::clone(&call_count),
    };

    let (mut session, mut transaction_handler, _tls) = Session::create(
        "[::]:25"
            .parse::<std::net::SocketAddr>()
            .map_err(|err| err.to_string())?,
        SessionConfig::builder()
            .banner(banner.clone())
            .transaction_handler(Some(Box::new(handler)))
            .build(),
    );

    // Test that the mock handler is called during validate_connect
    let valid = transaction_handler.validate_connect(&mut context).await;
    assert_eq!(call_count.load(Ordering::SeqCst), 1);
    assert!(!valid); // Handler should reject

    // Test that validation fails
    assert!(context.response.is_some());
    let (status, message) = context
        .response
        .as_ref()
        .ok_or_else(|| "missing response".to_string())?;
    assert_eq!(*status, Status::Unavailable);
    assert_eq!(message, "Connection rejected");

    // Manually transition to Reject state (orchestrator would do this)
    session.context.state = State::Reject(state::Reject);
    assert!(matches!(session.context.state, State::Reject(_)));
    Ok(())
}
