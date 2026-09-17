//! Token-based authentication middleware for the web dashboard.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use axum::{
    extract::{ConnectInfo, Request, State},
    http::{header, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};

use super::AppState;

/// Constant-time string comparison to prevent timing attacks on token values.
pub(crate) fn constant_time_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.bytes()
        .zip(b.bytes())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

/// Resolve the real client IP, trusting X-Forwarded-For only from loopback
/// (i.e., only when the request came through the cloudflared proxy).
pub(crate) fn resolve_client_ip(
    socket_addr: SocketAddr,
    headers: &axum::http::HeaderMap,
) -> IpAddr {
    let socket_ip = socket_addr.ip();
    if socket_ip.is_loopback() {
        if let Some(cf_ip) = headers.get("cf-connecting-ip") {
            if let Ok(ip_str) = cf_ip.to_str() {
                if let Ok(ip) = ip_str.trim().parse::<IpAddr>() {
                    return ip;
                }
            }
        }
        if let Some(xff) = headers.get("x-forwarded-for") {
            if let Ok(xff_str) = xff.to_str() {
                if let Some(last) = xff_str.rsplit(',').next() {
                    if let Ok(ip) = last.trim().parse::<IpAddr>() {
                        return ip;
                    }
                }
            }
        }
    }
    socket_ip
}

/// Whether `client_ip` should be treated as a same-host caller that already passes the
/// filesystem-permission trust boundary.
fn is_local_trusted(client_ip: IpAddr) -> bool {
    client_ip.is_loopback()
}

/// Build a Set-Cookie header value with optional Secure flag for HTTPS tunnels.
fn build_cookie(token: &str, secure: bool, max_age_secs: u64) -> String {
    let mut cookie = format!(
        "aoe_token={}; HttpOnly; SameSite=Strict; Path=/; Max-Age={}",
        token, max_age_secs
    );
    if secure {
        cookie.push_str("; Secure");
    }
    cookie
}

/// Write the `aoe_token` Set-Cookie and the companion `x-aoe-token` header into a
/// response's header map.
fn write_token_headers(
    headers: &mut axum::http::HeaderMap,
    token: &str,
    behind_tunnel: bool,
    max_age_secs: u64,
) {
    let cookie = build_cookie(token, behind_tunnel, max_age_secs);
    headers.append(
        header::SET_COOKIE,
        cookie.parse().expect("cookie format must be valid"),
    );
    if let Ok(value) = token.parse() {
        headers.insert("x-aoe-token", value);
    }
}

/// Attach both the Set-Cookie and X-Aoe-Token headers to a response.
async fn attach_token_headers(response: &mut Response, state: &AppState) {
    let Some(current) = state.token_manager.current_token().await else {
        return;
    };
    let max_age = state.token_manager.lifetime_secs().await;
    write_token_headers(
        response.headers_mut(),
        &current,
        state.behind_tunnel,
        max_age,
    );
}

/// Extract all token candidates from the request (cookie, query parameter, and
/// Authorization header).
fn extract_tokens(request: &Request) -> Vec<(&str, TokenSource)> {
    let mut tokens = Vec::new();

    // Check cookie
    if let Some(cookie_header) = request.headers().get(header::COOKIE) {
        if let Ok(cookie_str) = cookie_header.to_str() {
            for cookie in cookie_str.split(';') {
                let cookie = cookie.trim();
                if let Some(value) = cookie.strip_prefix("aoe_token=") {
                    tokens.push((value, TokenSource::Cookie));
                }
            }
        }
    }

    // Check query parameter
    if let Some(query) = request.uri().query() {
        for param in query.split('&') {
            if let Some(value) = param.strip_prefix("token=") {
                tokens.push((value, TokenSource::QueryParam));
            }
        }
    }

    // Check Authorization: Bearer header
    if let Some(auth_header) = request.headers().get(header::AUTHORIZATION) {
        if let Ok(auth_str) = auth_header.to_str() {
            if let Some(value) = auth_str.strip_prefix("Bearer ") {
                tokens.push((value.trim(), TokenSource::Bearer));
            }
        }
    }

    tokens
}

/// Extract all WebSocket sub-protocol values from the request.
fn extract_ws_protocols(request: &Request) -> Vec<String> {
    let mut protocols = Vec::new();
    if let Some(header) = request.headers().get("sec-websocket-protocol") {
        if let Ok(proto_str) = header.to_str() {
            for proto in proto_str.split(',') {
                let trimmed = proto.trim();
                if !trimmed.is_empty() {
                    protocols.push(trimmed.to_string());
                }
            }
        }
    }
    protocols
}

/// Strip a possible trailing slash from a path so suffix matches are not bypassed by
/// `/api/sessions/123/acp/prompt/` (axum routes both forms to the same handler).
fn normalize_path(path: &str) -> &str {
    path.strip_suffix('/').unwrap_or(path)
}

/// Whether a request path is exempt from the passphrase session + device-binding check.
fn is_login_session_exempt(path: &str) -> bool {
    path == "/"
        || path == "/login"
        || path.starts_with("/session/")
        || path == "/api/login"
        || path == "/api/login/status"
        || path == "/api/logout"
        || path == "/theme-bootstrap.js"
        || path.starts_with("/assets/")
        || path == "/manifest.json"
        || path == "/sw.js"
        || path.starts_with("/icon-")
        || path.starts_with("/fonts/")
}

/// Whether to append a sliding-window refresh of the `aoe_session` cookie on the response
/// for a session-authenticated request.
fn should_refresh_session_cookie(path: &str) -> bool {
    !is_login_session_exempt(path)
}

/// Decision for the post-token branch of `auth_middleware`.
#[derive(Debug, PartialEq, Eq)]
enum PostTokenAuthAction {
    /// Pass the request through to the next layer.
    Bypass,
    /// Return a `login_required` response so the SPA pops the
    /// passphrase prompt (or redirects to `/login` for HTML).
    RequireLogin,
}

