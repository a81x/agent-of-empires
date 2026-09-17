//! Serve dialog: chooses how far the local daemon is exposed. The daemon
//! always runs on this machine with at least a localhost listener; this view
//! switches between Localhost, Local network (0.0.0.0) and an HTTPS tunnel by
//! replacing it under the lifecycle transaction, and shows the QR, URL and
//! client command for exposed modes. It never turns the daemon off.

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent};
use rand::prelude::IndexedRandom;
use ratatui::prelude::*;
use ratatui::widgets::*;

use crate::cli::serve::{Exposure, ExposureRequest};
use crate::tui::styles::Theme;

/// Actions returned by [`ServeView::handle_key`], following the
/// full-page takeover pattern used by `SettingsAction` and `DiffAction`.
pub enum ServeAction {
    /// Keep the serve view open; no navigation change.
    Continue,
    /// Close the serve view and return to the home screen.
    Close,
}

/// Which HTTPS tunnel backend the user picked on the Confirm screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TunnelTransport {
    Tailscale,
    Cloudflare,
}

/// Per-transport readiness, evaluated when the Confirm screen opens.
/// Drives the card styling and whether the user can select that card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportStatus {
    /// Ready to spawn: CLI installed, logged in, and (for Tailscale)
    /// the `funnel` nodeAttr is granted.
    Ready,
    /// CLI is missing on PATH.
    NotInstalled,
    /// Tailscale-only: CLI + login OK but the ACL doesn't grant Funnel.
    /// User needs to visit login.tailscale.com/admin/acls/file.
    FunnelNotEnabled,
}

impl TransportStatus {
    fn is_ready(self) -> bool {
        matches!(self, TransportStatus::Ready)
    }
}

const EXPOSURES: [Exposure; 3] = [Exposure::Localhost, Exposure::Network, Exposure::Tunnel];

fn exposure_label(exposure: Exposure) -> &'static str {
    match exposure {
        Exposure::Localhost => "Localhost only",
        Exposure::Network => "Local network",
        Exposure::Tunnel => "Internet (HTTPS)",
    }
}

/// Transport of a running tunnel, from the mode the daemon recorded.
fn running_transport() -> TunnelTransport {
    let mode = crate::session::get_app_dir()
        .ok()
        .and_then(|dir| std::fs::read_to_string(dir.join("serve.mode")).ok());
    match mode.as_deref().map(str::trim) {
        Some("tunnel") => TunnelTransport::Cloudflare,
        _ => TunnelTransport::Tailscale,
    }
}

pub use crate::cli::serve::{read_serve_urls, ServeUrl};

/// Passphrase of the tunnel this TUI process started, so reopening the
/// dialog re-displays it instead of the "set at startup" placeholder.
static LAST_SPAWNED_PASSPHRASE: Mutex<Option<String>> = Mutex::new(None);

fn remember_passphrase(pp: &str) {
    if let Ok(mut guard) = LAST_SPAWNED_PASSPHRASE.lock() {
        *guard = Some(pp.to_string());
    }
}

fn recall_passphrase_in_memory() -> Option<String> {
    LAST_SPAWNED_PASSPHRASE.lock().ok()?.clone()
}

fn recall_passphrase() -> Option<String> {
    if let Some(pp) = recall_passphrase_in_memory() {
        tracing::debug!(target: "tui.dialog", "passphrase recalled from in-memory cache");
        return Some(pp);
    }
    // Durable saved passphrase (survives stop/start cycles).
    if let Some(pp) = load_saved_passphrase() {
        tracing::debug!(target: "tui.dialog", "passphrase recalled from serve.saved_passphrase");
        return Some(pp);
    }
    // Ephemeral file written by the server on startup. Lets the TUI
    // display the passphrase when the daemon was launched from the CLI.
    let dir = crate::session::get_app_dir().ok()?;
    let raw = std::fs::read_to_string(dir.join("serve.passphrase")).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        tracing::debug!(target: "tui.dialog", "passphrase recalled from serve.passphrase on disk");
        Some(trimmed.to_string())
    }
}

/// Load the durable saved passphrase that persists across daemon
/// stop/start cycles. Returns None if no saved passphrase exists.
fn load_saved_passphrase() -> Option<String> {
    let dir = crate::session::get_app_dir().ok()?;
    let raw = std::fs::read_to_string(dir.join("serve.saved_passphrase")).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Persist a passphrase to the durable file that survives daemon
/// stop/start cycles. Written with owner-only permissions.
fn save_passphrase_to_disk(pp: &str) {
    if let Ok(dir) = crate::session::get_app_dir() {
        #[cfg(unix)]
        {
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;
            if let Ok(mut file) = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(dir.join("serve.saved_passphrase"))
            {
                let _ = file.write_all(pp.as_bytes());
            }
        }
        #[cfg(not(unix))]
        {
            let _ = std::fs::write(dir.join("serve.saved_passphrase"), pp);
        }
    }
}

/// Load the saved passphrase if one exists, otherwise generate a
/// fresh random one and save it for future launches.
fn load_or_generate_passphrase() -> String {
    if let Some(pp) = load_saved_passphrase() {
        return pp;
    }
    let pp = generate_passphrase();
    save_passphrase_to_disk(&pp);
    pp
}

/// How long an exposed daemon may take to publish its URL after it is ready.
const URL_PUBLISH_TIMEOUT: Duration = Duration::from_secs(10);
/// Log lines kept for a failure report.
const LOG_TAIL_LINES: usize = 200;
/// How long a transient flash stays up.
const FLASH_TTL: Duration = Duration::from_millis(1500);

pub enum ServeViewState {
    /// Where the running daemon is reachable, and the exposure to switch to.
    Picker {
        selected: Exposure,
        /// `None` while no daemon answers (the TUI is reconnecting).
        current: Option<Exposure>,
        /// Localhost URL, when the daemon published one.
        local_url: Option<String>,
        /// Either tailscale OR cloudflared is available.
        tunnel_available: bool,
        /// First non-loopback interface, when there is one.
        network_address: Option<String>,
        flash: Option<(String, Instant)>,
    },
    /// Tunnel-only: risk explanation and transport picker on one screen.
    Confirm {
        selected: TunnelTransport,
        tailscale: TransportStatus,
        cloudflare: TransportStatus,
        flash: Option<(String, Instant)>,
    },
    /// The daemon is being replaced with the `target` exposure.
    Applying {
        target: Exposure,
        transport: Option<TunnelTransport>,
        passphrase: Option<String>,
        started_at: Instant,
        /// `None` once the replacement reported success.
        result: Option<tokio::sync::oneshot::Receiver<Result<(), String>>>,
        ready_at: Option<Instant>,
    },
    /// The daemon is exposed beyond localhost.
    Active {
        mode: Exposure,
        transport: Option<TunnelTransport>,
        urls: Vec<ServeUrl>,
        /// Which `urls` entry is the primary QR target; Tab cycles.
        url_index: usize,
        /// Unknown for a tunnel started outside this TUI without a saved passphrase.
        passphrase: Option<String>,
        opened_at: Instant,
    },
    Error(String),
}

/// A destructive action awaiting confirmation (press the key again).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingConfirm {
    /// Awaiting a second `[G]` press to generate a new passphrase and restart.
    NewPassphrase,
    /// Awaiting a second `[R]` press to restart, clearing tunnel sessions.
    Restart,
}

pub struct ServeView {
    state: ServeViewState,
    /// Passphrase used when the user picks Tunnel. Loaded from
    /// `serve.saved_passphrase` if available, otherwise generated and saved.
    pending_passphrase: String,
    /// Destructive action awaiting a second keypress to confirm.
    pending_confirm: Option<(PendingConfirm, Instant)>,
    show_help: bool,
}

impl Default for ServeView {
    fn default() -> Self {
        Self::new()
    }
}

impl ServeView {
    /// Open on the exposed view when the daemon is reachable beyond
    /// localhost, otherwise on the exposure picker.
    pub fn new() -> Self {
        let mut view = Self {
            state: ServeViewState::Error(String::new()),
            pending_passphrase: load_or_generate_passphrase(),
            pending_confirm: None,
            show_help: false,
        };
        match crate::cli::serve::current_exposure() {
            Some(mode @ (Exposure::Network | Exposure::Tunnel)) => view.show_active(mode),
            current => view.show_picker(current, None),
        }
        view
    }

    fn show_picker(&mut self, current: Option<Exposure>, flash: Option<&str>) {
        let tunnel_available = crate::server::tunnel::tailscale_available_sync()
            || crate::server::tunnel::check_cloudflared().is_ok();
        let network_address =
            crate::server::discover_tagged_ips()
                .into_iter()
                .next()
                .map(|(kind, ip)| match kind {
                    crate::server::IpKind::Tailscale => format!("{ip} (Tailscale)"),
                    crate::server::IpKind::Lan => format!("{ip} (LAN)"),
                    crate::server::IpKind::Loopback => format!("{ip} (loopback)"),
                });
        let local_url = matches!(current, Some(Exposure::Localhost))
            .then(read_serve_urls)
            .and_then(|urls| urls.into_iter().next())
            .map(|url| url.url);
        self.state = ServeViewState::Picker {
            selected: current.unwrap_or(Exposure::Localhost),
            current,
            local_url,
            tunnel_available,
            network_address,
            flash: flash.map(|message| (message.to_string(), Instant::now())),
        };
        self.pending_confirm = None;
        self.show_help = false;
    }

    fn show_active(&mut self, mode: Exposure) {
        let transport = matches!(mode, Exposure::Tunnel).then(running_transport);
        let passphrase = matches!(mode, Exposure::Tunnel)
            .then(recall_passphrase)
            .flatten();
        self.state = ServeViewState::Active {
            mode,
            transport,
            urls: read_serve_urls(),
            url_index: 0,
            passphrase,
            opened_at: Instant::now(),
        };
        self.pending_confirm = None;
        self.show_help = false;
    }

    /// Probe tunnel readiness on entering Confirm or pressing `[R]`
    /// after fixing an ACL.
    fn assess_transports() -> (TransportStatus, TransportStatus) {
        let tailscale = if !crate::server::tunnel::tailscale_available_sync() {
            TransportStatus::NotInstalled
        } else if !crate::server::tunnel::tailscale_funnel_cap_ready_sync() {
            TransportStatus::FunnelNotEnabled
        } else {
            TransportStatus::Ready
        };
        let cloudflare = if crate::server::tunnel::check_cloudflared().is_ok() {
            TransportStatus::Ready
        } else {
            TransportStatus::NotInstalled
        };
        (tailscale, cloudflare)
    }

