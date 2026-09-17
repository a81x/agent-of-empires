//! Live output of the selected remote session for the home view's preview
//! pane, over the daemon's live terminal socket.
//!
//! Watching never claims the remote pane's size: a viewer renders at whatever
//! grid the pane already has, so moving the cursor across remote rows cannot
//! resize a pane for anyone else. Only live-send takes the size, the same
//! take-over local live-send makes, and leaving it reconnects to let go.

use std::collections::VecDeque;
use std::sync::mpsc as std_mpsc;

use serde_json::json;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

use crate::acp::client::discovery::DaemonEndpoint;
use crate::tui::live_socket::{self, LiveMessage};

/// `(remote name, session id)`.
pub(crate) type RemoteKey = (String, String);

pub(crate) enum PreviewCommand {
    Watch {
        key: RemoteKey,
        endpoint: DaemonEndpoint,
        lines: usize,
    },
    Stop,
    Input(Vec<u8>),
    /// Claim the size-owner lock and size the pane to the preview.
    TakeOver {
        cols: u16,
        rows: u16,
    },
    Resize {
        cols: u16,
        rows: u16,
    },
    /// One wheel notch at a 0-based pane cell, for a pane this viewer only
    /// watches; the daemon ignores it unless the pane is full-screen.
    Wheel {
        up: bool,
        col: u16,
        row: u16,
    },
    /// Capture `lines` of history plus screen; `fast` while at the live edge.
    Window {
        lines: usize,
        fast: bool,
    },
}

#[derive(Debug)]
pub(crate) enum PreviewEvent {
    Frame {
        key: RemoteKey,
        content: String,
        cursor: crate::tmux::PaneCursor,
    },
    SizeOwner {
        key: RemoteKey,
        is_owner: bool,
    },
    Closed {
        key: RemoteKey,
        reason: String,
    },
}

/// Worker thread owning at most one live socket at a time.
pub struct RemotePreview {
    commands: mpsc::UnboundedSender<PreviewCommand>,
    events: std_mpsc::Receiver<PreviewEvent>,
}

impl RemotePreview {
    pub fn new() -> Self {
        let (commands, command_rx) = mpsc::unbounded_channel();
        let (event_tx, events) = std_mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("aoe-remote-preview".into())
            .spawn(move || {
                match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt.block_on(run(command_rx, event_tx)),
                    Err(e) => {
                        tracing::warn!(target: "tui.remote_preview", "runtime build failed: {e}")
                    }
                }
            });
        if let Err(e) = spawned {
            tracing::warn!(target: "tui.remote_preview", "worker spawn failed: {e}");
        }
        Self { commands, events }
    }

    /// A preview with no worker, whose commands land on the returned receiver.
    #[cfg(test)]
    pub(crate) fn recording() -> (Self, mpsc::UnboundedReceiver<PreviewCommand>) {
        let (commands, recorded) = mpsc::unbounded_channel();
        let (_, events) = std_mpsc::channel();
        (Self { commands, events }, recorded)
    }

    pub(crate) fn send(&self, command: PreviewCommand) {
        let _ = self.commands.send(command);
    }

    pub(crate) fn try_recv(&self) -> Option<PreviewEvent> {
        self.events.try_recv().ok()
    }
}

impl Default for RemotePreview {
    fn default() -> Self {
        Self::new()
    }
}

/// Whether typed bytes may reach the pane. The daemon drops input from a
/// client that does not hold the size-owner lock, so bytes typed between a
/// take-over and its grant wait here and go out in order once it lands.
#[derive(Debug, Default, PartialEq)]
enum InputGate {
    #[default]
    Viewer,
    Claiming(Vec<Vec<u8>>),
    Owner,
}

impl InputGate {
    fn claim(&mut self) {
        if !matches!(self, Self::Claiming(_)) {
            *self = Self::Claiming(Vec::new());
        }
    }

    /// Bytes to send now, if any; a viewer's input goes nowhere.
    fn input(&mut self, bytes: Vec<u8>) -> Option<Vec<u8>> {
        match self {
            Self::Owner => Some(bytes),
            Self::Claiming(buffered) => {
                buffered.push(bytes);
                None
            }
            Self::Viewer => None,
        }
    }