/// Resolve the post-token branch decision.
fn post_token_auth_action(
    login_enabled: bool,
    login_exempt: bool,
    client_ip: IpAddr,
) -> PostTokenAuthAction {
    if !login_enabled || login_exempt {
        return PostTokenAuthAction::Bypass;
    }
    if is_local_trusted(client_ip) {
        PostTokenAuthAction::Bypass
    } else {
        PostTokenAuthAction::RequireLogin
    }
}

/// Decision for the entry of `run_passphrase_wall`.
#[derive(Debug, PartialEq, Eq)]
enum PassphraseWallEntryAction {
    /// Path is in the login-bootstrap allow-list.
    BypassExempt,
    /// Caller is on loopback and no external ingress fronts the daemon.
    BypassLoopback,
    /// Run the full session + device-binding + elevation flow.
    Continue,
}

/// Resolve the entry decision for `run_passphrase_wall`.
fn passphrase_wall_entry_action(
    path: &str,
    client_ip: IpAddr,
    behind_ingress: bool,
) -> PassphraseWallEntryAction {
    if is_login_session_exempt(path) {
        return PassphraseWallEntryAction::BypassExempt;
    }
    if !behind_ingress && is_local_trusted(client_ip) {
        return PassphraseWallEntryAction::BypassLoopback;
    }
    PassphraseWallEntryAction::Continue
}

/// Emit the passphrase-only loopback-bypass event.
fn log_loopback_bypass_passphrase(client_ip: IpAddr, path: &str) {
    tracing::debug!(
        target: "auth.passphrase",
        ip = %client_ip,
        path = %path,
        "loopback bypass: skipping passphrase factor in passphrase-only mode"
    );
}

/// Emit the token-mode loopback-bypass event.
fn log_loopback_bypass_token(client_ip: IpAddr, path: &str) {
    tracing::debug!(
        target: "auth",
        ip = %client_ip,
        path = %path,
        "loopback bypass: valid token + loopback peer; skipping passphrase factor"
    );
}

/// Whether a request path + method needs an elevated login session (step-up auth, 15-minute
/// passphrase confirmation window).
fn requires_elevation(method: &axum::http::Method, path: &str) -> bool {
    use axum::http::Method;

    let path = normalize_path(path);

    if method == Method::GET || method == Method::HEAD {
        return false;
    }

    // Settings + profile mutations.
    if path == "/api/settings" && method == Method::PATCH {
        return true;
    }
    if path == "/api/default-profile" && method == Method::PATCH {
        return true;
    }
    if path == "/api/profiles" && method == Method::POST {
        return true;
    }
    if let Some(rest) = path.strip_prefix("/api/profiles/") {
        // Per-profile writes.
        if method == Method::PATCH && (rest.ends_with("/settings") || rest.ends_with("/settings/"))
        {
            return false;
        }
        return true;
    }

    // Device / login-session management.
    if path == "/api/login/logout-all" && method == Method::POST {
        return true;
    }
    if let Some(rest) = path.strip_prefix("/api/login/sessions/") {
        if method == Method::DELETE && !rest.is_empty() {
            return true;
        }
    }

    false
}

/// Strip the leading `<prefix>.` from a subprotocol value when present, returning the
/// suffix.
fn strip_ws_prefix<'a>(proto: &'a str, prefix: &str) -> Option<&'a str> {
    let with_dot = proto.strip_prefix(prefix)?;
    with_dot.strip_prefix('.')
}

/// Extract the device binding secret presented by the client.
pub(crate) fn extract_device_binding(request: &Request) -> Option<Vec<u8>> {
    if let Some(value) = request.headers().get("x-aoe-device-binding") {
        if let Ok(s) = value.to_str() {
            if let Some(bytes) = super::login::decode_binding_secret(s) {
                return Some(bytes);
            }
        }
    }
    for proto in extract_ws_protocols(request) {
        if let Some(secret) = strip_ws_prefix(&proto, "aoe-device") {
            if let Some(bytes) = super::login::decode_binding_secret(secret) {
                return Some(bytes);
            }
        }
    }
    None
}

#[derive(Debug, PartialEq)]
enum TokenSource {
    Cookie,
    QueryParam,
    WebSocketProtocol,
    Bearer,
}

/// Request extension carrying the SHA-256 hash of the bearer token that authenticated this
/// request.
#[derive(Clone, Copy, Debug)]
pub struct AuthenticatedTokenHash(pub [u8; 32]);

/// Request extension carrying the login session id used to authenticate the current
/// request.
#[derive(Clone, Debug)]
pub struct AuthenticatedSession(pub String);

/// Request extension marking a request whose resolved client IP is loopback (see
/// `is_local_trusted`).
#[derive(Clone, Copy, Debug)]
pub struct LoopbackTrusted;

/// Pure elevation decision, extracted so the matrix is unit-testable without standing up
/// `AppState`.
fn elevation_verdict(
    login_enabled: bool,
    loopback_trusted: bool,
    session_elevated: Option<bool>,
) -> bool {
    if !login_enabled || loopback_trusted {
        return true;
    }
    session_elevated.unwrap_or(false)
}

/// Shared resolver for handler-side (body-shape) elevation gates.
pub(crate) async fn handler_elevated(
    state: &AppState,
    session: Option<&AuthenticatedSession>,
    loopback_trusted: bool,
) -> bool {
    let session_elevated = match session {
        Some(AuthenticatedSession(id)) => Some(state.login_manager.is_elevated(id).await),
        None => None,
    };
    elevation_verdict(
        state.login_manager.is_enabled(),
        loopback_trusted,
        session_elevated,
    )
}

