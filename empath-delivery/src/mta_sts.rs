//! MTA-STS DNS discovery and policy evaluation helpers.
//!
//! This performs DNS TXT lookups for `_mta-sts.<domain>`, fetches the HTTPS
//! policy, and parses enforcement rules for delivery.

use std::fmt::{self, Display};
use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use http_body_util::{BodyExt, Empty};
use hyper::body::Bytes;
use hyper::{StatusCode, Uri};
use hyper_rustls::HttpsConnectorBuilder;
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use serde::{Deserialize, Serialize};

use crate::dns::{DnsError, DnsResolver};

const POLICY_PATH: &str = "/.well-known/mta-sts.txt";
const FETCH_TIMEOUT_SECS: u64 = 10;
const DEFAULT_MIN_CACHE_AGE_SECS: u64 = 300;
const DEFAULT_MAX_CACHE_AGE_SECS: u64 = 86_400;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MtaStsConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_min_cache_age_secs")]
    pub min_cache_age_secs: u64,
    #[serde(default = "default_max_cache_age_secs")]
    pub max_cache_age_secs: u64,
}

impl Default for MtaStsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            min_cache_age_secs: default_min_cache_age_secs(),
            max_cache_age_secs: default_max_cache_age_secs(),
        }
    }
}

const fn default_min_cache_age_secs() -> u64 {
    DEFAULT_MIN_CACHE_AGE_SECS
}

const fn default_max_cache_age_secs() -> u64 {
    DEFAULT_MAX_CACHE_AGE_SECS
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MtaStsResult {
    Present,
    None,
    TempError,
    PermError,
}

impl MtaStsResult {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Present => "present",
            Self::None => "none",
            Self::TempError => "temperror",
            Self::PermError => "permerror",
        }
    }
}

impl Display for MtaStsResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MtaStsMode {
    Enforce,
    Testing,
    None,
}

impl MtaStsMode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Enforce => "enforce",
            Self::Testing => "testing",
            Self::None => "none",
        }
    }
}

impl Display for MtaStsMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone)]
pub struct MtaStsPolicy {
    pub mode: MtaStsMode,
    pub mx: Vec<String>,
    pub max_age: u64,
}

impl MtaStsPolicy {
    #[must_use]
    pub fn matches_mx(&self, host: &str) -> bool {
        let host = normalize_host(host);
        self.mx.iter().any(|pattern| {
            pattern.strip_prefix("*.").map_or_else(
                || host == *pattern,
                |suffix| host == suffix || host.ends_with(&format!(".{suffix}")),
            )
        })
    }
}

#[derive(Debug, Clone)]
pub struct MtaStsEvaluation {
    pub result: MtaStsResult,
    pub policy_id: Option<String>,
    pub policy: Option<MtaStsPolicy>,
    pub error: Option<String>,
}

async fn fetch_policy(domain: &str) -> Result<MtaStsPolicy, String> {
    let uri: Uri = format!("https://mta-sts.{domain}{POLICY_PATH}")
        .parse()
        .map_err(|e| format!("Invalid MTA-STS URI: {e}"))?;

    let https = HttpsConnectorBuilder::new()
        .with_native_roots()
        .map_err(|e| format!("Failed to load native TLS roots: {e}"))?
        .https_only()
        .enable_http1()
        .build();
    let client: Client<_, Empty<Bytes>> = Client::builder(TokioExecutor::new()).build(https);

    let response: hyper::Response<hyper::body::Incoming> =
        tokio::time::timeout(Duration::from_secs(FETCH_TIMEOUT_SECS), client.get(uri))
            .await
            .map_err(|_| "MTA-STS policy fetch timed out".to_string())?
            .map_err(|e| format!("MTA-STS policy fetch failed: {e}"))?;

    if response.status() != StatusCode::OK {
        return Err(format!(
            "MTA-STS policy fetch failed with status {}",
            response.status()
        ));
    }

    let body: http_body_util::Collected<Bytes> = response
        .into_body()
        .collect()
        .await
        .map_err(|e| format!("MTA-STS policy body read failed: {e}"))?;
    let body_bytes = body.to_bytes();
    let body_str = std::str::from_utf8(&body_bytes)
        .map_err(|e| format!("MTA-STS policy is not UTF-8: {e}"))?;

    parse_policy(body_str)
}

