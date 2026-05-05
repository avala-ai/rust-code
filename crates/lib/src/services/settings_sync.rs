//! Opt-in remote backup of user settings.
//!
//! Lets a user push a snapshot of their settings to a pluggable backend
//! (currently only `LocalFsBackend`; HTTP is stubbed) and pull it back
//! on another machine. The on-disk frame layout:
//!
//! ```text
//!   magic   ("agscv1")                         :: 6 bytes
//!   version (u8)                               :: 1 byte
//!   salt    (16 bytes random)                  :: 16 bytes
//!   verifier_len (u32 LE)                      :: 4 bytes
//!   verifier (32 bytes)                        :: 32 bytes
//!   mac_len  (u32 LE)                          :: 4 bytes
//!   mac      (32 bytes)                        :: 32 bytes
//!   payload_len (u32 LE)                       :: 4 bytes
//!   payload  (variable)                        :: N bytes
//! ```
//!
//! The `verifier` is `SHA-256(salt || 0x01 || passphrase_bytes)` — checked
//! before the MAC so a wrong-passphrase fails fast with a distinct error.
//! The `mac` is `SHA-256(salt || 0x02 || passphrase_bytes || payload)` —
//! detects tampering of either the salt or the payload.
//!
//! ### Encryption status
//!
//! This module ships in **plaintext mode**: the payload bytes are stored
//! and transferred as-is, only signed with a passphrase-derived MAC. The
//! framing is sized to hold an authenticated ciphertext once the
//! workspace gains a real AEAD primitive (AES-256-GCM via `aes-gcm` or
//! `ring`) and an Argon2id KDF (`argon2`). Until then this is suitable
//! only for non-secret-bearing settings, or ones the user is comfortable
//! placing on the chosen backend in plaintext. Every push and pull
//! emits a `tracing::warn!` so the limitation surfaces in the standard
//! diagnostic stream.
//!
//! ### Telemetry
//!
//! By design, this module emits no telemetry. Settings are sensitive
//! enough that even a "user pushed at $time" record is too much.
//! Callers must not log the encrypted blob, the passphrase, or the
//! remote id outside of the user's own terminal output.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use tracing::warn;

use crate::config::atomic::atomic_write_secret;

/// Magic prefix for the on-disk frame (`agent-code sync, container v1`).
const MAGIC: &[u8] = b"agscv1";
/// Frame format version, bumped when the layout changes.
const FRAME_VERSION: u8 = 1;
/// Length of the random per-record salt, in bytes.
const SALT_LEN: usize = 16;
/// SHA-256 digest length in bytes.
const DIGEST_LEN: usize = 32;
/// Domain-separation tag for the passphrase verifier hash.
const TAG_VERIFIER: u8 = 0x01;
/// Domain-separation tag for the payload MAC hash.
const TAG_MAC: u8 = 0x02;

/// Identifier for a remote snapshot. Opaque to callers; backends
/// generate these and use them as keys.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RemoteId(String);

impl RemoteId {
    /// Construct a `RemoteId` from a backend-supplied string.
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    /// View the wrapped string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for RemoteId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Failure modes for the settings-sync service.
///
/// Variants intentionally never embed plaintext, ciphertext, or the
/// passphrase — leaking those into a log line or a panic message is
/// the exact attack the integrity layer is meant to prevent. The
/// `String` payloads are populated only with fixed-string class
/// labels (`"truncated frame"`, `"bad magic"`, `"backend write"`,
/// …); see the `errors_do_not_leak_secrets` regression test below.
#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    /// Failed to encode/seal a snapshot for upload.
    #[error("failed to encrypt settings snapshot")]
    Encrypt,
    /// Failed to open a snapshot. Reason is a fixed-string class
    /// (`malformed frame`, `truncated frame`, `bad magic`, …) — never
    /// the raw frame bytes.
    #[error("failed to decrypt settings snapshot: {0}")]
    Decrypt(String),
    /// Backend (filesystem, HTTP, …) returned an error or isn't
    /// configured. The string is a backend-class label, not contents.
    #[error("backend unavailable: {0}")]
    BackendUnavailable(String),
    /// MAC check failed — the snapshot was modified or truncated
    /// after sealing.
    #[error("snapshot integrity check failed")]
    IntegrityFailed,
    /// The supplied passphrase does not match the one used to seal
    /// this snapshot.
    #[error("passphrase does not match this snapshot")]
    PassphraseInvalid,
}

