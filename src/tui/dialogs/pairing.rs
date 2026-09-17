//! Pair-a-device panel over the serve view: mints a one-time code on the local
//! daemon and lists the devices already paired, with revoke.

use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::prelude::*;
use ratatui::widgets::*;
use serde::Deserialize;
use tokio::sync::mpsc;

use crate::tui::styles::Theme;

const CONFIRM_WINDOW: Duration = Duration::from_secs(3);
const MAX_DEVICE_ROWS: usize = 5;

enum CodeState {
    Minting,
    Ready { code: String, expires_at: Instant },
    Failed(String),
}

#[derive(Clone, Deserialize)]
struct PairedDevice {
    session_id: String,
    device_name: Option<String>,
    created_ip: String,
    last_seen: chrono::DateTime<chrono::Utc>,
}

#[derive(Deserialize)]
struct Minted {
    code: String,
    expires_in_secs: u64,
}

enum Update {
    Minted(Result<Minted, String>),
    Devices(Result<Vec<PairedDevice>, String>),
    Revoked(Result<(), String>),
}

pub(super) enum PanelAction {
    Continue,
    Close,
}

pub(super) struct PairingPanel {
    code: CodeState,
    /// `None` until the first listing arrives.
    devices: Option<Result<Vec<PairedDevice>, String>>,
    selected: usize,
    confirm_revoke: Option<(String, Instant)>,
    runtime: Option<tokio::runtime::Handle>,
    sender: mpsc::UnboundedSender<Update>,
    updates: mpsc::UnboundedReceiver<Update>,
    drawn_secs: Option<u64>,
}

async fn local_client() -> Result<crate::daemon::DaemonClient, String> {
    crate::acp::client::discovery::discover_local()
        .map_err(|e| e.to_string())?
        .daemon_client()
        .map_err(|e| e.to_string())
}

impl PairingPanel {
    /// Open the panel and start minting a code and listing devices.
    pub(super) fn open() -> Self {
        let mut panel = Self::detached(tokio::runtime::Handle::try_current().ok());
        panel.mint();
        panel.refresh_devices();
        panel
    }

    fn detached(runtime: Option<tokio::runtime::Handle>) -> Self {
        let (sender, updates) = mpsc::unbounded_channel();
        Self {
            code: CodeState::Minting,
            devices: None,
            selected: 0,
            confirm_revoke: None,
            runtime,
            sender,
            updates,
            drawn_secs: None,
        }
    }

    fn spawn(&self, work: impl std::future::Future<Output = Update> + Send + 'static) {
        let Some(runtime) = &self.runtime else {
            return;
        };
        let sender = self.sender.clone();
        runtime.spawn(async move {
            let _ = sender.send(work.await);
        });
    }

    fn mint(&mut self) {
        if self.runtime.is_none() {
            self.code = CodeState::Failed("No async runtime to reach the daemon.".into());
            return;
        }
        self.code = CodeState::Minting;
        self.spawn(async {
            let result = async {
                local_client()
                    .await?
                    .post_api::<_, Minted>("pair/codes", &serde_json::json!({}))
                    .await
                    .map_err(|e| e.to_string())
            };
            Update::Minted(result.await)
        });
    }

    fn refresh_devices(&mut self) {
        self.spawn(async {
            let result = async {
                local_client()
                    .await?
                    .get_api::<Vec<PairedDevice>>("devices", &[])
                    .await
                    .map_err(|e| e.to_string())
            };
            Update::Devices(result.await.map(|all| {
                all.into_iter()
                    .filter(|d| d.device_name.is_some())
                    .collect()
            }))
        });
    }

    fn revoke(&mut self, session_id: String) {
        self.spawn(async move {
            let result = async {
                local_client()
                    .await?
                    .delete_api(&format!("login/sessions/{session_id}"))
                    .await
                    .map_err(|e| e.to_string())
            };
            Update::Revoked(result.await)
        });
    }

    fn device_list(&self) -> &[PairedDevice] {
        match &self.devices {
            Some(Ok(devices)) => devices,
            _ => &[],
        }
    }

    fn apply(&mut self, update: Update) {
        match update {
            Update::Minted(Ok(minted)) => {
                self.code = CodeState::Ready {
                    code: minted.code,
                    expires_at: Instant::now() + Duration::from_secs(minted.expires_in_secs),
                };
            }
            Update::Minted(Err(error)) => self.code = CodeState::Failed(error),
            Update::Devices(devices) => {
                self.devices = Some(devices);
                self.selected = self
                    .selected
                    .min(self.device_list().len().saturating_sub(1));
            }
            Update::Revoked(Ok(())) => self.refresh_devices(),
            Update::Revoked(Err(error)) => self.devices = Some(Err(error)),
        }
    }