fn parse_dns_record(record: &str) -> Result<String, String> {
    let mut saw_version = false;
    let mut policy_id = None;

    for part in record.split(';') {
        let trimmed = part.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Some((key, value)) = trimmed.split_once('=') else {
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        let value = value.trim();
        match key.as_str() {
            "v" => {
                saw_version = value.eq_ignore_ascii_case("STSv1");
            }
            "id" => {
                if !value.is_empty() {
                    policy_id = Some(value.to_string());
                }
            }
            _ => {}
        }
    }

    if !saw_version {
        return Err("MTA-STS record missing v=STSv1 tag".to_string());
    }

    policy_id.ok_or_else(|| "MTA-STS record missing id= tag".to_string())
}

fn parse_policy(policy: &str) -> Result<MtaStsPolicy, String> {
    let mut version_ok = false;
    let mut mode = None;
    let mut mx = Vec::new();
    let mut max_age = None;

    for line in policy.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Some((key, value)) = trimmed.split_once(':') else {
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        let value = value.trim();
        match key.as_str() {
            "version" => {
                version_ok = value.eq_ignore_ascii_case("STSv1");
            }
            "mode" => {
                mode = match value.to_ascii_lowercase().as_str() {
                    "enforce" => Some(MtaStsMode::Enforce),
                    "testing" => Some(MtaStsMode::Testing),
                    "none" => Some(MtaStsMode::None),
                    _ => None,
                };
            }
            "mx" => {
                if !value.is_empty() {
                    mx.push(normalize_host(value));
                }
            }
            "max_age" => {
                let parsed = value
                    .parse::<u64>()
                    .map_err(|_| "Invalid MTA-STS max_age value".to_string())?;
                max_age = Some(parsed);
            }
            _ => {}
        }
    }

    if !version_ok {
        return Err("MTA-STS policy missing version: STSv1".to_string());
    }

    let mode = mode.ok_or_else(|| "MTA-STS policy missing mode".to_string())?;
    let max_age = max_age.ok_or_else(|| "MTA-STS policy missing max_age".to_string())?;

    if mx.is_empty() {
        return Err("MTA-STS policy missing mx directives".to_string());
    }

    Ok(MtaStsPolicy { mode, mx, max_age })
}

fn normalize_host(host: &str) -> String {
    host.trim_end_matches('.').to_ascii_lowercase()
}

#[derive(Debug, Clone)]
struct CacheEntry {
    policy_id: String,
    policy: MtaStsPolicy,
    expires_at: Instant,
}

#[derive(Debug, Clone)]
pub struct MtaStsPolicyCache {
    entries: Arc<DashMap<String, CacheEntry>>,
    config: MtaStsConfig,
}

impl MtaStsPolicyCache {
    #[must_use]
    pub fn builder() -> MtaStsPolicyCacheBuilder {
        MtaStsPolicyCacheBuilder::new()
    }

    #[must_use]
    pub fn new(config: MtaStsConfig) -> Self {
        Self {
            entries: Arc::new(DashMap::new()),
            config,
        }
    }

    pub async fn evaluate(&self, resolver: &dyn DnsResolver, domain: &str) -> MtaStsEvaluation {
        let lookup_domain = format!("_mta-sts.{domain}");
        let txt_lookup = match resolver.txt_lookup(&lookup_domain).await {
            Ok(records) => records,
            Err(err) => {
                let result = if err.is_temporary() {
                    MtaStsResult::TempError
                } else if matches!(err, DnsError::DomainNotFound(_)) {
                    MtaStsResult::None
                } else {
                    MtaStsResult::PermError
                };
                return MtaStsEvaluation {
                    result,
                    policy_id: None,
                    policy: None,
                    error: Some(err.to_string()),
                };
            }
        };

        let sts_records: Vec<&str> = txt_lookup
            .iter()
            .map(String::as_str)
            .filter(|record| {
                record
                    .trim_start()
                    .to_ascii_lowercase()
                    .starts_with("v=stsv1")
            })
            .collect();

        let Some(record_value) = (match sts_records.len() {
            0 => None,
            1 => Some(sts_records[0]),
            _ => {
                return MtaStsEvaluation {
                    result: MtaStsResult::PermError,
                    policy_id: None,
                    policy: None,
                    error: Some("Multiple MTA-STS records found".to_string()),
                };
            }
        }) else {
            return MtaStsEvaluation {
                result: MtaStsResult::None,
                policy_id: None,
                policy: None,
                error: None,
            };
        };

        let policy_id = match parse_dns_record(record_value) {
            Ok(policy_id) => policy_id,
            Err(err) => {
                return MtaStsEvaluation {
                    result: MtaStsResult::PermError,
                    policy_id: None,
                    policy: None,
                    error: Some(err),
                };
            }
        };

        match fetch_policy(domain).await {
            Ok(policy) => MtaStsEvaluation {
                result: MtaStsResult::Present,
                policy_id: Some(policy_id),
                policy: Some(policy),
                error: None,
            },
            Err(err) => MtaStsEvaluation {
                result: MtaStsResult::TempError,
                policy_id: Some(policy_id),
                policy: None,
                error: Some(err),
            },
        }
    }

    pub async fn evaluate_cached(
        &self,
        resolver: &dyn DnsResolver,
        domain: &str,
    ) -> MtaStsEvaluation {
        let now = Instant::now();
        if let Some(entry) = self.entries.get(domain)
            && now < entry.expires_at
        {
            return MtaStsEvaluation {
                result: MtaStsResult::Present,
                policy_id: Some(entry.policy_id.clone()),
                policy: Some(entry.policy.clone()),
                error: None,
            };
        }

        let evaluation = self.evaluate(resolver, domain).await;
        if let Some(policy) = &evaluation.policy {
            let ttl = policy.max_age.clamp(
                self.config.min_cache_age_secs,
                self.config.max_cache_age_secs,
            );
            let entry = CacheEntry {
                policy_id: evaluation
                    .policy_id
                    .clone()
                    .unwrap_or_else(|| "unknown".to_string()),
                policy: policy.clone(),
                expires_at: Instant::now() + Duration::from_secs(ttl),
            };
            self.entries.insert(domain.to_string(), entry);
        } else if matches!(
            evaluation.result,
            MtaStsResult::None | MtaStsResult::PermError
        ) {
            self.entries.remove(domain);
        }

        evaluation
    }
}

pub struct MtaStsPolicyCacheBuilder {
    config: MtaStsConfig,
}

impl Default for MtaStsPolicyCacheBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl MtaStsPolicyCacheBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self {
            config: MtaStsConfig::default(),
        }
    }

    #[must_use]
    pub const fn with_enabled(mut self, enabled: bool) -> Self {
        self.config.enabled = enabled;
        self
    }

    #[must_use]
    pub const fn with_min_cache_age_secs(mut self, min_cache_age_secs: u64) -> Self {
        self.config.min_cache_age_secs = min_cache_age_secs;
        self
    }

    #[must_use]
    pub const fn with_max_cache_age_secs(mut self, max_cache_age_secs: u64) -> Self {
        self.config.max_cache_age_secs = max_cache_age_secs;
        self
    }

    #[must_use]
    pub fn build(self) -> MtaStsPolicyCache {
        MtaStsPolicyCache::new(self.config)
    }
}