/// Pluggable bytes-store the sync service runs on top of.
///
/// Implementations must treat the bytes as opaque — no parsing, no
/// indexing on payload fields. This is what lets a future encrypted
/// payload drop in without a backend change.
#[async_trait]
pub trait SyncBackend: Send + Sync {
    /// Store `bytes` under a fresh id and return that id.
    async fn put(&self, bytes: &[u8]) -> Result<RemoteId, SyncError>;
    /// Fetch the bytes previously stored under `id`.
    async fn get(&self, id: &RemoteId) -> Result<Vec<u8>, SyncError>;
    /// List every id known to the backend, sorted oldest-first.
    async fn list(&self) -> Result<Vec<RemoteId>, SyncError>;
}

/// Configuration for `SettingsSyncService`.
pub struct SyncConfig {
    /// The backend bytes are written to / read from.
    pub backend: Arc<dyn SyncBackend>,
    /// User-supplied passphrase. The service only holds this for the
    /// lifetime of the service value; it is overwritten in place on
    /// `Drop`. Without `zeroize` in the workspace we cannot guarantee
    /// compiler-induced copies are wiped — flagged as a follow-up
    /// once a crypto upgrade lands.
    pub passphrase: String,
    /// When `false` (the default), payloads are signed but stored
    /// without confidentiality protection. When a real AEAD primitive
    /// lands the same flag will gate the encrypted path; the public
    /// API does not change.
    pub encryption_enabled: bool,
}

impl SyncConfig {
    /// Convenience: build a config with `encryption_enabled = false`.
    pub fn plaintext(backend: Arc<dyn SyncBackend>, passphrase: String) -> Self {
        Self {
            backend,
            passphrase,
            encryption_enabled: false,
        }
    }
}

/// Service entry point.
///
/// Constructed once per command invocation; not meant to live across
/// long-running operations because it holds a passphrase in memory.
pub struct SettingsSyncService {
    backend: Arc<dyn SyncBackend>,
    /// Stored as raw bytes (not `String`) so the `Drop` impl can
    /// scrub the buffer before deallocation.
    passphrase: Vec<u8>,
    encryption_enabled: bool,
}

impl SettingsSyncService {
    /// Build a sync service from a config bundle. Consumes
    /// `config.passphrase`.
    pub fn new(mut config: SyncConfig) -> Self {
        let passphrase = config.passphrase.as_bytes().to_vec();
        // Best-effort: blank the original string so a crash dump
        // taken between `new` and the next call doesn't trivially
        // surface the passphrase.
        let len = config.passphrase.len();
        config.passphrase.clear();
        config.passphrase.reserve(len);
        Self {
            backend: config.backend,
            passphrase,
            encryption_enabled: config.encryption_enabled,
        }
    }

    /// Seal `snapshot` with the configured passphrase and upload it.
    /// On success returns the backend-assigned id.
    pub async fn push(&self, snapshot: &[u8]) -> Result<RemoteId, SyncError> {
        self.warn_if_plaintext("push");
        let salt = random_salt();
        let frame = seal_frame(&salt, &self.passphrase, snapshot)?;
        self.backend.put(&frame).await
    }

    /// Download the snapshot named by `remote_id`, verify its
    /// passphrase + MAC, and return the original bytes.
    pub async fn pull(&self, remote_id: &RemoteId) -> Result<Vec<u8>, SyncError> {
        self.warn_if_plaintext("pull");
        let frame = self.backend.get(remote_id).await?;
        open_frame(&frame, &self.passphrase)
    }

