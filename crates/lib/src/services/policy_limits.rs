//! Policy / rate-limit gate.
//!
//! A centralized rate-limiter and quota service that providers and tools
//! consult before issuing expensive calls. Buckets are keyed by
//! `(provider, scope)` so the same engine can enforce per-user, per-org,
//! or per-project quotas without callers having to wire each one
//! separately.
//!
//! # Model
//!
//! Each provider has up to three independent limiters:
//!
//! - **Requests-per-window** — a sliding-window log of recent acquire
//!   timestamps. The window length is fixed per limit (per-minute,
//!   per-hour, per-day).
//! - **Tokens-per-window** — same sliding-window log, but each entry
//!   carries the token cost.
//! - **Concurrency cap** — a `tokio::sync::Semaphore` whose permit is
//!   held by the returned [`PolicyPermit`] for the duration of the call.
//!
//! When [`PolicyService::acquire`] is called the service:
//!
//! 1. Drains expired entries from each rolling window.
//! 2. Errors out fast with [`PolicyError::WouldExceedBudget`] if the
//!    requested estimate alone is bigger than a window cap.
//! 3. Waits (with exponential-ish backoff capped at 1s) until the
//!    request fits, then records the acquire.
//! 4. Acquires the concurrency semaphore via `acquire_owned`, so if the
//!    caller's tokio task is cancelled mid-await the permit is dropped
//!    cleanly rather than leaking.
//!
//! Callers should call [`PolicyPermit::commit_tokens`] after the
//!  upstream call returns so the actual token usage replaces the
//! pre-flight estimate. The permit itself releases the concurrency slot
//! on drop.
//!
//! # Opt-in
//!
//! If no `[limits]` section is configured the service runs in no-op
//! mode: [`PolicyService::acquire`] returns immediately with a permit
//! whose drop / commit are inert. This is what the default
//! [`PolicyConfig`] builds, so wiring callers behind the service has
//! zero effect on users who haven't configured limits.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore};
use tracing::debug;

/// Scope for a policy key — who the quota applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PolicyScope {
    User,
    Org,
    Project,
}

impl PolicyScope {
    fn as_str(self) -> &'static str {
        match self {
            PolicyScope::User => "user",
            PolicyScope::Org => "org",
            PolicyScope::Project => "project",
        }
    }
}

/// Identifier for a single limit bucket.
///
/// `provider` is the provider name as it appears in `[limits.<name>]`.
/// It is treated as opaque by the service — any string is valid; the
/// caller picks whatever identity scheme matches its provider
/// registry.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PolicyKey {
    pub provider: String,
    pub scope: PolicyScope,
}

impl PolicyKey {
    pub fn new(provider: impl Into<String>, scope: PolicyScope) -> Self {
        Self {
            provider: provider.into(),
            scope,
        }
    }
}

/// Configuration for a single provider's limits.
///
/// All fields are optional — an absent value means "no limit on this
/// dimension". A provider entry with every field `None` is equivalent
/// to having no entry at all (no-op).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ProviderLimits {
    /// Max requests in any rolling 60-second window.
    pub requests_per_minute: Option<u64>,
    /// Max requests in any rolling 60-minute window.
    pub requests_per_hour: Option<u64>,
    /// Max tokens in any rolling 60-second window.
    pub tokens_per_minute: Option<u64>,
    /// Max tokens in any rolling 60-minute window.
    pub tokens_per_hour: Option<u64>,
    /// Max tokens in any rolling 24-hour window.
    pub tokens_per_day: Option<u64>,
    /// Max number of in-flight requests at once. `None` = unbounded.
    pub max_concurrent: Option<u32>,
}

impl ProviderLimits {
    fn is_noop(&self) -> bool {
        self.requests_per_minute.is_none()
            && self.requests_per_hour.is_none()
            && self.tokens_per_minute.is_none()
            && self.tokens_per_hour.is_none()
            && self.tokens_per_day.is_none()
            && self.max_concurrent.is_none()
    }
}

/// Top-level `[limits]` config.
///
/// Maps a provider name (matching whatever the rest of the system uses
/// for provider identity) to that provider's limits. Absent / empty =
/// no-op.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(transparent)]
pub struct PolicyConfig {
    pub providers: HashMap<String, ProviderLimits>,
}

impl PolicyConfig {
    pub fn is_empty(&self) -> bool {
        self.providers.is_empty() || self.providers.values().all(ProviderLimits::is_noop)
    }
}

