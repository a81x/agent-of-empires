//! Remote daemons listed inline in the home view, against a fake daemon that
//! speaks the session list and live terminal socket the TUI relies on.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serial_test::parallel;
use tokio::sync::Notify;

use crate::harness::{app_dir_in, require_tmux, TuiTestHarness};

const TOKEN: &str = "remote-e2e-token";
const PANE_OUTPUT: &str = "REMOTE-PANE-OUTPUT";

#[derive(Clone, Default)]
struct Daemon {
    /// `Authorization` header of each live-ws upgrade.
    upgrades: Arc<Mutex<Vec<Option<String>>>>,
    /// Socket events in arrival order: `claim`, `granted`, `input:<text>`.
    events: Arc<Mutex<Vec<String>>>,
    grant: Arc<Notify>,
}

impl Daemon {
    async fn start() -> (Self, String) {
        let daemon = Self::default();
        let app = Router::new()
            .route("/api/sessions", get(sessions))
            .route(
                "/api/profiles",
                get(|| async {
                    Json(serde_json::json!([{"name": "default", "is_default": true}]))
                }),
            )
            .route("/api/agents", get(|| async { Json(serde_json::json!([])) }))
            .route(
                "/api/filesystem/home",
                get(|| async { Json(serde_json::json!({"path": "/home/remote"})) }),
            )
            .route(
                "/api/docker/status",
                get(|| async { Json(serde_json::json!({"available": false})) }),
            )
            .route("/sessions/{id}/live-ws", get(live_ws))
            .with_state(daemon.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        (daemon, url)
    }

    fn events(&self) -> Vec<String> {
        self.events.lock().unwrap().clone()
    }

    fn wait_until(&self, what: &str, done: impl Fn(&Self) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(15);
        while !done(self) {
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {what}; events: {:?}",
                self.events()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

fn authorized(headers: &HeaderMap) -> bool {
    headers.get("authorization").and_then(|v| v.to_str().ok()) == Some(&format!("Bearer {TOKEN}"))
}

async fn sessions(headers: HeaderMap) -> Response {
    if !authorized(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    Json(serde_json::json!({
        "sessions": [
            {"id": "alpha", "title": "Remote alpha", "project_path": "/home/remote/alpha", "status": "Running"},
            {"id": "beta", "title": "Remote beta", "project_path": "/home/remote/beta", "view": "structured"}
        ],
        "workspace_ordering": []
    }))
    .into_response()
}

async fn live_ws(
    State(daemon): State<Daemon>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    let authorization = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    daemon.upgrades.lock().unwrap().push(authorization);
    if !authorized(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    ws.on_upgrade(move |socket| pump(daemon, socket))
}

async fn pump(daemon: Daemon, mut socket: WebSocket) {
    let frame = serde_json::json!({"type": "frame", "content": format!("{PANE_OUTPUT}\n")});
    if socket
        .send(Message::Text(frame.to_string().into()))
        .await
        .is_err()
    {
        return;
    }
    let mut claimed = false;
    loop {
        tokio::select! {
            message = socket.recv() => match message {
                Some(Ok(Message::Text(text))) if text.as_str().contains(r#""claim""#) => {
                    claimed = true;
                    daemon.events.lock().unwrap().push("claim".into());
                }
                Some(Ok(Message::Binary(bytes))) => {
                    let text = String::from_utf8_lossy(&bytes).into_owned();
                    daemon.events.lock().unwrap().push(format!("input:{text}"));
                }
                Some(Ok(_)) => {}
                _ => return,
            },
            // The real daemon drops input until this grant, so the test holds
            // it back until the keystroke has already been typed.
            () = daemon.grant.notified(), if claimed => {
                daemon.events.lock().unwrap().push("granted".into());
                let grant = serde_json::json!({"type": "size_owner", "is_owner": true});
                if socket.send(Message::Text(grant.to_string().into())).await.is_err() {
                    return;
                }
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[parallel]
async fn a_registered_remote_lists_inline_and_live_send_waits_for_ownership() {
    require_tmux!();
    let (daemon, url) = Daemon::start().await;
    let mut harness = TuiTestHarness::new("remote_inline");

    let refused = harness.run_cli(&[
        "remote",
        "add",
        "plain",
        "http://example.test",
        "--token",
        TOKEN,
    ]);
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("HTTPS"),
        "a plaintext bearer must be refused at add time: {refused:?}"
    );
    let wrong_base = format!("{url}/not-the-api");
    let refused = harness.run_cli(&["remote", "add", "mini", &wrong_base, "--token", TOKEN]);
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("HTTP 404"),
        "a wrong base path must fail the add: {refused:?}"
    );
    let added = harness.run_cli(&["remote", "add", "mini", &url, "--token", TOKEN]);
    assert!(added.status.success(), "{added:?}");

    harness.spawn_tui();
    harness.wait_for("Remote alpha");
    harness.assert_screen_contains("mini");

    // Local header, then mini's header, then its rows sorted by title.
    let mut selected = false;
    for _ in 0..4 {
        harness.send_keys("Down");
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline && !harness.capture_screen().contains(PANE_OUTPUT) {
            std::thread::sleep(Duration::from_millis(50));
        }
        if harness.capture_screen().contains(PANE_OUTPUT) {
            selected = true;
            break;
        }
    }
    assert!(
        selected,
        "the remote pane never previewed:\n{}",
        harness.capture_screen()
    );
    assert_eq!(
        daemon.upgrades.lock().unwrap().first().cloned().flatten(),
        Some(format!("Bearer {TOKEN}")),
        "the live socket carries the bearer as a header"
    );

    // Unfenced from here: once the preview goes idle, tmux holds the TUI's
    // last write back from `pipe-pane`, so the F12 render ack never arrives.
    // The screen and the daemon's event log are the synchronization instead.
    let tui = harness.session_name().to_string();
    harness.send_session_keys(&tui, "Enter");
    harness.wait_for("LIVE");
    daemon.wait_until("the take-over", |d| d.events().iter().any(|e| e == "claim"));
    // `q` would quit aoe from the home view; in live-send it is the agent's.
    harness.send_session_keys(&tui, "q");
    daemon.grant.notify_one();
    daemon.wait_until("the held keystroke", |d| {
        d.events().iter().any(|e| e == "input:q")
    });
    assert_eq!(daemon.events(), ["claim", "granted", "input:q"]);

    harness.send_session_keys(&tui, "C-q");
    harness.wait_for_absent("LIVE", Duration::from_secs(5));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[parallel]
async fn aoe_daemon_url_lists_its_daemon_as_an_unsaved_remote() {
    require_tmux!();
    let (_daemon, url) = Daemon::start().await;
    let mut harness = TuiTestHarness::new("remote_env");
    harness.set_env("AOE_DAEMON_URL", &url);
    harness.set_env("AOE_DAEMON_TOKEN", TOKEN);

    harness.spawn_tui();
    harness.wait_for("Remote alpha");
    harness.assert_screen_contains(url.trim_start_matches("http://"));
    assert!(
        !app_dir_in(harness.home_path())
            .join("remotes.toml")
            .exists(),
        "the temporary remote is never saved"
    );
}
