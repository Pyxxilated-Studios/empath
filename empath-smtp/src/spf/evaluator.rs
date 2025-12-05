//! SPF record evaluation logic
//!
//! Implements RFC 7208 SPF evaluation with DNS lookups and mechanism checking.

#![allow(clippy::too_many_lines)]

use std::{net::IpAddr, sync::Arc, time::Instant};

use empath_common::email::EmailAddress;
use ipnetwork::{Ipv4Network, Ipv6Network};
use tracing::{debug, warn};

use super::{
    Mechanism, Modifier, Qualifier, SpfConfig, SpfError, SpfEvaluation, SpfRecord, SpfResult,
    dns::SpfDnsResolver,
};

/// SPF evaluator
///
/// Evaluates SPF records against sender/client IP pairs using DNS lookups.
pub struct SpfEvaluator<D: ?Sized> {
    dns: Arc<D>,
    config: SpfConfig,
}

impl<D: ?Sized + SpfDnsResolver> SpfEvaluator<D> {
    /// Create a new SPF evaluator with a DNS resolver
    #[must_use]
    pub const fn new(dns: Arc<D>, config: SpfConfig) -> Self {
        Self { dns, config }
    }

    /// Evaluate SPF for a sender and client IP
    ///
    /// # Arguments
    ///
    /// * `sender` - The email address from MAIL FROM
    /// * `client_ip` - The IP address of the connecting client
    /// * `helo_hostname` - The hostname from HELO/EHLO
    ///
    /// # Errors
    ///
    /// Returns `SpfError` for DNS failures or evaluation errors
    pub async fn evaluate(
        &self,
        sender: &EmailAddress,
        client_ip: IpAddr,
        helo_hostname: &str,
    ) -> SpfEvaluation {
        let start = Instant::now();

        // Parse sender email to extract domain
        let (sender_local, sender_domain) = Self::parse_sender(sender);

        debug!(
            sender = %sender,
            sender_domain = %sender_domain,
            client_ip = %client_ip,
            helo = %helo_hostname,
            "Evaluating SPF"
        );

        // Create evaluation context
        let mut ctx = EvaluationContext {
            _sender: sender.as_str().to_string(),
            _sender_local: sender_local.to_string(),
            sender_domain: sender_domain.to_string(),
            client_ip,
            _helo_hostname: helo_hostname.to_string(),
            dns: Arc::clone(&self.dns),
            lookup_count: 0,
            _void_lookup_count: 0,
            max_lookups: self.config.max_dns_lookups,
            _max_void_lookups: self.config.max_void_lookups,
            recursion_depth: 0,
            max_recursion_depth: self.config.max_recursion_depth,
        };

        // Look up SPF record for sender domain
        let result = match self.lookup_spf_record(&mut ctx, sender_domain).await {
            Ok(record) => {
                // Evaluate the record
                self.evaluate_record(&mut ctx, &record, sender_domain).await
            }
            Err(SpfError::NoRecord) => SpfEvaluation {
                result: SpfResult::None,
                record: None,
                matched_mechanism: None,
                explanation: Some("No SPF record found".to_string()),
                lookup_count: ctx.lookup_count,
                evaluation_time: start.elapsed(),
            },
            Err(e) if e.is_temporary() => SpfEvaluation {
                result: SpfResult::TempError,
                record: None,
                matched_mechanism: None,
                explanation: Some(e.to_string()),
                lookup_count: ctx.lookup_count,
                evaluation_time: start.elapsed(),
            },
            Err(e) => SpfEvaluation {
                result: SpfResult::PermError,
                record: None,
                matched_mechanism: None,
                explanation: Some(e.to_string()),
                lookup_count: ctx.lookup_count,
                evaluation_time: start.elapsed(),
            },
        };

        debug!(
            result = %result.result,
            lookup_count = result.lookup_count,
            evaluation_time_us = result.evaluation_time.as_micros(),
            "SPF evaluation complete"
        );

        result
    }

    /// Parse sender email into local and domain parts
    fn parse_sender(sender: &EmailAddress) -> (&str, &str) {
        let s = sender.as_str();
        let (local, domain) = s.rsplit_once('@').unwrap_or(("", ""));
        debug_assert!(
            !local.is_empty() && !domain.is_empty(),
            "EmailAddress guarantees a single '@' not at start or end"
        );
        (local, domain)
    }

    /// Look up SPF record for a domain
    async fn lookup_spf_record(
        &self,
        ctx: &mut EvaluationContext<D>,
        domain: &str,
    ) -> Result<SpfRecord, SpfError> {
        ctx.check_lookup_limit()?;

        debug!(domain = %domain, "Looking up SPF record");

        // Look up TXT records
        let txt_records = self.dns.txt_lookup(domain).await.map_err(|e| match e {
            super::dns::DnsLookupError::Timeout(_) => SpfError::DnsTimeout,
            super::dns::DnsLookupError::LookupFailed(msg) => SpfError::DnsLookupFailed(msg),
        })?;

        // Find SPF records (start with "v=spf1")
        let spf_records: Vec<_> = txt_records
            .iter()
            .filter(|record| record.starts_with("v=spf1"))
            .collect();

        match spf_records.len() {
            0 => {
                debug!(domain = %domain, "No SPF record found");
                Err(SpfError::NoRecord)
            }
            1 => {
                let record_str = spf_records[0];
                debug!(domain = %domain, record = %record_str, "Found SPF record");
                SpfRecord::parse(record_str)
            }
            _ => {
                warn!(
                    domain = %domain,
                    count = spf_records.len(),
                    "Multiple SPF records found (RFC 7208 violation)"
                );
                Err(SpfError::InvalidRecord(
                    "Multiple SPF records found".to_string(),
                ))
            }
        }
    }

