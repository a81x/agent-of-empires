//! A selected remote session in the preview pane: its live output, its info
//! panel, and live-send into it, mirroring a local session's pane.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Alignment, Position, Rect};
use ratatui::style::Style;
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::live_send;
use super::preview::PreviewCache;
use super::HomeView;
use crate::daemon::SessionResponse;
use crate::session::{Instance, RemoteShelf, SandboxInfo, Status, View, WorktreeInfo};
use crate::tui::app::Action;
use crate::tui::components::preview::{self, CachedPreview, Preview};
use crate::tui::remote_feed::shelf_of;
use crate::tui::remote_preview::{PreviewCommand, PreviewEvent, RemoteKey};
use crate::tui::styles::Theme;

/// Live-send routed to a remote pane rather than a local tmux one.
#[derive(Debug, Clone)]
pub(in crate::tui) struct RemoteLiveSend {
    pub(in crate::tui) remote: String,
    pub(in crate::tui) session_id: String,
    pub(in crate::tui) title: String,
    pub(in crate::tui) exit_chords: Vec<(KeyCode, KeyModifiers)>,
    /// The daemon granted the size-owner lock; until then typed input waits
    /// in the preview worker.
    pub(in crate::tui) granted: bool,
}

/// A render-only `Instance` for the info panel. Never enters the instance map:
/// it exists so the remote row reuses the local preview renderer verbatim.
pub(in crate::tui) fn remote_display_instance(remote: &str, row: &SessionResponse) -> Instance {
    let mut inst = Instance::new(&row.title, &row.project_path);
    inst.id = row.id.clone();
    inst.tool = row.tool.clone();
    inst.source_profile = if row.profile.is_empty() {
        remote.to_string()
    } else {
        format!("{}@{remote}", row.profile)
    };
    inst.status = Status::from_api_str(&row.status).unwrap_or(Status::Unknown);
    inst.last_error = row.last_error.clone();
    inst.worktree_info = match (&row.branch, &row.main_repo_path) {
        (Some(branch), Some(main_repo_path)) => Some(WorktreeInfo {
            branch: branch.clone(),
            main_repo_path: main_repo_path.clone(),
            managed_by_aoe: row.has_managed_worktree,
            created_at: chrono::Utc::now(),
            base_branch: row
                .base_branch_override
                .clone()
                .or_else(|| row.base_branch.clone()),
        }),
        _ => None,
    };
    // The wire row says whether a sandbox exists, not which container.
    inst.sandbox_info = row.is_sandboxed.then(|| SandboxInfo {
        enabled: true,
        container_id: None,
        image: String::new(),
        container_name: format!("container on {remote}"),
        extra_env: None,
        custom_instruction: None,
        container_workdir: None,
        before_start_env: Vec::new(),
    });
    inst
}

fn remote_endpoint(name: &str) -> Option<crate::acp::client::discovery::DaemonEndpoint> {
    crate::tui::remote_feed::enabled_remotes()
        .get(name)
        .map(|entry| entry.endpoint.clone())
}

impl HomeView {
    fn remote_row_watchable(&self, key: &RemoteKey) -> bool {
        self.remote_session(&key.0, &key.1)
            .is_some_and(|row| shelf_of(row) == RemoteShelf::Live && row.view != View::Structured)
    }

    /// Point the preview socket at the selected remote row, or drop it. Cheap
    /// when nothing changed, so it runs on every selection sync and on the
    /// remote poll, which also retries a watch whose socket closed.
    pub(super) fn sync_remote_preview(&mut self) {
        let want = self
            .selected_remote
            .clone()
            .filter(|key| self.remote_row_watchable(key));
        if want == self.remote_preview_key {
            return;
        }
        if let Some(live) = &self.remote_live {
            if want.as_ref() != Some(&(live.remote.clone(), live.session_id.clone())) {
                self.remote_live = None;
            }
        }
        self.remote_preview_cache = PreviewCache::default();
        self.remote_preview_cursor = None;
        self.remote_preview_key = None;
        let Some(key) = want else {
            self.remote_preview_error = None;
            self.remote_preview.send(PreviewCommand::Stop);
            return;
        };
        match remote_endpoint(&key.0) {
            Some(endpoint) => {
                self.remote_preview_error = None;
                let lines = (self.preview_visible_rows as u16).max(24);
                self.remote_preview.send(PreviewCommand::Watch {
                    key: key.clone(),
                    endpoint,
                    lines,
                });
                self.remote_preview_key = Some(key);
            }
            None => {
                self.remote_preview_error = Some(format!("remote {:?} is not configured", key.0));
                self.remote_preview.send(PreviewCommand::Stop);
            }
        }
    }

