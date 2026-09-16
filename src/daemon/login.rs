//! Passphrase login for a native client.
//!
//! A daemon started with `--remote` mandates both a bearer token and a
//! passphrase, so reaching one from the TUI needs the same exchange the
//! dashboard performs: `POST /api/login` with the passphrase and a
//! client-generated device-binding secret, then present the returned session
//! plus that secret on every later request (`X-Aoe-Device-Binding` for REST,
//! an `aoe-device.<secret>` subprotocol for WebSockets).
//!
//! The passphrase is used once and never persisted; only the session and the
//! binding are stored, so a stolen registry cannot mint fresh sessions.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use reqwest::header::{AUTHORIZATION, SET_COOKIE};
use thiserror::Error;

use crate::daemon::SessionCredential;

/// Raw length of the device-binding secret, matching the server's
/// `BINDING_SECRET_BYTES`.
const BINDING_SECRET_BYTES: usize = 32;

const LOGIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

#[derive(Debug, Error)]
pub enum LoginError {
    #[error("invalid daemon base URL: {reason}")]
    InvalidBaseUrl { reason: &'static str },
    #[error("a passphrase requires HTTPS or a loopback HTTP URL")]
    InsecureTransport,
    #[error("could not generate a device binding secret")]
    Entropy,
    #[error("daemon transport error: {0}")]
    Transport(#[source] reqwest::Error),
    #[error("no passphrase login at this URL (HTTP 404)")]
    NotEnabled,
    #[error("incorrect passphrase")]
    Unauthorized,
    #[error("too many failed attempts; wait before retrying")]
    RateLimited,
    #[error("daemon returned HTTP {0}")]
    Status(reqwest::StatusCode),
    #[error("login succeeded but the daemon set no session cookie")]
    MissingSession,
}

/// Mint a fresh device-binding secret, base64url-encoded the way the server
/// decodes it.
pub fn new_binding_secret() -> Result<String, LoginError> {
    let mut bytes = [0u8; BINDING_SECRET_BYTES];
    getrandom::fill(&mut bytes).map_err(|_| LoginError::Entropy)?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

/// Exchange a passphrase for a device-bound session.
///
/// `token` is still required: `--remote` keeps the token gate in front of the
/// passphrase wall, and `/api/login` is login-exempt but not token-exempt.
pub async fn login(
    base_url: &str,
    token: Option<&str>,
    passphrase: &str,
    binding: &str,
) -> Result<SessionCredential, LoginError> {
    ensure_secure_transport(base_url)?;
    let url = format!("{}/api/login", base_url.trim_end_matches('/'));
    let http = reqwest::Client::builder()
        .timeout(LOGIN_TIMEOUT)
        .user_agent(concat!("aoe-remote/", env!("CARGO_PKG_VERSION")))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(LoginError::Transport)?;

    let mut request = http.post(&url).json(&serde_json::json!({
        "passphrase": passphrase,
        "device_binding_secret": binding,
    }));
    if let Some(token) = token {
        request = request.header(AUTHORIZATION, format!("Bearer {token}"));
    }
    let response = request.send().await.map_err(LoginError::Transport)?;

    let status = response.status();
    if !status.is_success() {
        return Err(match status {
            reqwest::StatusCode::NOT_FOUND => LoginError::NotEnabled,
            reqwest::StatusCode::UNAUTHORIZED => LoginError::Unauthorized,
            reqwest::StatusCode::TOO_MANY_REQUESTS => LoginError::RateLimited,
            other => LoginError::Status(other),
        });
    }

    let session = response
        .headers()
        .get_all(SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find_map(session_from_set_cookie)
        .ok_or(LoginError::MissingSession)?;

    Ok(SessionCredential {
        session,
        binding: binding.to_string(),
    })
}

/// A passphrase is a stronger secret than the token and travels in a body, so
/// it gets the same transport rule the bearer already has.
fn ensure_secure_transport(base_url: &str) -> Result<(), LoginError> {
    let url = crate::daemon::native_url(base_url).map_err(|error| match error {
        crate::daemon::DaemonClientError::InvalidBaseUrl { reason } => {
            LoginError::InvalidBaseUrl { reason }
        }
        _ => LoginError::InvalidBaseUrl {
            reason: "could not parse URL",
        },
    })?;
    if url.scheme() == "http" && !crate::daemon::is_loopback_url(&url) {
        return Err(LoginError::InsecureTransport);
    }
    Ok(())
}

/// Pull `aoe_session` out of one `Set-Cookie` value. The daemon sets
/// `HttpOnly`/`SameSite` attributes a native client ignores.
fn session_from_set_cookie(value: &str) -> Option<String> {
    for part in value.split(';') {
        let part = part.trim();
        if let Some(id) = part.strip_prefix("aoe_session=") {
            if !id.is_empty() {
                return Some(id.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binding_secret_decodes_to_the_expected_length() {
        let encoded = new_binding_secret().unwrap();
        let raw = URL_SAFE_NO_PAD.decode(&encoded).unwrap();
        assert_eq!(raw.len(), BINDING_SECRET_BYTES);
    }

    #[test]
    fn binding_secrets_are_not_repeated() {
        assert_ne!(new_binding_secret().unwrap(), new_binding_secret().unwrap());
    }

    #[test]
    fn reads_the_session_out_of_a_full_cookie() {
        let value = "aoe_session=abc123; HttpOnly; SameSite=Strict; Path=/; Max-Age=2592000";
        assert_eq!(session_from_set_cookie(value).as_deref(), Some("abc123"));
    }

    #[test]
    fn ignores_unrelated_and_empty_cookies() {
        assert_eq!(session_from_set_cookie("aoe_token=xyz; Path=/"), None);
        assert_eq!(session_from_set_cookie("aoe_session=; Path=/"), None);
    }

    #[test]
    fn a_passphrase_is_refused_over_non_loopback_plaintext() {
        let err = ensure_secure_transport("http://mini.example.com:8080").unwrap_err();
        assert!(matches!(err, LoginError::InsecureTransport));
    }

    #[test]
    fn loopback_http_and_remote_https_are_allowed() {
        assert!(ensure_secure_transport("http://127.0.0.1:8080").is_ok());
        assert!(ensure_secure_transport("https://mini.example.ts.net").is_ok());
    }
}