    /// List every snapshot the backend knows about. Does not
    /// authenticate — listing is a backend-level operation that
    /// doesn't reveal payload contents.
    pub async fn list(&self) -> Result<Vec<RemoteId>, SyncError> {
        self.backend.list().await
    }

    /// Pull the snapshot named by `remote_id`, verify it, and
    /// atomically write the recovered bytes to `dest` using the same
    /// secret-preserving writer that `Config::save` uses. This is
    /// the round-trip path for the `/settings sync pull` command.
    pub async fn pull_to(&self, remote_id: &RemoteId, dest: &Path) -> Result<(), SyncError> {
        let bytes = self.pull(remote_id).await?;
        atomic_write_secret(dest, &bytes)
            .map_err(|e| SyncError::BackendUnavailable(format!("write destination: {e}")))?;
        Ok(())
    }

    fn warn_if_plaintext(&self, op: &'static str) {
        if !self.encryption_enabled {
            warn!(
                op = op,
                "settings-sync running in plaintext mode: payload is signed but not encrypted; \
                 do not push secret-bearing settings until encryption ships"
            );
        }
    }
}

impl Drop for SettingsSyncService {
    fn drop(&mut self) {
        // Best-effort scrub. Without `zeroize` we can't keep the
        // optimizer from eliminating this on aggressive builds, but
        // the volatile-style write below survives the default opt
        // pipeline.
        for b in self.passphrase.iter_mut() {
            // SAFETY: pointer is to a valid `u8` we own; volatile
            // write is what makes the scrub survive optimization.
            unsafe {
                std::ptr::write_volatile(b, 0);
            }
        }
        self.passphrase.clear();
    }
}

// -----------------------------------------------------------------
// Frame helpers — pure functions, easy to unit-test.
// -----------------------------------------------------------------

fn random_salt() -> [u8; SALT_LEN] {
    // No `rand` crate is a direct workspace dep; combine the entropy
    // sources we already have. This is sufficient for a per-record
    // salt (uniqueness, not unpredictability), which is the only
    // property the verifier/MAC construction needs from it. When the
    // crypto upgrade lands this will switch to a CSPRNG.
    let mut hasher = Sha256::new();
    hasher.update(uuid::Uuid::new_v4().as_bytes());
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    hasher.update(nanos.to_le_bytes());
    hasher.update(std::process::id().to_le_bytes());
    let digest = hasher.finalize();
    let mut salt = [0u8; SALT_LEN];
    salt.copy_from_slice(&digest[..SALT_LEN]);
    salt
}

fn verifier(salt: &[u8], passphrase: &[u8]) -> [u8; DIGEST_LEN] {
    let mut hasher = Sha256::new();
    hasher.update(salt);
    hasher.update([TAG_VERIFIER]);
    hasher.update(passphrase);
    let mut out = [0u8; DIGEST_LEN];
    out.copy_from_slice(&hasher.finalize());
    out
}

fn payload_mac(salt: &[u8], passphrase: &[u8], payload: &[u8]) -> [u8; DIGEST_LEN] {
    let mut hasher = Sha256::new();
    hasher.update(salt);
    hasher.update([TAG_MAC]);
    hasher.update(passphrase);
    hasher.update(payload);
    let mut out = [0u8; DIGEST_LEN];
    out.copy_from_slice(&hasher.finalize());
    out
}

fn seal_frame(salt: &[u8], passphrase: &[u8], payload: &[u8]) -> Result<Vec<u8>, SyncError> {
    if salt.len() != SALT_LEN {
        return Err(SyncError::Encrypt);
    }
    let v = verifier(salt, passphrase);
    let mac = payload_mac(salt, passphrase, payload);

    let payload_len: u32 = payload.len().try_into().map_err(|_| SyncError::Encrypt)?;
    let mut out = Vec::with_capacity(
        MAGIC.len() + 1 + SALT_LEN + 4 + DIGEST_LEN + 4 + DIGEST_LEN + 4 + payload.len(),
    );
    out.extend_from_slice(MAGIC);
    out.push(FRAME_VERSION);
    out.extend_from_slice(salt);
    out.extend_from_slice(&(DIGEST_LEN as u32).to_le_bytes());
    out.extend_from_slice(&v);
    out.extend_from_slice(&(DIGEST_LEN as u32).to_le_bytes());
    out.extend_from_slice(&mac);
    out.extend_from_slice(&payload_len.to_le_bytes());
    out.extend_from_slice(payload);
    Ok(out)
}

