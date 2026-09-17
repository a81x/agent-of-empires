//! Driven conversation reset: its deadlines and the outcomes callers see.

/// Outer cap on a driven reset's `session/new` round trip (#2979).
pub(super) const SESSION_RESET_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// The connection task's deadline for the whole reset sequence. Shorter than
/// `SESSION_RESET_TIMEOUT` so the task reports the specific failure and
/// resumes draining commands before the caller gives up.
pub(super) const SESSION_RESET_IN_TASK_TIMEOUT: std::time::Duration =
    std::time::Duration::from_secs(28);

#[derive(Debug)]
pub(super) enum ResetRequestError {
    Acp(agent_client_protocol::Error),
    TimedOut,
}

pub(super) async fn await_reset_request<T, F, Fut>(
    deadline: tokio::time::Instant,
    request: F,
) -> Result<T, ResetRequestError>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<T, agent_client_protocol::Error>>,
{
    // `send_request` enqueues synchronously, so the request is built lazily:
    // a command that expired in the queue must send nothing.
    if tokio::time::Instant::now() >= deadline {
        return Err(ResetRequestError::TimedOut);
    }
    match tokio::time::timeout_at(deadline, request()).await {
        Ok(Ok(response)) => Ok(response),
        Ok(Err(error)) => Err(ResetRequestError::Acp(error)),
        Err(_) => Err(ResetRequestError::TimedOut),
    }
}

#[derive(Debug)]
pub enum ResetSessionOutcome {
    /// The connection swapped onto a fresh ACP session.
    Reset { new_acp_session_id: String },
    /// No reset happened and the conversation keeps its context.
    Failed { message: String },
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::acp::acp_client::commands::ClientCmd;
    use crate::acp::acp_client::test_helpers::reset_fake_spawn_config;
    use crate::acp::acp_client::AcpClient;
    use crate::acp::state::{AcpSessionId, Event};
    use std::path::{Path, PathBuf};
    use std::time::Duration;
    use tokio::sync::oneshot;

    /// A scripted stdio agent (#2979): mints `sid-1`, `sid-2`, ... per
    /// `session/new` (each with a `thought_level` option), acks config
    /// requests, and answers each prompt with a chunk then a response. Hold
    /// flags gate the prompt response, the second `session/new`, or the second
    /// config request until the matching release file exists. Every request is
    /// captured before its gate.
    #[derive(Default)]
    struct FakeAgent {
        hold_prompt: bool,
        hold_reset_new: bool,
        hold_reset_config: bool,
        /// Emitted right after the first `session/new`.
        initial_update: Option<serde_json::Value>,
    }

    impl FakeAgent {
        fn write(self, dir: &Path) -> (PathBuf, PathBuf) {
            let capture = dir.join("capture.ndjson");
            let script_path = dir.join("fake-reset-agent.sh");
            let initial_notification = self
                .initial_update
                .map(|update| {
                    let payload = serde_json::json!({
                        "jsonrpc": "2.0",
                        "method": "session/update",
                        "params": { "sessionId": "sid-1", "update": update },
                    })
                    .to_string();
                    let quoted = format!("'{}'", payload.replace('\'', "'\"'\"'"));
                    format!("if [ \"$count\" -eq 1 ]; then printf '%s\\n' {quoted}; fi")
                })
                .unwrap_or_else(|| ":".into());
            let script = r#"#!/bin/sh
CAPTURE=__CAPTURE__
count=0
config_count=0
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$CAPTURE"
  id=$(printf '%s' "$line" | sed -En 's/.*"id":("[^"]*"|[0-9]+).*/\1/p')
  case $line in
    *'"method":"initialize"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":1,"agentCapabilities":{"loadSession":false}}}\n' "$id"
      ;;
    *'"method":"session/new"'*)
      count=$((count+1))
      if [ "$count" -eq 2 ] && [ __RESET_NEW_HOLD__ = true ]; then
        while [ -d "__DIR__" ] && [ ! -f "$CAPTURE.new-release" ]; do sleep 0.01; done
      fi
      printf '{"jsonrpc":"2.0","id":%s,"result":{"sessionId":"sid-%d","configOptions":[{"id":"effort","name":"Reasoning Effort","category":"thought_level","type":"select","currentValue":"default","options":[{"value":"default","name":"Default"},{"value":"high","name":"High"}]}]}}\n' "$id" "$count"
      __INITIAL_NOTIFICATION__
      ;;
    *'"method":"session/set_config_option"'*)
      config_count=$((config_count+1))
      if [ "$config_count" -eq 2 ] && [ __RESET_CONFIG_HOLD__ = true ]; then
        while [ -d "__DIR__" ] && [ ! -f "$CAPTURE.config-release" ]; do sleep 0.01; done
      fi
      printf '{"jsonrpc":"2.0","id":%s,"result":{"configOptions":[]}}\n' "$id"
      ;;
    *'"method":"session/prompt"'*)
      printf '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sid-%d","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"working"}}}}\n' "$count"
      if [ __HOLD__ = true ]; then
        while [ -d "__DIR__" ] && [ ! -f "$CAPTURE.prompt-release" ]; do sleep 0.01; done
      fi
      printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"end_turn"}}\n' "$id"
      ;;
  esac