    /// Evaluate an SPF record
    async fn evaluate_record(
        &self,
        ctx: &mut EvaluationContext<D>,
        record: &SpfRecord,
        domain: &str,
    ) -> SpfEvaluation {
        let record_str = format!("v=spf1 {}", self.format_record(record));

        // Evaluate mechanisms in order
        for mechanism in &record.mechanisms {
            match ctx.evaluate_mechanism(mechanism, domain).await {
                Ok(true) => {
                    // Mechanism matched - return result based on qualifier
                    let result = Self::qualifier_to_result(mechanism.qualifier());
                    let matched = Self::format_mechanism(mechanism);

                    debug!(
                        mechanism = %matched,
                        qualifier = %mechanism.qualifier(),
                        result = %result,
                        "SPF mechanism matched"
                    );

                    return SpfEvaluation {
                        result,
                        record: Some(record_str),
                        matched_mechanism: Some(matched),
                        explanation: None,
                        lookup_count: ctx.lookup_count,
                        evaluation_time: Instant::now().elapsed(),
                    };
                }
                Ok(false) => {
                    // Mechanism didn't match - continue to next
                }
                Err(e) if e.is_temporary() => {
                    // Temporary error - return temperror
                    warn!(error = %e, "Temporary error during SPF evaluation");
                    return SpfEvaluation {
                        result: SpfResult::TempError,
                        record: Some(record_str),
                        matched_mechanism: None,
                        explanation: Some(e.to_string()),
                        lookup_count: ctx.lookup_count,
                        evaluation_time: Instant::now().elapsed(),
                    };
                }
                Err(e) => {
                    // Permanent error - return permerror
                    warn!(error = %e, "Permanent error during SPF evaluation");
                    return SpfEvaluation {
                        result: SpfResult::PermError,
                        record: Some(record_str),
                        matched_mechanism: None,
                        explanation: Some(e.to_string()),
                        lookup_count: ctx.lookup_count,
                        evaluation_time: Instant::now().elapsed(),
                    };
                }
            }
        }

        // Check for redirect modifier
        if let Some(redirect_domain) = self.find_redirect_modifier(record) {
            debug!(redirect_domain = %redirect_domain, "Following redirect modifier");
            return self.evaluate_redirect(ctx, &redirect_domain).await;
        }

        // No mechanisms matched and no redirect - return neutral
        debug!("No SPF mechanisms matched, returning neutral");
        SpfEvaluation {
            result: SpfResult::Neutral,
            record: Some(record_str),
            matched_mechanism: None,
            explanation: None,
            lookup_count: ctx.lookup_count,
            evaluation_time: Instant::now().elapsed(),
        }
    }

    /// Find redirect modifier in record
    #[allow(clippy::unused_self)]
    fn find_redirect_modifier(&self, record: &SpfRecord) -> Option<String> {
        record.modifiers.iter().find_map(|m| {
            if let Modifier::Redirect(domain) = m {
                Some(domain.clone())
            } else {
                None
            }
        })
    }

