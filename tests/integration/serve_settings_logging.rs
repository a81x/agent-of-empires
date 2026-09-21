use agent_of_empires::logging;
use agent_of_empires::server::test_support::{build_router_for_test, build_test_app_state};
use agent_of_empires::session::{self, Config};
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use std::net::SocketAddr;
use tower::ServiceExt;

struct RestoreFilter(String);

impl Drop for RestoreFilter {
    fn drop(&mut self) {
        logging::set_filter(&self.0).expect("restore runtime filter");
    }
}

async fn patch(app: &axum::Router, uri: &str, body: Value) -> Value {
    let mut request = Request::builder()
        .method("PATCH")
        .uri(uri)
        .header("host", "127.0.0.1")
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    request
        .extensions_mut()
        .insert(ConnectInfo("127.0.0.1:5555".parse::<SocketAddr>().unwrap()));
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(
        status,
        StatusCode::OK,
        "{uri}: {}",
        String::from_utf8_lossy(&bytes)
    );
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
#[serial_test::serial]
async fn settings_only_reload_changed_log_filters() {
    let _home = crate::common::setup_temp_home();
    if logging::controller().is_none() {
        let init = logging::init_subscriber(logging::SubscriberTarget::Stdout, "off".into());
        logging::install_controller(init.controller.expect("reloadable subscriber"));
    }
    let _restore_filter = RestoreFilter(logging::current_filter().unwrap());
    session::update_config(|config| {
        config.logging.default_level = "info".into();
        config
            .logging
            .targets
            .insert("acp.protocol".into(), "warn".into());
    })
    .unwrap();
    let app = build_router_for_test(build_test_app_state(Vec::new()));
    let runtime_path = logging::runtime_filter_path(&session::get_app_dir().unwrap());
    let temporary = "agent_of_empires=debug,acp.protocol=trace";
    logging::set_filter("agent_of_empires=info").unwrap();

    for (name, body, filter_changed) in [
        ("empty patch", json!({}), false),
        ("empty logging", json!({"logging": {}}), false),
        (
            "same baseline",
            json!({"logging": {"default_level": "info"}}),
            false,
        ),
        (
            "same targets",
            json!({"logging": {"targets": {"acp.protocol": "warn"}}}),
            false,
        ),
        (
            "same filter",
            json!({"logging": {"default_level": "info", "targets": {"acp.protocol": "warn"}}}),
            false,
        ),
        (
            "sink settings",
            json!({"logging": {
                "output": "stdout", "file_path": "server.log", "rotation": "never",
                "max_size_mib": 17, "keep_count": 3, "show_spans": true
            }}),
            false,
        ),
        (
            "changed baseline",
            json!({"logging": {"default_level": "error"}}),
            true,
        ),
        (
            "changed targets",
            json!({"logging": {"targets": {"acp.protocol": "debug"}}}),
            true,
        ),
    ] {
        let runtime = patch(&app, "/api/log-level", json!({"filter": temporary})).await;
        assert_eq!(runtime["current"], temporary, "{name}");
        let response = patch(&app, "/api/settings", body.clone()).await;
        let saved = Config::load().unwrap();
        let saved_logging = serde_json::to_value(&saved.logging).unwrap();
        assert_eq!(response["logging"], saved_logging, "{name}");
        if let Some(fields) = body.get("logging").and_then(Value::as_object) {
            for (key, value) in fields {
                assert_eq!(&saved_logging[key], value, "{name}: {key}");
            }
        }
        let expected = if filter_changed {
            logging::build_filter_from_config(&saved.logging.default_level, &saved.logging.targets)
                .unwrap()
        } else {
            temporary.to_string()
        };
        assert_eq!(
            logging::current_filter().as_deref(),
            Some(expected.as_str()),
            "{name}"
        );
        assert_eq!(
            std::fs::read_to_string(&runtime_path).unwrap().trim(),
            expected,
            "{name}"
        );
    }
}