done
"#
            .replace("__CAPTURE__", capture.to_str().unwrap())
            .replace("__DIR__", dir.to_str().unwrap())
            .replace("__HOLD__", &self.hold_prompt.to_string())
            .replace("__RESET_NEW_HOLD__", &self.hold_reset_new.to_string())
            .replace("__RESET_CONFIG_HOLD__", &self.hold_reset_config.to_string())
            .replace("__INITIAL_NOTIFICATION__", &initial_notification);
            std::fs::write(&script_path, script).unwrap();
            (script_path, capture)
        }

        async fn spawn(self, dir: &Path, name: &str, effort: Option<&str>) -> (AcpClient, PathBuf) {
            let (script, capture) = self.write(dir);
            let mut config = reset_fake_spawn_config(&script, dir);
            config.default_effort = effort.map(Into::into);
            let client = AcpClient::spawn(config, AcpSessionId(name.into()))
                .await
                .expect("spawn scripted fake agent");
            (client, capture)
        }
    }

    fn requests(capture: &Path, method: &str) -> usize {
        let wire = std::fs::read_to_string(capture).unwrap();
        wire.matches(&format!("\"method\":\"{method}\"")).count()
    }

    async fn wait_for_captured_requests(capture: &Path, method: &str, count: usize) {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while requests(capture, method) < count {
            assert!(
                std::time::Instant::now() < deadline,
                "adapter did not receive {method}"
            );
            // Keeps paused time stationary until native I/O reaches the RPC.
            tokio::task::yield_now().await;
        }
    }

    /// Collect events until one matches `done`, returning all of them.
    async fn events_until(client: &mut AcpClient, done: impl Fn(&Event) -> bool) -> Vec<Event> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        let mut events = Vec::new();
        loop {
            let event = tokio::time::timeout_at(deadline, client.next_event())
                .await
                .expect("timed out waiting for events")
                .expect("event channel closed");
            let finished = done(&event);
            events.push(event);
            if finished {
                return events;
            }
        }
    }

    async fn reset_with_deadline(
        client: &AcpClient,
        deadline: tokio::time::Instant,
    ) -> ResetSessionOutcome {
        let (respond_to, response) = oneshot::channel();
        client
            .cmd_tx
            .as_ref()
            .unwrap()
            .send(ClientCmd::ResetSession {
                text: "/new".into(),
                deadline,
                respond_to,
            })
            .await
            .unwrap();
        tokio::time::timeout_at(deadline + Duration::from_secs(4), response)
            .await
            .expect("connection task must answer the reset")
            .unwrap()
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn unavailable_resume_reports_durable_context_reset() {
        use crate::acp::event_store::EventStore;
        use crate::acp::transcript::{TranscriptModel, TranscriptRowKind};

        // (name, stored, load_capable, load_fails, new_fails, fork, resets)
        for (name, stored, load_capable, load_fails, new_fails, fork, expected_resets) in [
            ("unavailable", true, false, false, false, false, 1),
            ("initial", false, false, false, false, false, 0),
            ("loaded", true, true, false, false, false, 0),
            ("load_failed", true, true, true, false, false, 1),
            ("unavailable_new_failed", true, false, false, true, false, 0),
            ("load_and_new_failed", true, true, true, true, false, 0),
            ("fork_unsupported", true, false, false, false, true, 1),
            (
                "fork_unsupported_load_failed",
                true,
                true,
                true,
                false,
                true,
                1,
            ),
        ] {
            let tmp = tempfile::TempDir::new().unwrap();
            let script = tmp.path().join("resume-agent.sh");
            let capture = tmp.path().join("capture.ndjson");
            let error = r#""error":{"code":-32603,"message":"fixture refused establishment"}"#;
            let new_ok = r#""result":{"sessionId":"replacement-session"}"#;
            let script_body = r#"#!/bin/sh
while IFS= read -r line; do
  printf '%s\n' "$line" >> '__CAPTURE__'
  id=$(printf '%s' "$line" | sed -En 's/.*"id":("[^"]*"|[0-9]+).*/\1/p')
  case $line in
    *'"method":"initialize"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":1,"agentCapabilities":{"loadSession":__LOAD_CAPABLE__}}}\n' "$id" ;;
    *'"method":"session/load"'*)
      printf '{"jsonrpc":"2.0","id":%s,__LOAD_REPLY__}\n' "$id" ;;
    *'"method":"session/new"'*)
      printf '{"jsonrpc":"2.0","id":%s,__NEW_REPLY__}\n' "$id" ;;
    *'"method":"session/prompt"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"stopReason":"end_turn"}}\n' "$id" ;;
  esac
done
"#
            .replace("__CAPTURE__", capture.to_str().unwrap())
            .replace("__LOAD_CAPABLE__", &load_capable.to_string())
            .replace("__LOAD_REPLY__", if load_fails { error } else { r#""result":{}"# })
            .replace("__NEW_REPLY__", if new_fails { error } else { new_ok });
            std::fs::write(&script, script_body).unwrap();
            let mut config = reset_fake_spawn_config(&script, tmp.path());
            config.stored_acp_session_id = stored.then(|| "previous-session".into());
            config.fork_from = fork.then(|| "parent-session".into());
            let mut client = AcpClient::spawn(config, AcpSessionId(name.into()))
                .await
                .unwrap();
            let mut events = Vec::new();
            if stored {
                events.push(Event::UserPromptSent {
                    prompt_id: None,
                    text: "prior user turn".into(),
                    attachments: vec![],
                });
                events.push(Event::AgentMessageChunk {
                    text: "prior assistant turn".into(),
                });
            }
            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
            while let Some(event) = tokio::time::timeout_at(deadline, client.next_event())
                .await
                .expect(name)
            {
                let assigned = matches!(event, Event::AcpSessionAssigned { .. });
                let stopped = matches!(event, Event::Stopped { .. });
                events.push(event);
                if assigned {
                    client.send_prompt("continue", &[]).await.unwrap();
                }
                if stopped {
                    client.shutdown().await.unwrap();
                }
            }
            client.shutdown().await.unwrap();
            let position = |pred: fn(&Event) -> bool| events.iter().position(pred);
            let assignment = position(|e| matches!(e, Event::AcpSessionAssigned { .. }));
            assert_eq!(assignment.is_some(), !new_fails, "{name}");
            assert_eq!(
                position(|e| matches!(e, Event::AgentStartupError { .. })).is_some(),
                new_fails,
                "{name}"
            );
            assert_eq!(
                position(|e| matches!(e, Event::Stopped { .. })).is_some(),
                !new_fails,
                "{name}"
            );
            let is_reset = |e: &Event| matches!(e, Event::SessionContextReset { .. });
            assert_eq!(
                events.iter().filter(|e| is_reset(e)).count(),
                expected_resets,
                "{name}"
            );
            if let Some(reset) = events.iter().position(is_reset) {
                assert!(
                    reset < assignment.unwrap(),
                    "{name}: reset must precede assignment"
                );
            }

            // The reset survives the event store and renders a divider.
            let db = tmp.path().join("events.db");
            let store = EventStore::open(&db, 100).unwrap();
            for (index, event) in events.iter().enumerate() {
                store.record(name, index as u64 + 1, event).unwrap();
            }
            drop(store);
            let store = EventStore::open(&db, 100).unwrap();
            let mut transcript = TranscriptModel::new();
            for (seq, event) in store.replay_from(name, 0) {
                transcript.apply_event(seq, &event);
            }
            let rows = transcript.rows();
            let dividers = rows
                .iter()
                .filter(|row| row.kind == TranscriptRowKind::ContextReset)
                .count();
            assert_eq!(dividers, expected_resets, "{name}");
            if stored {
                for text in ["prior user turn", "prior assistant turn"] {
                    assert!(rows.iter().any(|row| row.text == text), "{name}: {text}");
                }
            }

            let requests: Vec<serde_json::Value> = std::fs::read_to_string(capture)
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            let loads: Vec<_> = requests
                .iter()
                .filter(|r| r["method"] == "session/load")
                .collect();
            assert_eq!(loads.len(), usize::from(stored && load_capable), "{name}");
            if let Some(load) = loads.first() {
                assert_eq!(load["params"]["sessionId"], "previous-session", "{name}");
            }
            let news = requests
                .iter()
                .filter(|r| r["method"] == "session/new")
                .count();
            assert_eq!(
                news,
                usize::from(!(stored && load_capable && !load_fails)),
                "{name}"
            );
        }
    }

    /// The deadline starts before enqueueing, so an expired command never
    /// sends the stateful session/new, and the loop keeps serving commands.
    #[tokio::test]
    async fn expired_reset_deadline_does_not_send_session_new() {
        let _env = crate::session::test_support::EnvGuard::read_lock();
        let tmp = tempfile::TempDir::new().unwrap();
        let (client, capture) = FakeAgent::default()
            .spawn(tmp.path(), "reset-expired", None)
            .await;
        let outcome = reset_with_deadline(&client, tokio::time::Instant::now()).await;
        assert!(
            matches!(outcome, ResetSessionOutcome::Failed { .. }),
            "{outcome:?}"
        );
        assert_eq!(requests(&capture, "session/new"), 1);
        let valid = reset_with_deadline(
            &client,
            tokio::time::Instant::now() + Duration::from_secs(2),
        )
        .await;
        assert!(
            matches!(valid, ResetSessionOutcome::Reset { .. }),
            "{valid:?}"
        );
        let _ = client.shutdown().await;
    }

    /// Work that outlives its prompt (an open tool, or a tracked async
    /// sub-agent) would attach old-session events to the fresh conversation.
    #[tokio::test]
    #[serial_test::serial]
    async fn reset_between_prompts_with_open_work_is_refused() {
        let _env = crate::session::test_support::EnvGuard::read_lock();
        let tmp = tempfile::TempDir::new().unwrap();
        let transcript = tmp.path().join("background-agent.jsonl");
        std::fs::write(&transcript, "").unwrap();
        let open_tool = serde_json::json!({
            "sessionUpdate": "tool_call",
            "toolCallId": "between-prompt-tool",
            "title": "long-running tool",
            "kind": "other",
            "status": "in_progress",
            "rawInput": {},
        });
        let background_agent = serde_json::json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": "between-prompt-agent-tool",
            "_meta": { "claudeCode": {
                "toolName": "Agent",
                "toolResponse": {
                    "agentId": "between-prompt-agent",
                    "description": "keep working after the parent turn",
                    "prompt": "continue the delegated task",
                    "resolvedModel": "test-model",
                    "outputFile": transcript.to_str().unwrap(),
                    "status": "async_launched",
                },
            }},
        });
        let started: [(serde_json::Value, fn(&Event) -> bool); 2] = [
            (
                open_tool,
                |e| matches!(e, Event::ToolCallStarted { tool_call } if tool_call.id == "between-prompt-tool"),
            ),
            (
                background_agent,
                |e| matches!(e, Event::BackgroundAgentLaunched { agent_id, .. } if agent_id == "between-prompt-agent"),
            ),
        ];
        for (index, (initial_update, is_started)) in started.into_iter().enumerate() {
            let dir = tmp.path().join(index.to_string());
            std::fs::create_dir(&dir).unwrap();
            let agent = FakeAgent {
                initial_update: Some(initial_update),
                ..FakeAgent::default()
            };
            let (mut client, capture) = agent.spawn(&dir, "reset-open-work", None).await;
            events_until(&mut client, is_started).await;

            let outcome = client.reset_session("/new").await.unwrap();
            assert!(
                matches!(&outcome, ResetSessionOutcome::Failed { message }
                    if message.contains("agent work is still in flight")),
                "{outcome:?}"
            );
            let events = events_until(&mut client, |e| {
                matches!(e, Event::PromptRejected { .. } | Event::SessionCleared)
            })
            .await;
            assert!(matches!(
                events.last(),
                Some(Event::PromptRejected { reason, text }) if reason == "agent_busy" && text == "/new"
            ));
            assert_eq!(requests(&capture, "session/new"), 1);
            let _ = client.shutdown().await;
        }
    }

    /// A stalled RPC releases the loop at the caller's deadline. A stalled
    /// `session/new` fails the reset; a stalled post-commit config request
    /// keeps the committed fresh id.
    #[tokio::test]
    #[serial_test::serial]
    async fn stalled_reset_request_releases_the_connection_loop() {
        let _env = crate::session::test_support::EnvGuard::read_lock();
        for hold_new in [true, false] {
            let tmp = tempfile::TempDir::new().unwrap();
            let agent = FakeAgent {
                hold_reset_new: hold_new,
                hold_reset_config: !hold_new,
                ..FakeAgent::default()
            };
            let effort = (!hold_new).then_some("high");
            let (client, capture) = agent.spawn(tmp.path(), "reset-timeout", effort).await;
            let (method, release) = if hold_new {
                ("session/new", "new-release")
            } else {
                ("session/set_config_option", "config-release")
            };

            tokio::time::pause();
            let first = reset_with_deadline(
                &client,
                tokio::time::Instant::now() + Duration::from_secs(1),
            );
            tokio::pin!(first);
            tokio::select! {
                outcome = &mut first => panic!("reset finished before the RPC was held: {outcome:?}"),
                _ = wait_for_captured_requests(&capture, method, 2) => {}
            }
            tokio::time::advance(Duration::from_secs(1)).await;
            let first = first.await;
            tokio::time::resume();
            std::fs::write(
                tmp.path().join(format!("capture.ndjson.{release}")),
                "release",
            )
            .unwrap();

            if hold_new {
                assert!(
                    matches!(&first, ResetSessionOutcome::Failed { message }
                        if message.contains("before the reset deadline")),
                    "{first:?}"
                );
                let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
                let second = reset_with_deadline(&client, deadline).await;
                assert!(
                    matches!(&second, ResetSessionOutcome::Reset { new_acp_session_id }
                        if new_acp_session_id == "sid-3"),
                    "{second:?}"
                );
            } else {
                assert!(
                    matches!(&first, ResetSessionOutcome::Reset { new_acp_session_id }
                        if new_acp_session_id == "sid-2"),
                    "{first:?}"
                );
                client
                    .send_prompt("after config timeout", &[])
                    .await
                    .unwrap();
                let deadline = std::time::Instant::now() + Duration::from_secs(3);
                let prompt_line = loop {
                    let wire = std::fs::read_to_string(&capture).unwrap();
                    if let Some(line) = wire
                        .lines()
                        .find(|l| l.contains("\"method\":\"session/prompt\""))
                    {
                        break line.to_string();
                    }
                    assert!(
                        std::time::Instant::now() < deadline,
                        "follow-up not processed"
                    );
                    tokio::time::sleep(Duration::from_millis(20)).await;
                };
                assert!(
                    prompt_line.contains("\"sessionId\":\"sid-2\""),
                    "{prompt_line}"
                );
            }
            let _ = client.shutdown().await;
        }
    }

    /// #2979: a clear on a profile without a native reset opens a fresh
    /// session on the live worker, emits the ordered boundary events,
    /// re-applies the configured effort, and never forwards the alias text.
    #[tokio::test]
    async fn codex_clear_drives_fresh_session_new_on_live_worker() {
        let _env = crate::session::test_support::EnvGuard::read_lock();
        let tmp = tempfile::TempDir::new().unwrap();
        let (mut client, capture) = FakeAgent::default()
            .spawn(tmp.path(), "reset-2979", Some("high"))
            .await;

        let outcome = client.reset_session("/new").await.unwrap();
        assert!(
            matches!(&outcome, ResetSessionOutcome::Reset { new_acp_session_id } if new_acp_session_id == "sid-2"),
            "{outcome:?}"
        );
        let events = events_until(&mut client, |e| matches!(e, Event::Stopped { .. })).await;
        let position = |pred: &dyn Fn(&Event) -> bool| events.iter().position(pred).unwrap();
        let cleared = position(&|e| matches!(e, Event::SessionCleared));
        let reset = position(&|e| matches!(e, Event::SessionContextReset { .. }));
        let assigned = position(
            &|e| matches!(e, Event::AcpSessionAssigned { acp_session_id } if acp_session_id == "sid-2"),
        );
        assert!(cleared < reset && reset < assigned, "{events:?}");
        assert!(
            matches!(events.last(), Some(Event::Stopped { reason }) if reason == "session_reset")
        );

        client.send_prompt("hello", &[]).await.unwrap();
        events_until(&mut client, |e| matches!(e, Event::Stopped { .. })).await;

        let wire = std::fs::read_to_string(&capture).unwrap();
        assert_eq!(requests(&capture, "session/new"), 2, "{wire}");
        assert!(!wire.contains("\"text\":\"/new\""), "{wire}");
        assert!(wire.contains("\"sessionId\":\"sid-2\""), "{wire}");
        assert_eq!(requests(&capture, "session/set_config_option"), 2, "{wire}");
        assert_eq!(wire.matches("\"value\":\"high\"").count(), 2, "{wire}");
        let _ = client.shutdown().await;
    }

    /// #2979: resetting under an in-flight turn would orphan it, so it is
    /// refused with a retryable `PromptRejected` and no `SessionCleared`.
    #[tokio::test]
    #[serial_test::serial]
    async fn reset_during_in_flight_prompt_is_refused_with_prompt_rejected() {
        let _env = crate::session::test_support::EnvGuard::read_lock();
        let tmp = tempfile::TempDir::new().unwrap();
        let agent = FakeAgent {
            hold_prompt: true,
            ..FakeAgent::default()
        };
        let (mut client, capture) = agent.spawn(tmp.path(), "reset-busy-2979", None).await;
        client.send_prompt("hello", &[]).await.unwrap();
        // The chunk proves the task is inside the in-flight select.
        events_until(&mut client, |e| {
            matches!(e, Event::AgentMessageChunk { .. })
        })
        .await;

        let outcome = client.reset_session("/new").await.unwrap();
        assert!(
            matches!(&outcome, ResetSessionOutcome::Failed { message } if message.contains("turn is in flight")),
            "{outcome:?}"
        );
        let rejected =
            events_until(&mut client, |e| matches!(e, Event::PromptRejected { .. })).await;
        assert!(matches!(
            rejected.last(),
            Some(Event::PromptRejected { reason, text }) if reason == "agent_busy" && text == "/new"
        ));
        std::fs::write(tmp.path().join("capture.ndjson.prompt-release"), "release").unwrap();
        let rest = events_until(&mut client, |e| matches!(e, Event::Stopped { .. })).await;
        assert!(!rejected
            .iter()
            .chain(&rest)
            .any(|e| matches!(e, Event::SessionCleared)));
        assert_eq!(requests(&capture, "session/new"), 1);
        let _ = client.shutdown().await;
    }
}