    /// Evaluate redirect modifier
    fn evaluate_redirect<'a>(
        &'a self,
        ctx: &'a mut EvaluationContext<D>,
        redirect_domain: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = SpfEvaluation> + Send + 'a>> {
        Box::pin(async move {
            ctx.recursion_depth += 1;
            if ctx.recursion_depth > ctx.max_recursion_depth {
                return SpfEvaluation {
                    result: SpfResult::PermError,
                    record: None,
                    matched_mechanism: None,
                    explanation: Some(format!(
                        "Recursion depth exceeded (max: {})",
                        ctx.max_recursion_depth
                    )),
                    lookup_count: ctx.lookup_count,
                    evaluation_time: Instant::now().elapsed(),
                };
            }

            match self.lookup_spf_record(ctx, redirect_domain).await {
                Ok(record) => self.evaluate_record(ctx, &record, redirect_domain).await,
                Err(e) => SpfEvaluation {
                    result: if e.is_temporary() {
                        SpfResult::TempError
                    } else {
                        SpfResult::PermError
                    },
                    record: None,
                    matched_mechanism: None,
                    explanation: Some(e.to_string()),
                    lookup_count: ctx.lookup_count,
                    evaluation_time: Instant::now().elapsed(),
                },
            }
        })
    }

    /// Convert qualifier to SPF result
    const fn qualifier_to_result(qualifier: Qualifier) -> SpfResult {
        match qualifier {
            Qualifier::Pass => SpfResult::Pass,
            Qualifier::Fail => SpfResult::Fail,
            Qualifier::SoftFail => SpfResult::SoftFail,
            Qualifier::Neutral => SpfResult::Neutral,
        }
    }

    /// Format mechanism for display
    fn format_mechanism(mechanism: &Mechanism) -> String {
        match mechanism {
            Mechanism::All(_) => "all".to_string(),
            Mechanism::Ip4 { network, .. } => format!("ip4:{network}"),
            Mechanism::Ip6 { network, .. } => format!("ip6:{network}"),
            Mechanism::A { domain, .. } => domain
                .as_ref()
                .map_or_else(|| "a".to_string(), |d| format!("a:{d}")),
            Mechanism::Mx { domain, .. } => domain
                .as_ref()
                .map_or_else(|| "mx".to_string(), |d| format!("mx:{d}")),
            Mechanism::Ptr { domain, .. } => domain
                .as_ref()
                .map_or_else(|| "ptr".to_string(), |d| format!("ptr:{d}")),
            Mechanism::Exists { domain, .. } => format!("exists:{domain}"),
            Mechanism::Include { domain, .. } => format!("include:{domain}"),
        }
    }

    /// Format record for display
    #[allow(clippy::unused_self)]
    fn format_record(&self, record: &SpfRecord) -> String {
        let mechanisms: Vec<String> = record
            .mechanisms
            .iter()
            .map(|m| {
                let prefix = m.qualifier().as_char();
                let mech = Self::format_mechanism(m);
                if prefix == '+' {
                    mech
                } else {
                    format!("{prefix}{mech}")
                }
            })
            .collect();

        let mut parts = mechanisms;
        for modifier in &record.modifiers {
            let modifier_str = match modifier {
                Modifier::Redirect(d) => format!("redirect={d}"),
                Modifier::Exp(d) => format!("exp={d}"),
                Modifier::Unknown(name, value) => format!("{name}={value}"),
            };
            parts.push(modifier_str);
        }

        parts.join(" ")
    }
}

/// Evaluation context
///
/// Tracks state during SPF evaluation including DNS lookup counts.
struct EvaluationContext<D: ?Sized> {
    _sender: String,
    _sender_local: String,
    sender_domain: String,
    client_ip: IpAddr,
    _helo_hostname: String,
    dns: Arc<D>,
    lookup_count: usize,
    _void_lookup_count: usize,
    max_lookups: usize,
    _max_void_lookups: usize,
    recursion_depth: usize,
    max_recursion_depth: usize,
}

impl<D: ?Sized + SpfDnsResolver> EvaluationContext<D> {
    /// Check if we can perform another DNS lookup
    #[allow(clippy::missing_const_for_fn)] // Modifies self.lookup_count
    fn check_lookup_limit(&mut self) -> Result<(), SpfError> {
        if self.lookup_count >= self.max_lookups {
            return Err(SpfError::TooManyLookups {
                limit: self.max_lookups,
            });
        }
        self.lookup_count += 1;
        Ok(())
    }

    /// Evaluate a mechanism against the client IP
    fn evaluate_mechanism<'a>(
        &'a mut self,
        mechanism: &'a Mechanism,
        _domain: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<bool, SpfError>> + Send + 'a>>
    {
        Box::pin(async move {
            match mechanism {
                Mechanism::All(_) => {
                    // 'all' always matches
                    Ok(true)
                }

                Mechanism::Ip4 { network, .. } => {
                    // Check if client IP matches IPv4 network
                    Ok(self.ip_matches_v4(*network))
                }

                Mechanism::Ip6 { network, .. } => {
                    // Check if client IP matches IPv6 network
                    Ok(self.ip_matches_v6(*network))
                }

                Mechanism::A {
                    domain,
                    cidr4,
                    cidr6,
                    ..
                } => {
                    self.check_lookup_limit()?;
                    let target_domain =
                        domain.as_deref().unwrap_or(&self.sender_domain).to_string();
                    self.evaluate_a_mechanism(&target_domain, *cidr4, *cidr6)
                        .await
                }

                Mechanism::Mx {
                    domain,
                    cidr4,
                    cidr6,
                    ..
                } => {
                    self.check_lookup_limit()?;
                    let target_domain =
                        domain.as_deref().unwrap_or(&self.sender_domain).to_string();
                    self.evaluate_mx_mechanism(&target_domain, *cidr4, *cidr6)
                        .await
                }

                Mechanism::Ptr { domain, .. } => {
                    self.check_lookup_limit()?;
                    let target_domain =
                        domain.as_deref().unwrap_or(&self.sender_domain).to_string();
                    self.evaluate_ptr_mechanism(&target_domain).await
                }

                Mechanism::Exists { domain, .. } => {
                    self.check_lookup_limit()?;
                    self.evaluate_exists_mechanism(domain).await
                }

                Mechanism::Include { domain, .. } => {
                    self.check_lookup_limit()?;
                    self.evaluate_include_mechanism(domain).await
                }
            }
        })
    }

    /// Check if client IP matches IPv4 network
    fn ip_matches_v4(&self, network: Ipv4Network) -> bool {
        match self.client_ip {
            IpAddr::V4(ip) => network.contains(ip),
            IpAddr::V6(_) => false,
        }
    }

