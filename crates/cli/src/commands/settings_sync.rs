//! `/settings sync` — opt-in remote backup of `settings.toml`.
//!
//! Implements three subcommands:
//!
//! - `push` — read the user's current settings, seal them, and store
//!   them in the configured backend.
//! - `pull <id>` — fetch a snapshot, verify it, and atomically rewrite
//!   the user's settings file.
//! - `list` — list every snapshot id the backend knows about.
//!
//! Each command prompts for a passphrase. The passphrase is read with
//! terminal echo OFF via `crossterm`'s raw mode + a manual input loop.
//! If raw mode can't be enabled (non-tty stdin, locked-down sandboxes),
//! we fall back to reading the passphrase from `AGENT_CODE_SYNC_PASSPHRASE`
//! and surface a clear error if that isn't set.

use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::Arc;

use agent_code_lib::query::QueryEngine;
use agent_code_lib::services::settings_sync::{
    LocalFsBackend, RemoteId, SettingsSyncService, SyncBackend, SyncConfig, SyncError,
};
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use crossterm::terminal;

const PASSPHRASE_ENV: &str = "AGENT_CODE_SYNC_PASSPHRASE";

pub(super) fn print_usage() {
    println!("Settings sync — opt-in remote backup of settings.toml");
    println!();
    println!("  /settings sync push           Encrypt and upload current settings");
    println!("  /settings sync pull <id>      Download, verify, and write to settings.toml");
    println!("  /settings sync list           List known snapshot ids on the backend");
    println!();
    println!("Storage:");
    println!(
        "  Snapshots live under <config_dir>/agent-code/sync/. Set \
         {PASSPHRASE_ENV} to skip the interactive prompt (e.g. in scripts)."
    );
    println!();
    println!(
        "Note: this build runs settings-sync in PLAINTEXT mode (signed but not encrypted). \
         Do not push secret-bearing settings."
    );
}

pub(super) fn run_push(engine: &QueryEngine) {
    let Some(settings_path) = resolve_settings_path(engine) else {
        eprintln!(
            "No settings.toml found. Create user settings at \
             <config_dir>/agent-code/config.toml or run /init for a project file."
        );
        return;
    };
    let snapshot = match std::fs::read(&settings_path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("Failed to read {}: {e}", settings_path.display());
            return;
        }
    };

    let passphrase = match read_passphrase("Sync passphrase: ") {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return;
        }
    };

    let confirm = match read_passphrase("Confirm passphrase: ") {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return;
        }
    };
    if confirm != passphrase {
        eprintln!("Passphrases did not match. Aborting.");
        return;
    }

    let backend = match make_backend() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("{e}");
            return;
        }
    };

    let svc = SettingsSyncService::new(SyncConfig {
        backend,
        passphrase,
        encryption_enabled: false,
    });
    match block_on(svc.push(&snapshot)) {
        Ok(id) => {
            println!("Pushed snapshot id: {id}");
            println!(
                "Note: settings-sync currently runs in plaintext mode (signed but \
                 not encrypted). Do not push secret-bearing settings."
            );
        }
        Err(e) => eprintln!("Push failed: {e}"),
    }
}

pub(super) fn run_pull(engine: &QueryEngine, args: Option<&str>) {
    let id_str = args.unwrap_or("").trim();
    if id_str.is_empty() {
        eprintln!("Usage: /settings sync pull <id>");
        eprintln!("Run /settings sync list to see known ids.");
        return;
    }
    let id = RemoteId::new(id_str);

    let Some(dest) = resolve_settings_dest(engine) else {
        eprintln!(
            "No writable settings.toml destination. Set XDG_CONFIG_HOME or HOME to \
             a writable directory, or create a project at .agent/settings.toml first."
        );
        return;
    };

    let passphrase = match read_passphrase("Sync passphrase: ") {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return;
        }
    };

    let backend = match make_backend() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("{e}");
            return;
        }
    };

    let svc = SettingsSyncService::new(SyncConfig {
        backend,
        passphrase,
        encryption_enabled: false,
    });
    match block_on(svc.pull_to(&id, &dest)) {
        Ok(()) => {
            println!("Restored settings to {}", dest.display());
        }
        Err(SyncError::PassphraseInvalid) => {
            eprintln!("Pull failed: passphrase does not match this snapshot.");
        }
        Err(SyncError::IntegrityFailed) => {
            eprintln!("Pull failed: snapshot integrity check failed.");
        }
        Err(e) => eprintln!("Pull failed: {e}"),
    }
}

pub(super) fn run_list() {
    let backend = match make_backend() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("{e}");
            return;
        }
    };
    // No service, no passphrase — list is a backend-level operation.
    match block_on(backend.list()) {
        Ok(ids) if ids.is_empty() => println!("No snapshots."),
        Ok(ids) => {
            println!("Snapshots (oldest first):");
            for id in ids {
                println!("  {id}");
            }
        }
        Err(e) => eprintln!("List failed: {e}"),
    }
}

// ---------------------------------------------------------------------------
// Helpers.
// ---------------------------------------------------------------------------