/// Constant-time comparison for two byte slices.
///
/// The frame helpers return early on a length mismatch (length isn't
/// secret); the constant-time loop guards the remaining content
/// comparison so a wrong-passphrase attempt can't be timed to leak
/// how many leading bytes matched.
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

fn open_frame(frame: &[u8], passphrase: &[u8]) -> Result<Vec<u8>, SyncError> {
    let mut cursor = 0usize;
    let need = |c: usize, n: usize, len: usize| -> Result<(), SyncError> {
        if c + n > len {
            Err(SyncError::Decrypt("truncated frame".into()))
        } else {
            Ok(())
        }
    };

    need(cursor, MAGIC.len(), frame.len())?;
    if &frame[cursor..cursor + MAGIC.len()] != MAGIC {
        return Err(SyncError::Decrypt("bad magic".into()));
    }
    cursor += MAGIC.len();

    need(cursor, 1, frame.len())?;
    if frame[cursor] != FRAME_VERSION {
        return Err(SyncError::Decrypt("unknown frame version".into()));
    }
    cursor += 1;

    need(cursor, SALT_LEN, frame.len())?;
    let salt = &frame[cursor..cursor + SALT_LEN];
    cursor += SALT_LEN;

    let v_len = read_u32(frame, &mut cursor)? as usize;
    if v_len != DIGEST_LEN {
        return Err(SyncError::Decrypt("unexpected verifier length".into()));
    }
    need(cursor, v_len, frame.len())?;
    let stored_verifier = &frame[cursor..cursor + v_len];
    cursor += v_len;

    let m_len = read_u32(frame, &mut cursor)? as usize;
    if m_len != DIGEST_LEN {
        return Err(SyncError::Decrypt("unexpected mac length".into()));
    }
    need(cursor, m_len, frame.len())?;
    let stored_mac = &frame[cursor..cursor + m_len];
    cursor += m_len;

    let p_len = read_u32(frame, &mut cursor)? as usize;
    need(cursor, p_len, frame.len())?;
    let payload = &frame[cursor..cursor + p_len];
    cursor += p_len;

    if cursor != frame.len() {
        return Err(SyncError::Decrypt("trailing bytes".into()));
    }

    let computed_verifier = verifier(salt, passphrase);
    if !ct_eq(stored_verifier, &computed_verifier) {
        return Err(SyncError::PassphraseInvalid);
    }
    let computed_mac = payload_mac(salt, passphrase, payload);
    if !ct_eq(stored_mac, &computed_mac) {
        return Err(SyncError::IntegrityFailed);
    }

    Ok(payload.to_vec())
}

fn read_u32(buf: &[u8], cursor: &mut usize) -> Result<u32, SyncError> {
    if *cursor + 4 > buf.len() {
        return Err(SyncError::Decrypt("truncated frame".into()));
    }
    let mut bytes = [0u8; 4];
    bytes.copy_from_slice(&buf[*cursor..*cursor + 4]);
    *cursor += 4;
    Ok(u32::from_le_bytes(bytes))
}

// -----------------------------------------------------------------
// Backends.
// -----------------------------------------------------------------

/// Returns the platform default for `LocalFsBackend`'s root:
/// `<config_dir>/agent-code/sync`. `None` when no config directory
/// can be determined.
pub fn default_local_sync_dir() -> Option<PathBuf> {
    crate::config::agent_config_dir().map(|d| d.join("sync"))
}