impl From<&crate::config::ProviderLimitsConfig> for ProviderLimits {
    fn from(c: &crate::config::ProviderLimitsConfig) -> Self {
        Self {
            requests_per_minute: c.requests_per_minute,
            requests_per_hour: c.requests_per_hour,
            tokens_per_minute: c.tokens_per_minute,
            tokens_per_hour: c.tokens_per_hour,
            tokens_per_day: c.tokens_per_day,
            max_concurrent: c.max_concurrent,
        }
    }
}

impl From<&HashMap<String, crate::config::ProviderLimitsConfig>> for PolicyConfig {
    fn from(map: &HashMap<String, crate::config::ProviderLimitsConfig>) -> Self {
        let providers = map.iter().map(|(k, v)| (k.clone(), v.into())).collect();
        Self { providers }
    }
}

/// Errors returned by the policy service.
#[derive(Debug, Error, Clone, PartialEq)]
pub enum PolicyError {
    /// The request as estimated would exceed a hard budget cap. Even
    /// waiting indefinitely cannot satisfy it because a single request
    /// is bigger than the window cap. Caller must reduce the request
    /// (split it, or use a smaller model).
    #[error("would exceed budget: {0}")]
    WouldExceedBudget(String),
}

/// Snapshot of a bucket's current usage. Returned by
/// [`PolicyService::current_state`] for human-facing readouts (e.g.
/// `/status`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PolicyState {
    pub requests_last_minute: u64,
    pub requests_last_hour: u64,
    pub tokens_last_minute: u64,
    pub tokens_last_hour: u64,
    pub tokens_last_day: u64,
    pub in_flight: u32,
    pub max_concurrent: Option<u32>,
}

/// Telemetry hook fired on quota events.
///
/// Implementations are expected to be cheap — events fire on the hot
/// path of every provider call. The default service uses a no-op
/// implementation.
pub trait PolicyTelemetry: Send + Sync {
    fn on_event(&self, event: PolicyEvent<'_>);
}

/// A telemetry event emitted by the policy service.
#[derive(Debug, Clone)]
pub struct PolicyEvent<'a> {
    pub kind: PolicyEventKind,
    pub provider: &'a str,
    pub scope: PolicyScope,
    pub est_tokens: u64,
    pub actual_tokens: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyEventKind {
    /// A permit was granted.
    Acquired,
    /// `commit_tokens` was called on a permit, replacing its estimate
    /// with the actual usage.
    Committed,
    /// `acquire` failed with [`PolicyError::WouldExceedBudget`].
    Rejected,
}

struct NoopTelemetry;
impl PolicyTelemetry for NoopTelemetry {
    fn on_event(&self, event: PolicyEvent<'_>) {
        debug!(
            provider = event.provider,
            scope = event.scope.as_str(),
            kind = ?event.kind,
            est_tokens = event.est_tokens,
            actual_tokens = event.actual_tokens,
            "policy event"
        );
    }
}

/// One sliding-window entry: an instant + a token cost (1 for
/// per-request windows, the token estimate for per-token windows).
#[derive(Debug, Clone, Copy)]
struct WindowEntry {
    at: Instant,
    cost: u64,
    /// Index assigned at insert time so token-commit can find the
    /// right entry to update without colliding when multiple
    /// concurrent acquires share the same `Instant`.
    id: u64,
}

#[derive(Debug)]
struct SlidingWindow {
    cap: u64,
    duration: Duration,
    entries: VecDeque<WindowEntry>,
    /// Running sum of `entries.iter().map(|e| e.cost).sum()`. Kept in
    /// sync to avoid an O(n) scan on every check.
    used: u64,
}

impl SlidingWindow {
    fn new(cap: u64, duration: Duration) -> Self {
        Self {
            cap,
            duration,
            entries: VecDeque::new(),
            used: 0,
        }
    }

    fn drain_expired(&mut self, now: Instant) {
        while let Some(front) = self.entries.front() {
            if now.duration_since(front.at) >= self.duration {
                self.used = self.used.saturating_sub(front.cost);
                self.entries.pop_front();
            } else {
                break;
            }
        }
    }

    /// `Some(wait_until)` if `cost` doesn't fit right now.
    fn earliest_fit(&self, cost: u64, now: Instant) -> Option<Instant> {
        if self.used + cost <= self.cap {
            return None;
        }
        // We need enough headroom to free at least
        // `used + cost - cap` tokens. Walk the queue and find the
        // first entry whose expiry would free that much.
        let needed = (self.used + cost).saturating_sub(self.cap);
        let mut freed = 0u64;
        for entry in &self.entries {
            freed += entry.cost;
            if freed >= needed {
                return Some(entry.at + self.duration);
            }
        }
        // Caller should have caught oversize-vs-cap above; treat as
        // "wait for full window" defensively.
        Some(now + self.duration)
    }

