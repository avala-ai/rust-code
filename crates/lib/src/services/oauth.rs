//! Generic OAuth 2.0 + PKCE browser-based authentication service.
//!
//! Provider-agnostic. Callers configure an [`OAuthProviderConfig`]
//! (authorization endpoint, token endpoint, client id, scopes,
//! redirect uri) and drive the flow via [`OAuthService`].
//!
//! Flow:
//!
//! 1. Caller invokes [`OAuthService::login`].
//! 2. The service generates a PKCE verifier + S256 challenge
//!    (RFC 7636), spawns a loopback HTTP server on
//!    `127.0.0.1:0`, and opens the authorization URL in the
//!    user's browser.
//! 3. The provider redirects back to the loopback with a
//!    `code` query param; the service exchanges it for an
//!    access + refresh token at the token endpoint.
//! 4. The token set is persisted via the [`CredentialStore`]
//!    abstraction. The default store writes a JSON file under
//!    the agent config directory with `0o600` perms (unix);
//!    a future PR can plug an OS keychain backend in without
//!    touching this module.
//! 5. [`OAuthService::current_token`] returns a valid access
//!    token, refreshing it transparently when within the
//!    expiry skew. On refresh failure it surfaces
//!    [`OAuthError::ReauthRequired`] so the caller can drive
//!    a fresh browser flow.
//!
//! Security notes:
//!
//! - Token strings never appear in [`OAuthError`] variants
//!   nor in any `Debug` output. [`TokenSet`] has a custom
//!   `Debug` impl that prints `***` for the token fields.
//! - The loopback server times out after [`LOOPBACK_TIMEOUT`]
//!   (5 min) so an abandoned browser tab does not hang the
//!   CLI forever.
//! - The credential file is created with `0o600` on unix.
//! - On Linux without a writable config dir (some CI
//!   containers), the file store returns
//!   [`OAuthError::KeychainError`] — never panics.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Mutex;

/// How close to expiry a token is considered stale.
const REFRESH_SKEW_SECS: i64 = 60;

/// Hard ceiling on how long the loopback server waits for a
/// redirect before giving up.
pub const LOOPBACK_TIMEOUT: Duration = Duration::from_secs(300);

/// Per-provider OAuth configuration. Pure data — no network
/// behavior is encoded here. The same struct is reused for
/// every provider; differences are URLs and scopes.
#[derive(Debug, Clone)]
pub struct OAuthProviderConfig {
    /// Short, file-system-safe identifier used to namespace
    /// the credential store entry. Must be unique per
    /// provider so multiple providers can coexist on the
    /// same machine without colliding.
    pub provider_name: String,
    pub authorization_url: String,
    pub token_url: String,
    pub client_id: String,
    pub scopes: Vec<String>,
    /// Redirect URI registered with the provider. The
    /// `port` part is replaced at runtime with the
    /// loopback port the OS hands us; pass any placeholder
    /// (e.g. `http://127.0.0.1/callback`) — we only use
    /// the path component.
    pub redirect_uri: String,
}

impl OAuthProviderConfig {
    fn redirect_path(&self) -> String {
        match url_path(&self.redirect_uri) {
            Some(path) if !path.is_empty() => path,
            _ => "/callback".to_string(),
        }
    }
}

/// Tokens returned by the provider, plus a cached expiry.
///
/// `Debug` is hand-written so `tracing::trace!(?tokens, ...)`
/// in any caller does not leak credentials into log output.
#[derive(Clone, Serialize, Deserialize)]
pub struct TokenSet {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    /// Absolute UTC instant at which `access_token` expires,
    /// or `None` if the provider did not specify an expiry.
    #[serde(default)]
    pub expires_at: Option<DateTime<Utc>>,
}

impl std::fmt::Debug for TokenSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenSet")
            .field("access_token", &"***")
            .field("refresh_token", &self.refresh_token.as_ref().map(|_| "***"))
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

impl TokenSet {
    fn is_stale(&self) -> bool {
        match self.expires_at {
            Some(exp) => exp <= Utc::now() + chrono::Duration::seconds(REFRESH_SKEW_SECS),
            None => false,
        }
    }
}

/// Service-wide error type. **No variant carries a token
/// string**; bodies surfaced from the provider are passed
/// through unchanged because the provider is the one that
/// chose what goes in them — but the service never inserts
/// a token into an error.
#[derive(Debug, Error)]
pub enum OAuthError {
    #[error("failed to launch browser: {0}")]
    BrowserLaunchFailed(String),

    #[error("loopback server failed: {0}")]
    LoopbackFailed(String),

    #[error("token exchange failed (status {status}): {body}")]
    TokenExchangeFailed { status: u16, body: String },

    #[error("token refresh failed (status {status}): {body}")]
    RefreshFailed { status: u16, body: String },

    #[error("re-authentication required")]
    ReauthRequired,

    #[error("credential store error: {0}")]
    KeychainError(String),

    #[error("authorization cancelled")]
    Cancelled,
}

/// Pluggable persistent store for [`TokenSet`] entries.
///
/// The default implementation is a file under the agent
/// config dir with `0o600` permissions; a future PR can plug
/// in an OS keychain backend behind the same trait without
/// touching the OAuth flow itself.
pub trait CredentialStore: Send + Sync {
    fn load(&self, key: &str) -> Result<Option<TokenSet>, OAuthError>;
    fn save(&self, key: &str, tokens: &TokenSet) -> Result<(), OAuthError>;
    fn delete(&self, key: &str) -> Result<(), OAuthError>;
}