    /// A Ready Tailscale beats Cloudflare (stable URL); else whichever is
    /// Ready; else Tailscale so the user sees the fix instructions.
    fn default_transport(
        tailscale: TransportStatus,
        cloudflare: TransportStatus,
    ) -> TunnelTransport {
        match (tailscale, cloudflare) {
            (TransportStatus::Ready, _) => TunnelTransport::Tailscale,
            (_, TransportStatus::Ready) => TunnelTransport::Cloudflare,
            _ => TunnelTransport::Tailscale,
        }
    }

    /// Replace the daemon in the background; `tick` follows the result.
    fn apply(
        &mut self,
        target: Exposure,
        transport: Option<TunnelTransport>,
        passphrase: Option<String>,
    ) {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            self.state = ServeViewState::Error("No async runtime to change the exposure.".into());
            return;
        };
        let request = match target {
            Exposure::Tunnel => ExposureRequest::Tunnel {
                cloudflare: transport == Some(TunnelTransport::Cloudflare),
                passphrase: passphrase
                    .clone()
                    .unwrap_or_else(|| self.pending_passphrase.clone()),
            },
            Exposure::Network => ExposureRequest::Network,
            Exposure::Localhost => ExposureRequest::Localhost,
        };
        if let Some(passphrase) = &passphrase {
            remember_passphrase(passphrase);
            save_passphrase_to_disk(passphrase);
        }
        let (sender, result) = tokio::sync::oneshot::channel();
        runtime.spawn(async move {
            let outcome = crate::cli::serve::change_exposure(request)
                .await
                .map_err(|error| format!("{error:#}"));
            let _ = sender.send(outcome);
        });
        self.state = ServeViewState::Applying {
            target,
            transport,
            passphrase,
            started_at: Instant::now(),
            result: Some(result),
            ready_at: None,
        };
        self.pending_confirm = None;
        self.show_help = false;
    }

    /// Act on a picked exposure: apply it, or open the tunnel confirmation.
    fn choose(&mut self, target: Exposure) -> ServeAction {
        let ServeViewState::Picker {
            current,
            tunnel_available,
            network_address,
            flash,
            ..
        } = &mut self.state
        else {
            return ServeAction::Continue;
        };
        let refusal = match target {
            _ if *current == Some(target) && target == Exposure::Localhost => {
                Some("Already reachable from this machine only.")
            }
            Exposure::Network if network_address.is_none() => {
                Some("No non-loopback network interface available.")
            }
            Exposure::Tunnel if !*tunnel_available => {
                Some("Install tailscale or cloudflared to enable Tunnel mode.")
            }
            _ => None,
        };
        if let Some(message) = refusal {
            *flash = Some((message.to_string(), Instant::now()));
            return ServeAction::Continue;
        }
        if *current == Some(target) {
            self.show_active(target);
            return ServeAction::Continue;
        }
        match target {
            Exposure::Tunnel => {
                let (tailscale, cloudflare) = Self::assess_transports();
                self.state = ServeViewState::Confirm {
                    selected: Self::default_transport(tailscale, cloudflare),
                    tailscale,
                    cloudflare,
                    flash: None,
                };
            }
            target => self.apply(target, None, None),
        }
        ServeAction::Continue
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> ServeAction {
        match &mut self.state {
            ServeViewState::Picker {
                selected, flash, ..
            } => {
                if flash
                    .as_ref()
                    .is_some_and(|(_, at)| at.elapsed() > FLASH_TTL)
                {
                    *flash = None;
                }
                let index = EXPOSURES.iter().position(|e| e == selected).unwrap_or(0);
                match key.code {
                    KeyCode::Up | KeyCode::Char('k') => {
                        *selected = EXPOSURES[index.saturating_sub(1)];
                        ServeAction::Continue
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        *selected = EXPOSURES[(index + 1).min(EXPOSURES.len() - 1)];
                        ServeAction::Continue
                    }
                    KeyCode::Tab => {
                        *selected = EXPOSURES[(index + 1) % EXPOSURES.len()];
                        ServeAction::Continue
                    }
                    KeyCode::Char(digit @ '1'..='3') => {
                        let target = EXPOSURES[digit as usize - '1' as usize];
                        *selected = target;
                        self.choose(target)
                    }
                    KeyCode::Enter => {
                        let target = *selected;
                        self.choose(target)
                    }
                    KeyCode::Esc | KeyCode::Char('q') => ServeAction::Close,
                    _ => ServeAction::Continue,
                }
            }
            ServeViewState::Confirm {
                selected,
                tailscale,
                cloudflare,
                flash,
            } => {
                if flash
                    .as_ref()
                    .is_some_and(|(_, at)| at.elapsed() > FLASH_TTL)
                {
                    *flash = None;
                }
                let commit = |dialog: &mut ServeView, pick: TunnelTransport| -> ServeAction {
                    let ServeViewState::Confirm {
                        tailscale,
                        cloudflare,
                        flash,
                        ..
                    } = &mut dialog.state
                    else {
                        return ServeAction::Continue;
                    };
                    let status = match pick {
                        TunnelTransport::Tailscale => *tailscale,
                        TunnelTransport::Cloudflare => *cloudflare,
                    };
                    if !status.is_ready() {
                        let message = match (pick, status) {
                            (TunnelTransport::Tailscale, TransportStatus::FunnelNotEnabled) => {
                                "Tailscale Funnel isn't enabled for this node; pick Cloudflare or update your ACL."
                            }
                            (TunnelTransport::Tailscale, _) => {
                                "Tailscale isn't installed; pick Cloudflare."
                            }
                            (TunnelTransport::Cloudflare, _) => {
                                "cloudflared isn't installed; pick Tailscale."
                            }
                        };
                        *flash = Some((message.to_string(), Instant::now()));
                        return ServeAction::Continue;
                    }
                    let passphrase = dialog.pending_passphrase.clone();
                    dialog.apply(Exposure::Tunnel, Some(pick), Some(passphrase));
                    ServeAction::Continue
                };
                match key.code {
                    KeyCode::Left | KeyCode::Char('h') => {
                        *selected = TunnelTransport::Tailscale;
                        ServeAction::Continue
                    }
                    KeyCode::Right | KeyCode::Char('l') => {
                        *selected = TunnelTransport::Cloudflare;
                        ServeAction::Continue
                    }
                    KeyCode::Tab => {
                        *selected = match *selected {
                            TunnelTransport::Tailscale => TunnelTransport::Cloudflare,
                            TunnelTransport::Cloudflare => TunnelTransport::Tailscale,
                        };
                        ServeAction::Continue
                    }
                    KeyCode::Char('t') | KeyCode::Char('T') => {
                        commit(self, TunnelTransport::Tailscale)
                    }
                    KeyCode::Char('c') | KeyCode::Char('C') => {
                        commit(self, TunnelTransport::Cloudflare)
                    }
                    KeyCode::Enter => {
                        let pick = *selected;
                        commit(self, pick)
                    }
                    KeyCode::Char('r') | KeyCode::Char('R') => {
                        let (new_tailscale, new_cloudflare) = ServeView::assess_transports();
                        *tailscale = new_tailscale;
                        *cloudflare = new_cloudflare;
                        *flash = Some(("Refreshed.".to_string(), Instant::now()));
                        ServeAction::Continue
                    }
                    KeyCode::Esc | KeyCode::Char('q') => {
                        self.show_picker(crate::cli::serve::current_exposure(), None);
                        ServeAction::Continue
                    }
                    _ => ServeAction::Continue,
                }
            }
            // The change keeps going in the background after the view closes.
            ServeViewState::Applying { .. } => match key.code {
                KeyCode::Esc | KeyCode::Char('q') => ServeAction::Close,
                _ => ServeAction::Continue,
            },
            ServeViewState::Active {
                mode,
                transport,
                urls,
                url_index,
                passphrase,
                ..
            } => {
                if self.show_help {
                    self.show_help = false;
                    return ServeAction::Continue;
                }
                let confirmed = self
                    .pending_confirm
                    .take()
                    .filter(|(_, at)| at.elapsed() <= Duration::from_secs(3))
                    .map(|(action, _)| action)
                    .filter(|action| match action {
                        PendingConfirm::NewPassphrase => {
                            matches!(key.code, KeyCode::Char('g') | KeyCode::Char('G'))
                        }
                        PendingConfirm::Restart => {
                            matches!(key.code, KeyCode::Char('r') | KeyCode::Char('R'))
                        }
                    });
                let (mode, transport) = (*mode, *transport);
                match key.code {
                    KeyCode::Char('g') | KeyCode::Char('G') if mode == Exposure::Tunnel => {
                        if confirmed == Some(PendingConfirm::NewPassphrase) {
                            let passphrase = generate_passphrase();
                            self.pending_passphrase = passphrase.clone();
                            self.apply(mode, transport, Some(passphrase));
                        } else {
                            self.pending_confirm =
                                Some((PendingConfirm::NewPassphrase, Instant::now()));
                        }
                        ServeAction::Continue
                    }
                    KeyCode::Char('r') | KeyCode::Char('R') => {
                        if confirmed == Some(PendingConfirm::Restart) {
                            let passphrase = (mode == Exposure::Tunnel).then(|| {
                                passphrase
                                    .clone()
                                    .unwrap_or_else(|| self.pending_passphrase.clone())
                            });
                            self.apply(mode, transport, passphrase);
                        } else {
                            self.pending_confirm = Some((PendingConfirm::Restart, Instant::now()));
                        }
                        ServeAction::Continue
                    }
                    KeyCode::Char('e') | KeyCode::Char('E') => {
                        self.show_picker(Some(mode), None);
                        ServeAction::Continue
                    }
                    KeyCode::Tab if urls.len() > 1 => {
                        *url_index = (*url_index + 1) % urls.len();
                        ServeAction::Continue
                    }
                    KeyCode::Char('?') => {
                        self.show_help = true;
                        ServeAction::Continue
                    }
                    KeyCode::Esc | KeyCode::Char('q') => ServeAction::Close,
                    _ => ServeAction::Continue,
                }
            }
            ServeViewState::Error(msg) => match key.code {
                KeyCode::Char('r') | KeyCode::Char('R') if error_mentions_tailscale(msg) => {
                    // A stale funnel config commonly blocks port 443; resetting
                    // is safe even when no funnel is configured.
                    self.state = match run_tailscale_funnel_reset() {
                        Ok(()) => ServeViewState::Error(
                            "Ran `tailscale funnel reset`. The existing funnel \
                             config (if any) has been cleared.\n\n\
                             Close this dialog and press R to retry."
                                .to_string(),
                        ),
                        Err(e) => ServeViewState::Error(format!(
                            "`tailscale funnel reset` failed: {e}\n\n\
                             Try running it manually from a shell, then retry."
                        )),
                    };
                    ServeAction::Continue
                }
                KeyCode::Esc | KeyCode::Enter | KeyCode::Char(' ') | KeyCode::Char('q') => {
                    ServeAction::Close
                }
                _ => ServeAction::Continue,
            },
        }
    }

    /// Drive flashes, confirmations and the background exposure change.
    /// Returns true when a redraw is needed.
    pub fn tick(&mut self) -> bool {
        match &mut self.state {
            ServeViewState::Picker { flash, .. } | ServeViewState::Confirm { flash, .. } => {
                if flash
                    .as_ref()
                    .is_some_and(|(_, at)| at.elapsed() > FLASH_TTL)
                {
                    *flash = None;
                    return true;
                }
                false
            }
            ServeViewState::Applying {
                target,
                result,
                ready_at,
                ..
            } => {
                let target = *target;
                if let Some(receiver) = result {
                    match receiver.try_recv() {
                        Err(tokio::sync::oneshot::error::TryRecvError::Empty) => return false,
                        Ok(Ok(())) => {
                            *result = None;
                            *ready_at = Some(Instant::now());
                        }
                        Ok(Err(error)) => {
                            self.state = ServeViewState::Error(apply_error(target, &error));
                            return true;
                        }
                        Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                            self.state = ServeViewState::Error(
                                "The exposure change was interrupted.".to_string(),
                            );
                            return true;
                        }
                    }
                }
                if target == Exposure::Localhost {
                    self.show_picker(Some(Exposure::Localhost), Some("Now localhost only."));
                    return true;
                }
                if !read_serve_urls().is_empty() {
                    let ServeViewState::Applying {
                        transport,
                        passphrase,
                        ..
                    } = &mut self.state
                    else {
                        return false;
                    };
                    self.state = ServeViewState::Active {
                        mode: target,
                        transport: *transport,
                        urls: read_serve_urls(),
                        url_index: 0,
                        passphrase: passphrase.take(),
                        opened_at: Instant::now(),
                    };
                    return true;
                }
                if ready_at.is_some_and(|at| at.elapsed() > URL_PUBLISH_TIMEOUT) {
                    self.state = ServeViewState::Error(
                        "The daemon restarted but published no URL. Check `aoe serve --status`."
                            .to_string(),
                    );
                    return true;
                }
                false
            }
            ServeViewState::Active { .. } => {
                let expired = self
                    .pending_confirm
                    .as_ref()
                    .is_some_and(|(_, at)| at.elapsed() > Duration::from_secs(3));
                if expired {
                    self.pending_confirm = None;
                }
                expired
            }
            ServeViewState::Error(_) => false,
        }
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        match &self.state {
            ServeViewState::Picker {
                selected,
                current,
                local_url,
                tunnel_available,
                network_address,
                flash,
            } => render_picker(
                frame,
                area,
                theme,
                PickerModel {
                    selected: *selected,
                    current: *current,
                    local_url: local_url.as_deref(),
                    tunnel_available: *tunnel_available,
                    network_address: network_address.as_deref(),
                    flash: flash.as_ref().map(|(m, _)| m.as_str()),
                },
            ),
            ServeViewState::Confirm {
                selected,
                tailscale,
                cloudflare,
                flash,
            } => render_confirm(
                frame,
                area,
                theme,
                *selected,
                *tailscale,
                *cloudflare,
                flash.as_ref().map(|(m, _)| m.as_str()),
            ),
            ServeViewState::Applying {
                target, started_at, ..
            } => render_applying(frame, area, theme, *target, started_at.elapsed()),
            ServeViewState::Active {
                mode,
                urls,
                url_index,
                passphrase,
                opened_at,
                ..
            } => {
                render_active(
                    frame,
                    area,
                    theme,
                    *mode,
                    urls,
                    *url_index,
                    passphrase.as_deref(),
                    opened_at.elapsed(),
                    self.pending_confirm.as_ref().map(|(a, _)| *a),
                );
                if self.show_help {
                    render_help_overlay(frame, area, theme, *mode);
                }
            }
            ServeViewState::Error(msg) => render_error(frame, area, theme, msg),
        }
    }
}