impl Default for MtaStsPolicyCache {
    fn default() -> Self {
        Self::new(MtaStsConfig::default())
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::parse_dns_record;

    fn id_strategy() -> impl Strategy<Value = String> {
        let chars: Vec<char> = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-_"
            .chars()
            .collect();
        prop::collection::vec(prop::sample::select(chars), 1..32)
            .prop_map(|chars| chars.into_iter().collect())
    }

    proptest! {
        #[test]
        fn parse_dns_record_returns_id_when_version_present(id in id_strategy()) {
            let record = format!("v=STSv1; id={id}");
            let parsed = parse_dns_record(&record);
            prop_assert!(matches!(parsed, Ok(value) if value == id));
        }

        #[test]
        fn parse_dns_record_is_case_insensitive_for_version(id in id_strategy()) {
            let record = format!("v=stsv1; id={id}");
            let parsed = parse_dns_record(&record);
            prop_assert!(matches!(parsed, Ok(value) if value == id));
        }

        #[test]
        fn parse_dns_record_ignores_extra_fields(id in id_strategy()) {
            let record = format!("foo=bar; v=STSv1; id={id}; note=ignored");
            let parsed = parse_dns_record(&record);
            prop_assert!(matches!(parsed, Ok(value) if value == id));
        }

        #[test]
        fn parse_dns_record_requires_version_tag(id in id_strategy()) {
            let record = format!("id={id}; note=missing-version");
            prop_assert!(parse_dns_record(&record).is_err());
        }
    }
}
