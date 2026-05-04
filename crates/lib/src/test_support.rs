//! Test-only helpers for serialising tests that mutate shared environment
//! variables.
//!
//! `HOME`, `XDG_CONFIG_HOME`, and `XDG_DATA_HOME` are *process-global*
//! state. `cargo test` runs tests in parallel by default, so any test
//! that pokes one of those vars without a process-wide lock can be
//! observed mid-flight by another test that reads it — usually
//! manifesting as a flaky path resolution failure that nobody can
//! reproduce locally.
//!
//! This module exposes a single shared mutex and an RAII guard that
//! pins one or more env vars to a chosen value, blocks every other
//! `EnvGuard::redirect*` call until it drops, and restores the prior
//! value on drop (even on panic). All tests in this crate that touch
//! HOME or the XDG vars MUST funnel through this guard.
//!
//! cfg-test only — never compiled into the shipping library.

#![cfg(test)]

use std::ffi::OsString;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

/// Process-wide mutex shared by every env-mutating guard in this
/// crate. Public so out-of-module tests (notably the sandbox policy
/// tests) acquire the same lock and don't race the in-tool tests.
pub static ENV_LOCK: Mutex<()> = Mutex::new(());

/// RAII guard that overrides one or more environment variables for
/// the lifetime of the guard, restoring the prior values on drop and
/// holding [`ENV_LOCK`] while alive.
///
/// Multiple env vars get overridden through a single guard so a test
/// that needs both HOME and XDG_CONFIG_HOME can take the lock once
/// — taking it twice would deadlock.
pub struct EnvGuard {
    saved: Vec<(&'static str, Option<OsString>)>,
    _lock: MutexGuard<'static, ()>,
}

impl EnvGuard {
    /// Pin a single env var to `value` for the lifetime of the guard.
    pub fn set(name: &'static str, value: &Path) -> Self {
        Self::set_many(&[(name, value)])
    }

    /// Pin several env vars simultaneously. Use when a test depends on
    /// both HOME and an XDG override — taking [`EnvGuard::set`] twice
    /// in the same test would deadlock on [`ENV_LOCK`].
    pub fn set_many(entries: &[(&'static str, &Path)]) -> Self {
        let lock = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut saved = Vec::with_capacity(entries.len());
        for (name, value) in entries {
            saved.push((*name, std::env::var_os(name)));
            // SAFETY: ENV_LOCK is held for the lifetime of the guard,
            // so no other test thread can read or write any of these
            // vars between this set_var and the corresponding restore
            // in `drop`.
            unsafe {
                std::env::set_var(name, value);
            }
        }
        Self { saved, _lock: lock }
    }

    /// Pin a single env var to a string value (for tests that don't
    /// have a `Path` handy — usually `policy.rs` style fixtures).
    pub fn set_str(name: &'static str, value: &str) -> Self {
        let lock = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let prev = std::env::var_os(name);
        // SAFETY: see [`EnvGuard::set_many`].
        unsafe {
            std::env::set_var(name, value);
        }
        Self {
            saved: vec![(name, prev)],
            _lock: lock,
        }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        // Restore in reverse order so a guard that set HOME then
        // XDG_CONFIG_HOME unwinds them XDG-first then HOME — symmetric
        // with how callers reason about the override stack.
        for (name, prev) in self.saved.drain(..).rev() {
            // SAFETY: ENV_LOCK is held until this struct drops, which
            // happens after this loop completes.
            unsafe {
                match prev {
                    Some(v) => std::env::set_var(name, v),
                    None => std::env::remove_var(name),
                }
            }
        }
    }
}