/// The local daemon the TUI bootstraps, started when it is missing. Used by
/// explicit user actions that need the daemon API right away.
pub(crate) async fn start_local_daemon_and_wait(
) -> Result<crate::acp::client::DaemonEndpoint, String> {
    crate::acp::client::daemon_manager::ensure_local_daemon("")
        .await
        .map_err(|error| format!("{error:#}"))
}

fn apply_error(target: Exposure, error: &str) -> String {
    let tail = initial_log_tail();
    let hint = diagnose_daemon_exit(&tail.join("\n"), target);
    let compact: Vec<String> = tail.iter().map(|l| compact_log_line(l)).collect();
    let detail = if compact.is_empty() {
        String::new()
    } else {
        format!("\n\nLast log lines:\n{}", compact.join("\n"))
    };
    format!(
        "Could not switch to {}: {error}{hint}{detail}",
        exposure_label(target)
    )
}

/// Map a common errno string in the daemon log tail to a one-line hint,
/// prefixed with a blank line, or `""` when nothing is recognized.
fn diagnose_daemon_exit(log: &str, target: Exposure) -> &'static str {
    if log.contains("EADDRNOTAVAIL") || log.contains("Cannot assign requested address") {
        return match target {
            Exposure::Network => {
                "\n\nHint: the interface we tried to bind on went away. \
                 Is Tailscale still up?"
            }
            Exposure::Localhost | Exposure::Tunnel => "",
        };
    }
    if log.contains("EADDRINUSE") || log.contains("Address already in use") {
        return "\n\nHint: another process holds the daemon's port. \
                Free it, or delete serve.last_port in the app directory to pick a new one.";
    }
    if log.contains("Permission denied") {
        return "\n\nHint: permission denied on bind. Are you trying a \
                privileged port (<1024)? We normally pick a high port.";
    }
    ""
}

/// `aoe remote add` line for another machine, or `None` when `url` is
/// loopback-only. The token moves to `--token` so the stored URL stays clean.
fn client_command(url: &str, passphrase: Option<&str>, name: &str) -> Option<String> {
    let (base, token) = split_url_and_token(url);
    let base = base.trim_end_matches('/');
    let parsed = reqwest::Url::parse(base).ok()?;
    let host = parsed
        .host_str()?
        .trim_start_matches('[')
        .trim_end_matches(']');
    let loopback = host == "localhost"
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback());
    if loopback {
        return None;
    }
    let mut command = format!("aoe remote add {name} {base}");
    if let Some(token) = token {
        command.push_str(&format!(" --token {token}"));
    }
    if let Some(passphrase) = passphrase {
        let quoted = passphrase.replace('\'', "'\\''");
        command.push_str(&format!(" --passphrase '{quoted}'"));
    }
    if parsed.scheme() == "http" {
        command.push_str(" --insecure");
    }
    Some(command)
}

/// This machine's short hostname as a remote name: lowercase, `[a-z0-9-]`.
fn remote_name_for_host(hostname: &str) -> String {
    let short = hostname.split('.').next().unwrap_or_default();
    let name: String = short
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let name = name.trim_matches('-');
    if name.is_empty() {
        "aoe".to_string()
    } else {
        name.to_string()
    }
}

fn this_machine_remote_name() -> String {
    let hostname = nix::unistd::gethostname()
        .ok()
        .and_then(|name| name.into_string().ok())
        .unwrap_or_default();
    remote_name_for_host(&hostname)
}

fn log_file_path() -> Option<PathBuf> {
    crate::cli::serve::stdio_redirect_path().ok()
}

fn initial_log_tail() -> Vec<String> {
    let Some(path) = log_file_path() else {
        return Vec::new();
    };
    let Ok(contents) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let all: Vec<&str> = contents.lines().collect();
    // debug.log carries TUI + runner + daemon lines now that serve.log is
    // gone. Anchor the initial tail at the last [AOE_START_MARKER] (written
    // by `init_subscriber` for every process) so we show the current
    // daemon's run rather than mixed history. Falls back to the trailing
    // window when no marker is found.
    let anchor = all
        .iter()
        .rposition(|line| line.contains("[AOE_START_MARKER]"))
        .unwrap_or_else(|| all.len().saturating_sub(LOG_TAIL_LINES));
    let from = anchor.min(all.len());
    let window = &all[from..];
    let start = window.len().saturating_sub(LOG_TAIL_LINES);
    window[start..].iter().map(|s| s.to_string()).collect()
}

struct PickerModel<'a> {
    selected: Exposure,
    current: Option<Exposure>,
    local_url: Option<&'a str>,
    tunnel_available: bool,
    network_address: Option<&'a str>,
    flash: Option<&'a str>,
}