/// Build the default `LocalFsBackend` (under `<config_dir>/agent-code/sync`).
/// HTTP backend selection is a follow-up.
fn make_backend() -> Result<Arc<dyn SyncBackend>, String> {
    LocalFsBackend::default_root()
        .map(|b| Arc::new(b) as Arc<dyn SyncBackend>)
        .map_err(|e| format!("Could not initialise local sync backend: {e}"))
}

/// Resolve which settings file to read for `push`. Prefers the project
/// config (`.agent/settings.toml`) when one is reachable from the
/// session cwd, otherwise the user-level config.
fn resolve_settings_path(engine: &QueryEngine) -> Option<PathBuf> {
    if let Some(p) =
        agent_code_lib::config::find_project_config_from(std::path::Path::new(&engine.state().cwd))
        && p.exists()
    {
        return Some(p);
    }
    let user = agent_code_lib::config::user_config_path()?;
    if user.exists() {
        return Some(user);
    }
    None
}

/// Resolve where to write a pulled snapshot. Same precedence as
/// `resolve_settings_path` but allows a non-existent target so a user
/// can pull onto a new machine.
fn resolve_settings_dest(engine: &QueryEngine) -> Option<PathBuf> {
    if let Some(p) =
        agent_code_lib::config::find_project_config_from(std::path::Path::new(&engine.state().cwd))
    {
        return Some(p);
    }
    agent_code_lib::config::user_config_path()
}

/// Block on a future from sync code in the REPL. The REPL runs inside
/// a tokio runtime; use the current handle so we don't double-spawn.
fn block_on<F: std::future::Future>(fut: F) -> F::Output {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => tokio::task::block_in_place(|| handle.block_on(fut)),
        Err(_) => tokio::runtime::Runtime::new()
            .expect("create tokio runtime")
            .block_on(fut),
    }
}

/// Read a passphrase with terminal echo OFF.
///
/// First tries `crossterm` raw mode + a manual key-event loop. If raw
/// mode can't be enabled (non-tty stdin, restricted sandbox), falls
/// back to the `AGENT_CODE_SYNC_PASSPHRASE` env var. If neither path
/// is available, returns a `String` describing the problem so the
/// caller can present it.
fn read_passphrase(prompt: &str) -> Result<String, String> {
    if let Ok(env) = std::env::var(PASSPHRASE_ENV)
        && !env.is_empty()
    {
        return Ok(env);
    }

    print!("{prompt}");
    io::stdout().flush().ok();

    match read_passphrase_with_raw_mode() {
        Ok(s) => {
            println!();
            Ok(s)
        }
        Err(e) => Err(format!(
            "Could not read passphrase interactively ({e}). \
             Set {PASSPHRASE_ENV} and try again."
        )),
    }
}

fn read_passphrase_with_raw_mode() -> Result<String, String> {
    let raw_was_enabled = terminal::is_raw_mode_enabled().unwrap_or(false);
    if !raw_was_enabled {
        terminal::enable_raw_mode().map_err(|e| format!("enable raw mode: {e}"))?;
    }

    let result = read_passphrase_loop();

    if !raw_was_enabled {
        let _ = terminal::disable_raw_mode();
    }

    result
}

/// Inner loop: read key events until Enter; never echo the input.
/// Treats Ctrl+C / Ctrl+D / Esc as cancel and returns an error so
/// the caller can abort the sync without leaving raw mode dangling.
fn read_passphrase_loop() -> Result<String, String> {
    let mut buf = String::new();
    loop {
        let ev = event::read().map_err(|e| format!("read terminal event: {e}"))?;
        // Ignore non-key events (resize, mouse, focus).
        let Event::Key(key) = ev else { continue };
        match key.code {
            KeyCode::Enter => return Ok(buf),
            KeyCode::Esc => return Err("cancelled".into()),
            KeyCode::Backspace => {
                buf.pop();
            }
            KeyCode::Char(c) => {
                if key.modifiers.contains(KeyModifiers::CONTROL) && (c == 'c' || c == 'd') {
                    return Err("cancelled".into());
                }
                if key.modifiers.contains(KeyModifiers::CONTROL) {
                    // Ignore other control chords.
                    continue;
                }
                buf.push(c);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `read_passphrase` honors the env-var fallback when set.
    #[test]
    fn passphrase_env_var_fallback() {
        // Use a unique temporary value so we don't risk colliding with
        // a real value the developer may have exported. We restore the
        // prior value on exit.
        let saved = std::env::var(PASSPHRASE_ENV).ok();
        // SAFETY: tests in this module are the only readers of the var.
        unsafe {
            std::env::set_var(PASSPHRASE_ENV, "ephemeral-test-passphrase");
        }
        let got = read_passphrase("ignored: ").unwrap();
        assert_eq!(got, "ephemeral-test-passphrase");

        // SAFETY: restoring previous state.
        unsafe {
            match saved {
                Some(v) => std::env::set_var(PASSPHRASE_ENV, v),
                None => std::env::remove_var(PASSPHRASE_ENV),
            }
        }
    }
}