    /// Check if client IP matches IPv6 network
    fn ip_matches_v6(&self, network: Ipv6Network) -> bool {
        match self.client_ip {
            IpAddr::V6(ip) => network.contains(ip),
            IpAddr::V4(_) => false,
        }
    }

    /// Evaluate A mechanism
    async fn evaluate_a_mechanism(
        &self,
        domain: &str,
        cidr4: Option<u8>,
        cidr6: Option<u8>,
    ) -> Result<bool, SpfError> {
        let ips = self.dns.a_lookup(domain).await.map_err(|e| match e {
            super::dns::DnsLookupError::Timeout(_) => SpfError::DnsTimeout,
            super::dns::DnsLookupError::LookupFailed(msg) => SpfError::DnsLookupFailed(msg),
        })?;

        for ip in ips.iter() {
            if self.ip_matches_with_cidr(*ip, cidr4, cidr6) {
                return Ok(true);
            }
        }

        Ok(false)
    }

    /// Evaluate MX mechanism
    async fn evaluate_mx_mechanism(
        &mut self,
        domain: &str,
        cidr4: Option<u8>,
        cidr6: Option<u8>,
    ) -> Result<bool, SpfError> {
        let mail_servers = self.dns.mx_lookup(domain).await.map_err(|e| match e {
            super::dns::DnsLookupError::Timeout(_) => SpfError::DnsTimeout,
            super::dns::DnsLookupError::LookupFailed(msg) => SpfError::DnsLookupFailed(msg),
        })?;

        for mx in mail_servers.iter() {
            // Look up A/AAAA records for MX host
            self.check_lookup_limit()?;
            let ips = self.dns.a_lookup(&mx.host).await.map_err(|e| match e {
                super::dns::DnsLookupError::Timeout(_) => SpfError::DnsTimeout,
                super::dns::DnsLookupError::LookupFailed(msg) => SpfError::DnsLookupFailed(msg),
            })?;

            for ip in ips.iter() {
                if self.ip_matches_with_cidr(*ip, cidr4, cidr6) {
                    return Ok(true);
                }
            }
        }

        Ok(false)
    }

    /// Evaluate PTR mechanism
    async fn evaluate_ptr_mechanism(&mut self, domain: &str) -> Result<bool, SpfError> {
        // Look up PTR records for client IP
        let ptr_names = self
            .dns
            .ptr_lookup(self.client_ip)
            .await
            .map_err(|e| match e {
                super::dns::DnsLookupError::Timeout(_) => SpfError::DnsTimeout,
                super::dns::DnsLookupError::LookupFailed(msg) => SpfError::DnsLookupFailed(msg),
            })?;

        // Check if any PTR name ends with the domain
        for ptr_name in ptr_names.iter() {
            if ptr_name.ends_with(domain) || ptr_name == domain {
                // Validate PTR (forward-confirm reverse DNS)
                self.check_lookup_limit()?;
                let ips = self.dns.a_lookup(ptr_name).await.map_err(|e| match e {
                    super::dns::DnsLookupError::Timeout(_) => SpfError::DnsTimeout,
                    super::dns::DnsLookupError::LookupFailed(msg) => SpfError::DnsLookupFailed(msg),
                })?;

                if ips.contains(&self.client_ip) {
                    return Ok(true);
                }
            }
        }

        Ok(false)
    }

    /// Evaluate exists mechanism
    async fn evaluate_exists_mechanism(&self, domain: &str) -> Result<bool, SpfError> {
        // Look up A records - exists passes if any A records exist
        let ips = self.dns.a_lookup(domain).await.map_err(|e| match e {
            super::dns::DnsLookupError::Timeout(_) => SpfError::DnsTimeout,
            super::dns::DnsLookupError::LookupFailed(msg) => SpfError::DnsLookupFailed(msg),
        })?;

        Ok(!ips.is_empty())
    }