fn render_picker(frame: &mut Frame, area: Rect, theme: &Theme, model: PickerModel) {
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.accent))
        .title(Line::styled(
            " Remote Access ",
            Style::default().fg(theme.accent).bold(),
        ));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // status(4) + spacer + question + spacer + options(3 x 3) + flash + keys
    let content_height: u16 = 18;
    let max_width: u16 = 72;
    let width = max_width.min(inner.width.saturating_sub(2));
    let body = Rect {
        x: inner.x + inner.width.saturating_sub(width) / 2,
        y: inner.y + inner.height.saturating_sub(content_height) / 2,
        width,
        height: content_height.min(inner.height),
    };

    let dimmed = Style::default().fg(theme.dimmed);
    let text = Style::default().fg(theme.text);
    let (status, status_style) = match model.current {
        Some(exposure) => (exposure_label(exposure), Style::default().fg(theme.running)),
        None => (
            "Not reachable (reconnecting)",
            Style::default().fg(theme.error),
        ),
    };
    let heading = if model.current.is_some() {
        "aoe is running on this machine."
    } else {
        "aoe's daemon is not answering on this machine."
    };
    let mut lines = vec![
        Line::from(Span::styled(
            heading,
            Style::default().fg(theme.title).bold(),
        )),
        Line::from(vec![
            Span::styled("Exposure: ", dimmed),
            Span::styled(status, status_style.bold()),
        ]),
    ];
    // The token is what makes the URL usable, so split it rather than truncate.
    let accent = Style::default().fg(theme.accent);
    match model.local_url {
        Some(url) if url.chars().count() > width as usize => {
            let (base, token) = split_url_and_token(url);
            lines.push(Line::from(Span::styled(base, accent)));
            lines.push(Line::from(vec![
                Span::styled("token ", dimmed),
                Span::styled(token.unwrap_or_default().to_string(), accent),
            ]));
        }
        Some(url) => lines.extend([Line::from(Span::styled(url, accent)), Line::from("")]),
        None => lines.extend([Line::from(""), Line::from("")]),
    }
    lines.extend([
        Line::from(""),
        Line::from(Span::styled(
            "How should it be reachable?",
            Style::default().fg(theme.title).bold(),
        )),
        Line::from(""),
    ]);

    let network = model
        .network_address
        .map(|address| format!("{address}. Token auth, plain HTTP."))
        .unwrap_or_else(|| "No non-loopback interface available.".to_string());
    let tunnel = if model.tunnel_available {
        "Tailscale or Cloudflare. Token + passphrase."
    } else {
        "Install tailscale or cloudflared to enable."
    };
    let options = [
        (
            Exposure::Localhost,
            "This machine only. Token auth.".to_string(),
            true,
        ),
        (Exposure::Network, network, model.network_address.is_some()),
        (Exposure::Tunnel, tunnel.to_string(), model.tunnel_available),
    ];
    for (number, (exposure, description, available)) in options.into_iter().enumerate() {
        let selected = exposure == model.selected;
        let label_style = match (selected, available) {
            (true, true) => Style::default().fg(theme.accent).bold(),
            (_, false) => dimmed,
            (false, true) => text.bold(),
        };
        let mut label = vec![
            Span::styled(if selected { "\u{25B8} " } else { "  " }, label_style),
            Span::styled(format!("{} ", number + 1), dimmed),
            Span::styled(exposure_label(exposure), label_style),
        ];
        if model.current == Some(exposure) {
            label.push(Span::styled(
                "  current",
                Style::default().fg(theme.running),
            ));
        }
        lines.push(Line::from(label));
        lines.push(Line::from(Span::styled(
            truncate_to_width(&format!("    {description}"), width as usize),
            if available { text } else { dimmed },
        )));
        lines.push(Line::from(""));
    }
    lines.push(Line::from(Span::styled(
        model.flash.unwrap_or(""),
        Style::default().fg(theme.waiting).bold(),
    )));
    lines.push(Line::from(Span::styled(
        "[\u{2191}/\u{2193}] choose  [1-3] pick  [Enter] apply  [Esc] close",
        dimmed,
    )));
    frame.render_widget(Paragraph::new(lines), body);
}

fn truncate_to_width(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    let kept: String = text.chars().take(width.saturating_sub(1)).collect();
    format!("{kept}\u{2026}")
}

fn render_applying(
    frame: &mut Frame,
    area: Rect,
    theme: &Theme,
    target: Exposure,
    elapsed: Duration,
) {
    frame.render_widget(Clear, area);
    let (title, wait_line1, wait_line2) = match target {
        Exposure::Tunnel => (
            " Starting HTTPS tunnel... ",
            "Restarting the daemon behind a tunnel",
            "(first-time Tailscale cert provisioning can take 30\u{2013}60s).",
        ),
        Exposure::Network => (
            " Exposing on the local network... ",
            "Restarting the daemon on 0.0.0.0",
            "(usually a few seconds).",
        ),
        Exposure::Localhost => (
            " Returning to localhost... ",
            "Restarting the daemon on 127.0.0.1",
            "(usually a few seconds).",
        ),
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.border))
        .title(Line::styled(title, Style::default().fg(theme.title).bold()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let content_height: u16 = 5;
    let v_pad = inner.height.saturating_sub(content_height) / 2;
    let centered = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(v_pad),
            Constraint::Length(content_height),
            Constraint::Min(0),
        ])
        .split(inner);

    let banner = vec![
        Line::from(""),
        Line::from(Span::styled(wait_line1, Style::default().fg(theme.text))),
        Line::from(Span::styled(wait_line2, Style::default().fg(theme.text))),
        Line::from(""),
        Line::from(Span::styled(
            format!(
                "Elapsed: {}s    [Esc] close (the change continues)",
                elapsed.as_secs()
            ),
            Style::default().fg(theme.dimmed),
        )),
    ];
    frame.render_widget(
        Paragraph::new(banner).alignment(Alignment::Center),
        centered[1],
    );
}

fn render_confirm(
    frame: &mut Frame,
    area: Rect,
    theme: &Theme,
    selected: TunnelTransport,
    tailscale: TransportStatus,
    cloudflare: TransportStatus,
    flash: Option<&str>,
) {
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.accent))
        .title(Line::styled(
            " Expose to Internet? ",
            Style::default().fg(theme.accent).bold(),
        ));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Center content vertically and constrain width
    let content_height: u16 = 19; // risk(6) + picker(1) + cards(8) + flash + keybinds + margins
    let v_pad = inner.height.saturating_sub(content_height) / 2;
    let centered = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(v_pad),
            Constraint::Length(content_height),
            Constraint::Min(0),
        ])
        .split(inner);

    let max_w: u16 = 82;
    let h_pad = centered[1].width.saturating_sub(max_w) / 2;
    let h_centered = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(h_pad),
            Constraint::Length(max_w.min(centered[1].width)),
            Constraint::Min(0),
        ])
        .split(centered[1]);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(6), // risk explanation
            Constraint::Length(1), // "Pick a transport:"
            Constraint::Min(8),    // cards
            Constraint::Length(1), // flash
            Constraint::Length(1), // keybinds
        ])
        .split(h_centered[1]);

    // ── Risk explanation (compressed; picker below carries most of UI) ───
    let risk = vec![
        Line::from(Span::styled(
            "Your sessions become reachable from anywhere over HTTPS.",
            Style::default().fg(theme.text),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Two factors required to log in:",
            Style::default().fg(theme.title).bold(),
        )),
        Line::from(vec![
            Span::styled("  \u{2022} ", Style::default().fg(theme.running)),
            Span::styled(
                "token (in the URL / QR code)",
                Style::default().fg(theme.text),
            ),
        ]),
        Line::from(vec![
            Span::styled("  \u{2022} ", Style::default().fg(theme.running)),
            Span::styled(
                "passphrase (typed on the login page)",
                Style::default().fg(theme.text),
            ),
        ]),
        Line::from(Span::styled(
            "Don't share screenshots with BOTH. Press [E] for localhost when done.",
            Style::default().fg(theme.dimmed),
        )),
    ];
    frame.render_widget(Paragraph::new(risk), rows[0]);

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "Pick a transport:",
            Style::default().fg(theme.title).bold(),
        ))),
        rows[1],
    );

    // ── Transport cards ──────────────────────────────────────────────────
    let cards = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(50),
            Constraint::Length(1),
            Constraint::Percentage(50),
        ])
        .split(rows[2]);

    render_transport_card(
        frame,
        cards[0],
        theme,
        "Tailscale Funnel",
        &[
            "Stable URL across restarts",
            "PWA-friendly on phones",
            "https://<host>.<tailnet>.ts.net",
        ],
        tailscale,
        selected == TunnelTransport::Tailscale,
        /*is_tailscale=*/ true,
    );
    render_transport_card(
        frame,
        cards[2],
        theme,
        "Cloudflare Tunnel",
        &[
            "Works anywhere",
            "URL rotates each restart",
            "Not PWA-friendly",
        ],
        cloudflare,
        selected == TunnelTransport::Cloudflare,
        /*is_tailscale=*/ false,
    );

    // ── Flash ────────────────────────────────────────────────────────────
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            flash.unwrap_or(""),
            Style::default().fg(theme.error).bold(),
        )))
        .alignment(Alignment::Center),
        rows[3],
    );

    // ── Keybinds ─────────────────────────────────────────────────────────
    let keybinds =
        "[←/→] select  [T] Tailscale  [C] Cloudflare  [R] refresh  [Enter] confirm  [Esc] cancel";
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            keybinds,
            Style::default().fg(theme.dimmed),
        )))
        .alignment(Alignment::Center),
        rows[4],
    );
}

#[allow(clippy::too_many_arguments)]
fn render_transport_card(
    frame: &mut Frame,
    area: Rect,
    theme: &Theme,
    title: &str,
    body_lines: &[&str],
    status: TransportStatus,
    is_selected: bool,
    is_tailscale: bool,
) {
    let ready = status.is_ready();
    let (border, title_color, body_color) = if is_selected && ready {
        (theme.accent, theme.accent, theme.text)
    } else if !ready {
        (theme.dimmed, theme.dimmed, theme.dimmed)
    } else {
        (theme.border, theme.title, theme.text)
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border))
        .padding(Padding::horizontal(1))
        .title(Line::styled(
            format!(" {title} "),
            Style::default().fg(title_color).bold(),
        ));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines = vec![Line::from("")];
    for text in body_lines {
        lines.push(Line::from(Span::styled(
            *text,
            Style::default().fg(body_color),
        )));
    }
    lines.push(Line::from(""));

    let (status_icon, status_text, status_style) = match status {
        TransportStatus::Ready => (
            "\u{2713}",
            "Ready".to_string(),
            Style::default().fg(theme.running).bold(),
        ),
        TransportStatus::NotInstalled => (
            "\u{26A0}",
            if is_tailscale {
                "Not installed (tailscale up)".to_string()
            } else {
                "Not installed (brew install cloudflared)".to_string()
            },
            Style::default().fg(theme.dimmed),
        ),
        TransportStatus::FunnelNotEnabled => (
            "\u{26A0}",
            "Funnel not enabled for this node".to_string(),
            Style::default().fg(theme.error).bold(),
        ),
    };
    lines.push(Line::from(vec![
        Span::styled(format!("{status_icon} "), status_style),
        Span::styled(status_text, status_style),
    ]));

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
}

/// Shorten a tracing-formatted log line for the in-dialog tail pane.
///
/// Typical input:
///   `2026-04-19T23:43:44.609396Z  INFO agent_of_empires::server::tunnel: Warning: ...`
///
/// Output:
///   `INFO tunnel: Warning: ...`
///
/// Strips the ISO timestamp (the user can see the log is live), compresses
/// the fully-qualified module path down to its last segment, and keeps the
/// level so the user still sees WARN/ERROR when they matter. Leaves
/// non-tracing lines (e.g. stray stdout from `tailscale funnel`) untouched.
fn compact_log_line(raw: &str) -> String {
    let trimmed = raw.trim_end_matches('\n');
    // Detect the tracing prefix: "<ISO8601Z>  LEVEL module::path: message".
    // Require a YYYY-MM-DD-looking prefix rather than "first char is a
    // digit", so stray lines like "200 OK ..." pass through verbatim
    // instead of getting mis-parsed.
    if !looks_like_iso_year(trimmed) {
        return trimmed.to_string();
    }
    // Split off the timestamp (up to the first space after 'Z ').
    let rest = match trimmed.split_once("Z ") {
        Some((_, r)) => r.trim_start(),
        None => return trimmed.to_string(),
    };
    // Split level from the module::path: message remainder.
    let Some((level, after_level)) = rest.split_once(' ') else {
        return trimmed.to_string();
    };
    let after_level = after_level.trim_start();
    // Split "module::path: message" at the ": " that separates path from msg.
    let (path, message) = match after_level.split_once(": ") {
        Some((p, m)) => (p, m),
        None => return format!("{level} {after_level}"),
    };
    let short_path = path.rsplit("::").next().unwrap_or(path);
    format!("{level} {short_path}: {message}")
}