/// File-based credential store. Stores each entry as
/// `<config>/credentials/<key>.json`.
pub struct FileCredentialStore {
    root: PathBuf,
}

impl FileCredentialStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// Default location: `<agent-config>/credentials/`. Returns
    /// [`OAuthError::KeychainError`] if the agent config dir
    /// cannot be resolved (e.g. CI sandbox without HOME).
    pub fn default_location() -> Result<Self, OAuthError> {
        let dir = crate::config::agent_config_dir()
            .ok_or_else(|| OAuthError::KeychainError("agent config dir is not available".into()))?;
        Ok(Self::new(dir.join("credentials")))
    }

    fn path_for(&self, key: &str) -> PathBuf {
        let safe: String = key
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        self.root.join(format!("{safe}.json"))
    }
}

impl CredentialStore for FileCredentialStore {
    fn load(&self, key: &str) -> Result<Option<TokenSet>, OAuthError> {
        let path = self.path_for(key);
        match std::fs::read_to_string(&path) {
            Ok(s) => {
                let tokens: TokenSet = serde_json::from_str(&s)
                    .map_err(|e| OAuthError::KeychainError(e.to_string()))?;
                Ok(Some(tokens))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(OAuthError::KeychainError(e.to_string())),
        }
    }

    fn save(&self, key: &str, tokens: &TokenSet) -> Result<(), OAuthError> {
        std::fs::create_dir_all(&self.root)
            .map_err(|e| OAuthError::KeychainError(e.to_string()))?;
        let path = self.path_for(key);
        let data =
            serde_json::to_string(tokens).map_err(|e| OAuthError::KeychainError(e.to_string()))?;

        let mut options = std::fs::OpenOptions::new();
        options.create(true).truncate(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&path)
            .map_err(|e| OAuthError::KeychainError(e.to_string()))?;
        use std::io::Write;
        file.write_all(data.as_bytes())
            .map_err(|e| OAuthError::KeychainError(e.to_string()))
    }

    fn delete(&self, key: &str) -> Result<(), OAuthError> {
        let path = self.path_for(key);
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(OAuthError::KeychainError(e.to_string())),
        }
    }
}

/// Hook for spawning a browser. Defaults to the platform
/// default opener via [`std::process::Command`]; tests
/// override it with a closure that drives the flow
/// programmatically.
pub type BrowserLauncher = Arc<dyn Fn(&str) -> Result<(), OAuthError> + Send + Sync + 'static>;

fn default_browser_launcher(url: &str) -> Result<(), OAuthError> {
    #[cfg(target_os = "linux")]
    let candidates: &[&str] = &["xdg-open"];
    #[cfg(target_os = "macos")]
    let candidates: &[&str] = &["open"];
    #[cfg(target_os = "windows")]
    let candidates: &[&str] = &["cmd"];

    #[cfg(target_os = "windows")]
    let args_for = |bin: &str| {
        if bin == "cmd" {
            vec!["/C", "start", "", url]
        } else {
            vec![url]
        }
    };
    #[cfg(not(target_os = "windows"))]
    let args_for = |_bin: &str| vec![url];

    let mut last_err = String::from("no opener available");
    for bin in candidates {
        let res = std::process::Command::new(bin)
            .args(args_for(bin))
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        match res {
            Ok(_) => return Ok(()),
            Err(e) => last_err = e.to_string(),
        }
    }
    Err(OAuthError::BrowserLaunchFailed(last_err))
}

/// The service. Cheap to clone (`Arc` internally).
pub struct OAuthService {
    config: OAuthProviderConfig,
    store: Arc<dyn CredentialStore>,
    http: reqwest::Client,
    /// Service identifier baked into the credential key so
    /// keys do not collide with other agent-code data.
    service_name: String,
    browser: BrowserLauncher,
    /// Serialize concurrent token reads / refreshes so two
    /// concurrent callers cannot hammer the refresh endpoint.
    state: Arc<Mutex<()>>,
}

impl OAuthService {
    /// Build a service with the default credential store
    /// (file under the agent config directory) and the
    /// platform's default browser opener.
    pub fn new(config: OAuthProviderConfig) -> Self {
        let store: Arc<dyn CredentialStore> = match FileCredentialStore::default_location() {
            Ok(s) => Arc::new(s),
            Err(_) => Arc::new(NullCredentialStore),
        };
        Self::with_store(config, store)
    }

    /// Build a service with a caller-supplied credential
    /// store. Useful for tests and for embedding in a host
    /// that already manages secrets.
    pub fn with_store(config: OAuthProviderConfig, store: Arc<dyn CredentialStore>) -> Self {
        Self {
            config,
            store,
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .unwrap_or_default(),
            service_name: "agent-code".to_string(),
            browser: Arc::new(default_browser_launcher),
            state: Arc::new(Mutex::new(())),
        }
    }

    /// Override the browser launcher (tests; host apps that
    /// drive the flow programmatically).
    pub fn with_browser_launcher(mut self, launcher: BrowserLauncher) -> Self {
        self.browser = launcher;
        self
    }

    fn credential_key(&self) -> String {
        format!("{}:{}", self.service_name, self.config.provider_name)
    }

    /// Run the full browser-based OAuth flow and persist the
    /// resulting tokens. Errors with [`OAuthError::Cancelled`]
    /// if the user closes the browser before approving.
    pub async fn login(&self) -> Result<TokenSet, OAuthError> {
        let _guard = self.state.lock().await;

        let pkce = PkcePair::generate();
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|e| OAuthError::LoopbackFailed(e.to_string()))?;
        let port = listener
            .local_addr()
            .map_err(|e| OAuthError::LoopbackFailed(e.to_string()))?
            .port();