    /// Apply a `size_owner` notice; returns the held bytes a grant releases.
    fn ownership(&mut self, is_owner: bool) -> Vec<Vec<u8>> {
        match (std::mem::take(self), is_owner) {
            (Self::Claiming(buffered), true) => {
                *self = Self::Owner;
                buffered
            }
            (Self::Owner, true) => {
                *self = Self::Owner;
                Vec::new()
            }
            (Self::Viewer, true) | (_, false) => Vec::new(),
        }
    }
}

struct Connection {
    key: RemoteKey,
    tx: mpsc::Sender<Message>,
    task: tokio::task::JoinHandle<()>,
    gate: InputGate,
}

impl Connection {
    async fn control(&self, value: serde_json::Value) {
        let _ = self.tx.send(Message::Text(value.to_string().into())).await;
    }

    async fn size(&self, cols: u16, rows: u16) {
        self.control(json!({"type": "resize", "cols": cols.max(1), "rows": rows.max(1)}))
            .await;
    }

    async fn window(&self, lines: usize, fast: bool) {
        self.control(json!({"type": "window", "lines": lines.max(1)}))
            .await;
        self.control(json!({"type": "cadence", "fast": fast})).await;
    }

    fn close(self) {
        drop(self.tx);
        self.task.abort();
    }
}

async fn next_message(rx: &mut Option<mpsc::Receiver<LiveMessage>>) -> Option<LiveMessage> {
    match rx {
        Some(rx) => rx.recv().await,
        None => std::future::pending().await,
    }
}

/// A cursor sweeping across rows queues a Watch per row; only the last one (or
/// a later Stop) matters, and each stale connect would cost a round trip.
/// Commands after the first non-target one keep their order in `backlog`, so a
/// take-over or keystroke queued behind a Watch is never dropped.
fn latest_target(
    first: PreviewCommand,
    commands: &mut mpsc::UnboundedReceiver<PreviewCommand>,
    backlog: &mut VecDeque<PreviewCommand>,
) -> PreviewCommand {
    let mut latest = first;
    while let Ok(next) = commands.try_recv() {
        match next {
            PreviewCommand::Watch { .. } | PreviewCommand::Stop if backlog.is_empty() => {
                latest = next;
            }
            other => backlog.push_back(other),
        }
    }
    latest
}

async fn run(
    mut commands: mpsc::UnboundedReceiver<PreviewCommand>,
    events: std_mpsc::Sender<PreviewEvent>,
) {
    let mut conn: Option<Connection> = None;
    let mut rx: Option<mpsc::Receiver<LiveMessage>> = None;
    let mut backlog = VecDeque::new();
    loop {
        let command = match backlog.pop_front() {
            Some(command) => command,
            None => tokio::select! {
                command = commands.recv() => {
                    let Some(command) = command else { break };
                    command
                }
                message = next_message(&mut rx) => {
                    on_message(message, &mut conn, &mut rx, &events).await;
                    continue;
                }
            },
        };
        let command = match command {
            PreviewCommand::Watch { .. } | PreviewCommand::Stop => {
                latest_target(command, &mut commands, &mut backlog)
            }
            other => other,
        };
        match command {
            PreviewCommand::Watch {
                key,
                endpoint,
                lines,
            } => {
                if let Some(old) = conn.take() {
                    old.close();
                }
                rx = None;
                match live_socket::connect(&endpoint, &key.1).await {
                    Ok(socket) => {
                        let next = Connection {
                            key,
                            tx: socket.tx,
                            task: socket.task,
                            gate: InputGate::Viewer,
                        };
                        next.window(lines, true).await;
                        rx = Some(socket.rx);
                        conn = Some(next);
                    }
                    Err(e) => {
                        let _ = events.send(PreviewEvent::Closed {
                            key,
                            reason: format!("{e:#}"),
                        });
                    }
                }
            }
            PreviewCommand::Stop => {
                if let Some(old) = conn.take() {
                    old.close();
                }
                rx = None;
            }
            PreviewCommand::Input(bytes) => {
                if let Some(c) = &mut conn {
                    if let Some(bytes) = c.gate.input(bytes) {
                        let _ = c.tx.send(Message::Binary(bytes.into())).await;
                    }
                }
            }
            PreviewCommand::TakeOver { cols, rows } => {
                if let Some(c) = &mut conn {
                    c.gate.claim();
                    c.control(json!({"type": "claim"})).await;
                    c.size(cols, rows).await;
                }
            }
            PreviewCommand::Resize { cols, rows } => {
                if let Some(c) = &conn {
                    c.size(cols, rows).await;
                }
            }
            PreviewCommand::Wheel { up, col, row } => {
                if let Some(c) = &conn {
                    c.control(json!({"type": "wheel", "up": up, "col": col, "row": row}))
                        .await;
                }
            }
            PreviewCommand::Window { lines, fast } => {
                if let Some(c) = &conn {
                    c.window(lines, fast).await;
                }
            }
        }
    }
}

