//! Remote daemon sessions listed inline with local ones.

use std::sync::mpsc::TryRecvError;

use super::HomeView;
use crate::session::config::GroupByMode;
use crate::session::Item;
use crate::tui::remote_feed::{self, RemoteFeed};

impl HomeView {
    /// The grouping actually rendered. "Group by remote" is the default, but
    /// with no remote configured it would only wrap everything in a lone
    /// `local` header, so a default (never an explicit choice) renders as the
    /// pre-remote default until a remote exists.
    pub(in crate::tui) fn effective_group_by(&self) -> GroupByMode {
        if self.group_by == GroupByMode::Remote
            && self.group_by_is_default
            && !self.remotes_configured
        {
            self.fallback_group_by
        } else {
            self.group_by
        }
    }

    /// Show configured remotes as connecting before their first read lands.
    pub fn seed_remotes(&mut self, names: Vec<String>) {
        self.remotes_configured = !names.is_empty();
        self.remote_snapshots = remote_feed::pending(names);
        self.remote_fingerprint = remote_feed::fingerprint(&self.remote_snapshots);
        self.rebuild_flat_items_keeping_selection();
    }

    /// Ask the remote feed for a fresh read (non-blocking).
    pub fn request_remote_feed_refresh(&mut self) {
        self.sync_remote_preview();
        if self.pending_remote_feed {
            return;
        }
        self.remote_feed.request_refresh();
        self.pending_remote_feed = true;
    }

    /// Land a finished remote read. Returns whether the sidebar changed.
    pub fn apply_remote_feed(&mut self) -> bool {
        let read = match self.remote_feed.try_recv() {
            Ok(read) => read,
            Err(TryRecvError::Empty) => return false,
            Err(TryRecvError::Disconnected) => {
                self.remote_feed = RemoteFeed::new();
                self.pending_remote_feed = false;
                return false;
            }
        };
        self.pending_remote_feed = false;
        let snapshots = remote_feed::merge_read(&self.remote_snapshots, read);
        let fingerprint = remote_feed::fingerprint(&snapshots);
        self.remote_snapshots = snapshots;
        let configured = !self.remote_snapshots.is_empty();
        if fingerprint == self.remote_fingerprint && configured == self.remotes_configured {
            return false;
        }
        self.remote_fingerprint = fingerprint;
        self.remotes_configured = configured;
        self.rebuild_flat_items_keeping_selection();
        self.select_pending_remote_row();
        true
    }

    /// Remotes the new-session dialog can target: every enabled remote, ready
    /// once the feed has read its profiles and agents.
    pub(super) fn remote_dialog_targets(&self) -> Vec<crate::tui::dialogs::RemoteTarget> {
        use crate::tui::dialogs::{RemoteMachine, RemoteTarget, RemoteUnavailable};
        let remotes = remote_feed::enabled_remotes();
        self.remote_snapshots
            .iter()
            .filter_map(|snapshot| {
                let entry = remotes.get(&snapshot.name)?;
                let machine = match (&snapshot.sessions, &snapshot.meta) {
                    (None, _) => Err(RemoteUnavailable::Connecting),
                    (Some(Ok(_)), Some(meta)) => entry
                        .endpoint
                        .daemon_client()
                        .map(|client| {
                            let mut profiles = meta.profiles.clone();
                            profiles.sort_by_key(|p| !p.is_default);
                            RemoteMachine {
                                home: meta.home.clone(),
                                profiles: profiles.into_iter().map(|p| p.name).collect(),
                                tools: meta
                                    .agents
                                    .iter()
                                    .filter(|a| a.installed)
                                    .map(|a| a.name.clone())
                                    .collect(),
                                docker_available: meta.container_runtime_available,
                                client,
                            }
                        })
                        .map_err(|_| RemoteUnavailable::Unreachable),
                    _ => Err(RemoteUnavailable::Unreachable),
                };
                Some(RemoteTarget {
                    name: snapshot.name.clone(),
                    machine,
                })
            })
            .collect()
    }

    /// Hand a remote-targeted dialog submit to the create worker.
    pub(super) fn start_remote_create(
        &mut self,
        remote: String,
        data: &crate::tui::dialogs::NewSessionData,
    ) {
        let client = match remote_feed::remote_endpoint(&remote).map(|e| e.daemon_client()) {
            Some(Ok(client)) => client,
            Some(Err(error)) => {
                self.flash_status(format!("{remote}: {}", error.summary()));
                return;
            }
            None => {
                self.flash_status(format!("{remote} is no longer configured"));
                return;
            }
        };
        self.remote_create
            .request(crate::tui::remote_create::CreateRequest {
                remote: remote.clone(),
                client,
                body: crate::tui::remote_create::create_body(data),
            });
        self.flash_status(format!("Creating session on {remote}…"));
    }

