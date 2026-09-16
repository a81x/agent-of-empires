//! Client for a daemon's `/sessions/{id}/live-ws` capture stream, the same
//! stream the web dashboard renders. Frames arrive as ANSI text and keystrokes
//! go back as raw pane bytes, so no tmux client runs on this machine.

use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;

use crate::acp::client::discovery::DaemonEndpoint;
use crate::daemon::websocket::{self, NativeSocket};

#[derive(Debug, Deserialize)]
struct WireCursor {
    x: u16,
    y: u16,
}

#[derive(Debug, Deserialize)]
struct WireMessage {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    cursor: Option<WireCursor>,
    #[serde(default)]
    is_owner: Option<bool>,
}

/// What the reader task hands its owner.
#[derive(Debug)]
pub(crate) enum LiveMessage {
    Frame {
        content: String,
        cursor: Option<(u16, u16)>,
    },
    SizeOwner(bool),
    Closed(String),
}

/// Decoded view of one server frame. `patch` is never seen because this client
/// does not advertise `caps.patch`; the server then always sends full frames.
fn parse_text(text: &str) -> Option<LiveMessage> {
    let msg: WireMessage = serde_json::from_str(text).ok()?;
    match msg.kind.as_str() {
        "frame" => Some(LiveMessage::Frame {
            content: msg.content.unwrap_or_default(),
            cursor: msg.cursor.map(|c| (c.x, c.y)),
        }),
        "size_owner" => Some(LiveMessage::SizeOwner(msg.is_owner.unwrap_or(true))),
        _ => None,
    }
}

pub(crate) struct LiveSocket {
    pub(crate) rx: mpsc::Receiver<LiveMessage>,
    pub(crate) tx: mpsc::Sender<Message>,
    pub(crate) task: JoinHandle<()>,
}

/// Credentials travel in headers via the shared native WebSocket transport,
/// which also refuses them over non-loopback plaintext.
pub(crate) async fn connect(endpoint: &DaemonEndpoint, session_id: &str) -> Result<LiveSocket> {
    let session_id = crate::daemon::transport::path_segment(session_id)?;
    let path = format!("/sessions/{session_id}/live-ws");
    let (in_tx, in_rx) = mpsc::channel(64);
    let (out_tx, out_rx) = mpsc::channel(64);
    let task = match websocket::connect(endpoint, &path, None).await? {
        NativeSocket::Unix(stream) => tokio::spawn(pump(*stream, in_tx, out_rx)),
        NativeSocket::Tcp(stream) => tokio::spawn(pump(*stream, in_tx, out_rx)),
    };
    Ok(LiveSocket {
        rx: in_rx,
        tx: out_tx,
        task,
    })
}

async fn pump<S>(
    mut stream: WebSocketStream<S>,
    tx: mpsc::Sender<LiveMessage>,
    mut out_rx: mpsc::Receiver<Message>,
) where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    loop {
        tokio::select! {
            outbound = out_rx.recv() => {
                let Some(msg) = outbound else { break };
                if stream.send(msg).await.is_err() {
                    break;
                }
            }
            inbound = stream.next() => {
                match inbound {
                    Some(Ok(Message::Text(text))) => {
                        if let Some(msg) = parse_text(text.as_str()) {
                            if tx.send(msg).await.is_err() {
                                break;
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        let _ = tx
                            .send(LiveMessage::Closed(
                                "the daemon closed the live stream".to_string(),
                            ))
                            .await;
                        break;
                    }
                    Some(Ok(_)) => {}
                    Some(Err(error)) => {
                        let _ = tx.send(LiveMessage::Closed(error.to_string())).await;
                        break;
                    }
                }
            }
        }
    }
    let _ = stream.close(None).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_full_frame_with_a_cursor() {
        let msg = parse_text(
            r#"{"type":"frame","seq":7,"content":"hello\n","rows":2,"history":0,"cursor":{"x":3,"y":1}}"#,
        )
        .expect("frame parses");
        match msg {
            LiveMessage::Frame { content, cursor } => {
                assert_eq!(content, "hello\n");
                assert_eq!(cursor, Some((3, 1)));
            }
            other => panic!("expected a frame, got {other:?}"),
        }
    }

    #[test]
    fn a_hidden_cursor_is_absent_rather_than_zero() {
        let msg =
            parse_text(r#"{"type":"frame","content":"x\n","cursor":null}"#).expect("frame parses");
        match msg {
            LiveMessage::Frame { cursor, .. } => assert_eq!(cursor, None),
            other => panic!("expected a frame, got {other:?}"),
        }
    }

    #[test]
    fn reads_ownership_notices() {
        assert!(matches!(
            parse_text(r#"{"type":"size_owner","is_owner":false}"#),
            Some(LiveMessage::SizeOwner(false))
        ));
    }

    #[test]
    fn unknown_and_malformed_messages_are_dropped() {
        // A message type this client ignores must not be parsed as a frame.
        assert!(parse_text(r#"{"type":"clipboard","text":"hi"}"#).is_none());
        assert!(parse_text(r#"{"type":"transport","grid":false}"#).is_none());
        assert!(parse_text("not json").is_none());
    }

    #[tokio::test]
    async fn credentials_are_refused_over_non_loopback_plaintext() {
        use crate::acp::client::discovery::{DaemonEndpoint, Source};
        let login = crate::daemon::SessionCredential {
            session: "s".into(),
            binding: "b".into(),
        };
        for endpoint in [
            DaemonEndpoint::new(
                "http://mini.example.com:8080".into(),
                Some("t".into()),
                Source::Remote,
            ),
            DaemonEndpoint::new("http://mini.example.com:8080".into(), None, Source::Remote)
                .with_login(Some(login)),
        ] {
            let err = connect(&endpoint, "id").await.err().expect("must refuse");
            assert!(matches!(
                err.downcast_ref::<websocket::WsError>(),
                Some(websocket::WsError::Daemon(
                    crate::daemon::DaemonClientError::InsecureBearerTransport
                ))
            ));
        }
    }
}