    pub(super) fn handle_key(&mut self, key: KeyEvent) -> PanelAction {
        let confirmed = self
            .confirm_revoke
            .take()
            .filter(|(_, at)| at.elapsed() <= CONFIRM_WINDOW)
            .map(|(id, _)| id);
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => return PanelAction::Close,
            KeyCode::Char('n') | KeyCode::Char('N') => self.mint(),
            KeyCode::Up | KeyCode::Char('k') => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => {
                let last = self.device_list().len().saturating_sub(1);
                self.selected = (self.selected + 1).min(last);
            }
            KeyCode::Char('x') | KeyCode::Char('X') => {
                if let Some(device) = self.device_list().get(self.selected) {
                    let id = device.session_id.clone();
                    if confirmed.as_deref() == Some(id.as_str()) {
                        self.revoke(id);
                    } else {
                        self.confirm_revoke = Some((id, Instant::now()));
                    }
                }
            }
            _ => {}
        }
        PanelAction::Continue
    }

    /// Drain background results and advance the countdown. Returns true when
    /// a redraw is needed.
    pub(super) fn tick(&mut self) -> bool {
        let mut changed = false;
        while let Ok(update) = self.updates.try_recv() {
            self.apply(update);
            changed = true;
        }
        if self
            .confirm_revoke
            .as_ref()
            .is_some_and(|(_, at)| at.elapsed() > CONFIRM_WINDOW)
        {
            self.confirm_revoke = None;
            changed = true;
        }
        let secs = match &self.code {
            CodeState::Ready { expires_at, .. } => Some(
                expires_at
                    .saturating_duration_since(Instant::now())
                    .as_secs(),
            ),
            _ => None,
        };
        if secs != self.drawn_secs {
            self.drawn_secs = secs;
            changed = true;
        }
        changed
    }

    /// `command` is the `aoe remote add` line for the other machine, when
    /// the daemon has a non-loopback URL.
    pub(super) fn render(
        &self,
        frame: &mut Frame,
        area: Rect,
        theme: &Theme,
        command: Option<&str>,
    ) {
        let lines = self.lines(theme, command);
        let width = 72.min(area.width.saturating_sub(4));
        let inner_width = width.saturating_sub(4).max(1) as usize;
        let rows: usize = lines
            .iter()
            .map(|line| line.width().max(1).div_ceil(inner_width))
            .sum();
        let height = (rows as u16 + 2).min(area.height.saturating_sub(2));
        let dialog = Rect {
            x: area.x + area.width.saturating_sub(width) / 2,
            y: area.y + area.height.saturating_sub(height) / 2,
            width,
            height,
        };
        frame.render_widget(Clear, dialog);
        let block = Block::default()
            .style(Style::default().bg(theme.background))
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(theme.border))
            .padding(Padding::horizontal(1))
            .title(Line::styled(
                " Pair a device ",
                Style::default().fg(theme.accent).bold(),
            ));
        let inner = block.inner(dialog);
        frame.render_widget(block, dialog);
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
    }

    fn lines(&self, theme: &Theme, command: Option<&str>) -> Vec<Line<'static>> {
        let dimmed = Style::default().fg(theme.dimmed);
        let text = Style::default().fg(theme.text);
        let mut lines = vec![Line::from("")];
        match &self.code {
            CodeState::Minting => {
                lines.push(Line::styled("creating code...", dimmed).centered());
                lines.push(Line::from(""));
            }
            CodeState::Ready { code, expires_at } => {
                lines.push(
                    Line::styled(spaced(code), Style::default().fg(theme.accent).bold()).centered(),
                );
                let remaining = expires_at.saturating_duration_since(Instant::now());
                lines.push(if remaining.is_zero() {
                    Line::styled(
                        "expired; press N for a new code",
                        Style::default().fg(theme.waiting),
                    )
                    .centered()
                } else {
                    Line::styled(
                        format!("single use, expires in {}", countdown(remaining)),
                        dimmed,
                    )
                    .centered()
                });
            }
            CodeState::Failed(error) => {
                lines.push(
                    Line::styled(
                        format!("Could not create a code: {error}"),
                        Style::default().fg(theme.error),
                    )
                    .centered(),
                );
                lines.push(Line::from(""));
            }
        }
        lines.push(Line::from(""));
        match command {
            Some(command) => {
                lines.push(Line::styled("On the other machine run", dimmed));
                lines.push(Line::styled(format!("  {command}"), text));
                lines.push(Line::styled("and enter the code when asked.", dimmed));
            }
            None => lines.push(Line::styled(
                "Expose this daemon (E) so another machine can reach it.",
                dimmed,
            )),
        }
        lines.push(Line::from(""));
        lines.push(Line::styled(
            "Paired devices",
            Style::default().fg(theme.accent).bold(),
        ));
        match &self.devices {
            None => lines.push(Line::styled("  loading...", dimmed)),
            Some(Err(error)) => lines.push(Line::styled(
                format!("  {error}"),
                Style::default().fg(theme.error),
            )),
            Some(Ok(devices)) if devices.is_empty() => {
                lines.push(Line::styled("  none yet", dimmed))
            }
            Some(Ok(devices)) => {
                let now = chrono::Utc::now();
                let start = self.selected.saturating_sub(MAX_DEVICE_ROWS - 1);
                for (index, device) in devices.iter().enumerate().skip(start).take(MAX_DEVICE_ROWS)
                {
                    let selected = index == self.selected;
                    let seen = (now - device.last_seen).num_seconds().max(0) as u64;
                    lines.push(Line::from(vec![
                        Span::styled(
                            if selected { "\u{25b8} " } else { "  " },
                            Style::default().fg(theme.accent),
                        ),
                        Span::styled(
                            device.device_name.clone().unwrap_or_default(),
                            if selected { text.bold() } else { text },
                        ),
                        Span::styled(
                            format!("  {}  seen {} ago", device.created_ip, ago(seen)),
                            dimmed,
                        ),
                    ]));
                }
            }
        }
        lines.push(Line::from(""));
        let key = Style::default().fg(theme.accent);
        lines.push(match &self.confirm_revoke {
            Some(_) => Line::styled(
                "Press X again to revoke this device. Any other key cancels.",
                Style::default().fg(theme.waiting).bold(),
            ),
            None => Line::from(vec![
                Span::styled("N", key),
                Span::styled(": new code  ", dimmed),
                Span::styled("\u{2191}\u{2193}", key),
                Span::styled(": select  ", dimmed),
                Span::styled("X", key),
                Span::styled(": revoke  ", dimmed),
                Span::styled("Esc", key),
                Span::styled(": back", dimmed),
            ]),
        });
        lines
    }
}