/// Does `s` start with `YYYY-MM-DD`? Fast path for tracing-formatted
/// lines without pulling in a full datetime parser.
fn looks_like_iso_year(s: &str) -> bool {
    let mut iter = s.chars();
    for _ in 0..4 {
        if !iter.next().is_some_and(|c| c.is_ascii_digit()) {
            return false;
        }
    }
    matches!(iter.next(), Some('-'))
}

/// The scannable block for `url`, as terminal rows. Empty without the
/// dashboard bundle: a phone that scanned it would reach no page.
#[cfg(feature = "web")]
fn render_qr(url: &str) -> String {
    use qrcode::render::unicode::Dense1x2;
    use qrcode::QrCode;

    match QrCode::new(url.as_bytes()) {
        Ok(code) => code
            .render::<Dense1x2>()
            .quiet_zone(true)
            .dark_color(Dense1x2::Dark)
            .light_color(Dense1x2::Light)
            .build(),
        Err(_) => String::from("(QR unavailable; use the URL below)"),
    }
}

#[cfg(not(feature = "web"))]
fn render_qr(_url: &str) -> String {
    String::new()
}

/// Shown in place of the QR when the dashboard bundle is not embedded.
const API_ONLY_NOTICE: &str =
    "No dashboard bundle in this build: the URL serves the REST API only, and a browser gets a 404.";

/// Rows [`API_ONLY_NOTICE`] needs once wrapped at a usable terminal width.
const API_ONLY_ROWS: u16 = 2;

#[allow(clippy::too_many_arguments)]
fn render_active(
    frame: &mut Frame,
    area: Rect,
    theme: &Theme,
    mode: Exposure,
    urls: &[ServeUrl],
    url_index: usize,
    passphrase: Option<&str>,
    elapsed: Duration,
    pending_confirm: Option<PendingConfirm>,
) {
    let Some(active_url) = urls.get(url_index).or_else(|| urls.first()) else {
        render_error(
            frame,
            area,
            theme,
            "Daemon started but no URL available yet.",
        );
        return;
    };
    let url = &active_url.url;
    let kind_label = active_url.label.as_deref();

    let full_url = url.as_str();
    let url_prefix = "URL: ";
    let full_url_len = url_prefix.chars().count() + full_url.chars().count();
    let (split_url, split_token) = split_url_and_token(full_url);
    let is_tunnel = mode == Exposure::Tunnel;
    let command = client_command(
        full_url,
        passphrase.filter(|_| is_tunnel),
        &this_machine_remote_name(),
    );

    frame.render_widget(Clear, area);

    let page = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // header
            Constraint::Min(10),   // content
            Constraint::Length(3), // footer
        ])
        .split(area);

    // ── Header ───────────────────────────────────────────────────────────
    let eight_hours = Duration::from_secs(8 * 3600);
    let title_color = if elapsed >= eight_hours {
        theme.waiting
    } else {
        theme.title
    };
    let header_block = Block::default()
        .borders(Borders::BOTTOM)
        .border_style(Style::default().fg(theme.border));
    let header_inner = header_block.inner(page[0]);
    frame.render_widget(header_block, page[0]);

    let mode_label = match mode {
        Exposure::Network => "local network",
        Exposure::Tunnel => "tunnel",
        Exposure::Localhost => "localhost",
    };
    // Without the bundle this screen hands out an API endpoint, not a
    // dashboard, so the title says which one the user is looking at.
    let title = if cfg!(feature = "web") {
        format!(" Remote Access ({mode_label})")
    } else {
        format!(" Remote API Access ({mode_label})")
    };
    let mut header_spans = vec![
        Span::styled(title, Style::default().fg(title_color).bold()),
        Span::styled(
            format!("  open {}", format_elapsed(elapsed)),
            Style::default().fg(theme.dimmed),
        ),
    ];
    if elapsed >= eight_hours {
        header_spans.push(Span::styled(
            "  still need it?",
            Style::default().fg(theme.waiting),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(header_spans)), header_inner);

    // ── Content ──────────────────────────────────────────────────────────
    let content_area = page[1];
    let inner_width = content_area.width.saturating_sub(2).max(1) as usize;
    let url_fits_one_line = full_url_len <= inner_width;

    let show_kind_label = kind_label.is_some();
    let show_split_token = !url_fits_one_line && split_token.is_some();
    // No bundle means no QR (nothing would answer a scan) and a 404 for any
    // browser that follows the URL, so the screen says what it is good for.
    let show_api_only = !cfg!(feature = "web");
    let command_prefix = "From another aoe: ";
    let command_rows = command.as_ref().map_or(0, |command| {
        let chars = command_prefix.chars().count() + command.chars().count();
        chars.div_ceil(inner_width).min(4) as u16
    });

    let mut text_height: u16 = 1 /* url */;
    if show_api_only {
        text_height += API_ONLY_ROWS;
    }
    if show_kind_label {
        text_height += 1;
    }
    if show_split_token {
        text_height += 1;
    }
    if is_tunnel {
        text_height += 1;
    }
    if command_rows > 0 {
        text_height += 1 + command_rows;
    }
    // The URL and command are what the user needs, so the QR yields first on
    // short terminals.
    let qr_text = render_qr(url);
    let qr_lines: Vec<&str> = qr_text.lines().collect();
    let qr_fits = qr_lines.len() as u16 + 1 + text_height <= content_area.height;
    let qr_height = if qr_fits { qr_lines.len() as u16 } else { 0 };
    let inner_height = text_height + if qr_fits { qr_height + 1 } else { 0 };

    let v_pad = content_area.height.saturating_sub(inner_height) / 2;

    let mut constraints = vec![Constraint::Length(v_pad)];
    if qr_fits {
        constraints.push(Constraint::Length(qr_height));
    }
    if show_api_only {
        constraints.push(Constraint::Length(API_ONLY_ROWS));
    }
    if qr_fits {
        constraints.push(Constraint::Length(1)); // spacer after QR
    }
    if show_kind_label {
        constraints.push(Constraint::Length(1));
    }
    constraints.push(Constraint::Length(1)); // url
    if show_split_token {
        constraints.push(Constraint::Length(1));
    }
    if is_tunnel {
        constraints.push(Constraint::Length(1));
    }
    if command_rows > 0 {
        constraints.push(Constraint::Length(1));
        constraints.push(Constraint::Length(command_rows));
    }
    constraints.push(Constraint::Min(0));
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .horizontal_margin(1)
        .constraints(constraints)
        .split(content_area);

    let mut idx: usize = 1; // skip top padding

    if qr_fits {
        let qr_widget: Vec<Line> = qr_lines
            .iter()
            .map(|l| Line::from(Span::styled(*l, Style::default().fg(theme.text))))
            .collect();
        frame.render_widget(
            Paragraph::new(qr_widget).alignment(Alignment::Center),
            chunks[idx],
        );
        idx += 1;
    }

    if show_api_only {
        frame.render_widget(
            Paragraph::new(API_ONLY_NOTICE)
                .style(Style::default().fg(theme.dimmed))
                .wrap(Wrap { trim: true })
                .alignment(Alignment::Center),
            chunks[idx],
        );
        idx += 1;
    }

    if qr_fits {
        idx += 1;
    }

    if let Some(label) = kind_label {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!("via {}", label),
                Style::default().fg(theme.dimmed).italic(),
            )))
            .alignment(Alignment::Center),
            chunks[idx],
        );
        idx += 1;
    }

    if url_fits_one_line {
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(url_prefix, Style::default().fg(theme.dimmed)),
                Span::styled(full_url, Style::default().fg(theme.accent)),
            ]))
            .alignment(Alignment::Center),
            chunks[idx],
        );
        idx += 1;
    } else {
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(url_prefix, Style::default().fg(theme.dimmed)),
                Span::styled(split_url.as_str(), Style::default().fg(theme.accent)),
            ]))
            .alignment(Alignment::Center),
            chunks[idx],
        );
        idx += 1;
        if let Some(token) = split_token {
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled("Token: ", Style::default().fg(theme.dimmed)),
                    Span::styled(token, Style::default().fg(theme.accent)),
                ]))
                .alignment(Alignment::Center),
                chunks[idx],
            );
            idx += 1;
        }
    }

    if is_tunnel {
        let (pp_label, pp_style) = match passphrase {
            Some(pp) => (pp.to_string(), Style::default().fg(theme.accent).bold()),
            None => (
                "(set when the daemon started; check the shell that ran `aoe serve`)".to_string(),
                Style::default().fg(theme.dimmed),
            ),
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("Passphrase: ", Style::default().fg(theme.dimmed)),
                Span::styled(pp_label, pp_style),
            ]))
            .alignment(Alignment::Center),
            chunks[idx],
        );
        idx += 1;
    }

    // Wrapped rather than truncated: the whole line has to be copyable.
    if let Some(command) = command {
        idx += 1;
        let line = Line::from(vec![
            Span::styled(command_prefix, Style::default().fg(theme.dimmed)),
            Span::styled(command, Style::default().fg(theme.text)),
        ]);
        let paragraph = Paragraph::new(line).wrap(Wrap { trim: false });
        let paragraph = if command_rows == 1 {
            paragraph.alignment(Alignment::Center)
        } else {
            paragraph
        };
        frame.render_widget(paragraph, chunks[idx]);
    }

    // ── Footer ────────────────────────────────────────────────────────
    let footer_block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(theme.border));
    let footer_inner = footer_block.inner(page[2]);
    frame.render_widget(footer_block, page[2]);

    let key_style = Style::default().fg(theme.accent);
    let desc_style = Style::default().fg(theme.dimmed);

    let footer_line: Line = if let Some(confirm) = pending_confirm {
        let warn_style = Style::default().fg(theme.waiting).bold();
        match confirm {
            PendingConfirm::NewPassphrase => Line::from(Span::styled(
                "Press G again to confirm new passphrase (clients will need it). Any other key cancels.",
                warn_style,
            )),
            PendingConfirm::Restart => Line::from(Span::styled(
                "Press R again to confirm restart (clears all sessions). Any other key cancels.",
                warn_style,
            )),
        }
    } else {
        let mut spans: Vec<Span> = Vec::new();
        if urls.len() > 1 {
            spans.extend([
                Span::styled("Tab", key_style),
                Span::styled(": URL  ", desc_style),
            ]);
        }
        if is_tunnel {
            spans.extend([
                Span::styled("G", key_style),
                Span::styled(": new pass  ", desc_style),
            ]);
        }
        spans.extend([
            Span::styled("E", key_style),
            Span::styled(": exposure  ", desc_style),
            Span::styled("R", key_style),
            Span::styled(": restart  ", desc_style),
            Span::styled("?", key_style),
            Span::styled(": help  ", desc_style),
            Span::styled("Esc", key_style),
            Span::styled(": close", desc_style),
        ]);
        Line::from(spans)
    };
    frame.render_widget(
        Paragraph::new(footer_line).alignment(Alignment::Center),
        footer_inner,
    );
}