    /// Evaluate include mechanism
    ///
    /// RFC 7208: The include mechanism triggers a recursive evaluation of SPF records.
    /// Returns true (match) only if the recursive evaluation results in Pass.
    /// Propagates `TempError` and `PermError`. Returns false for other results.
    fn evaluate_include_mechanism<'a>(
        &'a mut self,
        domain: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<bool, SpfError>> + Send + 'a>>
    {
        Box::pin(async move {
            // Check recursion depth
            if self.recursion_depth >= self.max_recursion_depth {
                return Err(SpfError::InfiniteRecursion(self.max_recursion_depth));
            }

            debug!(
                domain = %domain,
                recursion_depth = self.recursion_depth,
                "Evaluating include mechanism"
            );

            // Increment recursion depth
            self.recursion_depth += 1;

            // Look up SPF record for included domain
            let txt_records = self.dns.txt_lookup(domain).await.map_err(|e| match e {
                super::dns::DnsLookupError::Timeout(_) => SpfError::DnsTimeout,
                super::dns::DnsLookupError::LookupFailed(msg) => SpfError::DnsLookupFailed(msg),
            })?;

            // Find SPF records (start with "v=spf1")
            let spf_records: Vec<_> = txt_records
                .iter()
                .filter(|record| record.starts_with("v=spf1"))
                .collect();

            let result = match spf_records.len() {
                0 => {
                    debug!(domain = %domain, "No SPF record in included domain");
                    // RFC 7208: None result from include → PermError
                    self.recursion_depth -= 1;
                    return Err(SpfError::NoRecord);
                }
                1 => {
                    let record_str = spf_records[0];
                    debug!(
                        domain = %domain,
                        record = %record_str,
                        "Found SPF record in included domain"
                    );

                    // Parse the record
                    let record = match SpfRecord::parse(record_str) {
                        Ok(rec) => rec,
                        Err(e) => {
                            self.recursion_depth -= 1;
                            return Err(e);
                        }
                    };

                    // Evaluate mechanisms in the included record
                    let mut matched_with_pass = false;

                    for mechanism in &record.mechanisms {
                        match self.evaluate_mechanism(mechanism, domain).await {
                            Ok(true) => {
                                // Mechanism matched - check qualifier
                                if mechanism.qualifier() == Qualifier::Pass {
                                    debug!(
                                        mechanism = ?mechanism,
                                        "Include matched with Pass qualifier"
                                    );
                                    matched_with_pass = true;
                                    break;
                                }
                                // RFC 7208: Fail/SoftFail/Neutral → no match, continue
                                debug!(
                                    mechanism = ?mechanism,
                                    qualifier = ?mechanism.qualifier(),
                                    "Include matched but not Pass, continuing"
                                );
                            }
                            Ok(false) => {
                                // Mechanism didn't match - continue to next
                            }
                            Err(e) => {
                                // Error in mechanism evaluation - propagate
                                self.recursion_depth -= 1;
                                return Err(e);
                            }
                        }
                    }

                    // Check redirect modifier if no mechanism matched
                    if !matched_with_pass {
                        for modifier in &record.modifiers {
                            if let Modifier::Redirect(redirect_domain) = modifier {
                                debug!(
                                    redirect_domain = %redirect_domain,
                                    "Include: No match, checking redirect"
                                );

                                // Recursively evaluate redirect domain
                                let redirect_result =
                                    self.evaluate_include_mechanism(redirect_domain).await?;

                                self.recursion_depth -= 1;
                                return Ok(redirect_result);
                            }
                        }
                    }

                    matched_with_pass
                }
                _ => {
                    warn!(
                        domain = %domain,
                        count = spf_records.len(),
                        "Multiple SPF records in included domain (RFC 7208 violation)"
                    );
                    self.recursion_depth -= 1;
                    return Err(SpfError::InvalidRecord(
                        "Multiple SPF records found".to_string(),
                    ));
                }
            };

            // Decrement recursion depth before returning
            self.recursion_depth -= 1;

            debug!(
                domain = %domain,
                result = result,
                "Include mechanism evaluation complete"
            );

            Ok(result)
        })
    }

    /// Check if IP matches with optional CIDR
    fn ip_matches_with_cidr(&self, ip: IpAddr, cidr4: Option<u8>, cidr6: Option<u8>) -> bool {
        match (self.client_ip, ip) {
            (IpAddr::V4(client), IpAddr::V4(target)) => {
                let prefix_len = cidr4.unwrap_or(32);
                // Try to create network with specified CIDR, fall back to /32 if invalid
                let network = Ipv4Network::new(target, prefix_len).unwrap_or_else(|_| {
                    // SAFETY: /32 is always valid for any IPv4 address
                    #[allow(clippy::expect_used)]
                    Ipv4Network::new(target, 32).expect("Valid /32 network")
                });
                network.contains(client)
            }
            (IpAddr::V6(client), IpAddr::V6(target)) => {
                let prefix_len = cidr6.unwrap_or(128);
                // Try to create network with specified CIDR, fall back to /128 if invalid
                let network = Ipv6Network::new(target, prefix_len).unwrap_or_else(|_| {
                    // SAFETY: /128 is always valid for any IPv6 address
                    #[allow(clippy::expect_used)]
                    Ipv6Network::new(target, 128).expect("Valid /128 network")
                });
                network.contains(client)
            }
            _ => false, // IPv4 vs IPv6 mismatch
        }
    }
}