    fn record(&mut self, cost: u64, at: Instant, id: u64) {
        self.entries.push_back(WindowEntry { at, cost, id });
        self.used += cost;
    }

    /// Replace the cost of the entry with `id`. Used when committing
    /// actual token counts after the call returns. Returns the delta
    /// applied.
    fn adjust(&mut self, id: u64, new_cost: u64) -> i64 {
        for entry in self.entries.iter_mut() {
            if entry.id == id {
                let old = entry.cost;
                entry.cost = new_cost;
                self.used = self.used.saturating_sub(old).saturating_add(new_cost);
                return new_cost as i64 - old as i64;
            }
        }
        0
    }
}

#[derive(Debug)]
struct BucketState {
    /// Sliding window for raw request count (cost always = 1).
    requests_per_min: Option<SlidingWindow>,
    requests_per_hour: Option<SlidingWindow>,
    /// Sliding windows for tokens (cost = est_tokens at acquire,
    /// updated to actual on commit).
    tokens_per_min: Option<SlidingWindow>,
    tokens_per_hour: Option<SlidingWindow>,
    tokens_per_day: Option<SlidingWindow>,
    next_id: u64,
}

impl BucketState {
    fn from_limits(limits: &ProviderLimits) -> Self {
        Self {
            requests_per_min: limits
                .requests_per_minute
                .map(|cap| SlidingWindow::new(cap, Duration::from_secs(60))),
            requests_per_hour: limits
                .requests_per_hour
                .map(|cap| SlidingWindow::new(cap, Duration::from_secs(3600))),
            tokens_per_min: limits
                .tokens_per_minute
                .map(|cap| SlidingWindow::new(cap, Duration::from_secs(60))),
            tokens_per_hour: limits
                .tokens_per_hour
                .map(|cap| SlidingWindow::new(cap, Duration::from_secs(3600))),
            tokens_per_day: limits
                .tokens_per_day
                .map(|cap| SlidingWindow::new(cap, Duration::from_secs(86400))),
            next_id: 0,
        }
    }

    fn drain_all(&mut self, now: Instant) {
        for w in [
            self.requests_per_min.as_mut(),
            self.requests_per_hour.as_mut(),
            self.tokens_per_min.as_mut(),
            self.tokens_per_hour.as_mut(),
            self.tokens_per_day.as_mut(),
        ]
        .into_iter()
        .flatten()
        {
            w.drain_expired(now);
        }
    }

    /// Check if `est_tokens` alone is bigger than any token window's
    /// cap. If so, even an empty bucket can't satisfy the request and
    /// we fail immediately rather than wait forever.
    fn check_oversized(&self, est_tokens: u64) -> Result<(), PolicyError> {
        for (label, window, dim) in [
            (
                "tokens_per_minute",
                self.tokens_per_min.as_ref(),
                est_tokens,
            ),
            ("tokens_per_hour", self.tokens_per_hour.as_ref(), est_tokens),
            ("tokens_per_day", self.tokens_per_day.as_ref(), est_tokens),
        ] {
            if let Some(w) = window
                && dim > w.cap
            {
                return Err(PolicyError::WouldExceedBudget(format!(
                    "request of {dim} tokens exceeds {label} cap of {}",
                    w.cap
                )));
            }
        }
        // Single requests can't exceed a per-minute / per-hour
        // request-count cap unless the cap is 0, but we still surface
        // that case clearly.
        for (label, window) in [
            ("requests_per_minute", self.requests_per_min.as_ref()),
            ("requests_per_hour", self.requests_per_hour.as_ref()),
        ] {
            if let Some(w) = window
                && w.cap == 0
            {
                return Err(PolicyError::WouldExceedBudget(format!(
                    "{label} cap is 0; no requests permitted",
                )));
            }
        }
        Ok(())
    }

    /// `None` = fits now. `Some(t)` = earliest moment it might fit.
    fn earliest_fit(&self, est_tokens: u64, now: Instant) -> Option<Instant> {
        let mut latest: Option<Instant> = None;
        let take = |latest: &mut Option<Instant>, candidate: Option<Instant>| {
            if let Some(c) = candidate {
                *latest = Some(latest.map_or(c, |cur| cur.max(c)));
            }
        };
        if let Some(w) = &self.requests_per_min {
            take(&mut latest, w.earliest_fit(1, now));
        }
        if let Some(w) = &self.requests_per_hour {
            take(&mut latest, w.earliest_fit(1, now));
        }
        if let Some(w) = &self.tokens_per_min {
            take(&mut latest, w.earliest_fit(est_tokens, now));
        }
        if let Some(w) = &self.tokens_per_hour {
            take(&mut latest, w.earliest_fit(est_tokens, now));
        }
        if let Some(w) = &self.tokens_per_day {
            take(&mut latest, w.earliest_fit(est_tokens, now));
        }
        latest
    }

