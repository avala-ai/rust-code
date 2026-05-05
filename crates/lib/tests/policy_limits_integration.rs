//! Integration tests for the policy / rate-limit service.
//!
//! Exercises the public API end-to-end: config -> service -> permit ->
//! commit_tokens, plus concurrent acquires through the gate.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use agent_code_lib::config::{Config, ProviderLimitsConfig};
use agent_code_lib::services::policy_limits::{
    PolicyConfig, PolicyKey, PolicyScope, PolicyService,
};

#[test]
fn config_limits_section_round_trips() {
    let toml_str = r#"
[limits.primary]
requests_per_minute = 100
tokens_per_day = 1000000
max_concurrent = 8

[limits.secondary]
requests_per_minute = 50
tokens_per_minute = 100000
"#;
    let config: Config = toml::from_str(toml_str).expect("valid limits TOML");
    assert_eq!(config.limits.len(), 2);
    let p1 = &config.limits["primary"];
    assert_eq!(p1.requests_per_minute, Some(100));
    assert_eq!(p1.tokens_per_day, Some(1_000_000));
    assert_eq!(p1.max_concurrent, Some(8));
    let p2 = &config.limits["secondary"];
    assert_eq!(p2.requests_per_minute, Some(50));
    assert_eq!(p2.tokens_per_minute, Some(100_000));

    // Bridge into PolicyConfig.
    let policy_cfg: PolicyConfig = (&config.limits).into();
    let svc = PolicyService::new(policy_cfg);
    assert!(!svc.is_noop());
}

#[test]
fn absent_limits_section_yields_noop_service() {
    // Default Config has no [limits] section.
    let config = Config::default();
    assert!(config.limits.is_empty());
    let svc = PolicyService::new((&config.limits).into());
    assert!(svc.is_noop());
}

#[tokio::test]
async fn two_concurrent_fake_provider_calls_flow_through_service() {
    let mut limits = std::collections::HashMap::new();
    limits.insert(
        "p".to_string(),
        ProviderLimitsConfig {
            max_concurrent: Some(1),
            tokens_per_minute: Some(10_000),
            ..Default::default()
        },
    );
    let svc = PolicyService::new((&limits).into());
    let key = PolicyKey::new("p", PolicyScope::User);

    let order = Arc::new(parking_lot_lite::Mutex::new(Vec::new()));

    let svc1 = svc.clone();
    let key1 = key.clone();
    let order1 = order.clone();
    let h1 = tokio::spawn(async move {
        let mut p = svc1.acquire(key1, 500).await.unwrap();
        order1.lock().push("a:start");
        tokio::time::sleep(Duration::from_millis(80)).await;
        order1.lock().push("a:end");
        p.commit_tokens(450).await;
    });

    // Stagger so task A enters the critical section first.
    tokio::time::sleep(Duration::from_millis(20)).await;

    let svc2 = svc.clone();
    let key2 = key.clone();
    let order2 = order.clone();
    let h2 = tokio::spawn(async move {
        let mut p = svc2.acquire(key2, 500).await.unwrap();
        order2.lock().push("b:start");
        tokio::time::sleep(Duration::from_millis(20)).await;
        order2.lock().push("b:end");
        p.commit_tokens(420).await;
    });

    h1.await.unwrap();
    h2.await.unwrap();

    let recorded = order.lock().clone();
    assert_eq!(
        recorded,
        vec!["a:start", "a:end", "b:start", "b:end"],
        "concurrency cap of 1 should serialize the two calls; got {recorded:?}"
    );

    // After both commits, the rolling window should reflect committed
    // totals (450 + 420 = 870), not the estimates (500 + 500 = 1000).
    let state = svc.current_state(key);
    assert_eq!(state.tokens_last_minute, 870);
    assert_eq!(state.in_flight, 0);
}

/// Tiny inline mutex helper so the integration test doesn't pull a new
/// dev-dep just to share a Vec across tasks. Regular `std::sync::Mutex`
/// works here too — abstracted so we don't poison-handle in the test.
mod parking_lot_lite {
    use std::sync::{Mutex as StdMutex, MutexGuard};

    pub struct Mutex<T>(StdMutex<T>);

    impl<T> Mutex<T> {
        pub fn new(t: T) -> Self {
            Self(StdMutex::new(t))
        }
        pub fn lock(&self) -> MutexGuard<'_, T> {
            self.0.lock().expect("policy test mutex poisoned")
        }
    }
}

#[tokio::test]
async fn telemetry_observes_acquire_and_commit() {
    use agent_code_lib::services::policy_limits::{PolicyEvent, PolicyEventKind, PolicyTelemetry};

    struct Counts {
        acquired: AtomicU32,
        committed: AtomicU32,
    }
    struct CountingTelemetry(Arc<Counts>);
    impl PolicyTelemetry for CountingTelemetry {
        fn on_event(&self, event: PolicyEvent<'_>) {
            match event.kind {
                PolicyEventKind::Acquired => {
                    self.0.acquired.fetch_add(1, Ordering::SeqCst);
                }
                PolicyEventKind::Committed => {
                    self.0.committed.fetch_add(1, Ordering::SeqCst);
                }
                PolicyEventKind::Rejected => {}
            }
        }
    }

    let mut limits = std::collections::HashMap::new();
    limits.insert(
        "p".to_string(),
        ProviderLimitsConfig {
            tokens_per_minute: Some(10_000),
            ..Default::default()
        },
    );
    let counts = Arc::new(Counts {
        acquired: AtomicU32::new(0),
        committed: AtomicU32::new(0),
    });
    let svc = PolicyService::with_telemetry(
        (&limits).into(),
        Box::new(CountingTelemetry(counts.clone())),
    );

    let mut p = svc
        .acquire(PolicyKey::new("p", PolicyScope::User), 100)
        .await
        .unwrap();
    p.commit_tokens(80).await;

    assert_eq!(counts.acquired.load(Ordering::SeqCst), 1);
    assert_eq!(counts.committed.load(Ordering::SeqCst), 1);
}