        let redirect_path = self.config.redirect_path();
        let redirect_uri = format!("http://127.0.0.1:{port}{redirect_path}");
        let state = random_url_token(16);

        let auth_url =
            build_authorization_url(&self.config, &redirect_uri, &pkce.code_challenge, &state);

        (self.browser)(&auth_url)?;

        let captured = capture_redirect(listener, &state).await?;

        let tokens = exchange_code(
            &self.http,
            &self.config,
            &captured,
            &redirect_uri,
            &pkce.code_verifier,
        )
        .await?;
        self.store.save(&self.credential_key(), &tokens)?;
        Ok(tokens)
    }

    /// Return a valid access token. Refreshes transparently
    /// if the cached token is within the expiry skew. Errors
    /// with [`OAuthError::ReauthRequired`] when no stored
    /// credentials exist or refresh has failed terminally.
    pub async fn current_token(&self) -> Result<String, OAuthError> {
        let _guard = self.state.lock().await;
        let key = self.credential_key();
        let mut tokens = self.store.load(&key)?.ok_or(OAuthError::ReauthRequired)?;

        if tokens.is_stale() {
            let refresh = tokens
                .refresh_token
                .clone()
                .ok_or(OAuthError::ReauthRequired)?;
            match refresh_tokens(&self.http, &self.config, &refresh).await {
                Ok(new_tokens) => {
                    tokens = merge_refresh(tokens, new_tokens);
                    self.store.save(&key, &tokens)?;
                }
                Err(OAuthError::RefreshFailed { status, .. }) if is_terminal_refresh(status) => {
                    let _ = self.store.delete(&key);
                    return Err(OAuthError::ReauthRequired);
                }
                Err(e) => return Err(e),
            }
        }

        Ok(tokens.access_token)
    }

    /// Clear stored credentials. Idempotent.
    pub fn logout(&self) -> Result<(), OAuthError> {
        self.store.delete(&self.credential_key())
    }
}

/// Used as the credential store when the agent config dir
/// is unavailable. Every call returns
/// [`OAuthError::KeychainError`] so the caller learns about
/// the misconfiguration the first time it tries to persist
/// or read tokens — instead of the service silently dropping
/// credentials.
struct NullCredentialStore;
impl CredentialStore for NullCredentialStore {
    fn load(&self, _key: &str) -> Result<Option<TokenSet>, OAuthError> {
        Err(OAuthError::KeychainError(
            "credential store unavailable: no writable config dir".into(),
        ))
    }
    fn save(&self, _key: &str, _tokens: &TokenSet) -> Result<(), OAuthError> {
        Err(OAuthError::KeychainError(
            "credential store unavailable: no writable config dir".into(),
        ))
    }
    fn delete(&self, _key: &str) -> Result<(), OAuthError> {
        Ok(())
    }
}

/// 4xx on refresh = the token has been revoked or rotated;
/// the user must re-authenticate. 5xx is treated as a
/// transient failure and surfaced verbatim.
fn is_terminal_refresh(status: u16) -> bool {
    (400..500).contains(&status)
}

fn merge_refresh(prev: TokenSet, new: TokenSet) -> TokenSet {
    TokenSet {
        access_token: new.access_token,
        // Some providers return a fresh refresh token on
        // every refresh; others rotate it only on login.
        refresh_token: new.refresh_token.or(prev.refresh_token),
        expires_at: new.expires_at,
    }
}

// ---- PKCE (RFC 7636) ----

#[derive(Debug, Clone)]
pub struct PkcePair {
    pub code_verifier: String,
    pub code_challenge: String,
}

impl PkcePair {
    /// Generate a fresh PKCE pair. Verifier is 64 chars from
    /// the unreserved alphabet (RFC 7636 §4.1: 43-128).
    pub fn generate() -> Self {
        let verifier = random_url_token(64);
        let mut hasher = Sha256::new();
        hasher.update(verifier.as_bytes());
        let digest = hasher.finalize();
        let challenge = base64_url_encode(&digest);
        Self {
            code_verifier: verifier,
            code_challenge: challenge,
        }
    }
}

/// RFC 4648 §5 base64url, no padding.
pub(crate) fn base64_url_encode(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    let mut i = 0;
    while i < input.len() {
        let b0 = input[i];
        let b1 = input.get(i + 1).copied().unwrap_or(0);
        let b2 = input.get(i + 2).copied().unwrap_or(0);
        let triple = (u32::from(b0) << 16) | (u32::from(b1) << 8) | u32::from(b2);
        out.push(ALPHABET[((triple >> 18) & 0x3f) as usize] as char);
        out.push(ALPHABET[((triple >> 12) & 0x3f) as usize] as char);
        if i + 1 < input.len() {
            out.push(ALPHABET[((triple >> 6) & 0x3f) as usize] as char);
        }
        if i + 2 < input.len() {
            out.push(ALPHABET[(triple & 0x3f) as usize] as char);
        }
        i += 3;
    }
    out
}

/// Generate `len` chars of url-safe random data. Sourced
/// from `uuid::Uuid::new_v4()` (which uses OS randomness)
/// so we do not need to add a `rand` workspace dep.
fn random_url_token(len: usize) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut bytes = Vec::with_capacity(len);
    while bytes.len() < len {
        bytes.extend_from_slice(uuid::Uuid::new_v4().as_bytes());
    }
    bytes
        .into_iter()
        .take(len)
        .map(|b| ALPHABET[(b as usize) % ALPHABET.len()] as char)
        .collect()
}

// ---- Loopback redirect capture ----