/// Shortcut rows of the help overlay.
fn help_shortcuts(is_tunnel: bool) -> Vec<(&'static str, &'static str)> {
    let mut shortcuts = Vec::new();
    if is_tunnel {
        shortcuts.push(("G", "New random passphrase and restart server"));
    }
    shortcuts.extend([
        ("E", "Change exposure (back to localhost, LAN, tunnel)"),
        ("R", "Restart server (clears all client sessions)"),
        ("Tab", "Cycle URLs (when multiple available)"),
        ("Esc / q", "Close this view (server keeps running)"),
        ("?", "Toggle this help"),
    ]);
    shortcuts
}

fn render_help_overlay(frame: &mut Frame, area: Rect, theme: &Theme, mode: Exposure) {
    let dialog_width: u16 = 72.min(area.width.saturating_sub(4));
    let is_tunnel = mode == Exposure::Tunnel;
    let dialog_height: u16 = if is_tunnel { 20 } else { 14 };
    let dialog_height = dialog_height.min(area.height.saturating_sub(4));
    let x = area.x + (area.width.saturating_sub(dialog_width)) / 2;
    let y = area.y + (area.height.saturating_sub(dialog_height)) / 2;
    let dialog_area = Rect {
        x,
        y,
        width: dialog_width,
        height: dialog_height,
    };

    frame.render_widget(Clear, dialog_area);
    let block = Block::default()
        .style(Style::default().bg(theme.background))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.border))
        .title(" Remote Access Help ")
        .title_style(
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        );
    let inner = block.inner(dialog_area);
    frame.render_widget(block, dialog_area);

    let mut lines: Vec<Line> = vec![
        Line::from(""),
        Line::from(Span::styled(
            "Keyboard Shortcuts",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
    ];
    for (key, desc) in help_shortcuts(is_tunnel) {
        lines.push(Line::from(vec![
            Span::styled(format!("  {:14}", key), Style::default().fg(theme.waiting)),
            Span::styled(desc, Style::default().fg(theme.text)),
        ]));
    }
    lines.push(Line::from(""));
    if is_tunnel {
        lines.extend([
            Line::from(Span::styled(
                "About the passphrase",
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::styled(
                "  Second factor for internet-exposed tunnels.",
                Style::default().fg(theme.text),
            )),
            Line::from(Span::styled(
                "  Persists across restarts. Press G to rotate.",
                Style::default().fg(theme.text),
            )),
        ]);
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  Press any key to close",
        Style::default().fg(theme.dimmed),
    )));

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

/// Split a URL of the form `https://host/?token=XYZ` into a "clean" base
/// URL and its token so the dialog can fall back to rendering them on
/// separate rows when the combined string would clip off the right edge
/// of the dialog. Returns `(url, None)` when the query param is missing
/// or empty.
fn split_url_and_token(url: &str) -> (String, Option<&str>) {
    // The server always emits the token as the first query param in
    // `{url}/?token={token}`, so `?token=` is a safe anchor.
    if let Some(q_start) = url.find("?token=") {
        let base = url[..q_start].trim_end_matches('?').to_string();
        let token_start = q_start + "?token=".len();
        // Stop at the next `&` in case other query params ever appear.
        let token_end = url[token_start..]
            .find('&')
            .map(|n| token_start + n)
            .unwrap_or(url.len());
        let token = &url[token_start..token_end];
        if !token.is_empty() {
            return (base, Some(token));
        }
    }
    (url.to_string(), None)
}

fn render_error(frame: &mut Frame, area: Rect, theme: &Theme, msg: &str) {
    // Error copy can be long (multi-line tailscale output, stacked log
    // tail, hints, plus remediation steps). Keep it wide + tall enough
    // that the whole message fits without clipping the bottom. Wrap is
    // still on for individual long lines.
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.error))
        .title(Line::styled(
            " Serve failed ",
            Style::default().fg(theme.error).bold(),
        ));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(inner);

    frame.render_widget(
        Paragraph::new(msg)
            .wrap(Wrap { trim: true })
            .style(Style::default().fg(theme.text)),
        chunks[0],
    );
    let keybinds = if error_mentions_tailscale(msg) {
        "[R] Reset tailscale funnel    [Enter] Close"
    } else {
        "[Enter] Close"
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            keybinds,
            Style::default().fg(theme.dimmed),
        )))
        .alignment(Alignment::Center),
        chunks[1],
    );
}

/// Whether to offer the `[R]` reset keybind for a Tailscale/funnel error.
fn error_mentions_tailscale(msg: &str) -> bool {
    let lower = msg.to_ascii_lowercase();
    lower.contains("tailscale") || lower.contains("funnel")
}

/// Run `tailscale funnel reset` synchronously from the TUI thread.
/// Returns a short error string on failure so the Error dialog can show it.
fn run_tailscale_funnel_reset() -> Result<(), String> {
    let output = std::process::Command::new("tailscale")
        .args(["funnel", "reset"])
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("could not spawn tailscale: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(if stderr.is_empty() {
            format!("exited with status {:?}", output.status.code())
        } else {
            stderr
        })
    }
}

fn format_elapsed(d: Duration) -> String {
    let total = d.as_secs();
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    if h > 0 {
        format!("{}h {:02}m", h, m)
    } else if m > 0 {
        format!("{}m {:02}s", m, s)
    } else {
        format!("{}s", s)
    }
}

/// Generate a four-word lowercase passphrase (1Password / diceware style).
/// Four words from a ~500-word list gives ~35 bits of entropy, which as a
/// *second* factor on top of the URL token is plenty; far easier to type
/// on a phone keyboard than a random alphanumeric soup.
fn generate_passphrase() -> String {
    let mut rng = rand::rng();
    let words: Vec<&'static str> = (0..4)
        .map(|_| {
            *PASSPHRASE_WORDS
                .choose(&mut rng)
                .expect("wordlist nonempty")
        })
        .collect();
    words.join(" ")
}