async fn on_message(
    message: Option<LiveMessage>,
    conn: &mut Option<Connection>,
    rx: &mut Option<mpsc::Receiver<LiveMessage>>,
    events: &std_mpsc::Sender<PreviewEvent>,
) {
    let Some(c) = conn.as_mut() else {
        *rx = None;
        return;
    };
    let key = c.key.clone();
    let closed = match message {
        Some(LiveMessage::Frame { content, cursor }) => {
            let _ = events.send(PreviewEvent::Frame {
                key,
                content,
                cursor,
            });
            None
        }
        Some(LiveMessage::SizeOwner(is_owner)) => {
            for bytes in c.gate.ownership(is_owner) {
                let _ = c.tx.send(Message::Binary(bytes.into())).await;
            }
            let _ = events.send(PreviewEvent::SizeOwner { key, is_owner });
            None
        }
        Some(LiveMessage::Closed(reason)) => Some((key, reason)),
        None => Some((key, "the live terminal stream ended".to_string())),
    };
    if let Some((key, reason)) = closed {
        if let Some(old) = conn.take() {
            old.close();
        }
        *rx = None;
        let _ = events.send(PreviewEvent::Closed { key, reason });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_waits_for_the_ownership_grant_and_leaves_in_order() {
        let mut gate = InputGate::default();
        assert_eq!(
            gate.input(b"ignored".to_vec()),
            None,
            "a viewer cannot type"
        );

        gate.claim();
        assert_eq!(gate.input(b"a".to_vec()), None);
        assert_eq!(gate.input(b"b".to_vec()), None);
        assert_eq!(gate.ownership(true), [b"a".to_vec(), b"b".to_vec()]);
        assert_eq!(gate.input(b"c".to_vec()), Some(b"c".to_vec()));
        assert!(
            gate.ownership(true).is_empty(),
            "a repeat grant flushes nothing"
        );

        gate.claim();
        gate.input(b"lost".to_vec());
        assert!(
            gate.ownership(false).is_empty(),
            "a refusal drops held input"
        );
        assert_eq!(gate, InputGate::Viewer);
    }

    #[test]
    fn a_take_over_queued_behind_a_watch_is_kept() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        tx.send(PreviewCommand::Stop).unwrap();
        tx.send(PreviewCommand::TakeOver { cols: 80, rows: 24 })
            .unwrap();
        tx.send(PreviewCommand::Input(b"x".to_vec())).unwrap();
        tx.send(PreviewCommand::Stop).unwrap();
        let mut backlog = VecDeque::new();
        assert!(matches!(
            latest_target(PreviewCommand::Stop, &mut rx, &mut backlog),
            PreviewCommand::Stop
        ));
        let kinds: Vec<_> = backlog
            .iter()
            .map(|c| match c {
                PreviewCommand::TakeOver { .. } => "take-over",
                PreviewCommand::Input(_) => "input",
                PreviewCommand::Stop => "stop",
                _ => "other",
            })
            .collect();
        assert_eq!(kinds, ["take-over", "input", "stop"]);
    }
}