/// Outcome of the loopback wait. Either a `code` arrived,
/// the provider redirected back with an `error`, or the
/// timeout fired.
#[derive(Debug)]
struct CapturedCode {
    code: String,
}

async fn capture_redirect(
    listener: TcpListener,
    expected_state: &str,
) -> Result<CapturedCode, OAuthError> {
    let fut = async {
        loop {
            let (mut socket, _) = listener
                .accept()
                .await
                .map_err(|e| OAuthError::LoopbackFailed(e.to_string()))?;
            let mut buf = vec![0u8; 4096];
            let n = socket
                .read(&mut buf)
                .await
                .map_err(|e| OAuthError::LoopbackFailed(e.to_string()))?;
            if n == 0 {
                continue;
            }
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            let path_and_query = parse_request_target(&req);
            let Some(path_and_query) = path_and_query else {
                let _ = socket
                    .write_all(http_response("Bad Request").as_bytes())
                    .await;
                continue;
            };
            let params = parse_query(&path_and_query);

            if let Some(err) = params.get("error") {
                let _ = socket
                    .write_all(http_response("Authorization failed.").as_bytes())
                    .await;
                if err == "access_denied" {
                    return Err(OAuthError::Cancelled);
                }
                return Err(OAuthError::TokenExchangeFailed {
                    status: 0,
                    body: format!("authorization error: {err}"),
                });
            }

            // State check guards against forged callbacks.
            if let Some(state) = params.get("state")
                && state != expected_state
            {
                let _ = socket
                    .write_all(http_response("State mismatch.").as_bytes())
                    .await;
                continue;
            }

            if let Some(code) = params.get("code") {
                let _ = socket
                    .write_all(
                        http_response("You can close this tab and return to the terminal.")
                            .as_bytes(),
                    )
                    .await;
                return Ok(CapturedCode { code: code.clone() });
            }

            let _ = socket
                .write_all(http_response("Waiting for code...").as_bytes())
                .await;
        }
    };

    match tokio::time::timeout(LOOPBACK_TIMEOUT, fut).await {
        Ok(res) => res,
        Err(_) => Err(OAuthError::LoopbackFailed(
            "timed out waiting for redirect".into(),
        )),
    }
}

fn http_response(body: &str) -> String {
    let html = format!(
        "<!doctype html><meta charset=utf-8><title>Auth</title>\
        <body style=\"font-family:system-ui;text-align:center;padding:3em;\">\
        <h1>{}</h1></body>",
        html_escape(body)
    );
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        html.len(),
        html
    )
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn parse_request_target(req: &str) -> Option<String> {
    let line = req.lines().next()?;
    let mut parts = line.split_whitespace();
    let _method = parts.next()?;
    let target = parts.next()?;
    Some(target.to_string())
}

fn parse_query(path_and_query: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let Some(query) = path_and_query.split_once('?').map(|(_, q)| q) else {
        return out;
    };
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (k, v) = match pair.split_once('=') {
            Some(kv) => kv,
            None => (pair, ""),
        };
        out.insert(percent_decode(k), percent_decode(v));
    }
    out
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hi = (bytes[i + 1] as char).to_digit(16);
                let lo = (bytes[i + 2] as char).to_digit(16);
                match (hi, lo) {
                    (Some(h), Some(l)) => {
                        out.push((h * 16 + l) as u8);
                        i += 3;
                    }
                    _ => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            other => {
                out.push(other);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn url_path(uri: &str) -> Option<String> {
    let after_scheme = uri.split_once("://").map(|(_, rest)| rest).unwrap_or(uri);
    let path_idx = after_scheme.find('/')?;
    let path = &after_scheme[path_idx..];
    let path = path.split('?').next().unwrap_or(path);
    Some(path.to_string())
}

fn build_authorization_url(
    cfg: &OAuthProviderConfig,
    redirect_uri: &str,
    code_challenge: &str,
    state: &str,
) -> String {
    let scope = cfg.scopes.join(" ");
    let mut url = cfg.authorization_url.clone();
    let sep = if url.contains('?') { '&' } else { '?' };
    url.push(sep);
    url.push_str(&format!(
        "response_type=code&client_id={}&redirect_uri={}&scope={}&state={}&code_challenge={}&code_challenge_method=S256",
        url_encode(&cfg.client_id),
        url_encode(redirect_uri),
        url_encode(&scope),
        url_encode(state),
        url_encode(code_challenge),
    ));
    url
}

fn url_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

// ---- Token endpoint ----

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<i64>,
}

impl From<TokenResponse> for TokenSet {
    fn from(t: TokenResponse) -> Self {
        TokenSet {
            access_token: t.access_token,
            refresh_token: t.refresh_token,
            expires_at: t
                .expires_in
                .map(|secs| Utc::now() + chrono::Duration::seconds(secs)),
        }
    }
}

async fn exchange_code(
    http: &reqwest::Client,
    cfg: &OAuthProviderConfig,
    captured: &CapturedCode,
    redirect_uri: &str,
    code_verifier: &str,
) -> Result<TokenSet, OAuthError> {
    let form = [
        ("grant_type", "authorization_code"),
        ("code", captured.code.as_str()),
        ("redirect_uri", redirect_uri),
        ("client_id", cfg.client_id.as_str()),
        ("code_verifier", code_verifier),
    ];
    let response = http
        .post(&cfg.token_url)
        .form(&form)
        .send()
        .await
        .map_err(|e| OAuthError::TokenExchangeFailed {
            status: 0,
            body: e.to_string(),
        })?;
    let status = response.status().as_u16();
    if !response.status().is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(OAuthError::TokenExchangeFailed { status, body });
    }
    let parsed: TokenResponse =
        response
            .json()
            .await
            .map_err(|e| OAuthError::TokenExchangeFailed {
                status,
                body: e.to_string(),
            })?;
    Ok(parsed.into())
}