/// Curated list of short, unambiguous lowercase English words chosen for
/// phone-typability. No words shorter than 3 letters or longer than 6.
/// No near-homophones (e.g., "their"/"there") or visually confusable pairs.
#[rustfmt::skip]
const PASSPHRASE_WORDS: &[&str] = &[
    "able", "acid", "aged", "acorn", "agent", "alarm", "album", "alert",
    "algae", "alien", "alive", "alley", "alloy", "alpha", "amber", "amigo",
    "amino", "amuse", "angel", "anger", "angle", "angry", "ankle", "anvil",
    "apple", "apron", "arbor", "arena", "argon", "armor", "arrow", "ashen",
    "aside", "aspen", "asset", "atlas", "atom", "audio", "audit", "aunt",
    "avoid", "awake", "award", "aware", "awful", "axis", "bacon", "badge",
    "bagel", "baker", "balmy", "banjo", "baron", "basil", "basin", "basis",
    "batch", "baton", "beach", "beads", "beard", "beast", "beaver", "bench",
    "berry", "bingo", "birch", "bison", "black", "blade", "blaze", "blend",
    "bliss", "block", "bloom", "blues", "blunt", "blush", "board", "boast",
    "bold", "bolt", "bonus", "boost", "booth", "boots", "bored", "boss",
    "botany", "bowl", "brave", "bread", "break", "brick", "bride", "brief",
    "bring", "brisk", "brook", "brown", "brush", "bucket", "bugle", "built",
    "bulk", "bunny", "burly", "butter", "buzz", "cabin", "cable", "cactus",
    "caddy", "camel", "camp", "candle", "candy", "canoe", "canon", "canyon",
    "cape", "caper", "card", "care", "cargo", "carry", "cart", "carve",
    "cash", "cast", "catch", "cedar", "chair", "chalk", "charm", "chart",
    "chase", "cheek", "cheer", "chef", "chess", "chief", "child", "chill",
    "chimp", "chip", "chirp", "choir", "chose", "chunk", "cider", "cinema",
    "civic", "claim", "clamp", "clean", "clerk", "click", "cliff", "climb",
    "cling", "clock", "clone", "cloth", "cloud", "clove", "clown", "club",
    "clue", "coach", "coast", "cobra", "cocoa", "code", "coin", "colon",
    "color", "comet", "coral", "cord", "corn", "cost", "couch", "cover",
    "cozy", "craft", "crane", "crash", "crate", "cream", "crest", "crew",
    "cross", "crowd", "crown", "crumb", "crush", "crust", "cube", "curl",
    "cycle", "daisy", "dance", "dare", "dash", "data", "deal", "deck",
    "delta", "dense", "depth", "derby", "desk", "diary", "dice", "diner",
    "disco", "diver", "dock", "dodo", "dog", "doll", "dolly", "donkey",
    "dough", "dove", "downy", "draft", "dragon", "drape", "dream", "drift",
    "drill", "drive", "drop", "drum", "duck", "dusk", "dusty", "eager",
    "eagle", "early", "earth", "ebony", "echo", "edge", "eject", "elbow",
    "elder", "elf", "elite", "elk", "elm", "email", "empty", "enact",
    "energy", "engine", "enjoy", "enter", "entry", "envoy", "epic", "equal",
    "era", "error", "essay", "ether", "event", "every", "exact", "exile",
    "exit", "extra", "eye", "fable", "face", "fact", "fade", "fair",
    "fairy", "faith", "fall", "false", "fame", "family", "fancy", "farm",
    "fast", "fat", "fate", "fault", "fawn", "fear", "feast", "feed",
    "fern", "ferry", "fever", "few", "fiber", "field", "fifth", "fig",
    "film", "find", "fine", "finer", "finish", "fire", "firm", "first",
    "fish", "five", "fix", "flag", "flame", "flash", "flat", "flax",
    "flex", "flint", "float", "flock", "flood", "floor", "flora", "flour",
    "flow", "flower", "fluff", "fluid", "fluke", "flute", "fly", "foam",
    "fog", "foil", "fold", "folk", "fond", "food", "foot", "force",
    "ford", "forge", "fork", "form", "fort", "forum", "fossil", "fox",
    "frame", "free", "fresh", "friar", "fries", "frog", "from", "front",
    "frost", "froth", "fruit", "fry", "fuel", "full", "fun", "fund",
    "funny", "fur", "fury", "fuse", "gable", "gadget", "gain", "gala",
    "gamma", "gap", "garden", "gargle", "garlic", "gate", "gauge", "gear",
    "gecko", "gem", "gentle", "gift", "ginger", "girl", "glad", "glide",
    "glitch", "globe", "gloom", "gloss", "glove", "glow", "glue", "gnat",
    "goat", "gold", "golf", "gone", "good", "goose", "gospel", "grab",
    "grace", "grade", "grain", "grape", "graph", "grasp", "grass", "grate",
    "gravy", "great", "grid", "grief", "grim", "grin", "grip", "grit",
    "groan", "groom", "gross", "group", "grout", "grove", "grow", "grub",
    "guess", "guide", "guild", "guilt", "guitar", "gulf", "gum", "guru",
    "habit", "haiku", "hair", "half", "hall", "halt", "ham", "hand",
    "hang", "happy", "harbor", "hard", "hare", "harm", "harp", "hash",
    "haste", "hat", "hatch", "have", "haven", "hawk", "hay", "hazel",
    "head", "heal", "heap", "heart", "heat", "heavy", "hedge", "heel",
    "help", "hemp", "hen", "herb", "hero", "hex", "hide", "high",
    "hike", "hill", "hip", "hive", "hobby", "hog", "hold", "hole",
    "hollow", "holy", "home", "honey", "honor", "hood", "hoof", "hook",
    "hoop", "hope", "horn", "horse", "host", "hot", "hound", "hour",
    "house", "hub", "hug", "human", "humble", "humor", "hump", "hunch",
    "hunt", "hurry", "husk", "hut", "hyena", "hymn", "ice", "icon",
    "idea", "igloo", "imp", "index", "indigo", "infant", "inlet", "ink",
    "inlay", "inner", "input", "iris", "iron", "ivory", "ivy", "jade",
    "jam", "jar", "java", "jaw", "jazz", "jeans", "jelly", "jest",
    "jet", "jewel", "jiffy", "jig", "job", "join", "joke", "jolly",
    "joy", "judge", "juice", "jump", "jungle", "junior", "junk", "jury",
    "kayak", "keep", "kept", "kettle", "key", "kick", "kid", "kilt",
    "kind", "king", "kite", "kitten", "knack", "knee", "knife", "knock",
    "koala", "label", "lace", "ladder", "lake", "lamb", "lamp", "lance",
    "land", "lane", "laser", "later", "latte", "laugh", "lava", "lawn",
    "layer", "lazy", "leaf", "lean", "leap", "learn", "lease", "led",
    "ledge", "left", "legal", "lemon", "lend", "lens", "level", "lever",
    "lick", "lid", "life", "lift", "light", "lilac", "lime", "line",
    "link", "lint", "lion", "lip", "list", "live", "load", "loaf",
    "loan", "lobby", "lobe", "local", "lock", "loft", "log", "logic",
    "long", "look", "loop", "loose", "lotus", "loud", "lounge", "love",
    "low", "loyal", "luck", "lunar", "lunch", "lung", "lure", "lush",
    "lute", "lynx", "lyric", "mace", "madam", "made", "magic", "main",
    "make", "mallet", "malt", "mango", "manor", "mantle", "maple", "march",
    "mare", "mark", "mars", "marsh", "mask", "mast", "match", "mate",
    "math", "maze", "meadow", "meal", "meat", "medal", "meet", "mellow",
    "melody", "melt", "memo", "menu", "mercy", "merge", "merit", "merry",
    "mesh", "metal", "meter", "mew", "mice", "midst", "might", "mild",
    "mile", "milk", "mill", "mimic", "mind", "mine", "mint", "minus",
    "mirror", "mist", "moat", "mocha", "modal", "model", "modem", "moist",
    "mole", "money", "month", "moon", "moose", "moral", "more", "moth",
    "motor", "mount", "mouse", "move", "movie", "much", "muffin", "mulch",
    "mule", "muse", "music", "mute", "myth",
];

#[cfg(test)]
mod qr_seam {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    /// Unicode half-blocks the QR renderer draws with. Their presence is the
    /// only way to tell a rendered code from an empty panel.
    const QR_GLYPHS: [char; 3] = ['\u{2588}', '\u{2580}', '\u{2584}'];