    /// Land frames and socket state from the preview worker. Returns whether
    /// the pane needs a redraw.
    pub fn apply_remote_preview(&mut self) -> bool {
        let mut changed = false;
        while let Some(event) = self.remote_preview.try_recv() {
            let current = self.remote_preview_key.clone();
            match event {
                PreviewEvent::Frame {
                    key,
                    content,
                    cursor,
                } if current.as_ref() == Some(&key) => {
                    let dims = (self.preview_pane_area.width, self.preview_pane_area.height);
                    self.remote_preview_cache.store_capture(
                        content,
                        key.1.clone(),
                        format!("remote:{}", key.0),
                        0,
                        dims,
                        None,
                    );
                    self.remote_preview_cursor = cursor;
                    self.remote_preview_error = None;
                    changed = true;
                }
                PreviewEvent::SizeOwner { key, is_owner }
                    if self.remote_live_key().as_ref() == Some(&key) =>
                {
                    let Some(live) = self.remote_live.as_mut() else {
                        continue;
                    };
                    if is_owner {
                        live.granted = true;
                        continue;
                    }
                    // Refused, or another viewer took the pane; local
                    // live-send yields the same way rather than fighting.
                    let message = if live.granted {
                        format!("Another viewer took over {} on {}", live.title, key.0)
                    } else {
                        format!("{} did not grant input to {}", key.0, live.title)
                    };
                    self.exit_remote_live_send();
                    self.flash_status(message);
                    changed = true;
                }
                PreviewEvent::Closed { key, reason } if current.as_ref() == Some(&key) => {
                    if self.remote_live_key().as_ref() == Some(&key) {
                        if let Some(live) = self.remote_live.take() {
                            self.flash_status(format!(
                                "Live input to {} ended: {reason}",
                                live.title
                            ));
                        }
                    }
                    self.remote_preview_error = Some(reason);
                    // Cleared so the next remote poll retries the watch.
                    self.remote_preview_key = None;
                    changed = true;
                }
                _ => {}
            }
        }
        changed
    }

    fn remote_live_key(&self) -> Option<RemoteKey> {
        self.remote_live
            .as_ref()
            .map(|l| (l.remote.clone(), l.session_id.clone()))
    }

    /// Enter live-send on the selected remote row: claim the pane's size and
    /// route keys to it. Rows that cannot take input explain why instead.
    pub(super) fn start_remote_live_send(&mut self) -> Option<Action> {
        let (remote, id) = self.selected_remote.clone()?;
        let row = self.remote_session(&remote, &id)?;
        if row.view == View::Structured {
            return Some(Action::SetTransientStatus(
                "A structured session has no terminal to live-send into; press Enter to open it"
                    .to_string(),
            ));
        }
        match shelf_of(row) {
            RemoteShelf::Archived => {
                return Some(Action::SetTransientStatus(format!(
                    "Archived on {remote}; restore it there to open it"
                )));
            }
            RemoteShelf::Trashed => {
                return Some(Action::SetTransientStatus(format!(
                    "In {remote}'s trash; restore it there to open it"
                )));
            }
            RemoteShelf::Live => {}
        }
        let title = row.title.clone();
        self.exit_live_send_if_active();
        self.sync_remote_preview();
        let key = (remote.clone(), id.clone());
        if self.remote_preview_key.as_ref() != Some(&key) {
            return Some(Action::SetTransientStatus(format!("Can't reach {remote}")));
        }
        let exit_chords = live_send::parse_chord_list(
            &crate::session::config::profile_config::resolve_config_or_warn(&self.config_profile())
                .session
                .live_send_exit_chord,
        );
        let (cols, rows) = (
            self.preview_pane_area.width.max(1),
            self.preview_pane_area.height.max(1),
        );
        // Deliberate policy: live-send owns the pane's size, as local
        // live-send does.
        self.remote_preview
            .send(PreviewCommand::TakeOver { cols, rows });
        self.remote_live_size = (cols, rows);
        self.remote_live = Some(RemoteLiveSend {
            remote,
            session_id: id,
            title,
            exit_chords,
            granted: false,
        });
        None
    }

    /// Leave remote live-send and reconnect the watch, which releases the
    /// size-owner lock the live-send took.
    pub(in crate::tui) fn exit_remote_live_send(&mut self) {
        if self.remote_live.take().is_some() {
            self.remote_preview_key = None;
            self.sync_remote_preview();
        }
    }

