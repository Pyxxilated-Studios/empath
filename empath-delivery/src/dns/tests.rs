//! DNS module tests

use super::error::{DnsError, DnssecStatus};
use super::hickory::HickoryDnsResolver;
use super::types::MailServer;

type TestResult = std::result::Result<(), String>;

#[tokio::test]
#[ignore = "Requires network access"]
async fn test_mx_lookup_gmail() -> TestResult {
    let resolver = HickoryDnsResolver::new().map_err(|err| err.to_string())?;
    let servers = resolver
        .resolve_mail_servers("gmail.com")
        .await
        .map_err(|err| err.to_string())?;

    assert!(!servers.is_empty());
    assert!(servers.iter().all(|s| s.port == 25));
    // Verify sorted by priority
    assert!(servers.windows(2).all(|w| w[0].priority <= w[1].priority));
    Ok(())
}

#[tokio::test]
#[ignore = "Requires network access"]
async fn test_a_record_fallback() -> TestResult {
    // Use a domain that has A records but no MX records
    // Note: This test may fail if the domain changes its DNS configuration
    let resolver = HickoryDnsResolver::new().map_err(|err| err.to_string())?;
    let servers = resolver.resolve_mail_servers("example.com").await;

    // Either MX or A/AAAA should work
    assert!(servers.is_ok());
    Ok(())
}

#[tokio::test]
#[ignore = "Requires network access"]
async fn test_domain_not_found() -> TestResult {
    let resolver = HickoryDnsResolver::new().map_err(|err| err.to_string())?;
    let result = resolver
        .resolve_mail_servers("this-domain-definitely-does-not-exist-12345.com")
        .await;

    assert!(result.is_err());
    Ok(())
}

#[test]
fn test_mail_server_address() {
    let server = MailServer::new("mail.example.com".to_string(), 10, 25);
    assert_eq!(server.address(), "mail.example.com:25");
}

#[test]
fn test_priority_sorting() {
    let mut servers = [
        MailServer::new("mx3.example.com".to_string(), 30, 25),
        MailServer::new("mx1.example.com".to_string(), 10, 25),
        MailServer::new("mx2.example.com".to_string(), 20, 25),
    ];

    servers.sort_by_key(|s| s.priority);

    assert_eq!(servers[0].priority, 10);
    assert_eq!(servers[1].priority, 20);
    assert_eq!(servers[2].priority, 30);
}

#[test]
fn test_dns_error_is_temporary() {
    assert!(DnsError::Timeout("example.com".to_string()).is_temporary());
    assert!(!DnsError::NoMailServers("example.com".to_string()).is_temporary());
    assert!(!DnsError::DomainNotFound("example.com".to_string()).is_temporary());
    assert!(
        DnsError::DnssecValidationFailed {
            target: "example.com".to_string(),
            status: DnssecStatus::Indeterminate,
        }
        .is_temporary()
    );
    assert!(
        !DnsError::DnssecValidationFailed {
            target: "example.com".to_string(),
            status: DnssecStatus::Bogus,
        }
        .is_temporary()
    );
}

#[test]
fn test_randomize_equal_priority_preserves_priority_order() {
    // Create servers with mixed priorities
    let mut servers = vec![
        MailServer::new("mx1a.example.com".to_string(), 10, 25),
        MailServer::new("mx1b.example.com".to_string(), 10, 25),
        MailServer::new("mx2a.example.com".to_string(), 20, 25),
        MailServer::new("mx2b.example.com".to_string(), 20, 25),
        MailServer::new("mx3.example.com".to_string(), 30, 25),
    ];

    // Randomize
    HickoryDnsResolver::randomize_equal_priority(&mut servers);

    // Verify priority boundaries are maintained
    assert_eq!(servers[0].priority, 10);
    assert_eq!(servers[1].priority, 10);
    assert_eq!(servers[2].priority, 20);
    assert_eq!(servers[3].priority, 20);
    assert_eq!(servers[4].priority, 30);
}

#[test]
fn test_randomize_equal_priority_shuffles_within_groups() {
    // Create servers with equal priority
    let original = vec![
        MailServer::new("mx1.example.com".to_string(), 10, 25),
        MailServer::new("mx2.example.com".to_string(), 10, 25),
        MailServer::new("mx3.example.com".to_string(), 10, 25),
        MailServer::new("mx4.example.com".to_string(), 10, 25),
    ];

    // Run randomization multiple times and check if we get different orderings
    // With 4 servers, there are 24 possible orderings. Getting the same order
    // multiple times in a row would be very unlikely if randomization works.
    let mut orderings = std::collections::HashSet::new();

    for _ in 0..10 {
        let mut servers = original.clone();
        HickoryDnsResolver::randomize_equal_priority(&mut servers);

        // Create a signature for this ordering
        let signature: Vec<_> = servers.iter().map(|s| s.host.clone()).collect();
        orderings.insert(signature);
    }

    // We should see at least 2 different orderings (very likely more)
    assert!(
        orderings.len() >= 2,
        "Expected randomization to produce different orderings, got only {:?}",
        orderings.len()
    );
}

#[test]
fn test_randomize_equal_priority_single_server() {
    let mut servers = vec![MailServer::new("mx1.example.com".to_string(), 10, 25)];

    // Should not panic with single server
    HickoryDnsResolver::randomize_equal_priority(&mut servers);
    assert_eq!(servers.len(), 1);
}

#[test]
fn test_randomize_equal_priority_empty() {
    let mut servers: Vec<MailServer> = vec![];

    // Should not panic with empty slice
    HickoryDnsResolver::randomize_equal_priority(&mut servers);
    assert_eq!(servers.len(), 0);
}