    /// Land finished remote creates. Returns whether anything changed.
    pub fn apply_remote_create(&mut self) -> bool {
        let mut changed = false;
        while let Ok((remote, outcome)) = self.remote_create.try_recv() {
            match outcome {
                Ok(id) => {
                    self.flash_status(format!("Created on {remote}"));
                    self.collapsed_remotes
                        .remove(&(remote.clone(), crate::session::RemoteShelf::Live));
                    self.pending_remote_select = Some((remote, id));
                    self.request_remote_feed_refresh();
                }
                Err(message) => self.flash_status(format!("{remote}: {message}")),
            }
            changed = true;
        }
        changed
    }

    fn select_pending_remote_row(&mut self) {
        let Some((remote, id)) = self.pending_remote_select.clone() else {
            return;
        };
        let found = self.flat_items.iter().position(|item| {
            matches!(item, Item::RemoteSession { remote: r, id: i, .. } if *r == remote && *i == id)
        });
        if let Some(idx) = found {
            self.cursor = idx;
            self.update_selected();
            self.pending_remote_select = None;
        }
    }

    /// The instance a sidebar session row renders: the local session, or the
    /// display instance built from a remote row.
    pub(in crate::tui) fn row_instance(&self, item: &Item) -> Option<&crate::session::Instance> {
        match item {
            Item::Session { id, .. } => self.get_instance(id),
            Item::RemoteSession { remote, id, .. } => self.remote_instance(remote, id),
            _ => None,
        }
    }

    /// [`Self::row_instance`] for the selected row.
    pub(in crate::tui) fn selected_row_instance(&self) -> Option<&crate::session::Instance> {
        match (&self.selected_session, &self.selected_remote) {
            (Some(id), _) => self.get_instance(id),
            (None, Some((remote, id))) => self.remote_instance(remote, id),
            _ => None,
        }
    }

    pub(in crate::tui) fn remote_instance(
        &self,
        remote: &str,
        id: &str,
    ) -> Option<&crate::session::Instance> {
        self.remote_instances.get(remote)?.get(id)
    }

    /// [`Self::build_flat_items`] with the remote sessions placed.
    pub(super) fn build_flat_items_with_remotes(&self) -> Vec<Item> {
        if self.effective_group_by() == GroupByMode::Remote {
            return self.build_flat_items_by_machine();
        }
        let mut items = self.build_flat_items();
        self.insert_remote_sections(&mut items);
        self.insert_remote_shelves(&mut items);
        items
    }

    /// One section per machine, this one first, each one indent deep, with the
    /// Archived/Trash shelf still pinned last.
    fn build_flat_items_by_machine(&self) -> Vec<Item> {
        let pool = self.cloned_instances_in_active_view();
        let mut items = crate::session::flatten_local_machine(
            &pool,
            self.sort_order,
            self.local_machine_collapsed,
        );
        if matches!(self.view_mode, super::ViewMode::Structured) {
            items.extend(remote_feed::remote_items(
                &self.remote_snapshots,
                &self.remote_instances,
                &self.collapsed_remotes,
                self.sort_order,
            ));
        }
        crate::session::append_archived_section(&mut items, &pool, self.archived_section_collapsed);
        crate::session::append_trash_section(&mut items, &pool, self.trashed_section_collapsed);
        self.insert_remote_shelves(&mut items);
        items
    }