    fn record(&mut self, est_tokens: u64, now: Instant) -> u64 {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        if let Some(w) = self.requests_per_min.as_mut() {
            w.record(1, now, id);
        }
        if let Some(w) = self.requests_per_hour.as_mut() {
            w.record(1, now, id);
        }
        if let Some(w) = self.tokens_per_min.as_mut() {
            w.record(est_tokens, now, id);
        }
        if let Some(w) = self.tokens_per_hour.as_mut() {
            w.record(est_tokens, now, id);
        }
        if let Some(w) = self.tokens_per_day.as_mut() {
            w.record(est_tokens, now, id);
        }
        id
    }

    fn commit(&mut self, id: u64, actual_tokens: u64) {
        for w in [
            self.tokens_per_min.as_mut(),
            self.tokens_per_hour.as_mut(),
            self.tokens_per_day.as_mut(),
        ]
        .into_iter()
        .flatten()
        {
            w.adjust(id, actual_tokens);
        }
    }

    fn snapshot(&self, max_concurrent: Option<u32>, in_flight: u32) -> PolicyState {
        PolicyState {
            requests_last_minute: self.requests_per_min.as_ref().map_or(0, |w| w.used),
            requests_last_hour: self.requests_per_hour.as_ref().map_or(0, |w| w.used),
            tokens_last_minute: self.tokens_per_min.as_ref().map_or(0, |w| w.used),
            tokens_last_hour: self.tokens_per_hour.as_ref().map_or(0, |w| w.used),
            tokens_last_day: self.tokens_per_day.as_ref().map_or(0, |w| w.used),
            in_flight,
            max_concurrent,
        }
    }
}

#[derive(Debug)]
struct Bucket {
    state: Mutex<BucketState>,
    semaphore: Option<Arc<Semaphore>>,
    max_concurrent: Option<u32>,
}

/// Handle for an in-flight call.
///
/// Holds the concurrency permit for its provider until dropped, and
/// remembers the bucket entry id so [`commit_tokens`] can replace the
/// pre-flight estimate with the actual usage.
///
/// Cancelling the awaiting task before `acquire` returns drops the
/// future cleanly and never produces a permit, so there is nothing to
/// leak. After `acquire` returns, dropping the permit releases the
/// concurrency slot.
///
/// [`commit_tokens`]: PolicyPermit::commit_tokens
#[must_use = "permit must be held until the underlying call completes"]
pub struct PolicyPermit {
    inner: Option<PermitInner>,
}

impl std::fmt::Debug for PolicyPermit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PolicyPermit")
            .field("active", &self.inner.is_some())
            .finish()
    }
}

struct PermitInner {
    service: Arc<PolicyServiceInner>,
    key: PolicyKey,
    bucket_id: u64,
    est_tokens: u64,
    /// Owned semaphore permit; tied to this struct's lifetime, so a
    /// panic or drop releases the slot.
    _semaphore_permit: Option<OwnedSemaphorePermit>,
}

impl PolicyPermit {
    /// No-op permit for unconfigured providers.
    fn noop() -> Self {
        Self { inner: None }
    }

    /// Replace the pre-flight token estimate with the actual usage.
    ///
    /// Call this once after the upstream provider returns, with the
    /// real total token count. It updates the rolling token windows
    /// in place. Calling it twice on the same permit is a no-op on
    /// the second call.
    pub async fn commit_tokens(&mut self, actual_tokens: u64) {
        let Some(inner) = self.inner.as_mut() else {
            return;
        };
        if let Some(bucket) = inner.service.buckets.get(&inner.key) {
            let mut state = bucket.state.lock().await;
            state.commit(inner.bucket_id, actual_tokens);
        }
        inner.service.telemetry.on_event(PolicyEvent {
            kind: PolicyEventKind::Committed,
            provider: &inner.key.provider,
            scope: inner.key.scope,
            est_tokens: inner.est_tokens,
            actual_tokens: Some(actual_tokens),
        });
        // Mark committed so a second call is inert.
        inner.est_tokens = actual_tokens;
        inner.bucket_id = u64::MAX;
    }
}