/// Passphrase login wall used when the token gate is disabled (`--auth=passphrase`).
async fn run_passphrase_wall(
    state: &AppState,
    request: Request,
    client_ip: IpAddr,
    next: Next,
) -> Response {
    let path = request.uri().path().to_string();
    let method = request.method().clone();

    match passphrase_wall_entry_action(&path, client_ip, state.behind_tunnel) {
        PassphraseWallEntryAction::BypassExempt => return next.run(request).await,
        PassphraseWallEntryAction::BypassLoopback => {
            log_loopback_bypass_passphrase(client_ip, &path);
            return next.run(request).await;
        }
        PassphraseWallEntryAction::Continue => {}
    }

    let session_id = super::login::extract_login_session(&request);
    let presented_binding = extract_device_binding(&request);

    let has_valid_session = match (&session_id, &presented_binding) {
        (Some(id), Some(binding)) => state.login_manager.validate_session(id, binding).await,
        _ => false,
    };

    if !has_valid_session {
        if path.starts_with("/api/") || path.contains("/ws") {
            tracing::warn!(
                target: "auth",
                ip = %client_ip,
                path = %path,
                had_session_cookie = session_id.is_some(),
                had_device_binding = presented_binding.is_some(),
                "passphrase wall: rejecting api/ws with 401"
            );
            return (
                StatusCode::UNAUTHORIZED,
                axum::Json(serde_json::json!({
                    "error": "login_required",
                    "message": "Passphrase login required"
                })),
            )
                .into_response();
        }
        return axum::response::Redirect::temporary("/login").into_response();
    }

    let session_id = session_id.expect("valid session implies session_id exists");

    if requires_elevation(&method, &path) && !state.login_manager.is_elevated(&session_id).await {
        tracing::info!(
            target: "auth.passphrase",
            ip = %client_ip,
            path = %path,
            "passphrase wall: sensitive route required elevation; returning 403"
        );
        return (
            StatusCode::FORBIDDEN,
            axum::Json(serde_json::json!({
                "error": "elevation_required",
                "message": "Re-enter the passphrase to continue"
            })),
        )
            .into_response();
    }

    let mut request = request;
    request
        .extensions_mut()
        .insert(AuthenticatedSession(session_id.clone()));

    let mut response = next.run(request).await;

    // Refresh login session cookie (sliding window).
    let login_cookie = super::login::build_login_cookie(&session_id, state.behind_tunnel);
    response.headers_mut().append(
        header::SET_COOKIE,
        login_cookie.parse().expect("cookie format must be valid"),
    );

    response
}

pub async fn auth_middleware(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    mut request: Request,
    next: Next,
) -> Response {
    let client_ip = resolve_client_ip(addr, request.headers());

    // Mark same-host callers before any auth branch so handler-side elevation gates see the
    // #1168 carve-out no matter which path (token, session, passphrase wall, loopback
    // bypass) handled the request.
    let wall_covers_loopback = state.behind_tunnel
        && state.token_manager.is_no_auth().await
        && state.login_manager.is_enabled();
    if !wall_covers_loopback && is_local_trusted(client_ip) {
        request.extensions_mut().insert(LoopbackTrusted);
    }

    // Trace structured view ws specifically so we can see whether the browser ever reached
    // the server when the structured view live updates get stuck.
    if request.uri().path().contains("/acp/ws") {
        let token_sources: Vec<&'static str> = extract_tokens(&request)
            .iter()
            .map(|(_, src)| match src {
                TokenSource::Cookie => "cookie",
                TokenSource::QueryParam => "query",
                TokenSource::Bearer => "bearer",
                TokenSource::WebSocketProtocol => "ws-proto",
            })
            .collect();
        let ws_protocols = extract_ws_protocols(&request);
        tracing::debug!(
            target: "auth",
            ip = %client_ip,
            token_sources = ?token_sources,
            ws_protocol_count = ws_protocols.len(),
            "auth_middleware entered for structured view ws"
        );
    }

    // Token gate disabled (--auth=none or --auth=passphrase).
    if state.token_manager.is_no_auth().await {
        static NO_AUTH_LOGGED: std::sync::Once = std::sync::Once::new();
        if state.login_manager.is_enabled() {
            NO_AUTH_LOGGED.call_once(|| {
                tracing::info!(
                    target: "auth.token",
                    "token gate disabled (--auth=passphrase); passphrase login wall remains active"
                );
            });
            request
                .extensions_mut()
                .insert(AuthenticatedTokenHash([0u8; 32]));
            return run_passphrase_wall(&state, request, client_ip, next).await;
        }
        NO_AUTH_LOGGED.call_once(|| {
            tracing::info!(
                target: "auth.token",
                "running in no-auth mode; requests pass through without token check"
            );
        });
        request
            .extensions_mut()
            .insert(AuthenticatedTokenHash([0u8; 32]));
        return next.run(request).await;
    }

    // Rate limit check BEFORE token validation
    if let Some(remaining_secs) = state.rate_limiter.check_locked(client_ip).await {
        tracing::warn!(
            target: "auth.rate_limit",
            ip = %client_ip,
            remaining_secs,
            "rejecting request from locked-out IP"
        );
        return (
            StatusCode::TOO_MANY_REQUESTS,
            [("Retry-After", remaining_secs.to_string())],
            axum::Json(serde_json::json!({
                "error": "rate_limited",
                "message": format!(
                    "Too many failed attempts. Try again in {} seconds.",
                    remaining_secs
                )
            })),
        )
            .into_response();
    }

    // Steady-state path.
    let login_enabled = state.login_manager.is_enabled();
    let presented_session_id = if login_enabled {
        super::login::extract_login_session(&request)
    } else {
        None
    };
    let presented_binding = if login_enabled {
        extract_device_binding(&request)
    } else {
        None
    };
    let session_valid = if login_enabled {
        match (&presented_session_id, &presented_binding) {
            (Some(id), Some(b)) => state.login_manager.validate_session(id, b).await,
            _ => false,
        }
    } else {
        false
    };

    if session_valid {
        let session_id = presented_session_id.expect("session_valid implies session_id exists");
        return handle_session_authenticated(&state, client_ip, request, next, session_id).await;
    }

    // Token-check path.
    let mut matched_source = None;
    let mut needs_upgrade = false;
    let mut matched_token_hash: Option<[u8; 32]> = None;

    for (token_value, source) in extract_tokens(&request) {
        let (valid, upgrade) = state.token_manager.validate(token_value).await;
        if valid {
            matched_source = Some(source);
            needs_upgrade = upgrade;
            matched_token_hash = Some(super::push::sha256_token(token_value));
            break;
        }
    }

    // WebSocket sub-protocol fallback.
    if matched_source.is_none() {
        for proto in extract_ws_protocols(&request) {
            let candidate = strip_ws_prefix(&proto, "aoe-token").unwrap_or(&proto);
            let (valid, upgrade) = state.token_manager.validate(candidate).await;
            if valid {
                matched_source = Some(TokenSource::WebSocketProtocol);
                needs_upgrade = upgrade;
                matched_token_hash = Some(super::push::sha256_token(candidate));
                break;
            }
        }
    }

    let Some(source) = matched_source else {
        // No valid token and no valid session.
        let path = request.uri().path();
        let is_api_or_ws = path.starts_with("/api/") || path.contains("/ws");
        if !is_api_or_ws {
            return next.run(request).await;
        }
        let locked = state.rate_limiter.record_failure(client_ip).await;
        let reason =
            if extract_tokens(&request).is_empty() && extract_ws_protocols(&request).is_empty() {
                "missing"
            } else {
                "invalid"
            };
        tracing::warn!(
            target: "auth.middleware",
            ip = %client_ip,
            path = %path,
            locked = locked,
            reason = %reason,
            "auth rejected"
        );
        return (
            StatusCode::UNAUTHORIZED,
            axum::Json(serde_json::json!({
                "error": "unauthorized",
                "message": "Invalid or missing auth token"
            })),
        )
            .into_response();
    };

    // Token valid: record success, stamp owner, record device.
    state.rate_limiter.record_success(client_ip).await;
    tracing::trace!(
        target: "auth.middleware",
        ip = %client_ip,
        path = %request.uri().path(),
        source = ?source,
        "auth accepted via token (bootstrap)"
    );
    if let Some(hash) = matched_token_hash {
        request
            .extensions_mut()
            .insert(AuthenticatedTokenHash(hash));
    }
    let path = request.uri().path().to_string();
    let should_attach_token =
        matches!(source, TokenSource::QueryParam | TokenSource::Bearer) || needs_upgrade;

    // When login is enabled, a valid token alone is not enough for non-bootstrap paths.
    let login_exempt = is_login_session_exempt(&path);
    match post_token_auth_action(login_enabled, login_exempt, client_ip) {
        PostTokenAuthAction::Bypass => {
            if login_enabled && !login_exempt && is_local_trusted(client_ip) {
                log_loopback_bypass_token(client_ip, &path);
            }
        }
        PostTokenAuthAction::RequireLogin => {
            tracing::warn!(
                target: "auth",
                ip = %client_ip,
                path = %path,
                had_session_cookie = presented_session_id.is_some(),
                had_device_binding = presented_binding.is_some(),
                "valid token but no session on non-login-exempt path; returning login_required"
            );
            if path.starts_with("/api/") || path.contains("/ws") {
                return (
                    StatusCode::UNAUTHORIZED,
                    axum::Json(serde_json::json!({
                        "error": "login_required",
                        "message": "Passphrase login required"
                    })),
                )
                    .into_response();
            } else {
                let mut response = axum::response::Redirect::temporary("/login").into_response();
                if should_attach_token {
                    attach_token_headers(&mut response, &state).await;
                }
                return response;
            }
        }
    }

    // Bootstrap path (login enabled + /login, /api/login, etc.), token-only mode, or
    // loopback bypass per the match above.
    let mut response = next.run(request).await;
    if should_attach_token {
        attach_token_headers(&mut response, &state).await;
    }
    response
}

