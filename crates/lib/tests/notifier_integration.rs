//! Integration tests for the desktop notifier service.
//!
//! Exercises the public API end-to-end: config -> service -> recorded
//! call, plus the focus-probe override path that lets host code drive
//! the `when_focused` gate without shelling out to AppleScript.

use agent_code_lib::config::{Config, NotifierConfig};
use agent_code_lib::services::notifier::{NotificationKind, NotifierService};

#[test]
fn config_notifier_section_round_trips_through_toml() {
    let toml_str = r#"
[notifier]
enabled = true
when_focused = true
on_task_complete = false
on_permission_prompt = true
on_error = false
min_duration_secs = 90
"#;
    let config: Config = toml::from_str(toml_str).expect("valid notifier TOML");
    assert!(config.notifier.enabled);
    assert!(config.notifier.when_focused);
    assert!(!config.notifier.on_task_complete);
    assert!(config.notifier.on_permission_prompt);
    assert!(!config.notifier.on_error);
    assert_eq!(config.notifier.min_duration_secs, 90);
}

#[test]
fn absent_notifier_section_uses_default() {
    let config = Config::default();
    assert!(config.notifier.enabled);
    assert_eq!(config.notifier.min_duration_secs, 30);
}

#[test]
fn test_mode_records_each_kind_through_public_api() {
    let svc = NotifierService::new_for_test(NotifierConfig::default());
    svc.notify(NotificationKind::Info, "info", "i");
    svc.notify(NotificationKind::PermissionPrompt, "perm", "p");
    svc.notify(NotificationKind::Error, "err", "e");
    svc.notify_task_complete("done", "long task", 600);

    let rec = svc.recorded();
    assert_eq!(rec.len(), 4);
    let kinds: Vec<NotificationKind> = rec.iter().map(|r| r.kind).collect();
    assert_eq!(
        kinds,
        vec![
            NotificationKind::Info,
            NotificationKind::PermissionPrompt,
            NotificationKind::Error,
            NotificationKind::TaskComplete,
        ]
    );
    assert_eq!(rec[3].duration_secs, Some(600));
}

#[test]
fn min_duration_floor_filters_short_tasks() {
    let svc = NotifierService::new_for_test(NotifierConfig {
        min_duration_secs: 60,
        ..NotifierConfig::default()
    });
    svc.notify_task_complete("short", "5s", 5);
    svc.notify_task_complete("long", "120s", 120);
    let rec = svc.recorded();
    assert_eq!(rec.len(), 1);
    assert_eq!(rec[0].title, "long");
}

#[test]
fn enabled_false_drops_every_kind_from_config() {
    let svc = NotifierService::new_for_test(NotifierConfig {
        enabled: false,
        ..NotifierConfig::default()
    });
    svc.notify(NotificationKind::Info, "i", "b");
    svc.notify(NotificationKind::PermissionPrompt, "p", "b");
    svc.notify(NotificationKind::Error, "e", "b");
    svc.notify_task_complete("t", "b", 9999);
    assert!(svc.recorded().is_empty());
}

#[test]
fn focus_probe_override_drives_when_focused_gate() {
    // when_focused = true with a probe that says focused -> dropped.
    let svc = NotifierService::new_for_test(NotifierConfig {
        when_focused: true,
        ..NotifierConfig::default()
    })
    .with_focus_probe(|| true);
    svc.notify(NotificationKind::Info, "i", "b");
    svc.notify_task_complete("t", "b", 600);
    assert!(svc.recorded().is_empty());

    // when_focused = true with a probe that says unfocused -> fired.
    let svc = NotifierService::new_for_test(NotifierConfig {
        when_focused: true,
        ..NotifierConfig::default()
    })
    .with_focus_probe(|| false);
    svc.notify(NotificationKind::Info, "i", "b");
    assert_eq!(svc.recorded().len(), 1);
}