/// Inner shared state. `Arc`-wrapped so [`PolicyPermit`] can reach
/// back into the service from `commit_tokens`.
struct PolicyServiceInner {
    buckets: HashMap<PolicyKey, Bucket>,
    telemetry: Box<dyn PolicyTelemetry>,
}

impl std::fmt::Debug for PolicyServiceInner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PolicyServiceInner")
            .field("bucket_count", &self.buckets.len())
            .finish()
    }
}

/// Centralized policy / rate-limit gate.
///
/// Cheap to clone: the inner state is `Arc`-shared.
#[derive(Clone, Debug)]
pub struct PolicyService {
    inner: Arc<PolicyServiceInner>,
}

impl PolicyService {
    /// Build a service from the given config. An empty config (no
    /// providers, or every provider with all-`None` fields) produces
    /// a service that grants permits instantly with no bookkeeping.
    pub fn new(config: PolicyConfig) -> Self {
        Self::with_telemetry(config, Box::new(NoopTelemetry))
    }

    /// Build a service with a custom telemetry sink.
    pub fn with_telemetry(config: PolicyConfig, telemetry: Box<dyn PolicyTelemetry>) -> Self {
        let mut buckets = HashMap::new();
        for (provider, limits) in config.providers {
            if limits.is_noop() {
                continue;
            }
            for scope in [PolicyScope::User, PolicyScope::Org, PolicyScope::Project] {
                let key = PolicyKey::new(provider.clone(), scope);
                let semaphore = limits
                    .max_concurrent
                    .map(|n| Arc::new(Semaphore::new(n as usize)));
                buckets.insert(
                    key,
                    Bucket {
                        state: Mutex::new(BucketState::from_limits(&limits)),
                        semaphore,
                        max_concurrent: limits.max_concurrent,
                    },
                );
            }
        }
        Self {
            inner: Arc::new(PolicyServiceInner { buckets, telemetry }),
        }
    }

    /// Returns true when the service has no work to do — every
    /// `acquire` returns instantly.
    pub fn is_noop(&self) -> bool {
        self.inner.buckets.is_empty()
    }

    /// Acquire a permit for an upcoming provider call.
    ///
    /// Awaits if the bucket is full but could become non-full. Errors
    /// immediately if `est_tokens` alone exceeds a hard cap.
    ///
    /// The returned [`PolicyPermit`] holds the concurrency slot until
    /// dropped. Call [`PolicyPermit::commit_tokens`] when the call
    /// returns to record actual usage; otherwise the pre-flight
    /// estimate stays in the rolling window.
    pub async fn acquire(
        &self,
        key: PolicyKey,
        est_tokens: u64,
    ) -> Result<PolicyPermit, PolicyError> {
        let Some(bucket) = self.inner.buckets.get(&key) else {
            return Ok(PolicyPermit::noop());
        };

        // Concurrency admission FIRST. Recording usage in the sliding
        // windows before waiting on the semaphore would let a queued
        // (or canceled-while-queued) caller consume quota without ever
        // making a provider call — with `max_concurrent = 1`, repeated
        // cancellations would permanently throttle later real callers
        // until the windows expired. Holding the permit before we
        // record means the future is now committed: if the caller
        // drops us here, the `OwnedSemaphorePermit` releases on its
        // own (acquire_owned ties it to a struct, not the await
        // point) and nothing else has been touched.
        let semaphore_permit = if let Some(sem) = bucket.semaphore.clone() {
            Some(sem.acquire_owned().await.expect("semaphore never closed"))
        } else {
            None
        };

        // Window admission. Loop until it fits or we error out. The
        // permit we hold above keeps `max_concurrent` honest while we
        // wait for headroom; on cancellation here, the permit drops
        // and no quota was recorded.
        let bucket_id;
        loop {
            let now = Instant::now();
            let wait_until = {
                let mut state = bucket.state.lock().await;
                state.drain_all(now);
                state.check_oversized(est_tokens).inspect_err(|_| {
                    self.inner.telemetry.on_event(PolicyEvent {
                        kind: PolicyEventKind::Rejected,
                        provider: &key.provider,
                        scope: key.scope,
                        est_tokens,
                        actual_tokens: None,
                    });
                })?;
                match state.earliest_fit(est_tokens, now) {
                    None => {
                        // Fits — record and exit the loop. We hold
                        // the semaphore permit, so the caller is now
                        // committed: any subsequent drop releases the
                        // permit and the `est_tokens` counted here
                        // are reconciled by `commit_tokens` (or
                        // remain as the worst-case estimate if the
                        // caller never commits).
                        let id = state.record(est_tokens, now);
                        Some(Ok(id))
                    }
                    Some(t) => Some(Err(t)),
                }
            };
            match wait_until {
                Some(Ok(id)) => {
                    bucket_id = id;
                    break;
                }
                Some(Err(t)) => {
                    let sleep = t
                        .saturating_duration_since(Instant::now())
                        .min(Duration::from_secs(1))
                        .max(Duration::from_millis(5));
                    tokio::time::sleep(sleep).await;
                }
                None => unreachable!(),
            }
        }

        self.inner.telemetry.on_event(PolicyEvent {
            kind: PolicyEventKind::Acquired,
            provider: &key.provider,
            scope: key.scope,
            est_tokens,
            actual_tokens: None,
        });

        Ok(PolicyPermit {
            inner: Some(PermitInner {
                service: Arc::clone(&self.inner),
                key,
                bucket_id,
                est_tokens,
                _semaphore_permit: semaphore_permit,
            }),
        })
    }

