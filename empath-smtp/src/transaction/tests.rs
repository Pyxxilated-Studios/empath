//! Transaction handler tests

use std::sync::Arc;

use empath_common::address::{Address, AddressList};
use empath_common::address_parser;
use empath_common::audit::AuditLoggerBuilder;
use empath_common::context::Context;

use super::email_auth::dmarc_enforcement_action;
use super::handler::DefaultSmtpTransactionHandler;
use super::helpers::requires_validation;
use crate::MailParameters;

type TestResult = std::result::Result<(), String>;

#[test]
fn test_default_handler_creation() -> TestResult {
    let audit_logger = AuditLoggerBuilder::default().build();
    let handler = DefaultSmtpTransactionHandler::new(
        None,
        "127.0.0.1:1234"
            .parse::<std::net::SocketAddr>()
            .map_err(|err| err.to_string())?,
        None,
        crate::spf::SpfConfig::default(),
        false,
        false,
        audit_logger,
    );
    assert!(handler.spool.is_none());
    assert_eq!(handler.peer.to_string(), "127.0.0.1:1234");
    Ok(())
}

#[test]
fn test_default_handler_with_spool() -> TestResult {
    use empath_spool::MemoryBackingStore;

    let spool = Arc::new(MemoryBackingStore::default());
    let audit_logger = AuditLoggerBuilder::default().build();
    let handler = DefaultSmtpTransactionHandler::new(
        Some(spool),
        "127.0.0.1:1234"
            .parse::<std::net::SocketAddr>()
            .map_err(|err| err.to_string())?,
        None,
        crate::spf::SpfConfig::default(),
        false,
        false,
        audit_logger,
    );
    assert!(handler.spool.is_some());
    Ok(())
}

#[test]
fn test_requires_validation() {
    use crate::state::*;

    // States that require validation
    assert!(requires_validation(&crate::State::Connect(Connect)));
    assert!(requires_validation(&crate::State::Ehlo(Ehlo {
        id: "test".to_string()
    })));
    assert!(requires_validation(&crate::State::Helo(Helo {
        id: "test".to_string()
    })));
    assert!(requires_validation(&crate::State::MailFrom(MailFrom {
        sender: None,
        params: MailParameters::default()
    })));
    assert!(requires_validation(&crate::State::Auth(Auth {
        mechanism: "PLAIN".to_string(),
        awaiting_response: false,
    })));

    // States that don't require validation
    assert!(!requires_validation(&crate::State::Data(Data)));
    assert!(!requires_validation(&crate::State::Quit(Quit)));
    assert!(!requires_validation(&crate::State::Invalid(Invalid {
        reason: String::new()
    })));
}

#[test]
fn test_dmarc_enforcement_action() {
    assert_eq!(
        dmarc_enforcement_action("fail", Some("reject")),
        Some("reject")
    );
    assert_eq!(
        dmarc_enforcement_action("fail", Some("quarantine")),
        Some("quarantine")
    );
    assert_eq!(dmarc_enforcement_action("fail", Some("none")), None);
    assert_eq!(dmarc_enforcement_action("pass", Some("reject")), None);
    assert_eq!(dmarc_enforcement_action("temperror", Some("reject")), None);
}

#[test]
#[cfg_attr(miri, ignore)]
fn received_header_is_inserted_before_body() -> TestResult {
    let audit_logger = AuditLoggerBuilder::default().build();
    let handler = DefaultSmtpTransactionHandler::new(
        None,
        "203.0.113.5:2525"
            .parse::<std::net::SocketAddr>()
            .map_err(|err| err.to_string())?,
        None,
        crate::spf::SpfConfig::default(),
        false,
        false,
        audit_logger,
    );

    let message = b"Subject: hello\r\n\r\nhello world";
    let mut ctx = Context {
        data: Some(Arc::from(message.as_ref())),
        id: "client.example.com".to_string(),
        extended: true,
        banner: Arc::from("mail.example.com"),
        ..Context::default()
    };

    let recipient_mailbox = address_parser::parse_forward_path("<recipient@example.com>")
        .map_err(|err| format!("{err:?}"))?;
    let recipient_address = Address::from(recipient_mailbox);
    Arc::make_mut(&mut ctx.envelope)
        .recipients_mut()
        .replace(AddressList::from(vec![recipient_address]));

    ctx.set_tls_info("TLSv1.3".to_string(), "TLS_AES_256_GCM_SHA384".to_string());

    handler.insert_received_header(&mut ctx);

    let data = ctx
        .data
        .as_ref()
        .ok_or_else(|| "message data missing".to_string())?;
    let header_end = data
        .windows(2)
        .position(|window| window == b"\r\n")
        .ok_or_else(|| "missing CRLF".to_string())?;
    let first_line = std::str::from_utf8(&data[..header_end]).map_err(|err| err.to_string())?;

    assert!(first_line.starts_with("Received: from client.example.com"));
    assert!(first_line.contains("by mail.example.com (Empath)"));
    assert!(first_line.contains("with ESMTPS id <"));
    assert!(first_line.contains("for <recipient@example.com>"));
    Ok(())
}