async fn refresh_tokens(
    http: &reqwest::Client,
    cfg: &OAuthProviderConfig,
    refresh_token: &str,
) -> Result<TokenSet, OAuthError> {
    let form = [
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", cfg.client_id.as_str()),
    ];
    let response = http
        .post(&cfg.token_url)
        .form(&form)
        .send()
        .await
        .map_err(|e| OAuthError::RefreshFailed {
            status: 0,
            body: e.to_string(),
        })?;
    let status = response.status().as_u16();
    if !response.status().is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(OAuthError::RefreshFailed { status, body });
    }
    let parsed: TokenResponse = response
        .json()
        .await
        .map_err(|e| OAuthError::RefreshFailed {
            status,
            body: e.to_string(),
        })?;
    Ok(parsed.into())
}

// ---- Tests ----

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // RFC 7636 §4.1: `code_verifier = high-entropy
    // cryptographic random STRING using the unreserved
    // characters [A-Z] / [a-z] / [0-9] / "-" / "." / "_" /
    // "~", with a minimum length of 43 characters and a
    // maximum length of 128 characters.`
    #[test]
    fn pkce_verifier_meets_rfc7636_length_and_alphabet() {
        for _ in 0..32 {
            let pair = PkcePair::generate();
            assert!(
                pair.code_verifier.len() >= 43 && pair.code_verifier.len() <= 128,
                "verifier length out of range: {}",
                pair.code_verifier.len()
            );
            for c in pair.code_verifier.chars() {
                assert!(
                    c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_' | '~'),
                    "verifier contains illegal char {c:?}"
                );
            }
        }
    }

    // RFC 7636 §4.2: `code_challenge = BASE64URL-ENCODE(
    // SHA256(ASCII(code_verifier)))`
    #[test]
    fn pkce_challenge_is_base64url_sha256_of_verifier() {
        let pair = PkcePair::generate();
        let mut hasher = Sha256::new();
        hasher.update(pair.code_verifier.as_bytes());
        let expected = base64_url_encode(&hasher.finalize());
        assert_eq!(pair.code_challenge, expected);
        // No padding.
        assert!(!pair.code_challenge.contains('='));
    }

    // RFC 7636 Appendix B test vector.
    #[test]
    fn pkce_challenge_matches_rfc7636_test_vector() {
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let mut hasher = Sha256::new();
        hasher.update(verifier.as_bytes());
        let challenge = base64_url_encode(&hasher.finalize());
        assert_eq!(challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }

    #[test]
    fn base64_url_encode_no_padding_url_safe_alphabet() {
        // RFC 4648 §10 test vectors transposed to url-safe
        // alphabet.
        assert_eq!(base64_url_encode(b""), "");
        assert_eq!(base64_url_encode(b"f"), "Zg");
        assert_eq!(base64_url_encode(b"fo"), "Zm8");
        assert_eq!(base64_url_encode(b"foo"), "Zm9v");
        assert_eq!(base64_url_encode(b"foobar"), "Zm9vYmFy");
        // Bytes that would produce '+' / '/' in standard
        // base64 must produce '-' / '_' here.
        let with_specials = &[0xfb, 0xff, 0xbf];
        let encoded = base64_url_encode(with_specials);
        assert!(!encoded.contains('+'));
        assert!(!encoded.contains('/'));
    }

    #[tokio::test]
    async fn loopback_captures_code_param() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let state = "the-state".to_string();

        let server = tokio::spawn(async move { capture_redirect(listener, "the-state").await });

        // Drive the loopback as a real HTTP client would.
        let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        let req =
            format!("GET /callback?code=abc123&state={state} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n");
        s.write_all(req.as_bytes()).await.unwrap();
        let mut response = Vec::new();
        s.read_to_end(&mut response).await.unwrap();
        assert!(String::from_utf8_lossy(&response).contains("close this tab"));

        let captured = server.await.unwrap().unwrap();
        assert_eq!(captured.code, "abc123");
    }

    #[tokio::test]
    async fn loopback_rejects_state_mismatch_then_accepts_correct_state() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        let server = tokio::spawn(async move { capture_redirect(listener, "the-state").await });

        // First a bad-state attempt (server keeps waiting).
        let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        s.write_all(b"GET /callback?code=evil&state=wrong HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
            .await
            .unwrap();
        let mut buf = Vec::new();
        s.read_to_end(&mut buf).await.unwrap();
        assert!(String::from_utf8_lossy(&buf).contains("State mismatch"));

        // Then the real one.
        let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        s.write_all(b"GET /callback?code=good&state=the-state HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
            .await
            .unwrap();
        let mut buf = Vec::new();
        s.read_to_end(&mut buf).await.unwrap();
        let _ = buf;

        let captured = server.await.unwrap().unwrap();
        assert_eq!(captured.code, "good");
    }

    #[test]
    fn parse_query_handles_percent_encoding_and_plus() {
        let q = parse_query("/cb?code=a%2Fb%2Bc&state=hello+world&empty=");
        assert_eq!(q.get("code").map(String::as_str), Some("a/b+c"));
        assert_eq!(q.get("state").map(String::as_str), Some("hello world"));
        assert_eq!(q.get("empty").map(String::as_str), Some(""));
    }

    #[test]
    fn url_path_extracts_path_component() {
        assert_eq!(
            url_path("http://127.0.0.1/callback").as_deref(),
            Some("/callback")
        );
        assert_eq!(
            url_path("http://example.com:8080/cb?x=1").as_deref(),
            Some("/cb")
        );
        assert_eq!(url_path("http://example.com").as_deref(), None);
    }

    #[test]
    fn redirect_path_falls_back_to_default() {
        let cfg = OAuthProviderConfig {
            provider_name: "p".into(),
            authorization_url: "https://x/auth".into(),
            token_url: "https://x/token".into(),
            client_id: "c".into(),
            scopes: vec![],
            redirect_uri: "http://example.com".into(),
        };
        assert_eq!(cfg.redirect_path(), "/callback");
    }

    #[test]
    fn build_authorization_url_includes_pkce_and_state() {
        let cfg = OAuthProviderConfig {
            provider_name: "p".into(),
            authorization_url: "https://x/auth".into(),
            token_url: "https://x/token".into(),
            client_id: "client-id".into(),
            scopes: vec!["read".into(), "write".into()],
            redirect_uri: "http://127.0.0.1/cb".into(),
        };
        let url =
            build_authorization_url(&cfg, "http://127.0.0.1:1234/cb", "challenge", "the-state");
        assert!(url.contains("response_type=code"));
        assert!(url.contains("client_id=client-id"));
        assert!(url.contains("code_challenge=challenge"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("state=the-state"));
        assert!(url.contains("scope=read%20write"));
        assert!(url.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A1234%2Fcb"));
    }

    #[test]
    fn build_authorization_url_appends_to_existing_query_string() {
        let cfg = OAuthProviderConfig {
            provider_name: "p".into(),
            authorization_url: "https://x/auth?audience=api".into(),
            token_url: "https://x/token".into(),
            client_id: "c".into(),
            scopes: vec![],
            redirect_uri: "http://127.0.0.1/cb".into(),
        };
        let url = build_authorization_url(&cfg, "http://127.0.0.1:1/cb", "ch", "st");
        assert!(url.contains("audience=api&response_type=code"));
    }

    // ---- Token-leak regression: error Display impls must
    // not contain any token strings. ----

    #[test]
    fn errors_do_not_carry_or_leak_token_strings() {
        let fake_token = "FAKE_ACCESS_TOKEN_DO_NOT_LEAK_xyzzy";
        let cases: Vec<OAuthError> = vec![
            OAuthError::BrowserLaunchFailed("xdg-open missing".into()),
            OAuthError::LoopbackFailed("bind failed".into()),
            OAuthError::TokenExchangeFailed {
                status: 400,
                body: "{\"error\":\"invalid_grant\"}".into(),
            },
            OAuthError::RefreshFailed {
                status: 401,
                body: "{\"error\":\"invalid_grant\"}".into(),
            },
            OAuthError::ReauthRequired,
            OAuthError::KeychainError("locked".into()),
            OAuthError::Cancelled,
        ];
        for err in &cases {
            let displayed = err.to_string();
            let debugged = format!("{err:?}");
            assert!(
                !displayed.contains(fake_token),
                "Display leaked token: {displayed}"
            );
            assert!(
                !debugged.contains(fake_token),
                "Debug leaked token: {debugged}"
            );
        }
    }

    #[test]
    fn token_set_debug_redacts_secrets() {
        let ts = TokenSet {
            access_token: "REAL_ACCESS_DO_NOT_LEAK".into(),
            refresh_token: Some("REAL_REFRESH_DO_NOT_LEAK".into()),
            expires_at: None,
        };
        let dbg = format!("{ts:?}");
        assert!(!dbg.contains("REAL_ACCESS_DO_NOT_LEAK"));
        assert!(!dbg.contains("REAL_REFRESH_DO_NOT_LEAK"));
        assert!(dbg.contains("***"));
    }

    #[test]
    fn token_set_is_stale_uses_refresh_skew() {
        let mut t = TokenSet {
            access_token: "x".into(),
            refresh_token: None,
            expires_at: Some(Utc::now() + chrono::Duration::seconds(30)),
        };
        assert!(t.is_stale(), "expiring within skew window must be stale");

        t.expires_at = Some(Utc::now() + chrono::Duration::seconds(600));
        assert!(!t.is_stale());

        t.expires_at = None;
        assert!(!t.is_stale(), "no expiry => never stale");
    }

    #[test]
    fn merge_refresh_keeps_old_refresh_token_if_new_omits_it() {
        let prev = TokenSet {
            access_token: "a1".into(),
            refresh_token: Some("r1".into()),
            expires_at: None,
        };
        let new = TokenSet {
            access_token: "a2".into(),
            refresh_token: None,
            expires_at: None,
        };
        let merged = merge_refresh(prev, new);
        assert_eq!(merged.access_token, "a2");
        assert_eq!(merged.refresh_token.as_deref(), Some("r1"));
    }

    #[test]
    fn is_terminal_refresh_classifies_4xx_only() {
        assert!(is_terminal_refresh(400));
        assert!(is_terminal_refresh(401));
        assert!(is_terminal_refresh(403));
        assert!(!is_terminal_refresh(500));
        assert!(!is_terminal_refresh(503));
        assert!(!is_terminal_refresh(0));
    }

    // ---- Token endpoint mock ----

    /// Tiny single-shot HTTP/1.1 server. Returns once it
    /// has answered a single token POST. Avoids adding
    /// `wiremock` / `mockito` as workspace deps.
    async fn spawn_fake_token_server(
        responses: Vec<(u16, String)>,
    ) -> (String, tokio::task::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let url = format!("http://127.0.0.1:{port}/token");
        let handle = tokio::spawn(async move {
            let mut bodies = Vec::new();
            for (status, body) in responses {
                let (mut s, _) = listener.accept().await.unwrap();
                let mut buf = vec![0u8; 8192];
                let n = s.read(&mut buf).await.unwrap();
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let req_body = req.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
                bodies.push(req_body);
                let resp = format!(
                    "HTTP/1.1 {status} STATUS\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                s.write_all(resp.as_bytes()).await.unwrap();
                let _ = s.shutdown().await;
            }
            bodies
        });
        (url, handle)
    }

    fn cfg_with_token_url(token_url: &str) -> OAuthProviderConfig {
        OAuthProviderConfig {
            provider_name: "stub".into(),
            authorization_url: "http://unused/auth".into(),
            token_url: token_url.into(),
            client_id: "client-id".into(),
            scopes: vec!["read".into()],
            redirect_uri: "http://127.0.0.1/cb".into(),
        }
    }

    #[tokio::test]
    async fn refresh_flow_returns_new_access_token() {
        let (url, handle) = spawn_fake_token_server(vec![(
            200,
            r#"{"access_token":"new-access","refresh_token":"new-refresh","expires_in":3600}"#
                .into(),
        )])
        .await;
        let http = reqwest::Client::new();
        let cfg = cfg_with_token_url(&url);
        let new = refresh_tokens(&http, &cfg, "old-refresh").await.unwrap();
        assert_eq!(new.access_token, "new-access");
        assert_eq!(new.refresh_token.as_deref(), Some("new-refresh"));
        assert!(new.expires_at.is_some());

        let bodies = handle.await.unwrap();
        assert_eq!(bodies.len(), 1);
        assert!(bodies[0].contains("grant_type=refresh_token"));
        assert!(bodies[0].contains("refresh_token=old-refresh"));
        assert!(bodies[0].contains("client_id=client-id"));
    }

    #[tokio::test]
    async fn refresh_flow_4xx_is_terminal() {
        let (url, handle) =
            spawn_fake_token_server(vec![(400, r#"{"error":"invalid_grant"}"#.into())]).await;
        let http = reqwest::Client::new();
        let cfg = cfg_with_token_url(&url);
        let err = refresh_tokens(&http, &cfg, "rev").await.unwrap_err();
        match err {
            OAuthError::RefreshFailed { status, body } => {
                assert_eq!(status, 400);
                assert!(body.contains("invalid_grant"));
            }
            other => panic!("unexpected: {other:?}"),
        }
        let _ = handle.await;
    }

    #[tokio::test]
    async fn current_token_refreshes_when_stale_and_persists_new_set() {
        let (url, _handle) = spawn_fake_token_server(vec![(
            200,
            r#"{"access_token":"refreshed","expires_in":3600}"#.into(),
        )])
        .await;
        let store = Arc::new(InMemoryStore::default());
        let initial = TokenSet {
            access_token: "stale".into(),
            refresh_token: Some("rt".into()),
            expires_at: Some(Utc::now() - chrono::Duration::seconds(1)),
        };
        store.save("agent-code:stub", &initial).unwrap();

        let svc = OAuthService::with_store(
            cfg_with_token_url(&url),
            store.clone() as Arc<dyn CredentialStore>,
        );
        let token = svc.current_token().await.unwrap();
        assert_eq!(token, "refreshed");

        let saved = store.load("agent-code:stub").unwrap().unwrap();
        assert_eq!(saved.access_token, "refreshed");
        // Refresh token preserved when provider does not
        // rotate it.
        assert_eq!(saved.refresh_token.as_deref(), Some("rt"));
    }

    #[tokio::test]
    async fn current_token_clears_creds_and_demands_reauth_on_4xx() {
        let (url, _handle) =
            spawn_fake_token_server(vec![(401, r#"{"error":"invalid_grant"}"#.into())]).await;
        let store = Arc::new(InMemoryStore::default());
        let initial = TokenSet {
            access_token: "stale".into(),
            refresh_token: Some("rt".into()),
            expires_at: Some(Utc::now() - chrono::Duration::seconds(1)),
        };
        store.save("agent-code:stub", &initial).unwrap();

        let svc = OAuthService::with_store(
            cfg_with_token_url(&url),
            store.clone() as Arc<dyn CredentialStore>,
        );
        let err = svc.current_token().await.unwrap_err();
        assert!(matches!(err, OAuthError::ReauthRequired));
        assert!(store.load("agent-code:stub").unwrap().is_none());
    }

    #[tokio::test]
    async fn current_token_returns_reauth_required_when_no_creds_stored() {
        let store = Arc::new(InMemoryStore::default()) as Arc<dyn CredentialStore>;
        let svc = OAuthService::with_store(cfg_with_token_url("http://unused/token"), store);
        let err = svc.current_token().await.unwrap_err();
        assert!(matches!(err, OAuthError::ReauthRequired));
    }

    #[tokio::test]
    async fn logout_removes_stored_credentials() {
        let store = Arc::new(InMemoryStore::default());
        store
            .save(
                "agent-code:stub",
                &TokenSet {
                    access_token: "x".into(),
                    refresh_token: None,
                    expires_at: None,
                },
            )
            .unwrap();

        let svc = OAuthService::with_store(
            cfg_with_token_url("http://unused/token"),
            store.clone() as Arc<dyn CredentialStore>,
        );
        svc.logout().unwrap();
        assert!(store.load("agent-code:stub").unwrap().is_none());

        // Idempotent.
        svc.logout().unwrap();
    }

    // ---- Full login integration test (browser-launch
    // hook driven by the test) ----

    #[tokio::test]
    async fn login_drives_loopback_and_persists_token_set() {
        // Token endpoint that answers one POST.
        let (token_url, token_handle) = spawn_fake_token_server(vec![(
            200,
            r#"{"access_token":"login-access","refresh_token":"login-refresh","expires_in":3600}"#
                .into(),
        )])
        .await;

        // Authorization URL is unused here — the fake
        // browser launcher is what actually drives the
        // redirect. We only need an arbitrary string.
        let cfg = OAuthProviderConfig {
            provider_name: "stub".into(),
            authorization_url: "http://unused/auth".into(),
            token_url,
            client_id: "client-id".into(),
            scopes: vec!["read".into()],
            redirect_uri: "http://127.0.0.1/callback".into(),
        };
        let store = Arc::new(InMemoryStore::default());

        // The browser launcher captures the auth URL
        // and fires the redirect from a background task.
        let launches = Arc::new(AtomicUsize::new(0));
        let launches_clone = launches.clone();
        let launcher: BrowserLauncher = Arc::new(move |url: &str| {
            launches_clone.fetch_add(1, Ordering::SeqCst);
            let url = url.to_string();
            // Pull port + state out of the auth URL.
            let q = url.split_once('?').map(|(_, q)| q).unwrap_or("");
            let mut port = 0u16;
            let mut state = String::new();
            for pair in q.split('&') {
                if let Some(("redirect_uri", v)) = pair.split_once('=') {
                    let decoded = percent_decode(v);
                    if let Some(host_and_port) = decoded
                        .split("://")
                        .nth(1)
                        .and_then(|r| r.split('/').next())
                        && let Some(p) = host_and_port.split(':').nth(1)
                    {
                        port = p.parse().unwrap_or(0);
                    }
                }
                if let Some(("state", v)) = pair.split_once('=') {
                    state = percent_decode(v);
                }
            }
            assert!(port != 0, "loopback port not present in auth url");
            // Fire the callback in the background.
            tokio::spawn(async move {
                // Tiny delay so the loopback is accepting
                // before we connect.
                tokio::time::sleep(Duration::from_millis(20)).await;
                let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port))
                    .await
                    .unwrap();
                let req = format!(
                    "GET /callback?code=abc&state={state} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n",
                );
                s.write_all(req.as_bytes()).await.unwrap();
                let mut buf = Vec::new();
                let _ = s.read_to_end(&mut buf).await;
            });
            Ok(())
        });

        let svc = OAuthService::with_store(cfg, store.clone() as Arc<dyn CredentialStore>)
            .with_browser_launcher(launcher);
        let tokens = svc.login().await.unwrap();
        assert_eq!(tokens.access_token, "login-access");
        assert_eq!(tokens.refresh_token.as_deref(), Some("login-refresh"));
        assert_eq!(launches.load(Ordering::SeqCst), 1);

        let saved = store.load("agent-code:stub").unwrap().unwrap();
        assert_eq!(saved.access_token, "login-access");

        let bodies = token_handle.await.unwrap();
        assert_eq!(bodies.len(), 1);
        assert!(bodies[0].contains("grant_type=authorization_code"));
        assert!(bodies[0].contains("code=abc"));
        assert!(bodies[0].contains("code_verifier="));
    }

    // ---- File credential store ----

    #[test]
    fn file_credential_store_round_trips_and_is_idempotent_on_delete() {
        let tmp = tempfile::tempdir().unwrap();
        let store = FileCredentialStore::new(tmp.path().to_path_buf());
        assert!(store.load("agent-code:p").unwrap().is_none());

        let ts = TokenSet {
            access_token: "a".into(),
            refresh_token: Some("r".into()),
            expires_at: None,
        };
        store.save("agent-code:p", &ts).unwrap();
        let loaded = store.load("agent-code:p").unwrap().unwrap();
        assert_eq!(loaded.access_token, "a");
        assert_eq!(loaded.refresh_token.as_deref(), Some("r"));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let path = store.path_for("agent-code:p");
            let perms = std::fs::metadata(&path).unwrap().permissions();
            // Mode 0o600: owner read/write only.
            assert_eq!(perms.mode() & 0o777, 0o600);
        }

        store.delete("agent-code:p").unwrap();
        assert!(store.load("agent-code:p").unwrap().is_none());
        // Idempotent.
        store.delete("agent-code:p").unwrap();
    }

    #[test]
    fn file_credential_store_sanitizes_unsafe_chars_in_keys() {
        let tmp = tempfile::tempdir().unwrap();
        let store = FileCredentialStore::new(tmp.path().to_path_buf());
        let path = store.path_for("agent-code:weird/../name");
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        assert!(!name.contains('/'));
        assert!(!name.contains(".."));
    }

    /// In-memory store for tests (avoids touching HOME).
    #[derive(Default, Clone)]
    struct InMemoryStore {
        inner: Arc<std::sync::Mutex<HashMap<String, TokenSet>>>,
    }
    impl CredentialStore for InMemoryStore {
        fn load(&self, key: &str) -> Result<Option<TokenSet>, OAuthError> {
            Ok(self.inner.lock().unwrap().get(key).cloned())
        }
        fn save(&self, key: &str, tokens: &TokenSet) -> Result<(), OAuthError> {
            self.inner
                .lock()
                .unwrap()
                .insert(key.to_string(), tokens.clone());
            Ok(())
        }
        fn delete(&self, key: &str) -> Result<(), OAuthError> {
            self.inner.lock().unwrap().remove(key);
            Ok(())
        }
    }
}