/// Steady-state handler for a bound device.
async fn handle_session_authenticated(
    state: &Arc<AppState>,
    client_ip: IpAddr,
    mut request: Request,
    next: Next,
    session_id: String,
) -> Response {
    state.rate_limiter.record_success(client_ip).await;
    tracing::trace!(
        target: "auth.middleware",
        ip = %client_ip,
        path = %request.uri().path(),
        "auth accepted via session+binding"
    );

    let owner_hash = match state.token_manager.current_token().await {
        Some(t) => super::push::sha256_token(&t),
        None => [0u8; 32],
    };
    request
        .extensions_mut()
        .insert(AuthenticatedTokenHash(owner_hash));

    let path = request.uri().path().to_string();
    let method = request.method().clone();

    // Loopback callers skip the step-up gate even with a session.
    if requires_elevation(&method, &path)
        && request.extensions().get::<LoopbackTrusted>().is_none()
        && !state.login_manager.is_elevated(&session_id).await
    {
        tracing::info!(
            target: "auth.passphrase",
            ip = %client_ip,
            path = %path,
            "sensitive route required elevation; returning 403 elevation_required"
        );
        return (
            StatusCode::FORBIDDEN,
            axum::Json(serde_json::json!({
                "error": "elevation_required",
                "message": "Re-enter the passphrase to continue"
            })),
        )
            .into_response();
    }

    request
        .extensions_mut()
        .insert(AuthenticatedSession(session_id.clone()));

    let mut response = next.run(request).await;

    if should_refresh_session_cookie(&path) {
        let login_cookie = super::login::build_login_cookie(&session_id, state.behind_tunnel);
        response.headers_mut().append(
            header::SET_COOKIE,
            login_cookie.parse().expect("cookie format must be valid"),
        );
    }

    response
}

#[cfg(test)]
mod tests {
    use super::*;

    // Records (level, target) for every event so the loopback-bypass
    // helpers' log level is assertable without standing up the full
    // axum middleware (AppState is too heavy to build in a unit test).
    #[derive(Clone, Default)]
    struct LevelCapture(std::sync::Arc<std::sync::Mutex<Vec<(tracing::Level, String)>>>);

    impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for LevelCapture {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _ctx: tracing_subscriber::layer::Context<'_, S>,
        ) {
            let meta = event.metadata();
            self.0
                .lock()
                .unwrap()
                .push((*meta.level(), meta.target().to_string()));
        }
    }

    fn capture_events(f: impl FnOnce()) -> Vec<(tracing::Level, String)> {
        use tracing_subscriber::layer::SubscriberExt;
        let capture = LevelCapture::default();
        let events = capture.0.clone();
        let subscriber = tracing_subscriber::registry::Registry::default().with(capture);
        tracing::subscriber::with_default(subscriber, f);
        let recorded = events.lock().unwrap();
        recorded.clone()
    }

    // #1647.
    #[test]
    fn loopback_bypass_passphrase_logs_at_debug() {
        let loopback: IpAddr = "127.0.0.1".parse().unwrap();
        let events = capture_events(|| log_loopback_bypass_passphrase(loopback, "/api/sessions"));
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].0, tracing::Level::DEBUG);
        assert_eq!(events[0].1, "auth.passphrase");
    }

    #[test]
    fn loopback_bypass_token_logs_at_debug() {
        let loopback: IpAddr = "127.0.0.1".parse().unwrap();
        let events = capture_events(|| log_loopback_bypass_token(loopback, "/api/sessions"));
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].0, tracing::Level::DEBUG);
        assert_eq!(events[0].1, "auth");
    }

    #[test]
    fn constant_time_eq_matching() {
        assert!(constant_time_eq("abc123", "abc123"));
        assert!(constant_time_eq("", ""));
    }

    #[test]
    fn constant_time_eq_different_content() {
        assert!(!constant_time_eq("abc123", "abc124"));
        assert!(!constant_time_eq("abc123", "xyz789"));
    }

    #[test]
    fn constant_time_eq_different_length() {
        assert!(!constant_time_eq("short", "longer_string"));
        assert!(!constant_time_eq("abc", "ab"));
    }

    #[test]
    fn constant_time_eq_empty_vs_nonempty() {
        assert!(!constant_time_eq("", "x"));
        assert!(!constant_time_eq("x", ""));
    }

    #[test]
    fn resolve_ip_prefers_cf_connecting_ip() {
        let socket: SocketAddr = "127.0.0.1:12345".parse().unwrap();
        let mut headers = axum::http::HeaderMap::new();
        headers.insert("cf-connecting-ip", "203.0.113.50".parse().unwrap());
        headers.insert("x-forwarded-for", "10.0.0.1".parse().unwrap());
        let ip = resolve_client_ip(socket, &headers);
        assert_eq!(ip, "203.0.113.50".parse::<IpAddr>().unwrap());
    }

    #[test]
    fn resolve_ip_falls_back_to_xff_last() {
        let socket: SocketAddr = "127.0.0.1:12345".parse().unwrap();
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            "x-forwarded-for",
            "spoofed.by.client, 203.0.113.50".parse().unwrap(),
        );
        let ip = resolve_client_ip(socket, &headers);
        assert_eq!(ip, "203.0.113.50".parse::<IpAddr>().unwrap());
    }

    #[test]
    fn resolve_ip_loopback_without_xff() {
        let socket: SocketAddr = "127.0.0.1:12345".parse().unwrap();
        let headers = axum::http::HeaderMap::new();
        let ip = resolve_client_ip(socket, &headers);
        assert!(ip.is_loopback());
    }

    #[test]
    fn resolve_ip_remote_ignores_xff() {
        let socket: SocketAddr = "192.168.1.100:12345".parse().unwrap();
        let mut headers = axum::http::HeaderMap::new();
        headers.insert("x-forwarded-for", "10.0.0.1".parse().unwrap());
        let ip = resolve_client_ip(socket, &headers);
        assert_eq!(ip, "192.168.1.100".parse::<IpAddr>().unwrap());
    }

    #[test]
    fn resolve_ip_malformed_xff() {
        let socket: SocketAddr = "127.0.0.1:12345".parse().unwrap();
        let mut headers = axum::http::HeaderMap::new();
        headers.insert("x-forwarded-for", "not-an-ip".parse().unwrap());
        let ip = resolve_client_ip(socket, &headers);
        assert!(ip.is_loopback());
    }

    // Full matrix of the handler-side elevation decision.
    #[test]
    fn elevation_verdict_matrix() {
        // Login disabled: elevation does not exist as a concept.
        assert!(elevation_verdict(false, false, None));
        assert!(elevation_verdict(false, true, None));
        // Loopback trusted.
        assert!(elevation_verdict(true, true, None));
        assert!(elevation_verdict(true, true, Some(false)));
        assert!(elevation_verdict(true, true, Some(true)));
        // Remote with login enabled: only an elevated session passes.
        assert!(!elevation_verdict(true, false, None));
        assert!(!elevation_verdict(true, false, Some(false)));
        assert!(elevation_verdict(true, false, Some(true)));
    }

    #[test]
    fn is_local_trusted_recognizes_loopback() {
        assert!(is_local_trusted("127.0.0.1".parse().unwrap()));
        assert!(is_local_trusted("::1".parse().unwrap()));
        assert!(!is_local_trusted("192.168.1.50".parse().unwrap()));
        assert!(!is_local_trusted("100.64.0.5".parse().unwrap()));
        assert!(!is_local_trusted("203.0.113.10".parse().unwrap()));
    }

    // Per-row coverage of the post-token branch policy from #1168.
    #[test]
    fn post_token_auth_action_matrix() {
        let loopback: IpAddr = "127.0.0.1".parse().unwrap();
        let loopback_v6: IpAddr = "::1".parse().unwrap();
        let remote: IpAddr = "100.64.0.5".parse().unwrap();

        // login disabled.
        assert_eq!(
            post_token_auth_action(false, false, loopback),
            PostTokenAuthAction::Bypass
        );
        assert_eq!(
            post_token_auth_action(false, false, remote),
            PostTokenAuthAction::Bypass
        );

        // login enabled + login-bootstrap path.
        assert_eq!(
            post_token_auth_action(true, true, loopback),
            PostTokenAuthAction::Bypass
        );
        assert_eq!(
            post_token_auth_action(true, true, remote),
            PostTokenAuthAction::Bypass
        );

        // login enabled + non-bootstrap path + loopback.
        assert_eq!(
            post_token_auth_action(true, false, loopback),
            PostTokenAuthAction::Bypass
        );
        assert_eq!(
            post_token_auth_action(true, false, loopback_v6),
            PostTokenAuthAction::Bypass
        );

        // login enabled + non-bootstrap path + remote: still requires
        // the passphrase. This is the threat passphrase auth was
        // built to mitigate (leaked token from a remote attacker).
        // Note: when a real reverse proxy forwards a remote request
        // to a loopback socket, `resolve_client_ip` already returns
        // the proxy-supplied remote IP, so the input here would be
        // non-loopback and we land on RequireLogin correctly.
        assert_eq!(
            post_token_auth_action(true, false, remote),
            PostTokenAuthAction::RequireLogin
        );
    }

    // Per-row coverage of the passphrase-wall entry policy added in #1525.
    #[test]
    fn passphrase_wall_entry_action_matrix() {
        let loopback: IpAddr = "127.0.0.1".parse().unwrap();
        let loopback_v6: IpAddr = "::1".parse().unwrap();
        let remote: IpAddr = "100.64.0.5".parse().unwrap();

        // Non-exempt path + loopback (IPv4 and IPv6).
        assert_eq!(
            passphrase_wall_entry_action("/api/sessions", loopback, false),
            PassphraseWallEntryAction::BypassLoopback
        );
        assert_eq!(
            passphrase_wall_entry_action("/sessions/abc/acp/ws", loopback, false),
            PassphraseWallEntryAction::BypassLoopback
        );
        assert_eq!(
            passphrase_wall_entry_action("/api/settings", loopback_v6, false),
            PassphraseWallEntryAction::BypassLoopback
        );

        // Non-exempt path + remote.
        assert_eq!(
            passphrase_wall_entry_action("/api/sessions", remote, false),
            PassphraseWallEntryAction::Continue
        );
        assert_eq!(
            passphrase_wall_entry_action("/sessions/abc/acp/ws", remote, false),
            PassphraseWallEntryAction::Continue
        );

        // Login-bootstrap allow-list wins regardless of IP, so the
        // SPA can pull assets and POST to `/api/login` from any peer.
        assert_eq!(
            passphrase_wall_entry_action("/login", remote, false),
            PassphraseWallEntryAction::BypassExempt
        );
        assert_eq!(
            passphrase_wall_entry_action("/api/login", remote, false),
            PassphraseWallEntryAction::BypassExempt
        );
        assert_eq!(
            passphrase_wall_entry_action("/assets/index.css", loopback, false),
            PassphraseWallEntryAction::BypassExempt
        );

        // #3843.
        assert_eq!(
            passphrase_wall_entry_action("/api/sessions", loopback, true),
            PassphraseWallEntryAction::Continue
        );
        assert_eq!(
            passphrase_wall_entry_action("/sessions/abc/acp/ws", loopback_v6, true),
            PassphraseWallEntryAction::Continue
        );
        // The login surfaces stay exempt, otherwise nobody could reach
        // the wall to get past it.
        assert_eq!(
            passphrase_wall_entry_action("/api/login", loopback, true),
            PassphraseWallEntryAction::BypassExempt
        );
    }

    #[test]
    fn build_cookie_without_secure() {
        let cookie = build_cookie("mytoken", false, 14400);
        assert!(cookie.contains("aoe_token=mytoken"));
        assert!(cookie.contains("HttpOnly"));
        assert!(cookie.contains("SameSite=Strict"));
        assert!(cookie.contains("Max-Age=14400"));
        assert!(!cookie.contains("Secure"));
    }

    fn build_request_with_headers(headers: Vec<(&'static str, &'static str)>) -> Request {
        let mut builder = Request::builder().uri("/api/sessions");
        for (name, value) in headers {
            builder = builder.header(name, value);
        }
        builder.body(axum::body::Body::empty()).unwrap()
    }

    #[test]
    fn extract_tokens_reads_bearer_header() {
        let req = build_request_with_headers(vec![("authorization", "Bearer abc123")]);
        let tokens = extract_tokens(&req);
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].0, "abc123");
        assert_eq!(tokens[0].1, TokenSource::Bearer);
    }

    #[test]
    fn extract_tokens_cookie_wins_over_bearer() {
        let req = build_request_with_headers(vec![
            ("cookie", "aoe_token=cookie_tok"),
            ("authorization", "Bearer bearer_tok"),
        ]);
        let tokens = extract_tokens(&req);
        // Priority order.
        assert_eq!(tokens[0].0, "cookie_tok");
        assert_eq!(tokens[0].1, TokenSource::Cookie);
        assert_eq!(tokens[1].0, "bearer_tok");
        assert_eq!(tokens[1].1, TokenSource::Bearer);
    }

    #[test]
    fn extract_tokens_ignores_non_bearer_authorization() {
        let req = build_request_with_headers(vec![("authorization", "Basic dXNlcjpwYXNz")]);
        let tokens = extract_tokens(&req);
        assert!(tokens.is_empty());
    }

    #[test]
    fn extract_tokens_trims_bearer_value() {
        let req = build_request_with_headers(vec![("authorization", "Bearer   padded  ")]);
        let tokens = extract_tokens(&req);
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].0, "padded");
    }

    #[test]
    fn build_cookie_with_secure() {
        let cookie = build_cookie("mytoken", true, 14400);
        assert!(cookie.contains("Secure"));
        assert!(cookie.contains("Max-Age=14400"));
    }

    #[test]
    fn strip_ws_prefix_works() {
        assert_eq!(strip_ws_prefix("aoe-token.abc", "aoe-token"), Some("abc"));
        assert_eq!(strip_ws_prefix("aoe-device.xyz", "aoe-device"), Some("xyz"));
        // No leading dot -> not a prefixed value, just a coincidentally matching string.
        assert_eq!(strip_ws_prefix("aoe-tokenabc", "aoe-token"), None);
        // Unrelated subprotocol.
        assert_eq!(strip_ws_prefix("graphql-ws", "aoe-token"), None);
    }

    #[test]
    fn extract_device_binding_from_header() {
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        use base64::Engine;
        let raw = [0xAB; 32];
        let encoded = URL_SAFE_NO_PAD.encode(raw);
        let req = build_request_with_headers(vec![(
            "x-aoe-device-binding",
            Box::leak(encoded.into_boxed_str()),
        )]);
        let bytes = extract_device_binding(&req).expect("decodes");
        assert_eq!(bytes, raw);
    }

    #[test]
    fn extract_device_binding_from_ws_subprotocol() {
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        use base64::Engine;
        let raw = [0xCD; 32];
        let encoded = URL_SAFE_NO_PAD.encode(raw);
        let proto = format!("aoe-token.tok123, aoe-device.{}", encoded);
        let req = build_request_with_headers(vec![(
            "sec-websocket-protocol",
            Box::leak(proto.into_boxed_str()),
        )]);
        let bytes = extract_device_binding(&req).expect("decodes");
        assert_eq!(bytes, raw);
    }

    #[test]
    fn extract_device_binding_missing_returns_none() {
        let req = build_request_with_headers(vec![]);
        assert!(extract_device_binding(&req).is_none());
    }

    #[test]
    fn extract_device_binding_rejects_malformed() {
        let req = build_request_with_headers(vec![(
            "x-aoe-device-binding",
            "not-base64-and-wrong-length",
        )]);
        assert!(extract_device_binding(&req).is_none());
    }

    // Regression test for the token-cookie clobber bug.
    #[test]
    fn write_token_headers_preserves_prior_set_cookie() {
        use axum::http::{HeaderMap, HeaderValue};

        let mut headers = HeaderMap::new();
        // Simulate a handler that already set its own Set-Cookie
        // (e.g., `login_handler` setting `aoe_session=...`).
        headers.insert(
            header::SET_COOKIE,
            HeaderValue::from_static(
                "aoe_session=abc; HttpOnly; SameSite=Strict; Path=/; Max-Age=2592000",
            ),
        );

        // Middleware writes the `aoe_token` cookie + companion header on top.
        write_token_headers(&mut headers, "tok123", false, 14400);

        let cookies: Vec<String> = headers
            .get_all(header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .map(str::to_string)
            .collect();
        assert_eq!(
            cookies.len(),
            2,
            "both Set-Cookie values must survive, got {cookies:?}"
        );
        assert!(
            cookies.iter().any(|c| c.contains("aoe_session=abc")),
            "handler's aoe_session cookie was clobbered: {cookies:?}"
        );
        assert!(
            cookies.iter().any(|c| c.contains("aoe_token=tok123")),
            "middleware's aoe_token cookie missing: {cookies:?}"
        );
        assert_eq!(
            headers.get("x-aoe-token").and_then(|v| v.to_str().ok()),
            Some("tok123")
        );
    }

    // Regression test for the logout-clobber bug.
    #[test]
    fn session_cookie_refresh_skips_login_exempt_paths() {
        // Login-exempt: must not refresh.
        assert!(!should_refresh_session_cookie("/api/logout"));
        assert!(!should_refresh_session_cookie("/api/login"));
        assert!(!should_refresh_session_cookie("/"));
        assert!(!should_refresh_session_cookie("/login"));
        assert!(!should_refresh_session_cookie("/session/abc"));
        assert!(!should_refresh_session_cookie("/api/login/status"));
        assert!(!should_refresh_session_cookie("/theme-bootstrap.js"));
        assert!(!should_refresh_session_cookie("/assets/index.js"));
        assert!(!should_refresh_session_cookie("/manifest.json"));
        assert!(!should_refresh_session_cookie("/sw.js"));

        // Non-exempt: must refresh (sliding window).
        assert!(should_refresh_session_cookie("/api/sessions"));
        assert!(should_refresh_session_cookie("/api/sessions/abc/acp/ws"));
        assert!(should_refresh_session_cookie("/api/settings"));
        // /api/login/elevate is gated by the session check (not
        // exempt), so its response should slide the window.
        assert!(should_refresh_session_cookie("/api/login/elevate"));
    }

    #[test]
    fn login_session_exempt_paths() {
        // Bootstrap + status endpoints.
        assert!(is_login_session_exempt("/"));
        assert!(is_login_session_exempt("/login"));
        assert!(is_login_session_exempt("/session/abc"));
        assert!(is_login_session_exempt("/session/2626c6af68754732"));
        assert!(is_login_session_exempt("/api/login"));
        assert!(is_login_session_exempt("/api/login/status"));
        assert!(is_login_session_exempt("/api/logout"));

        // Static assets: pre-load the SPA shell before any auth.
        assert!(is_login_session_exempt("/theme-bootstrap.js"));
        assert!(is_login_session_exempt("/assets/index.css"));
        assert!(is_login_session_exempt("/assets/index-abc123.js"));
        assert!(is_login_session_exempt("/manifest.json"));
        assert!(is_login_session_exempt("/sw.js"));
        assert!(is_login_session_exempt("/icon-192.png"));
        assert!(is_login_session_exempt("/fonts/inter.woff2"));

        // Everything else stays gated, including real data/API and
        // websocket attach routes that can send a device binding.
        assert!(!is_login_session_exempt("/api/sessions"));
        assert!(!is_login_session_exempt("/api/login/elevate"));
        assert!(!is_login_session_exempt("/api/settings"));
        assert!(!is_login_session_exempt("/sessions/abc/ws"));
        assert!(!is_login_session_exempt("/api/sessions/abc/ws"));
        // /api/login/foo is not the same as /api/login exactly.
        assert!(!is_login_session_exempt("/api/login/foo"));
        // /logins is not /login.
        assert!(!is_login_session_exempt("/logins"));
    }

    // Pin the session lifetime to 30 days.
    #[tokio::test]
    async fn login_session_lifetime_is_thirty_days_sliding() {
        use crate::server::login::{LoginManager, SESSION_LIFETIME};
        use std::time::Duration;

        assert_eq!(
            SESSION_LIFETIME,
            Duration::from_secs(30 * 24 * 60 * 60),
            "SESSION_LIFETIME must be 30 days; see #1167"
        );

        // Cookie's advertised Max-Age must equal the server TTL so
        // the browser and server agree on when the session is gone.
        let mgr = LoginManager::new(Some("test"));
        let binding = vec![0xAB; 32];
        let session_id = mgr
            .create_session(&binding, "127.0.0.1", "test-agent")
            .await;
        assert!(mgr.validate_session(&session_id, &binding).await);

        let cookie = super::super::login::build_login_cookie(&session_id, false);
        let expected = format!("Max-Age={}", SESSION_LIFETIME.as_secs());
        assert!(
            cookie.contains(&expected),
            "cookie {cookie:?} must advertise {expected} to match server TTL"
        );
    }

    #[test]
    fn requires_elevation_paths() {
        use axum::http::Method;
        // Sensitive.
        assert!(requires_elevation(&Method::PATCH, "/api/settings"));
        assert!(requires_elevation(&Method::PATCH, "/api/default-profile"));
        assert!(requires_elevation(&Method::POST, "/api/profiles"));
        assert!(requires_elevation(
            &Method::PATCH,
            "/api/profiles/work/rename"
        ));
        assert!(requires_elevation(&Method::DELETE, "/api/profiles/work"));
        // Trailing slash must not bypass the gate.
        assert!(requires_elevation(&Method::PATCH, "/api/settings/"));

        // `PATCH /api/profiles/{name}/settings` is body-gated inside the handler
        // (`update_profile_settings` validates each leaf via
        // `settings_schema::validate_patch` and re-issues the 403 elevation_required
        // payload for `requires_elevation` fields).
        assert!(!requires_elevation(
            &Method::PATCH,
            "/api/profiles/work/settings"
        ));
        assert!(!requires_elevation(
            &Method::PATCH,
            "/api/profiles/work/settings/"
        ));

        // NOT gated.
        assert!(!requires_elevation(&Method::GET, "/api/sessions/abc/ws"));
        assert!(!requires_elevation(
            &Method::GET,
            "/api/sessions/abc/ws-readonly"
        ));
        assert!(!requires_elevation(&Method::GET, "/sessions/abc/acp/ws"));
        assert!(!requires_elevation(
            &Method::POST,
            "/api/sessions/abc/acp/prompt"
        ));
        assert!(!requires_elevation(
            &Method::POST,
            "/api/sessions/abc/acp/cancel"
        ));
        assert!(!requires_elevation(
            &Method::POST,
            "/api/sessions/abc/acp/approvals/nonce1"
        ));
        assert!(!requires_elevation(&Method::POST, "/api/sessions"));
        assert!(!requires_elevation(&Method::DELETE, "/api/sessions/abc"));
        assert!(!requires_elevation(&Method::DELETE, "/api/workspaces"));
        assert!(!requires_elevation(&Method::POST, "/api/sessions/abc/send"));
        assert!(!requires_elevation(&Method::PATCH, "/api/sessions/abc"));
        assert!(!requires_elevation(
            &Method::POST,
            "/api/sessions/abc/ensure"
        ));
        assert!(!requires_elevation(
            &Method::PATCH,
            "/api/sessions/abc/notifications"
        ));
        assert!(!requires_elevation(&Method::POST, "/api/git/clone"));
        assert!(!requires_elevation(&Method::POST, "/api/projects"));
        assert!(!requires_elevation(&Method::DELETE, "/api/projects/myproj"));
        assert!(!requires_elevation(&Method::PATCH, "/api/projects/myproj"));
        assert!(!requires_elevation(&Method::POST, "/api/push/subscribe"));
        assert!(!requires_elevation(&Method::POST, "/api/push/unsubscribe"));
        // Cosmetic UI state.
        assert!(!requires_elevation(
            &Method::POST,
            "/api/app-state/web-tour-seen"
        ));
        // Dismissing the update banner persists one version string and grants
        // no capability, so it stays off the passphrase wall too (the handler
        // still enforces read_only).
        assert!(!requires_elevation(
            &Method::POST,
            "/api/app-state/dismiss-update"
        ));
        // Web UI-state sync persists only cosmetic preferences and grants no capability, so
        // it stays off the passphrase wall (read_only still blocks it).
        assert!(!requires_elevation(
            &Method::PATCH,
            "/api/app-state/web-ui-state"
        ));

        // Read-only GETs are NOT gated even on settings/profile paths.
        assert!(!requires_elevation(&Method::GET, "/api/settings"));
        assert!(!requires_elevation(&Method::GET, "/api/profiles"));
        assert!(!requires_elevation(
            &Method::GET,
            "/api/profiles/work/settings"
        ));
        assert!(!requires_elevation(&Method::GET, "/api/sessions"));
        assert!(!requires_elevation(&Method::GET, "/api/sessions/abc"));

        // Out-of-scope paths never gate.
        assert!(!requires_elevation(&Method::GET, "/api/about"));
        assert!(!requires_elevation(&Method::POST, "/api/login"));
        assert!(!requires_elevation(&Method::POST, "/api/login/elevate"));

        // Device / login-session management IS gated.
        assert!(requires_elevation(&Method::POST, "/api/login/logout-all"));
        assert!(requires_elevation(
            &Method::DELETE,
            "/api/login/sessions/abc123"
        ));
        // But listing devices (read-only) and self-logout are not gated.
        assert!(!requires_elevation(&Method::GET, "/api/devices"));
        assert!(!requires_elevation(&Method::POST, "/api/logout"));
    }
}