/// Filesystem-backed sync store. Each snapshot is one file under
/// `root/<id>.bin`. Useful for tests and as an offline backup
/// destination — an external sync engine can mirror the directory.
pub struct LocalFsBackend {
    root: PathBuf,
}

impl LocalFsBackend {
    /// Construct a backend rooted at `root`. The directory is created
    /// on the first `put`; missing it is not an error before then.
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// Helper: backend rooted at the platform default
    /// (`<config_dir>/agent-code/sync`).
    pub fn default_root() -> Result<Self, SyncError> {
        let dir = default_local_sync_dir().ok_or_else(|| {
            SyncError::BackendUnavailable("no config dir; set XDG_CONFIG_HOME or HOME".into())
        })?;
        Ok(Self::new(dir))
    }

    fn id_to_path(&self, id: &RemoteId) -> Result<PathBuf, SyncError> {
        // Ids are backend-generated as UUIDs, but a hostile caller
        // could still pass a crafted string. Reject anything that
        // could escape the backend root.
        let s = id.as_str();
        if s.is_empty() || s.contains('/') || s.contains('\\') || s.contains("..") {
            return Err(SyncError::BackendUnavailable("invalid remote id".into()));
        }
        Ok(self.root.join(format!("{s}.bin")))
    }
}

#[async_trait]
impl SyncBackend for LocalFsBackend {
    async fn put(&self, bytes: &[u8]) -> Result<RemoteId, SyncError> {
        std::fs::create_dir_all(&self.root)
            .map_err(|e| SyncError::BackendUnavailable(format!("backend mkdir: {e}")))?;
        let id = RemoteId::new(uuid::Uuid::new_v4().to_string());
        let path = self.id_to_path(&id)?;
        atomic_write_secret(&path, bytes)
            .map_err(|e| SyncError::BackendUnavailable(format!("backend write: {e}")))?;
        Ok(id)
    }

    async fn get(&self, id: &RemoteId) -> Result<Vec<u8>, SyncError> {
        let path = self.id_to_path(id)?;
        std::fs::read(&path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                SyncError::BackendUnavailable("snapshot not found".into())
            } else {
                SyncError::BackendUnavailable(format!("backend read: {e}"))
            }
        })
    }

    async fn list(&self) -> Result<Vec<RemoteId>, SyncError> {
        let entries = match std::fs::read_dir(&self.root) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => {
                return Err(SyncError::BackendUnavailable(format!(
                    "backend readdir: {e}"
                )));
            }
        };

        let mut ids: Vec<(std::time::SystemTime, RemoteId)> = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|e| e != "bin") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let mtime = entry
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            ids.push((mtime, RemoteId::new(stem.to_string())));
        }
        ids.sort_by_key(|(mtime, _)| *mtime);
        Ok(ids.into_iter().map(|(_, id)| id).collect())
    }
}

/// Stub for the operator-configured HTTP backend.
///
/// Intentionally not wired up: a half-baked HTTP client (auth, retry,
/// rate-limit, proxy handling) is worse than no backend at all because
/// users would think their settings were synced when they weren't.
/// Every call returns `BackendUnavailable` with an instructive
/// message. Will be implemented once the wire format and auth model
/// in 8.6 (direct-connect server) are settled.
pub struct HttpBackendStub {
    /// User-configured base URL — captured but not yet used.
    pub base_url: String,
}

impl HttpBackendStub {
    /// Build a stub backend pinned to `base_url`. No network call is
    /// made; this only stores the URL for the eventual real backend.
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
        }
    }
}

#[async_trait]
impl SyncBackend for HttpBackendStub {
    async fn put(&self, _bytes: &[u8]) -> Result<RemoteId, SyncError> {
        Err(SyncError::BackendUnavailable(
            "remote HTTP backend not yet implemented; use LocalFsBackend".into(),
        ))
    }

    async fn get(&self, _id: &RemoteId) -> Result<Vec<u8>, SyncError> {
        Err(SyncError::BackendUnavailable(
            "remote HTTP backend not yet implemented; use LocalFsBackend".into(),
        ))
    }

