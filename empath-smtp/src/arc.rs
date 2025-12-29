//! ARC verification helpers.
//!
//! Uses the `mail-auth` crate to verify ARC seals, summarize results,
//! and detect replayed chains for observability.

use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use mail_auth::{ArcOutput, AuthenticatedMessage, DkimResult, MessageAuthenticator};
use parking_lot::Mutex;
use sha2::{Digest, Sha256};

const ARC_REPLAY_CACHE_SIZE: usize = 2048;
const ARC_REPLAY_TTL_SECS: u64 = 6 * 60 * 60;

#[derive(Debug, Clone)]
pub struct ArcEvaluation {
    pub result: &'static str,
    pub chain_len: usize,
    pub chain_hash: Option<String>,
    pub error: Option<String>,
    pub replayed: bool,
}

pub async fn verify(raw_message: &[u8]) -> ArcEvaluation {
    let Some(message) = AuthenticatedMessage::parse_with_opts(raw_message, true) else {
        return ArcEvaluation {
            result: "permerror",
            chain_len: 0,
            chain_hash: None,
            error: Some("Failed to parse message for ARC verification".to_string()),
            replayed: false,
        };
    };

    let Some(resolver) = arc_resolver() else {
        return ArcEvaluation {
            result: "temperror",
            chain_len: 0,
            chain_hash: None,
            error: Some("ARC resolver unavailable".to_string()),
            replayed: false,
        };
    };

    let output = resolver.verify_arc(&message).await;
    let chain_len = output.sets().len();
    let chain_hash = chain_digest(&output);
    let replayed = chain_hash
        .as_ref()
        .is_some_and(|digest| output.result() == &DkimResult::Pass && replay_cache().note(digest));

    let (result, error) = summarize_result(output.result());

    ArcEvaluation {
        result,
        chain_len,
        chain_hash,
        error,
        replayed,
    }
}

fn arc_resolver() -> Option<Arc<MessageAuthenticator>> {
    static RESOLVER: OnceLock<Option<Arc<MessageAuthenticator>>> = OnceLock::new();
    RESOLVER
        .get_or_init(|| MessageAuthenticator::new_system_conf().ok().map(Arc::new))
        .clone()
}

fn summarize_result(result: &DkimResult) -> (&'static str, Option<String>) {
    match result {
        DkimResult::Pass => ("pass", None),
        DkimResult::Fail(err) => ("fail", Some(err.to_string())),
        DkimResult::Neutral(err) => ("neutral", Some(err.to_string())),
        DkimResult::PermError(err) => ("permerror", Some(err.to_string())),
        DkimResult::TempError(err) => ("temperror", Some(err.to_string())),
        DkimResult::None => ("none", None),
    }
}

fn chain_digest(output: &ArcOutput<'_>) -> Option<String> {
    let sets = output.sets();
    if sets.is_empty() {
        return None;
    }

    let mut hasher = Sha256::new();
    for set in sets {
        hasher.update(set.seal.name);
        hasher.update(b":");
        hasher.update(set.seal.value);
        hasher.update(b"\n");
        hasher.update(set.signature.name);
        hasher.update(b":");
        hasher.update(set.signature.value);
        hasher.update(b"\n");
        hasher.update(set.results.name);
        hasher.update(b":");
        hasher.update(set.results.value);
        hasher.update(b"\n");
    }

    Some(hex::encode(hasher.finalize()))
}

fn replay_cache() -> &'static ArcReplayCache {
    static CACHE: OnceLock<ArcReplayCache> = OnceLock::new();
    CACHE.get_or_init(|| {
        ArcReplayCache::new(
            Duration::from_secs(ARC_REPLAY_TTL_SECS),
            ARC_REPLAY_CACHE_SIZE,
        )
    })
}

struct ArcReplayCache {
    ttl: Duration,
    max_entries: usize,
    inner: Mutex<ArcReplayState>,
}

struct ArcReplayState {
    entries: VecDeque<(String, Instant)>,
    seen: HashSet<String>,
}

impl ArcReplayCache {
    fn new(ttl: Duration, max_entries: usize) -> Self {
        Self {
            ttl,
            max_entries,
            inner: Mutex::new(ArcReplayState {
                entries: VecDeque::new(),
                seen: HashSet::new(),
            }),
        }
    }

    fn note(&self, digest: &str) -> bool {
        let now = Instant::now();
        let mut state = self.inner.lock();
        state.purge_expired(now);

        if state.seen.contains(digest) {
            return true;
        }

        let expires_at = now + self.ttl;
        state.entries.push_back((digest.to_string(), expires_at));
        state.seen.insert(digest.to_string());
        state.evict_overflow(self.max_entries);
        false
    }
}

impl ArcReplayState {
    fn purge_expired(&mut self, now: Instant) {
        while let Some((digest, expires_at)) = self.entries.front() {
            if *expires_at > now {
                break;
            }
            let digest = digest.clone();
            self.entries.pop_front();
            self.seen.remove(&digest);
        }
    }

    fn evict_overflow(&mut self, max_entries: usize) {
        while self.entries.len() > max_entries {
            if let Some((digest, _)) = self.entries.pop_front() {
                self.seen.remove(&digest);
            }
        }
    }
}