    fn active_screen(width: u16, height: u16) -> String {
        let urls = vec![ServeUrl {
            label: Some("lan".to_string()),
            url: "http://192.168.1.42:8080/?token=abc123def456".to_string(),
        }];
        let mut term = Terminal::new(TestBackend::new(width, height)).expect("terminal");
        term.draw(|f| {
            render_active(
                f,
                f.area(),
                &Theme::default(),
                Exposure::Network,
                &urls,
                0,
                None,
                std::time::Duration::from_secs(42),
                None,
            )
        })
        .expect("draw");
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

    /// The client command wraps instead of clipping on a narrow terminal,
    /// and the QR gives way so the URL and command stay on screen.
    #[test]
    fn narrow_active_screen_keeps_url_and_command() {
        let screen = active_screen(48, 24);
        let flat: String = screen.chars().filter(|c| !c.is_whitespace()).collect();
        assert!(
            flat.contains("aoeremoteadd") && flat.contains("--tokenabc123def456--insecure"),
            "{screen}"
        );
        assert!(!screen.chars().any(|c| QR_GLYPHS.contains(&c)), "{screen}");
    }

    /// The URL is what a user needs off this screen, so it must survive the
    /// QR being compiled out. Without the dashboard bundle the code is not
    /// drawn (nothing would answer a scan of it), and the layout must absorb
    /// the missing rows rather than leaving a gap or panicking on a
    /// zero-height chunk.
    #[test]
    fn active_screen_keeps_the_url_and_drops_the_code_without_web() {
        let screen = active_screen(100, 40);
        assert!(
            screen.contains("http://192.168.1.42:8080/?token=abc123def456"),
            "URL must render in both feature corners:\n{screen}"
        );
        let drawn = screen.chars().any(|c| QR_GLYPHS.contains(&c));
        assert_eq!(
            drawn,
            cfg!(feature = "web"),
            "QR code should be drawn only with the dashboard bundle:\n{screen}"
        );
    }

    /// A QR-less screen offering a bare URL reads as a dashboard link, and a
    /// browser following it gets a bodiless 404. Both the title and the panel
    /// have to say the endpoint is API-only, and neither may say it in a build
    /// that does embed the bundle.
    #[test]
    fn active_screen_says_api_only_without_web() {
        let screen = active_screen(100, 40);
        for needle in ["Remote API Access", "No dashboard bundle in this build:"] {
            assert_eq!(
                screen.contains(needle),
                !cfg!(feature = "web"),
                "{needle:?} belongs on the screen only without the dashboard bundle:\n{screen}"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passphrase_is_four_lowercase_words() {
        let pw = generate_passphrase();
        let words: Vec<&str> = pw.split(' ').collect();
        assert_eq!(words.len(), 4, "passphrase should be 4 words: {:?}", pw);
        for w in &words {
            assert!(!w.is_empty(), "empty word in passphrase: {:?}", pw);
            assert!(
                w.chars().all(|c| c.is_ascii_lowercase()),
                "non-lowercase-letter in word {:?} of {:?}",
                w,
                pw
            );
        }
    }

    #[test]
    fn passphrase_words_are_from_the_wordlist() {
        let pw = generate_passphrase();
        for w in pw.split(' ') {
            assert!(
                PASSPHRASE_WORDS.contains(&w),
                "word {:?} not in the embedded wordlist",
                w
            );
        }
    }

    #[test]
    fn compact_log_line_strips_tracing_prefix() {
        let input = "2026-04-19T23:43:44.609396Z  INFO agent_of_empires::server::tunnel: Warning: funnel=on for foo, but no serve config";
        assert_eq!(
            compact_log_line(input),
            "INFO tunnel: Warning: funnel=on for foo, but no serve config"
        );
    }

    #[test]
    fn compact_log_line_preserves_passthrough() {
        // Lines without a tracing-style leading timestamp (e.g. stray
        // stdout from tailscale funnel) should pass through unchanged.
        let raw = "Available on the internet: https://foo.ts.net";
        assert_eq!(compact_log_line(raw), raw);
    }

    #[test]
    fn compact_log_line_handles_levels() {
        let error = "2026-04-19T23:43:44.669741Z ERROR agent_of_empires::server: boom";
        assert_eq!(compact_log_line(error), "ERROR server: boom");
    }

    #[test]
    fn compact_log_line_leaves_digit_prefixed_non_tracing_alone() {
        // Regression: earlier heuristic flagged anything starting with a
        // digit as a tracing line, mangling lines like HTTP status codes.
        let line = "200 OK received";
        assert_eq!(compact_log_line(line), "200 OK received");
    }

    #[test]
    fn wordlist_is_well_formed() {
        assert!(
            PASSPHRASE_WORDS.len() >= 256,
            "wordlist too small for reasonable entropy: {}",
            PASSPHRASE_WORDS.len()
        );
        for w in PASSPHRASE_WORDS {
            assert!(!w.is_empty(), "empty word in list");
            assert!(
                w.chars().all(|c| c.is_ascii_lowercase()),
                "non-lowercase word in list: {:?}",
                w
            );
        }
    }

    #[test]
    fn format_elapsed_shows_units() {
        assert_eq!(format_elapsed(Duration::from_secs(5)), "5s");
        assert_eq!(format_elapsed(Duration::from_secs(65)), "1m 05s");
        assert_eq!(format_elapsed(Duration::from_secs(3600 + 120)), "1h 02m");
    }

    // The only test touching the module-global LAST_SPAWNED_PASSPHRASE;
    // the in-memory helpers keep it off the real serve.passphrase file.
    #[test]
    fn passphrase_cache_roundtrip() {
        for passphrase in ["four word diceware phrase", "a different phrase later"] {
            remember_passphrase(passphrase);
            assert_eq!(recall_passphrase_in_memory().as_deref(), Some(passphrase));
        }
    }

    #[test]
    fn split_url_and_token_extracts_token() {
        let (base, token) =
            split_url_and_token("https://foo-bar.trycloudflare.com/?token=abc123def456");
        assert_eq!(base, "https://foo-bar.trycloudflare.com/");
        assert_eq!(token, Some("abc123def456"));
    }

    #[test]
    fn split_url_and_token_preserves_url_without_token() {
        let (base, token) = split_url_and_token("https://foo-bar.trycloudflare.com/");
        assert_eq!(base, "https://foo-bar.trycloudflare.com/");
        assert_eq!(token, None);
    }

    #[test]
    fn split_url_and_token_handles_additional_query_params() {
        let (base, token) =
            split_url_and_token("https://foo.trycloudflare.com/?token=abc123&foo=bar");
        assert_eq!(base, "https://foo.trycloudflare.com/");
        assert_eq!(token, Some("abc123"));
    }

    /// Exercises the fit logic that the render path uses: full URL on
    /// one line when it fits, split when it doesn't. Copies the arithmetic
    /// from render_active (url_inner_width = dialog_width - 4).
    fn url_fits_one_line(url: &str, dialog_width: u16) -> bool {
        let url_prefix = "URL: ";
        let full_url_len = url_prefix.chars().count() + url.chars().count();
        let url_inner_width = dialog_width.saturating_sub(4).max(1) as usize;
        full_url_len <= url_inner_width
    }

    #[test]
    fn url_fits_one_line_on_wide_terminal() {
        // Typical tunnel URL: ~115 chars including "URL: " prefix.
        let url = "https://foo-bar.trycloudflare.com/?token=a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2";
        assert!(
            url_fits_one_line(url, 120),
            "120-wide should fit ~115 chars"
        );
        assert!(
            url_fits_one_line(url, 115),
            "exact-fit boundary should pass"
        );
    }

    #[test]
    fn url_splits_on_narrow_terminal() {
        // 80-col terminal can't fit the combined tunnel URL; force the
        // split fallback so the token doesn't clip off the edge.
        let url = "https://foo-bar.trycloudflare.com/?token=a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2";
        assert!(!url_fits_one_line(url, 80));
        // Local URL is shorter (~70 with token) — depends on IP/port.
        let local = "http://192.168.1.42:54321/?token=a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2";
        assert!(!url_fits_one_line(local, 80));
        assert!(url_fits_one_line(local, 110));
    }

    #[test]
    fn diagnose_daemon_exit_recognizes_common_errnos() {
        let unavailable = "ERROR: bind: Cannot assign requested address";
        for (log, target, needle) in [
            (unavailable, Exposure::Network, Some("interface")),
            (unavailable, Exposure::Tunnel, None),
            ("Address already in use", Exposure::Network, Some("port")),
            ("Permission denied", Exposure::Tunnel, Some("permission")),
            ("some unrelated line", Exposure::Network, None),
        ] {
            let hint = diagnose_daemon_exit(log, target);
            match needle {
                Some(needle) => assert!(hint.contains(needle), "{log:?}: {hint:?}"),
                None => assert_eq!(hint, "", "{log:?}"),
            }
        }
    }

    #[test]
    fn client_command_targets_the_base_url_with_flags() {
        let token = "?token=abc123";
        for (url, passphrase, expected) in [
            (
                format!("http://192.168.1.20:54321/{token}"),
                None,
                Some("aoe remote add box http://192.168.1.20:54321 --token abc123 --insecure"),
            ),
            (
                format!("https://box.tailnet.ts.net/{token}"),
                Some("four word pass phrase"),
                Some("aoe remote add box https://box.tailnet.ts.net --token abc123 --passphrase 'four word pass phrase'"),
            ),
            ("https://box.example.com/".to_string(), Some("it's"), Some("aoe remote add box https://box.example.com --passphrase 'it'\\''s'")),
            (format!("http://localhost:54321/{token}"), None, None),
            (format!("http://127.0.0.1:54321/{token}"), None, None),
            (format!("http://[::1]:54321/{token}"), None, None),
        ] {
            assert_eq!(
                client_command(&url, passphrase, "box").as_deref(),
                expected,
                "{url}"
            );
        }
    }

    #[test]
    fn remote_name_for_host_sanitizes_the_short_hostname() {
        for (hostname, expected) in [
            ("MacBook-Pro.local", "macbook-pro"),
            ("dev box_1", "dev-box-1"),
            ("--weird--", "weird"),
            ("", "aoe"),
            ("...", "aoe"),
        ] {
            assert_eq!(remote_name_for_host(hostname), expected, "{hostname:?}");
        }
    }

    fn picker(current: Option<Exposure>) -> ServeView {
        ServeView {
            state: ServeViewState::Picker {
                selected: current.unwrap_or(Exposure::Localhost),
                current,
                local_url: None,
                tunnel_available: false,
                network_address: None,
                flash: None,
            },
            pending_passphrase: "pass".into(),
            pending_confirm: None,
            show_help: false,
        }
    }

    fn press(view: &mut ServeView, code: KeyCode) {
        view.handle_key(KeyEvent::new(code, crossterm::event::KeyModifiers::NONE));
    }

    /// Unavailable or unchanged exposures explain themselves instead of
    /// restarting the daemon.
    #[test]
    fn picker_refuses_unusable_choices_without_restarting() {
        for (current, keys, flash) in [
            (
                Some(Exposure::Localhost),
                vec![KeyCode::Enter],
                "Already reachable from this machine only.",
            ),
            (
                Some(Exposure::Localhost),
                vec![KeyCode::Down, KeyCode::Enter],
                "No non-loopback network interface available.",
            ),
            (
                None,
                vec![KeyCode::Char('3')],
                "Install tailscale or cloudflared to enable Tunnel mode.",
            ),
        ] {
            let mut view = picker(current);
            for key in keys {
                press(&mut view, key);
            }
            let ServeViewState::Picker {
                flash: Some((message, _)),
                ..
            } = &view.state
            else {
                panic!("expected a flash on the picker for {current:?}");
            };
            assert_eq!(message, flash);
        }
    }

    #[test]
    fn picker_navigation_stays_in_bounds() {
        let mut view = picker(Some(Exposure::Localhost));
        let selected = |view: &ServeView| match &view.state {
            ServeViewState::Picker { selected, .. } => *selected,
            _ => panic!("left the picker"),
        };
        press(&mut view, KeyCode::Up);
        assert_eq!(selected(&view), Exposure::Localhost);
        for _ in 0..4 {
            press(&mut view, KeyCode::Down);
        }
        assert_eq!(selected(&view), Exposure::Tunnel);
        press(&mut view, KeyCode::Tab);
        assert_eq!(selected(&view), Exposure::Localhost);
    }

    // ── read_serve_urls ───────────────────────────────────────────────────
    //
    // The helper reads from $APP_DIR/serve.url, which is outside our
    // control in unit tests. These tests exercise the parsing logic via a
    // small shim that mirrors read_serve_urls' line-by-line behavior; the
    // integration with the real file lives in e2e.
    fn parse_serve_url_contents(raw: &str) -> Vec<ServeUrl> {
        let mut out: Vec<ServeUrl> = Vec::new();
        for (i, line) in raw.lines().enumerate() {
            let line = line.trim_end_matches('\r');
            if line.is_empty() {
                continue;
            }
            if i == 0 {
                out.push(ServeUrl {
                    label: None,
                    url: line.to_string(),
                });
            } else if let Some((label, url)) = line.split_once('\t') {
                out.push(ServeUrl {
                    label: Some(label.to_string()),
                    url: url.to_string(),
                });
            } else {
                out.push(ServeUrl {
                    label: None,
                    url: line.to_string(),
                });
            }
        }
        out
    }

    #[test]
    fn serve_url_parses_single_line_backward_compat() {
        // Older tunnel daemons wrote only the public URL on line 1.
        let out = parse_serve_url_contents("https://foo.trycloudflare.com/?token=abc\n");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].label, None);
        assert_eq!(out[0].url, "https://foo.trycloudflare.com/?token=abc");
    }

    #[test]
    fn serve_url_parses_multi_line_with_labels() {
        // Current daemons write primary on line 1, `kind\turl` on alternates.
        let raw = "\
http://100.64.0.5:54321/?token=abc\n\
lan\thttp://192.168.1.20:54321/?token=abc\n\
localhost\thttp://localhost:54321/?token=abc\n";
        let out = parse_serve_url_contents(raw);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].label, None);
        assert_eq!(out[0].url, "http://100.64.0.5:54321/?token=abc");
        assert_eq!(out[1].label.as_deref(), Some("lan"));
        assert_eq!(out[1].url, "http://192.168.1.20:54321/?token=abc");
        assert_eq!(out[2].label.as_deref(), Some("localhost"));
    }

    #[test]
    fn serve_url_tolerates_empty_and_unlabeled_extras() {
        // Defensive: if someone hand-edits serve.url and an extra line
        // has no tab, we treat it as an unlabeled alt rather than
        // dropping it.
        let raw = "http://primary/\n\nhttp://no-label-here/\n";
        let out = parse_serve_url_contents(raw);
        assert_eq!(out.len(), 2);
        assert_eq!(out[1].label, None);
        assert_eq!(out[1].url, "http://no-label-here/");
    }

    /// The help overlay has a fixed width. Every shortcut line must fit
    /// within `dialog_width - borders(2) - padding(0)` columns so text
    /// doesn't clip. This test catches the bug before it ships.
    #[test]
    fn help_overlay_text_fits_within_dialog_width() {
        let dialog_width: usize = 72;
        // Inner width = dialog_width - 2 (left/right border)
        let inner_width = dialog_width - 2;
        let key_col: usize = 14; // format!("{:14}", key)
        let indent: usize = 2; // leading "  "

        let descriptions: Vec<&str> = help_shortcuts(true)
            .into_iter()
            .map(|(_, desc)| desc)
            .chain([
                "Keyboard Shortcuts",
                "About the passphrase",
                "Second factor for internet-exposed tunnels.",
                "Persists across restarts. Press G to rotate.",
                "Press any key to close",
            ])
            .collect();

        for desc in descriptions {
            let line_len = indent + key_col + desc.len();
            assert!(
                line_len <= inner_width,
                "Help text clips: {:?} needs {} cols but only {} available \
                 (dialog_width={}, inner={})",
                desc,
                line_len,
                inner_width,
                dialog_width,
                inner_width,
            );
        }
    }
}