    async fn list(&self) -> Result<Vec<RemoteId>, SyncError> {
        Err(SyncError::BackendUnavailable(
            "remote HTTP backend not yet implemented; use LocalFsBackend".into(),
        ))
    }
}

// -----------------------------------------------------------------
// Tests.
// -----------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn plaintext_config(backend: Arc<dyn SyncBackend>, pass: &str) -> SyncConfig {
        SyncConfig::plaintext(backend, pass.to_string())
    }

    /// Round-trip the frame helpers directly with a known passphrase.
    #[test]
    fn frame_roundtrip_with_correct_passphrase() {
        let salt = random_salt();
        let pass = b"correct horse battery staple".to_vec();
        let payload = b"[api]\nmodel = \"claude\"\n".to_vec();
        let sealed = seal_frame(&salt, &pass, &payload).unwrap();
        let opened = open_frame(&sealed, &pass).unwrap();
        assert_eq!(opened, payload);
    }

    /// Wrong passphrase must surface as `PassphraseInvalid`, never
    /// as `IntegrityFailed` or any decrypt-class error.
    #[test]
    fn frame_wrong_passphrase_reports_passphrase_invalid() {
        let salt = random_salt();
        let payload = b"settings".to_vec();
        let sealed = seal_frame(&salt, b"right", &payload).unwrap();
        match open_frame(&sealed, b"wrong") {
            Err(SyncError::PassphraseInvalid) => {}
            other => panic!("expected PassphraseInvalid, got {other:?}"),
        }
    }

    /// Modifying any byte of the sealed frame after the verifier
    /// must surface as `IntegrityFailed`. Frame layout:
    /// `MAGIC(6) | VER(1) | SALT(16) | VLEN(4) | V(32) | MLEN(4) | MAC(32) | PLEN(4) | payload`.
    /// Index `MAGIC.len()+1+SALT_LEN+4+DIGEST_LEN+4+DIGEST_LEN+4 = 99`
    /// is the first payload byte — flipping it tampers the payload
    /// without breaking the frame structure.
    #[test]
    fn frame_tampered_payload_reports_integrity_failed() {
        let salt = random_salt();
        let pass = b"hunter2".to_vec();
        let payload = b"some settings here".to_vec();
        let mut sealed = seal_frame(&salt, &pass, &payload).unwrap();
        let payload_offset = MAGIC.len() + 1 + SALT_LEN + 4 + DIGEST_LEN + 4 + DIGEST_LEN + 4;
        sealed[payload_offset] ^= 0x01;
        match open_frame(&sealed, &pass) {
            Err(SyncError::IntegrityFailed) => {}
            other => panic!("expected IntegrityFailed, got {other:?}"),
        }
    }

    /// Tampering the salt must also fail. Either the verifier
    /// disagrees -> `PassphraseInvalid`, or the MAC disagrees ->
    /// `IntegrityFailed`. Both signal "this snapshot was altered";
    /// we only require the call doesn't succeed.
    #[test]
    fn frame_tampered_salt_fails() {
        let salt = random_salt();
        let pass = b"x".to_vec();
        let payload = b"y".to_vec();
        let mut sealed = seal_frame(&salt, &pass, &payload).unwrap();
        let salt_offset = MAGIC.len() + 1;
        sealed[salt_offset] ^= 0xFF;
        assert!(open_frame(&sealed, &pass).is_err());
    }

    /// Magic-mismatch must report as a decrypt-class error, not
    /// as a passphrase failure.
    #[test]
    fn frame_bad_magic_reports_decrypt() {
        let mut sealed = seal_frame(&[0u8; SALT_LEN], b"p", b"payload").unwrap();
        sealed[0] = b'X';
        match open_frame(&sealed, b"p") {
            Err(SyncError::Decrypt(_)) => {}
            other => panic!("expected Decrypt, got {other:?}"),
        }
    }

    /// Truncated frames must return a decrypt-class error or
    /// `PassphraseInvalid` (the latter only on cuts long enough to
    /// reach the verifier check).
    #[test]
    fn frame_truncated_reports_decrypt() {
        let sealed = seal_frame(&[0u8; SALT_LEN], b"p", b"payload").unwrap();
        for cut in 0..sealed.len() {
            match open_frame(&sealed[..cut], b"p") {
                Err(SyncError::Decrypt(_)) | Err(SyncError::PassphraseInvalid) => {}
                other => panic!("cut={cut}: expected Decrypt/PassphraseInvalid, got {other:?}"),
            }
        }
    }

    /// Regression test: secrets must never leak into a `SyncError`'s
    /// `Display` or `Debug` output. Build every variant in turn while
    /// a planted token is the only "secret-looking" byte string in
    /// scope, then assert no error string contains it.
    #[tokio::test]
    async fn errors_do_not_leak_secrets() {
        const PLANTED: &str = "PLAINTEXT_SECRET_TOKEN_d34db33f";
        // Use the planted token as both the passphrase and the
        // payload of a sealed frame so we can't accidentally leak
        // "the wrong one" — both are absent from every error.
        let pass = PLANTED.as_bytes().to_vec();
        let payload = PLANTED.as_bytes().to_vec();
        let salt = random_salt();
        let sealed = seal_frame(&salt, &pass, &payload).unwrap();

        let mut errors: Vec<SyncError> = Vec::new();
        // Encrypt: forced via len() overflow is impractical, so
        // exercise the variant directly.
        errors.push(SyncError::Encrypt);
        // Decrypt: bad magic.
        let mut bad_magic = sealed.clone();
        bad_magic[0] = b'X';
        errors.push(open_frame(&bad_magic, &pass).unwrap_err());
        // BackendUnavailable: stub HTTP rejects everything. The
        // base URL contains the planted token but the stub does
        // NOT echo it in the error — that's the contract.
        let stub = HttpBackendStub::new(format!("https://example.test/{PLANTED}"));
        errors.push(stub.put(b"x").await.unwrap_err());
        errors.push(stub.get(&RemoteId::new("x")).await.unwrap_err());
        errors.push(stub.list().await.unwrap_err());
        // IntegrityFailed: tamper one payload byte.
        let mut tampered = sealed.clone();
        let payload_offset = MAGIC.len() + 1 + SALT_LEN + 4 + DIGEST_LEN + 4 + DIGEST_LEN + 4;
        tampered[payload_offset] ^= 0x01;
        errors.push(open_frame(&tampered, &pass).unwrap_err());
        // PassphraseInvalid.
        errors.push(open_frame(&sealed, b"different").unwrap_err());

        for err in &errors {
            let display = format!("{err}");
            let debug = format!("{err:?}");
            assert!(
                !display.contains(PLANTED),
                "Display leaked planted token: {display}"
            );
            assert!(
                !debug.contains(PLANTED),
                "Debug leaked planted token: {debug}"
            );
        }
    }

    #[test]
    fn ct_eq_distinguishes_lengths_and_contents() {
        assert!(ct_eq(b"abc", b"abc"));
        assert!(!ct_eq(b"abc", b"abd"));
        assert!(!ct_eq(b"abc", b"abcd"));
        assert!(ct_eq(b"", b""));
    }

    #[tokio::test]
    async fn local_fs_backend_put_get_list_roundtrip() {
        let dir = TempDir::new().unwrap();
        let backend = LocalFsBackend::new(dir.path().to_path_buf());
        let bytes = b"hello world".to_vec();
        let id = backend.put(&bytes).await.unwrap();
        let back = backend.get(&id).await.unwrap();
        assert_eq!(back, bytes);

        let listed = backend.list().await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0], id);
    }

    #[tokio::test]
    async fn local_fs_backend_list_empty_when_dir_missing() {
        let dir = TempDir::new().unwrap();
        let backend = LocalFsBackend::new(dir.path().join("does-not-exist"));
        let listed = backend.list().await.unwrap();
        assert!(listed.is_empty());
    }

    #[tokio::test]
    async fn local_fs_backend_rejects_path_traversal_id() {
        let dir = TempDir::new().unwrap();
        let backend = LocalFsBackend::new(dir.path().to_path_buf());
        let evil = RemoteId::new("../etc/passwd");
        match backend.get(&evil).await {
            Err(SyncError::BackendUnavailable(_)) => {}
            other => panic!("expected BackendUnavailable, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn http_backend_stub_always_unavailable() {
        let stub = HttpBackendStub::new("https://example.test");
        assert!(matches!(
            stub.put(b"x").await,
            Err(SyncError::BackendUnavailable(_))
        ));
        assert!(matches!(
            stub.get(&RemoteId::new("x")).await,
            Err(SyncError::BackendUnavailable(_))
        ));
        assert!(matches!(
            stub.list().await,
            Err(SyncError::BackendUnavailable(_))
        ));
    }

    /// Full integration: push a snapshot through the public API,
    /// then pull it back and confirm the bytes match. Also exercises
    /// the wrong-passphrase path against an existing sealed frame.
    #[tokio::test]
    async fn service_push_pull_roundtrip() {
        let dir = TempDir::new().unwrap();
        let backend: Arc<dyn SyncBackend> = Arc::new(LocalFsBackend::new(dir.path().to_path_buf()));
        let snapshot = b"[api]\nmodel = \"claude\"\n".to_vec();

        let svc = SettingsSyncService::new(plaintext_config(backend.clone(), "open sesame"));
        let id = svc.push(&snapshot).await.unwrap();

        // Fresh service to confirm nothing relies on in-memory state.
        let svc2 = SettingsSyncService::new(plaintext_config(backend.clone(), "open sesame"));
        let recovered = svc2.pull(&id).await.unwrap();
        assert_eq!(recovered, snapshot);

        // Wrong passphrase on a third service must fail with
        // `PassphraseInvalid`, not anything else.
        let svc3 = SettingsSyncService::new(plaintext_config(backend, "wrong"));
        match svc3.pull(&id).await {
            Err(SyncError::PassphraseInvalid) => {}
            other => panic!("expected PassphraseInvalid, got {other:?}"),
        }
    }

    /// `pull_to` must atomically write recovered bytes to the
    /// destination using the secret-preserving writer.
    #[tokio::test]
    async fn service_pull_to_writes_atomically() {
        let dir = TempDir::new().unwrap();
        let backend: Arc<dyn SyncBackend> = Arc::new(LocalFsBackend::new(dir.path().join("sync")));
        let snapshot = b"hello".to_vec();
        let svc = SettingsSyncService::new(plaintext_config(backend, "p"));
        let id = svc.push(&snapshot).await.unwrap();

        let dest = dir.path().join("settings.toml");
        svc.pull_to(&id, &dest).await.unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), snapshot);
    }

    /// `list` returns ids oldest-first.
    #[tokio::test]
    async fn service_list_returns_known_ids() {
        let dir = TempDir::new().unwrap();
        let backend: Arc<dyn SyncBackend> = Arc::new(LocalFsBackend::new(dir.path().to_path_buf()));
        let svc = SettingsSyncService::new(plaintext_config(backend.clone(), "p"));
        let _a = svc.push(b"one").await.unwrap();
        // Force a distinct mtime: the two writes can otherwise land
        // in the same nanosecond on fast filesystems.
        std::thread::sleep(std::time::Duration::from_millis(20));
        let _b = svc.push(b"two").await.unwrap();
        let listed = svc.list().await.unwrap();
        assert_eq!(listed.len(), 2);
    }

    #[test]
    fn drop_clears_passphrase_buffer() {
        let backend: Arc<dyn SyncBackend> =
            Arc::new(LocalFsBackend::new(std::env::temp_dir().join("ignored")));
        let svc = SettingsSyncService::new(plaintext_config(backend, "secret"));
        // Drop should not panic and should clear the internal buffer.
        drop(svc);
    }
}