#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn test_spf_evaluation_integration() -> TestResult {
    use std::net::IpAddr;
    use std::sync::Mutex;

    use empath_common::address_parser::Mailbox;
    use empath_common::envelope::Envelope;

    use crate::spf::dns::{DnsLookupError, SpfDnsResolver};
    use crate::spf::{MailServer, SpfConfig, SpfResult};
    use crate::transaction::traits::SmtpTransactionHandler;

    // Mock DNS resolver that returns SPF records
    #[derive(Debug)]
    struct MockSpfResolver {
        txt_records: Mutex<ahash::AHashMap<String, Vec<String>>>,
    }

    impl MockSpfResolver {
        fn new() -> Self {
            Self {
                txt_records: Mutex::new(ahash::AHashMap::new()),
            }
        }

        fn add_txt(&self, domain: &str, records: Vec<String>) -> Result<(), DnsLookupError> {
            self.txt_records
                .lock()
                .map_err(|_| DnsLookupError::LookupFailed("lock poisoned".to_string()))?
                .insert(domain.to_string(), records);
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl SpfDnsResolver for MockSpfResolver {
        async fn txt_lookup(
            &self,
            domain: &str,
        ) -> Result<Arc<Vec<String>>, crate::spf::dns::DnsLookupError> {
            let records = self
                .txt_records
                .lock()
                .map_err(|_| DnsLookupError::LookupFailed("lock poisoned".to_string()))?;
            Ok(Arc::new(records.get(domain).cloned().unwrap_or_default()))
        }

        async fn ptr_lookup(
            &self,
            _ip: IpAddr,
        ) -> Result<Arc<Vec<String>>, crate::spf::dns::DnsLookupError> {
            Ok(Arc::new(Vec::new()))
        }

        async fn a_lookup(
            &self,
            _domain: &str,
        ) -> Result<Arc<Vec<IpAddr>>, crate::spf::dns::DnsLookupError> {
            let ip = "192.0.2.100"
                .parse::<IpAddr>()
                .map_err(|err| DnsLookupError::LookupFailed(err.to_string()))?;
            Ok(Arc::new(vec![ip]))
        }

        async fn mx_lookup(
            &self,
            _domain: &str,
        ) -> Result<Arc<Vec<MailServer>>, crate::spf::dns::DnsLookupError> {
            Ok(Arc::new(Vec::new()))
        }
    }

    // Setup mock resolver with SPF record
    let resolver = Arc::new(MockSpfResolver::new());
    resolver
        .add_txt(
            "example.com",
            vec!["v=spf1 ip4:192.0.2.0/24 -all".to_string()],
        )
        .map_err(|err| err.to_string())?;

    // Create transaction handler with SPF resolver
    let audit_logger = AuditLoggerBuilder::default().build();
    let mut handler = DefaultSmtpTransactionHandler::new(
        None,
        "192.0.2.100:1234"
            .parse::<std::net::SocketAddr>()
            .map_err(|err| err.to_string())?,
        Some(resolver),
        SpfConfig::default(),
        false,
        false,
        audit_logger,
    );

    // Create context with sender from example.com
    let mut envelope = Envelope::default();
    let mailbox = Mailbox {
        local_part: "user".to_string(),
        domain: "example.com".to_string(),
    };
    *envelope.sender_mut() = Some(mailbox.into());

    let mut ctx = Context {
        extended: true,
        envelope: Arc::new(envelope),
        id: "client.example.com".to_string(),
        banner: Arc::from("test.example.com"),
        ..Context::default()
    };

    // Call validate_mail_from which should perform SPF evaluation
    let _result = handler.validate_mail_from(&mut ctx).await;

    // Check that SPF result was stored in transaction context
    // (Note: module dispatch may return a non-accept result when no modules loaded,
    // but SPF evaluation should still have occurred)
    let spf_result = ctx
        .spf_result()
        .ok_or_else(|| "spf_result missing".to_string())?;
    assert_eq!(spf_result, SpfResult::Pass.to_string().as_str());
    Ok(())
}