    /// Snapshot of the current bucket state for the given key.
    ///
    /// For unconfigured keys returns an all-zero state. Useful for
    /// surfacing usage in `/status` and similar UI.
    pub fn current_state(&self, key: PolicyKey) -> PolicyState {
        let Some(bucket) = self.inner.buckets.get(&key) else {
            return PolicyState::default();
        };
        // Best-effort sync read. The lock is short-lived.
        let state = match bucket.state.try_lock() {
            Ok(s) => s,
            Err(_) => return PolicyState::default(),
        };
        let in_flight = bucket
            .semaphore
            .as_ref()
            .and_then(|s| {
                bucket
                    .max_concurrent
                    .map(|cap| cap.saturating_sub(s.available_permits() as u32))
            })
            .unwrap_or(0);
        let mut snapshot = state.snapshot(bucket.max_concurrent, in_flight);
        // Drain expired entries lazily for a more accurate read.
        // (Read-only callers don't see updates without this because
        // drain only happens on acquire.)
        drop(state);
        let now = Instant::now();
        if let Ok(mut s) = bucket.state.try_lock() {
            s.drain_all(now);
            snapshot = s.snapshot(bucket.max_concurrent, in_flight);
        }
        snapshot
    }
}

impl Default for PolicyService {
    fn default() -> Self {
        Self::new(PolicyConfig::default())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Duration;

    fn cfg(provider: &str, limits: ProviderLimits) -> PolicyConfig {
        let mut c = PolicyConfig::default();
        c.providers.insert(provider.to_string(), limits);
        c
    }

    #[tokio::test]
    async fn empty_config_is_noop() {
        let svc = PolicyService::new(PolicyConfig::default());
        assert!(svc.is_noop());
        // Should grant a permit immediately and not panic on commit.
        let mut permit = svc
            .acquire(PolicyKey::new("anything", PolicyScope::User), 9_999_999)
            .await
            .unwrap();
        permit.commit_tokens(1_000).await;
        // State for an unconfigured key is all zero.
        let s = svc.current_state(PolicyKey::new("anything", PolicyScope::User));
        assert_eq!(s, PolicyState::default());
    }

    #[tokio::test]
    async fn all_none_provider_is_noop() {
        let svc = PolicyService::new(cfg("p", ProviderLimits::default()));
        assert!(svc.is_noop());
        let _ = svc
            .acquire(PolicyKey::new("p", PolicyScope::User), 100)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn token_bucket_drains_and_refills() {
        let svc = PolicyService::new(cfg(
            "p",
            ProviderLimits {
                tokens_per_minute: Some(1_000),
                ..Default::default()
            },
        ));
        let key = PolicyKey::new("p", PolicyScope::User);

        // First acquire fills 600/1000.
        let _p1 = svc.acquire(key.clone(), 600).await.unwrap();
        let s = svc.current_state(key.clone());
        assert_eq!(s.tokens_last_minute, 600);

        // Second fits in remaining 400.
        let _p2 = svc.acquire(key.clone(), 400).await.unwrap();
        let s = svc.current_state(key.clone());
        assert_eq!(s.tokens_last_minute, 1000);

        // Bucket is full — third would block. Manually probe via
        // earliest_fit instead of waiting a full minute.
        let bucket = svc.inner.buckets.get(&key).unwrap();
        let st = bucket.state.lock().await;
        assert!(st.earliest_fit(1, Instant::now()).is_some());
    }

    #[tokio::test]
    async fn would_exceed_budget_errors_fast() {
        let svc = PolicyService::new(cfg(
            "p",
            ProviderLimits {
                tokens_per_minute: Some(1_000),
                ..Default::default()
            },
        ));
        let err = svc
            .acquire(PolicyKey::new("p", PolicyScope::User), 1_500)
            .await
            .unwrap_err();
        assert!(matches!(err, PolicyError::WouldExceedBudget(_)));
        let msg = format!("{err}");
        assert!(msg.contains("1500"), "msg = {msg}");
        assert!(msg.contains("1000"), "msg = {msg}");
    }

    #[tokio::test]
    async fn concurrency_cap_enforced() {
        let svc = PolicyService::new(cfg(
            "p",
            ProviderLimits {
                max_concurrent: Some(3),
                ..Default::default()
            },
        ));
        let key = PolicyKey::new("p", PolicyScope::User);
        let in_flight = Arc::new(AtomicU32::new(0));
        let observed_max = Arc::new(AtomicU32::new(0));

        let mut handles = Vec::new();
        for _ in 0..10 {
            let svc = svc.clone();
            let key = key.clone();
            let in_flight = in_flight.clone();
            let observed_max = observed_max.clone();
            handles.push(tokio::spawn(async move {
                let _permit = svc.acquire(key, 0).await.unwrap();
                let cur = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                let mut prev = observed_max.load(Ordering::SeqCst);
                while cur > prev {
                    match observed_max.compare_exchange(
                        prev,
                        cur,
                        Ordering::SeqCst,
                        Ordering::SeqCst,
                    ) {
                        Ok(_) => break,
                        Err(p) => prev = p,
                    }
                }
                tokio::time::sleep(Duration::from_millis(30)).await;
                in_flight.fetch_sub(1, Ordering::SeqCst);
            }));
        }
        for h in handles {
            h.await.unwrap();
        }
        let max = observed_max.load(Ordering::SeqCst);
        assert!(max <= 3, "observed max in-flight = {max}, expected <= 3");
        assert!(max >= 1);
    }

    #[tokio::test]
    async fn permit_drop_releases_concurrency_slot() {
        let svc = PolicyService::new(cfg(
            "p",
            ProviderLimits {
                max_concurrent: Some(1),
                ..Default::default()
            },
        ));
        let key = PolicyKey::new("p", PolicyScope::User);

        {
            let _p = svc.acquire(key.clone(), 0).await.unwrap();
            // While held, a second acquire would block. Verify that
            // by racing with a short timeout.
            let svc2 = svc.clone();
            let key2 = key.clone();
            let pending = tokio::spawn(async move {
                let _ = svc2.acquire(key2, 0).await.unwrap();
            });
            let r = tokio::time::timeout(Duration::from_millis(50), pending).await;
            assert!(r.is_err(), "second acquire should still be blocked");
        }
        // Drop released — second acquire should now succeed quickly.
        let r = tokio::time::timeout(Duration::from_millis(500), svc.acquire(key, 0))
            .await
            .expect("should not time out");
        assert!(r.is_ok());
    }

    #[tokio::test]
    async fn commit_tokens_replaces_estimate() {
        let svc = PolicyService::new(cfg(
            "p",
            ProviderLimits {
                tokens_per_minute: Some(10_000),
                ..Default::default()
            },
        ));
        let key = PolicyKey::new("p", PolicyScope::User);
        let mut p = svc.acquire(key.clone(), 5_000).await.unwrap();
        assert_eq!(svc.current_state(key.clone()).tokens_last_minute, 5_000);

        p.commit_tokens(1_234).await;
        assert_eq!(svc.current_state(key.clone()).tokens_last_minute, 1_234);

        // Second commit is a no-op.
        p.commit_tokens(99).await;
        assert_eq!(svc.current_state(key).tokens_last_minute, 1_234);
    }

    #[tokio::test]
    async fn current_state_reflects_concurrency() {
        let svc = PolicyService::new(cfg(
            "p",
            ProviderLimits {
                max_concurrent: Some(2),
                requests_per_minute: Some(100),
                ..Default::default()
            },
        ));
        let key = PolicyKey::new("p", PolicyScope::User);
        let _p1 = svc.acquire(key.clone(), 0).await.unwrap();
        let _p2 = svc.acquire(key.clone(), 0).await.unwrap();
        let s = svc.current_state(key);
        assert_eq!(s.max_concurrent, Some(2));
        assert_eq!(s.in_flight, 2);
        assert_eq!(s.requests_last_minute, 2);
    }

    #[tokio::test]
    async fn telemetry_fires_for_each_event() {
        #[derive(Default)]
        struct Counting {
            acquired: AtomicU32,
            committed: AtomicU32,
            rejected: AtomicU32,
        }
        impl PolicyTelemetry for Arc<Counting> {
            fn on_event(&self, event: PolicyEvent<'_>) {
                match event.kind {
                    PolicyEventKind::Acquired => {
                        self.acquired.fetch_add(1, Ordering::SeqCst);
                    }
                    PolicyEventKind::Committed => {
                        self.committed.fetch_add(1, Ordering::SeqCst);
                    }
                    PolicyEventKind::Rejected => {
                        self.rejected.fetch_add(1, Ordering::SeqCst);
                    }
                }
            }
        }

        let counter = Arc::new(Counting::default());
        let svc = PolicyService::with_telemetry(
            cfg(
                "p",
                ProviderLimits {
                    tokens_per_minute: Some(100),
                    ..Default::default()
                },
            ),
            Box::new(counter.clone()),
        );
        let key = PolicyKey::new("p", PolicyScope::User);

        let mut p = svc.acquire(key.clone(), 50).await.unwrap();
        p.commit_tokens(40).await;
        // Oversized — should reject.
        let _ = svc.acquire(key, 1_000).await.unwrap_err();

        assert_eq!(counter.acquired.load(Ordering::SeqCst), 1);
        assert_eq!(counter.committed.load(Ordering::SeqCst), 1);
        assert_eq!(counter.rejected.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn cancellation_does_not_leak_permits() {
        let svc = PolicyService::new(cfg(
            "p",
            ProviderLimits {
                max_concurrent: Some(1),
                ..Default::default()
            },
        ));
        let key = PolicyKey::new("p", PolicyScope::User);

        let _held = svc.acquire(key.clone(), 0).await.unwrap();

        // Spawn a task that blocks on acquire, then cancel it.
        let svc2 = svc.clone();
        let key2 = key.clone();
        let h = tokio::spawn(async move {
            let _p = svc2.acquire(key2, 0).await.unwrap();
            // Hold briefly; should never get here in this test.
            tokio::time::sleep(Duration::from_secs(60)).await;
        });
        // Give it time to enter the await.
        tokio::time::sleep(Duration::from_millis(20)).await;
        h.abort();
        let _ = h.await;

        // Drop the held permit; a fresh acquire should succeed promptly.
        drop(_held);
        let r = tokio::time::timeout(Duration::from_millis(500), svc.acquire(key, 0))
            .await
            .expect("acquire should not time out after cancellation");
        assert!(r.is_ok());
    }

    #[tokio::test]
    async fn keys_isolate_by_provider_and_scope() {
        let mut config = PolicyConfig::default();
        config.providers.insert(
            "a".to_string(),
            ProviderLimits {
                tokens_per_minute: Some(100),
                ..Default::default()
            },
        );
        config.providers.insert(
            "b".to_string(),
            ProviderLimits {
                tokens_per_minute: Some(100),
                ..Default::default()
            },
        );
        let svc = PolicyService::new(config);
        let _ = svc
            .acquire(PolicyKey::new("a", PolicyScope::User), 90)
            .await
            .unwrap();
        // Different provider — independent bucket.
        let _ = svc
            .acquire(PolicyKey::new("b", PolicyScope::User), 90)
            .await
            .unwrap();
        // Different scope — independent bucket.
        let _ = svc
            .acquire(PolicyKey::new("a", PolicyScope::Org), 90)
            .await
            .unwrap();

        let s_a_user = svc.current_state(PolicyKey::new("a", PolicyScope::User));
        let s_b_user = svc.current_state(PolicyKey::new("b", PolicyScope::User));
        let s_a_org = svc.current_state(PolicyKey::new("a", PolicyScope::Org));
        assert_eq!(s_a_user.tokens_last_minute, 90);
        assert_eq!(s_b_user.tokens_last_minute, 90);
        assert_eq!(s_a_org.tokens_last_minute, 90);
    }

    #[tokio::test]
    async fn config_round_trip_through_toml() {
        let toml = r#"
            [primary]
            requests_per_minute = 100
            tokens_per_day = 1000000
            max_concurrent = 8

            [secondary]
            requests_per_minute = 50
            tokens_per_minute = 100000
        "#;
        let cfg: PolicyConfig = toml::from_str(toml).unwrap();
        assert_eq!(cfg.providers["primary"].requests_per_minute, Some(100));
        assert_eq!(cfg.providers["primary"].tokens_per_day, Some(1_000_000));
        assert_eq!(cfg.providers["primary"].max_concurrent, Some(8));
        assert_eq!(cfg.providers["secondary"].tokens_per_minute, Some(100_000));

        let svc = PolicyService::new(cfg);
        assert!(!svc.is_noop());
    }
}