impl Mechanism {
    /// Get the qualifier for this mechanism
    #[must_use]
    pub const fn qualifier(&self) -> Qualifier {
        match self {
            Self::All(q)
            | Self::Ip4 { qualifier: q, .. }
            | Self::Ip6 { qualifier: q, .. }
            | Self::A { qualifier: q, .. }
            | Self::Mx { qualifier: q, .. }
            | Self::Ptr { qualifier: q, .. }
            | Self::Exists { qualifier: q, .. }
            | Self::Include { qualifier: q, .. } => *q,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use std::sync::Mutex;

    use ahash::AHashMap;

    use super::*;
    use crate::spf::dns::{DnsLookupError, MailServer};

    fn sender(s: &str) -> EmailAddress {
        EmailAddress::new(s).unwrap()
    }

    /// Mock DNS resolver for testing
    #[allow(clippy::struct_field_names)]
    struct MockDnsResolver {
        txt_records: Mutex<AHashMap<String, Vec<String>>>,
        a_records: Mutex<AHashMap<String, Vec<IpAddr>>>,
        mx_records: Mutex<AHashMap<String, Vec<MailServer>>>,
        ptr_records: Mutex<AHashMap<String, Vec<String>>>,
    }

    impl MockDnsResolver {
        fn new() -> Self {
            Self {
                txt_records: Mutex::new(AHashMap::new()),
                a_records: Mutex::new(AHashMap::new()),
                mx_records: Mutex::new(AHashMap::new()),
                ptr_records: Mutex::new(AHashMap::new()),
            }
        }

        fn add_txt(&self, domain: &str, records: Vec<String>) {
            self.txt_records
                .lock()
                .unwrap()
                .insert(domain.to_string(), records);
        }

        fn add_a(&self, domain: &str, ips: Vec<IpAddr>) {
            self.a_records
                .lock()
                .unwrap()
                .insert(domain.to_string(), ips);
        }

        fn add_mx(&self, domain: &str, servers: Vec<MailServer>) {
            self.mx_records
                .lock()
                .unwrap()
                .insert(domain.to_string(), servers);
        }

        fn add_ptr(&self, ip: IpAddr, names: Vec<String>) {
            self.ptr_records
                .lock()
                .unwrap()
                .insert(ip.to_string(), names);
        }
    }

    #[async_trait::async_trait]
    impl SpfDnsResolver for MockDnsResolver {
        async fn txt_lookup(&self, domain: &str) -> Result<Arc<Vec<String>>, DnsLookupError> {
            // Return empty vector if no records (not an error - DNS succeeded but found nothing)
            Ok(Arc::new(
                self.txt_records
                    .lock()
                    .unwrap()
                    .get(domain)
                    .cloned()
                    .unwrap_or_default(),
            ))
        }

        async fn ptr_lookup(&self, ip: IpAddr) -> Result<Arc<Vec<String>>, DnsLookupError> {
            // Return empty vector if no records
            Ok(Arc::new(
                self.ptr_records
                    .lock()
                    .unwrap()
                    .get(&ip.to_string())
                    .cloned()
                    .unwrap_or_default(),
            ))
        }

        async fn a_lookup(&self, domain: &str) -> Result<Arc<Vec<IpAddr>>, DnsLookupError> {
            // Return empty vector if no records
            Ok(Arc::new(
                self.a_records
                    .lock()
                    .unwrap()
                    .get(domain)
                    .cloned()
                    .unwrap_or_default(),
            ))
        }

        async fn mx_lookup(&self, domain: &str) -> Result<Arc<Vec<MailServer>>, DnsLookupError> {
            // Return empty vector if no records
            Ok(Arc::new(
                self.mx_records
                    .lock()
                    .unwrap()
                    .get(domain)
                    .cloned()
                    .unwrap_or_default(),
            ))
        }
    }

    #[tokio::test]
    async fn test_ip4_mechanism_match() {
        let resolver = Arc::new(MockDnsResolver::new());
        resolver.add_txt(
            "example.com",
            vec!["v=spf1 ip4:192.0.2.0/24 -all".to_string()],
        );

        let evaluator = SpfEvaluator::new(resolver, SpfConfig::default());
        let result = evaluator
            .evaluate(
                &sender("user@example.com"),
                "192.0.2.100".parse().unwrap(),
                "mail.example.com",
            )
            .await;

        assert_eq!(result.result, SpfResult::Pass);
        assert_eq!(
            result.matched_mechanism,
            Some("ip4:192.0.2.0/24".to_string())
        );
    }

    #[tokio::test]
    async fn test_ip4_mechanism_no_match() {
        let resolver = Arc::new(MockDnsResolver::new());
        resolver.add_txt(
            "example.com",
            vec!["v=spf1 ip4:192.0.2.0/24 -all".to_string()],
        );

        let evaluator = SpfEvaluator::new(resolver, SpfConfig::default());
        let result = evaluator
            .evaluate(
                &sender("user@example.com"),
                "198.51.100.1".parse().unwrap(),
                "mail.example.com",
            )
            .await;

        assert_eq!(result.result, SpfResult::Fail);
        assert_eq!(result.matched_mechanism, Some("all".to_string()));
    }

    #[tokio::test]
    async fn test_ip6_mechanism() {
        let resolver = Arc::new(MockDnsResolver::new());
        resolver.add_txt(
            "example.com",
            vec!["v=spf1 ip6:2001:db8::/32 -all".to_string()],
        );

        let evaluator = SpfEvaluator::new(resolver, SpfConfig::default());
        let result = evaluator
            .evaluate(
                &sender("user@example.com"),
                "2001:db8::1".parse().unwrap(),
                "mail.example.com",
            )
            .await;

        assert_eq!(result.result, SpfResult::Pass);
    }

    #[tokio::test]
    async fn test_a_mechanism() {
        let resolver = Arc::new(MockDnsResolver::new());
        resolver.add_txt("example.com", vec!["v=spf1 a -all".to_string()]);
        resolver.add_a("example.com", vec!["192.0.2.1".parse().unwrap()]);

        let evaluator = SpfEvaluator::new(resolver, SpfConfig::default());
        let result = evaluator
            .evaluate(
                &sender("user@example.com"),
                "192.0.2.1".parse().unwrap(),
                "mail.example.com",
            )
            .await;

        assert_eq!(result.result, SpfResult::Pass);
        assert_eq!(result.matched_mechanism, Some("a".to_string()));
    }

    #[tokio::test]
    async fn test_mx_mechanism() {
        let resolver = Arc::new(MockDnsResolver::new());
        resolver.add_txt("example.com", vec!["v=spf1 mx -all".to_string()]);
        resolver.add_mx(
            "example.com",
            vec![MailServer {
                host: "mail.example.com".to_string(),
                priority: 10,
            }],
        );
        resolver.add_a("mail.example.com", vec!["192.0.2.10".parse().unwrap()]);

        let evaluator = SpfEvaluator::new(resolver, SpfConfig::default());
        let result = evaluator
            .evaluate(
                &sender("user@example.com"),
                "192.0.2.10".parse().unwrap(),
                "mail.example.com",
            )
            .await;

        assert_eq!(result.result, SpfResult::Pass);
        assert_eq!(result.matched_mechanism, Some("mx".to_string()));
    }

    #[tokio::test]
    async fn test_all_mechanism() {
        let resolver = Arc::new(MockDnsResolver::new());
        resolver.add_txt("example.com", vec!["v=spf1 -all".to_string()]);

        let evaluator = SpfEvaluator::new(resolver, SpfConfig::default());
        let result = evaluator
            .evaluate(
                &sender("user@example.com"),
                "1.2.3.4".parse().unwrap(),
                "mail.example.com",
            )
            .await;

        assert_eq!(result.result, SpfResult::Fail);
        assert_eq!(result.matched_mechanism, Some("all".to_string()));
    }

    #[tokio::test]
    async fn test_softfail_qualifier() {
        let resolver = Arc::new(MockDnsResolver::new());
        resolver.add_txt("example.com", vec!["v=spf1 ~all".to_string()]);

        let evaluator = SpfEvaluator::new(resolver, SpfConfig::default());
        let result = evaluator
            .evaluate(
                &sender("user@example.com"),
                "1.2.3.4".parse().unwrap(),
                "mail.example.com",
            )
            .await;

        assert_eq!(result.result, SpfResult::SoftFail);
    }

    #[tokio::test]
    async fn test_neutral_qualifier() {
        let resolver = Arc::new(MockDnsResolver::new());
        resolver.add_txt("example.com", vec!["v=spf1 ?all".to_string()]);

        let evaluator = SpfEvaluator::new(resolver, SpfConfig::default());
        let result = evaluator
            .evaluate(
                &sender("user@example.com"),
                "1.2.3.4".parse().unwrap(),
                "mail.example.com",
            )
            .await;

        assert_eq!(result.result, SpfResult::Neutral);
    }

    #[tokio::test]
    async fn test_include_mechanism_pass() {
        let resolver = Arc::new(MockDnsResolver::new());
        resolver.add_txt(
            "example.com",
            vec!["v=spf1 include:_spf.example.com -all".to_string()],
        );
        resolver.add_txt(
            "_spf.example.com",
            vec!["v=spf1 ip4:192.0.2.0/24 -all".to_string()],
        );

        let evaluator = SpfEvaluator::new(resolver, SpfConfig::default());
        let result = evaluator
            .evaluate(
                &sender("user@example.com"),
                "192.0.2.100".parse().unwrap(),
                "mail.example.com",
            )
            .await;

        assert_eq!(result.result, SpfResult::Pass);
        assert_eq!(
            result.matched_mechanism,
            Some("include:_spf.example.com".to_string())
        );
    }

    #[tokio::test]
    async fn test_include_mechanism_no_match() {
        let resolver = Arc::new(MockDnsResolver::new());
        resolver.add_txt(
            "example.com",
            vec!["v=spf1 include:_spf.example.com -all".to_string()],
        );
        resolver.add_txt(
            "_spf.example.com",
            vec!["v=spf1 ip4:192.0.2.0/24 -all".to_string()],
        );

        let evaluator = SpfEvaluator::new(resolver, SpfConfig::default());
        let result = evaluator
            .evaluate(
                &sender("user@example.com"),
                "198.51.100.1".parse().unwrap(),
                "mail.example.com",
            )
            .await;

        // Include should not match (included domain returns Fail)
        // So we fall through to -all
        assert_eq!(result.result, SpfResult::Fail);
        assert_eq!(result.matched_mechanism, Some("all".to_string()));
    }

    #[tokio::test]
    async fn test_redirect_modifier() {
        let resolver = Arc::new(MockDnsResolver::new());
        resolver.add_txt(
            "example.com",
            vec!["v=spf1 redirect=_spf.example.com".to_string()],
        );
        resolver.add_txt(
            "_spf.example.com",
            vec!["v=spf1 ip4:192.0.2.0/24 -all".to_string()],
        );

        let evaluator = SpfEvaluator::new(resolver, SpfConfig::default());
        let result = evaluator
            .evaluate(
                &sender("user@example.com"),
                "192.0.2.100".parse().unwrap(),
                "mail.example.com",
            )
            .await;

        assert_eq!(result.result, SpfResult::Pass);
    }

    #[tokio::test]
    async fn test_dns_lookup_limit() {
        let resolver = Arc::new(MockDnsResolver::new());

        // Create a chain of includes that exceeds the limit
        resolver.add_txt(
            "example.com",
            vec!["v=spf1 include:spf1.example.com -all".to_string()],
        );

        for i in 1..=12 {
            resolver.add_txt(
                &format!("spf{i}.example.com"),
                vec![format!("v=spf1 include:spf{}.example.com -all", i + 1)],
            );
        }

        let evaluator = SpfEvaluator::new(resolver, SpfConfig::default());
        let result = evaluator
            .evaluate(
                &sender("user@example.com"),
                "192.0.2.1".parse().unwrap(),
                "mail.example.com",
            )
            .await;

        assert_eq!(result.result, SpfResult::PermError);
        assert!(result.explanation.unwrap().contains("Too many DNS lookups"));
    }

    #[tokio::test]
    async fn test_recursion_depth_limit() {
        let resolver = Arc::new(MockDnsResolver::new());

        // Create a deep redirect chain
        resolver.add_txt(
            "example.com",
            vec!["v=spf1 redirect=spf1.example.com".to_string()],
        );

        for i in 1..=12 {
            resolver.add_txt(
                &format!("spf{i}.example.com"),
                vec![format!("v=spf1 redirect=spf{}.example.com", i + 1)],
            );
        }

        // Use higher DNS limit to ensure we hit recursion depth limit first
        let config = SpfConfig {
            max_dns_lookups: 50,
            ..Default::default()
        };

        let evaluator = SpfEvaluator::new(resolver, config);
        let result = evaluator
            .evaluate(
                &sender("user@example.com"),
                "192.0.2.1".parse().unwrap(),
                "mail.example.com",
            )
            .await;

        assert_eq!(result.result, SpfResult::PermError);
        assert!(result.explanation.unwrap().contains("Recursion depth"));
    }

    #[tokio::test]
    async fn test_no_spf_record() {
        let resolver = Arc::new(MockDnsResolver::new());

        let evaluator = SpfEvaluator::new(resolver, SpfConfig::default());
        let result = evaluator
            .evaluate(
                &sender("user@example.com"),
                "192.0.2.1".parse().unwrap(),
                "mail.example.com",
            )
            .await;

        assert_eq!(result.result, SpfResult::None);
    }

    #[tokio::test]
    async fn test_exists_mechanism() {
        let resolver = Arc::new(MockDnsResolver::new());
        resolver.add_txt(
            "example.com",
            vec!["v=spf1 exists:%{i}.whitelist.example.com -all".to_string()],
        );
        // For now, exists just checks if the domain has A records (macro expansion not implemented)
        resolver.add_a(
            "%{i}.whitelist.example.com",
            vec!["127.0.0.1".parse().unwrap()],
        );

        let evaluator = SpfEvaluator::new(resolver, SpfConfig::default());
        let result = evaluator
            .evaluate(
                &sender("user@example.com"),
                "192.0.2.1".parse().unwrap(),
                "mail.example.com",
            )
            .await;

        assert_eq!(result.result, SpfResult::Pass);
        assert!(
            result
                .matched_mechanism
                .unwrap()
                .contains("exists:%{i}.whitelist.example.com")
        );
    }

    #[tokio::test]
    async fn test_ptr_mechanism() {
        let resolver = Arc::new(MockDnsResolver::new());
        resolver.add_txt("example.com", vec!["v=spf1 ptr -all".to_string()]);

        let client_ip: IpAddr = "192.0.2.1".parse().unwrap();
        resolver.add_ptr(client_ip, vec!["mail.example.com".to_string()]);
        resolver.add_a("mail.example.com", vec![client_ip]);

        let evaluator = SpfEvaluator::new(resolver, SpfConfig::default());
        let result = evaluator
            .evaluate(&sender("user@example.com"), client_ip, "mail.example.com")
            .await;

        assert_eq!(result.result, SpfResult::Pass);
        assert_eq!(result.matched_mechanism, Some("ptr".to_string()));
    }

    #[tokio::test]
    async fn test_multiple_mechanisms() {
        let resolver = Arc::new(MockDnsResolver::new());
        resolver.add_txt(
            "example.com",
            vec!["v=spf1 ip4:192.0.2.0/24 ip4:198.51.100.0/24 -all".to_string()],
        );

        let evaluator = SpfEvaluator::new(resolver, SpfConfig::default());

        // First IP range
        let result1 = evaluator
            .evaluate(
                &sender("user@example.com"),
                "192.0.2.50".parse().unwrap(),
                "mail.example.com",
            )
            .await;
        assert_eq!(result1.result, SpfResult::Pass);

        // Second IP range
        let result2 = evaluator
            .evaluate(
                &sender("user@example.com"),
                "198.51.100.50".parse().unwrap(),
                "mail.example.com",
            )
            .await;
        assert_eq!(result2.result, SpfResult::Pass);

        // Neither range
        let result3 = evaluator
            .evaluate(
                &sender("user@example.com"),
                "203.0.113.1".parse().unwrap(),
                "mail.example.com",
            )
            .await;
        assert_eq!(result3.result, SpfResult::Fail);
    }
}