/// `K7F-3QX` as `K 7 F - 3 Q X`, easier to read across a room.
fn spaced(code: &str) -> String {
    code.chars().map(String::from).collect::<Vec<_>>().join(" ")
}

fn countdown(remaining: Duration) -> String {
    let secs = remaining.as_secs();
    format!("{}:{:02}", secs / 60, secs % 60)
}

fn ago(secs: u64) -> String {
    match secs {
        0..60 => "<1m".to_string(),
        60..3600 => format!("{}m", secs / 60),
        3600..86400 => format!("{}h", secs / 3600),
        _ => format!("{}d", secs / 86400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn press(panel: &mut PairingPanel, code: KeyCode) -> PanelAction {
        panel.handle_key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn device(id: &str, name: &str) -> PairedDevice {
        PairedDevice {
            session_id: id.into(),
            device_name: Some(name.into()),
            created_ip: "192.168.1.9".into(),
            last_seen: chrono::Utc::now(),
        }
    }

    fn screen(panel: &PairingPanel, command: Option<&str>) -> String {
        let mut term = Terminal::new(TestBackend::new(80, 30)).unwrap();
        term.draw(|f| panel.render(f, f.area(), &Theme::default(), command))
            .unwrap();
        let buf = term.backend().buffer().clone();
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn shows_the_code_countdown_and_client_instruction() {
        let mut panel = PairingPanel::detached(None);
        panel.apply(Update::Minted(Ok(Minted {
            code: "K7F-3QX".into(),
            expires_in_secs: 600,
        })));
        panel.apply(Update::Devices(Ok(vec![device("s1", "laptop")])));
        let text = screen(
            &panel,
            Some("aoe remote add box http://192.168.1.5:8081 --insecure"),
        );
        assert!(text.contains("K 7 F - 3 Q X"), "{text}");
        assert!(text.contains("expires in 9:59") || text.contains("expires in 10:00"));
        assert!(text.contains("aoe remote add box http://192.168.1.5:8081 --insecure"));
        assert!(text.contains("laptop"), "{text}");
        assert!(tick_redraws_once(&mut panel));
    }

    fn tick_redraws_once(panel: &mut PairingPanel) -> bool {
        panel.tick() && !panel.tick()
    }

    #[test]
    fn revoke_needs_a_second_press_on_the_same_device() {
        let mut panel = PairingPanel::detached(None);
        panel.apply(Update::Devices(Ok(vec![
            device("s1", "laptop"),
            device("s2", "phone"),
        ])));
        press(&mut panel, KeyCode::Char('x'));
        assert_eq!(
            panel.confirm_revoke.as_ref().map(|(id, _)| id.as_str()),
            Some("s1")
        );
        press(&mut panel, KeyCode::Down);
        assert!(panel.confirm_revoke.is_none(), "any other key cancels");
        press(&mut panel, KeyCode::Char('x'));
        press(&mut panel, KeyCode::Char('x'));
        assert!(panel.confirm_revoke.is_none());
        assert_eq!(panel.selected, 1);
        assert!(matches!(
            press(&mut panel, KeyCode::Esc),
            PanelAction::Close
        ));
    }

    #[test]
    fn without_a_runtime_minting_fails_visibly() {
        let mut panel = PairingPanel::detached(None);
        press(&mut panel, KeyCode::Char('n'));
        assert!(screen(&panel, None).contains("Could not create a code"));
    }
}