    pub(super) fn handle_remote_live_key(&mut self, key: KeyEvent) {
        let Some(state) = self.remote_live.clone() else {
            return;
        };
        if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
            return;
        }
        if live_send::chord_list_matches(&state.exit_chords, key) {
            self.exit_remote_live_send();
            return;
        }
        if let live_send::LiveDispatch::Send(tmux_key) = live_send::translate(key) {
            let bytes = live_send::encode_key_bytes(&tmux_key, false);
            if !bytes.is_empty() {
                self.remote_preview.send(PreviewCommand::Input(bytes));
            }
        }
    }

    /// Paste into remote live-send. Returns whether it was consumed.
    pub(super) fn paste_into_remote_live(&mut self, text: &str) -> bool {
        if self.remote_live.is_none() {
            return false;
        }
        for key in super::input::split_paste_for_live_send(text) {
            let bytes = live_send::encode_key_bytes(&key, false);
            if !bytes.is_empty() {
                self.remote_preview.send(PreviewCommand::Input(bytes));
            }
        }
        true
    }

    /// Render the selected remote row into the preview pane.
    pub(super) fn render_remote_preview(
        &mut self,
        frame: &mut Frame,
        inner: Rect,
        theme: &Theme,
        compact: bool,
    ) {
        let Some((remote, id)) = self.selected_remote.clone() else {
            return;
        };
        let Some(row) = self.remote_session(&remote, &id).cloned() else {
            return;
        };
        let inst = remote_display_instance(&remote, &row);
        let layout = preview::PreviewLayout::compute(
            inner,
            compact,
            self.show_preview_info,
            preview::agent_info_height(&inst),
        );
        self.preview_pane_area = layout.output;
        self.preview_visible_rows = layout.output.height as usize;

        if self.remote_live.is_some() {
            let size = (layout.output.width.max(1), layout.output.height.max(1));
            if size != self.remote_live_size {
                self.remote_live_size = size;
                self.remote_preview.send(PreviewCommand::Resize {
                    cols: size.0,
                    rows: size.1,
                });
            }
        }

        let hint = if row.view == View::Structured {
            Some(format!(
                "Press Enter to open this structured session on {remote}"
            ))
        } else if shelf_of(&row) != RemoteShelf::Live {
            Some(format!("Restore this session on {remote} to preview it"))
        } else {
            self.remote_preview_error
                .as_ref()
                .filter(|_| self.remote_preview_cache.is_pending_for(&id))
                .map(|e| format!("{remote}: {e}"))
        };
        if let Some(hint) = hint {
            if let Some(info) = layout.info {
                Preview::render_info(frame, info, &inst, theme, self.idle_decay_window);
            }
            frame.render_widget(
                Paragraph::new(hint)
                    .style(Style::default().fg(theme.dimmed))
                    .alignment(Alignment::Center),
                layout.output,
            );
            return;
        }

        self.remote_preview_cache.ensure_parsed();
        let line_count = self
            .remote_preview_cache
            .parsed_text
            .as_ref()
            .map_or(0, |t| t.lines.len());
        Preview::render_with_cache(
            frame,
            inner,
            &inst,
            CachedPreview::new(
                self.remote_preview_cache.parsed_text.as_ref(),
                self.remote_preview_cache.is_pending_for(&id),
            ),
            self.preview_scroll_offset,
            theme,
            self.idle_decay_window,
            compact,
            self.show_preview_info,
        );

        // Typed echo needs a caret. Only at the live edge: a scrolled-back
        // view shows history, where the pane's cursor row is off screen.
        if self.remote_live.is_some() && self.preview_scroll_offset == 0 {
            if let Some((x, y)) = self.remote_preview_cursor {
                let visible = layout.output.height as usize;
                let first = line_count.saturating_sub(visible);
                let y = y as usize;
                if y >= first && y - first < visible && x < layout.output.width {
                    frame.set_cursor_position(Position::new(
                        layout.output.x + x,
                        layout.output.y + (y - first) as u16,
                    ));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_info_panel_instance_carries_the_remote_rows_details() {
        let row: SessionResponse = serde_json::from_value(serde_json::json!({
            "id": "r1",
            "title": "refactor",
            "project_path": "/Users/me/scm/app-wt",
            "tool": "codex",
            "status": "Running",
            "profile": "work",
            "branch": "feature/x",
            "main_repo_path": "/Users/me/scm/app",
            "base_branch": "main",
            "is_sandboxed": true,
        }))
        .unwrap();

        let inst = remote_display_instance("mini", &row);
        assert_eq!(inst.id, "r1");
        assert_eq!(inst.tool, "codex");
        assert_eq!(inst.source_profile, "work@mini");
        assert_eq!(inst.status, Status::Running);
        let wt = inst.worktree_info.expect("worktree shown");
        assert_eq!(wt.branch, "feature/x");
        assert_eq!(wt.main_repo_path, "/Users/me/scm/app");
        assert_eq!(wt.base_branch.as_deref(), Some("main"));
        assert!(inst.sandbox_info.is_some_and(|s| s.enabled));
    }

    #[test]
    fn a_row_without_worktree_or_sandbox_shows_neither() {
        let row: SessionResponse =
            serde_json::from_value(serde_json::json!({"id": "r1", "title": "t"})).unwrap();
        let inst = remote_display_instance("mini", &row);
        assert!(inst.worktree_info.is_none());
        assert!(inst.sandbox_info.is_none());
        assert_eq!(inst.source_profile, "mini");
    }
}