    /// Put remote archived and trashed sessions inside the local Archived and
    /// Trash sections, one machine sub-header each, creating a section header
    /// when only a remote has rows for it. The section's count covers both, and
    /// its collapse hides both.
    fn insert_remote_shelves(&self, items: &mut Vec<Item>) {
        if !matches!(self.view_mode, super::ViewMode::Structured) {
            return;
        }
        use crate::session::RemoteShelf;
        for (shelf, path, name, collapsed) in [
            (
                RemoteShelf::Archived,
                crate::session::ARCHIVED_SECTION_PATH,
                crate::session::ARCHIVED_SECTION_NAME,
                self.archived_section_collapsed,
            ),
            (
                RemoteShelf::Trashed,
                crate::session::TRASH_SECTION_PATH,
                crate::session::TRASH_SECTION_NAME,
                self.trashed_section_collapsed,
            ),
        ] {
            let (rows, total) = remote_feed::remote_shelf_items(
                &self.remote_snapshots,
                &self.remote_instances,
                shelf,
                &self.collapsed_remotes,
            );
            if total == 0 {
                continue;
            }
            let trash_header = items.iter().position(
                |it| matches!(it, Item::Group { path: p, .. } if p == crate::session::TRASH_SECTION_PATH),
            );
            let header = items
                .iter()
                .position(|it| matches!(it, Item::Group { path: p, .. } if p == path));
            // Archived ends where Trash begins; Trash is always last.
            let section_end = match shelf {
                RemoteShelf::Archived => trash_header.unwrap_or(items.len()),
                _ => items.len(),
            };
            match header {
                Some(idx) => {
                    if let Some(Item::Group { session_count, .. }) = items.get_mut(idx) {
                        *session_count += total;
                    }
                    if !collapsed {
                        items.splice(section_end..section_end, rows);
                    }
                }
                None => {
                    let mut section = vec![Item::Group {
                        path: path.to_string(),
                        name: name.to_string(),
                        depth: 0,
                        collapsed,
                        session_count: total,
                        profile: None,
                        archived_at: None,
                    }];
                    if !collapsed {
                        section.extend(rows);
                    }
                    items.splice(section_end..section_end, section);
                }
            }
        }
    }

    /// Splice remote sections just above the Archived/Trash shelf, which must
    /// stay a contiguous suffix. Only the main agent list shows them: the
    /// Terminal and Tool views attach to local panes.
    fn insert_remote_sections(&self, items: &mut Vec<Item>) {
        if !matches!(self.view_mode, super::ViewMode::Structured)
            || self.remote_snapshots.is_empty()
        {
            return;
        }
        let remote = remote_feed::remote_items(
            &self.remote_snapshots,
            &self.remote_instances,
            &self.collapsed_remotes,
            self.sort_order,
        );
        let at = items
            .iter()
            .position(|it| match it {
                Item::Group { path, .. } => {
                    crate::session::is_within_archived_section(path)
                        || crate::session::is_within_trash_section(path)
                }
                _ => false,
            })
            .unwrap_or(items.len());
        items.splice(at..at, remote);
    }

    /// Toggle the machine header under the cursor. Returns false when the
    /// cursor is not on one.
    pub(super) fn toggle_machine_header_at_cursor(&mut self) -> bool {
        match self.flat_items.get(self.cursor) {
            Some(Item::LocalGroup { .. }) => {
                self.local_machine_collapsed = !self.local_machine_collapsed;
            }
            Some(Item::RemoteGroup { name, shelf, .. }) => {
                let key = (name.clone(), *shelf);
                if !self.collapsed_remotes.remove(&key) {
                    self.collapsed_remotes.insert(key);
                }
            }
            _ => return false,
        }
        self.rebuild_flat_items_keeping_selection();
        true
    }

    /// Rebuild after rows moved, keeping the cursor on the row the user was on
    /// rather than whatever slid into its index.
    fn rebuild_flat_items_keeping_selection(&mut self) {
        let before = self.flat_items.get(self.cursor).cloned();
        self.rebuild_flat_items();
        let found = before.and_then(|prev| {
            self.flat_items.iter().position(|it| match (it, &prev) {
                (Item::Session { id: a, .. }, Item::Session { id: b, .. }) => a == b,
                (
                    Item::RemoteSession {
                        remote: ra, id: a, ..
                    },
                    Item::RemoteSession {
                        remote: rb, id: b, ..
                    },
                ) => ra == rb && a == b,
                (Item::LocalGroup { .. }, Item::LocalGroup { .. }) => true,
                (
                    Item::RemoteGroup {
                        name: a, shelf: sa, ..
                    },
                    Item::RemoteGroup {
                        name: b, shelf: sb, ..
                    },
                ) => a == b && sa == sb,
                (Item::Group { path: a, .. }, Item::Group { path: b, .. }) => a == b,
                _ => false,
            })
        });
        if let Some(idx) = found {
            self.cursor = idx;
        } else if self.cursor >= self.flat_items.len() {
            self.cursor = self.flat_items.len().saturating_sub(1);
        }
        self.update_selected();
    }
}
