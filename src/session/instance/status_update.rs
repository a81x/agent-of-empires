//! Status polling: pane capture, hook reconciliation, and the transition
//! bookkeeping that decides what a row displays.

use super::*;

/// Whether this poll can reuse the last verdict instead of capturing the pane.
fn skip_capture(
    activity: Option<i64>,
    last_activity: Option<i64>,
    last_capture_second: Option<i64>,
    has_hook: bool,
    pending: bool,
) -> bool {
    let Some(activity) = activity else {
        return false;
    };
    Some(activity) == last_activity
        && last_capture_second.is_some_and(|taken| taken > activity)
        && !has_hook
        && !pending
}

/// How long a `running` hook write keeps its authority for an agent that is still on a hand-written
/// detector.
const LEGACY_RUNNING_HOOK_MAX_AGE: std::time::Duration = std::time::Duration::from_secs(900);

impl Instance {
    /// Update status using pre-fetched pane metadata to avoid per-instance subprocess spawns.
    pub fn update_status_with_metadata(
        &mut self,
        metadata: Option<&tmux::PaneMetadata>,
        resolved_name: Option<&str>,
    ) {
        self.poll_status(metadata, resolved_name, false);
    }

    /// Update status for a caller that observes this session exactly once.
    pub fn update_status_once(
        &mut self,
        metadata: Option<&tmux::PaneMetadata>,
        resolved_name: Option<&str>,
    ) {
        self.poll_status(metadata, resolved_name, true);
    }

    /// The body both entry points share. `single_poll` says no further
    /// observation is coming, which is what decides a held proposal.
    fn poll_status(
        &mut self,
        metadata: Option<&tmux::PaneMetadata>,
        resolved_name: Option<&str>,
        single_poll: bool,
    ) {
        if single_poll {
            // This observation decides, so only this observation may propose.
            self.detection.pending = None;
        }
        let baseline = self.live_status_baseline;
        self.update_status_with_metadata_inner(metadata, resolved_name);
        if single_poll {
            if let Some(pending) = self.detection.pending.take() {
                // Only a plain Idle is ever held, so there is no error explanation to keep or
                // derive.
                self.status = pending;
                self.last_error = None;
            }
        }
        if let Some(prev) = baseline {
            if prev != self.status {
                self.log_status_transition(prev);
                // last_accessed_at is deliberately NOT restamped here.
                let now = Utc::now();
                self.idle_entered_at = if self.status == Status::Idle {
                    Some(now)
                } else {
                    None
                };
            }
        }
        self.live_status_baseline = Some(self.status);
    }

    /// One `info` line per observed status transition, carrying the evidence a wrong-state report
    /// needs.
    fn log_status_transition(&self, prev: Status) {
        let detection_tool =
            tmux::status_rules::detection_tool(&self.source_profile, &self.tool, &self.detect_as);
        let hook = crate::hooks::read_hook_status(&self.id);
        let hook_age_ms = crate::hooks::read_hook_status_age(&self.id).map(|age| age.as_millis());
        tracing::info!(target: "session.status_change",
            "{} [{}] {:?} -> {:?} (hook={:?} hook_age_ms={:?} rule={})",
            self.id, detection_tool, prev, self.status, hook, hook_age_ms,
            self.detection.rule.unwrap_or("none")
        );
    }

    /// Drop a [`TMUX_SESSION_GONE_ERROR`] left on a row that no longer has a tmux pane to speak for
    /// it, so the UI stops showing a message that cannot apply to it any more (a session converted
    /// to, or restarted in, the structured view).
    pub(crate) fn clear_stale_tmux_error(&mut self) {
        if self.last_error.as_deref() == Some(TMUX_SESSION_GONE_ERROR) {
            self.last_error = None;
        }
    }

    pub(super) fn update_status_with_metadata_inner(
        &mut self,
        metadata: Option<&tmux::PaneMetadata>,
        resolved_name: Option<&str>,
    ) {
        if matches!(
            self.status,
            Status::Stopped | Status::Deleting | Status::Creating
        ) {
            return;
        }

        // Archived sessions have their tmux torn down on purpose, so probing tmux here only ever
        // produces a spurious "tmux session is gone" Error transition.
        if self.is_archived() {
            self.settle_archived_status();
            return;
        }

        // Acp-mode sessions are not backed by a tmux pane; the structured view worker supervisor
        // owns their lifecycle and emits typed health events over the broadcast.
        if self.is_structured() {
            self.clear_stale_tmux_error();
            if self.status == Status::Error {
                self.status = Status::Idle;
            }
            return;
        }

        if self.status == Status::Error && self.last_error.is_some() {
            if let Some(last_check) = self.last_error_check {
                if last_check.elapsed().as_secs() < 30 {
                    return;
                }
            }
        }

        if let Some(start_time) = self.last_start_time {
            if start_time.elapsed().as_secs() < 3 {
                self.status = Status::Starting;
                return;
            }
        }

        let session = match resolved_name {
            Some(name) => tmux::Session::from_name(name),
            None => match self.tmux_session() {
                Ok(s) => s,
                Err(_) => {
                    tracing::trace!(target: "session.store",
                        "status '{}': tmux_session() failed, setting Error",
                        self.title
                    );
                    self.status = Status::Error;
                    if self.last_error.is_none() {
                        self.last_error = Some(
                            "Could not reach tmux. Is tmux still running on the host?".to_string(),
                        );
                    }
                    self.last_error_check = Some(std::time::Instant::now());
                    return;
                }
            },
        };

        match session.existence() {
            tmux::SessionExistence::Absent => {
                tracing::trace!(target: "session.store",
                    "status '{}': session.existence()=Absent (tmux name={}), setting Error",
                    self.title,
                    session.name()
                );
                self.unknown_since = None;
                self.status = Status::Error;
                if self.last_error.is_none() {
                    self.last_error = Some(TMUX_SESSION_GONE_ERROR.to_string());
                }
                self.last_error_check = Some(std::time::Instant::now());
                return;
            }
            tmux::SessionExistence::Unknown => {
                // The tmux server itself was unreachable (stale socket, refused connection), not a
                // confirmed-absent session.
                let window = if self.ever_confirmed_present {
                    UNKNOWN_ERROR_WINDOW_CONFIRMED_PRESENT
                } else {
                    UNKNOWN_ERROR_WINDOW_NEVER_PRESENT
                };
                let unknown_since = *self
                    .unknown_since
                    .get_or_insert_with(std::time::Instant::now);
                if unknown_since.elapsed() < window {
                    tracing::debug!(target: "session.store",
                        "status '{}': tmux server unreachable for {:?} (< {:?} window, ever_confirmed_present={}), retaining status {:?}",
                        self.title,
                        unknown_since.elapsed(),
                        window,
                        self.ever_confirmed_present,
                        self.status
                    );
                    return;
                }
                tracing::trace!(target: "session.store",
                    "status '{}': tmux server unreachable for {:?} (>= {:?} window, ever_confirmed_present={}), setting Error",
                    self.title,
                    unknown_since.elapsed(),
                    window,
                    self.ever_confirmed_present
                );
                self.status = Status::Error;
                if self.last_error.is_none() {
                    self.last_error = Some(TMUX_SERVER_UNREACHABLE_ERROR.to_string());
                }
                self.last_error_check = Some(std::time::Instant::now());
                return;
            }
            tmux::SessionExistence::Present => {
                self.unknown_since = None;
                self.ever_confirmed_present = true;
            }
        }

        let is_dead = metadata
            .map(|m| m.pane_dead)
            .unwrap_or_else(|| session.is_pane_dead());

        let pane_cmd = metadata
            .and_then(|m| m.pane_current_command.clone())
            .or_else(|| tmux::utils::pane_current_command(session.name()));

        tracing::trace!(target: "session.store",
            "status '{}': exists=true, is_dead={}, pane_cmd={:?}, tool={}, cmd_override={}",
            self.title,
            is_dead,
            pane_cmd,
            self.tool,
            self.has_command_override()
        );

        // Two detection identities: hooks are installed for (and must be interpreted by) the
        // `agent_detect_as` alias when one is set, so hook reconciliation keeps the alias identity.
        let hook_alias = tmux::status_rules::effective_detect_as(
            &self.source_profile,
            &self.tool,
            &self.detect_as,
        );
        let hook_tool: &str = if hook_alias.is_empty() {
            &self.tool
        } else {
            &hook_alias
        };
        let pane_tool =
            tmux::status_rules::detection_tool(&self.source_profile, &self.tool, &self.detect_as);

        let hook =
            crate::hooks::read_hook_status(&self.id).map(|status| tmux::detect::HookObservation {
                status,
                age: crate::hooks::read_hook_status_age(&self.id),
            });

        // A dead pane outranks every other signal, and only for a session that
        // reported hooks at all: a hookless agent's pane is allowed to end.
        if is_dead && hook.is_some() {
            self.status = Status::Error;
            if self.last_error.is_none() {
                let pane_content = session.capture_pane(20).unwrap_or_default();
                self.last_error = Some(summarize_error_from_pane(&pane_content));
            }
            return;
        }

        if tmux::detect::has_manifest(hook_tool) {
            // Owned: both identities borrow `self`, which the manifest path
            // mutates.
            let agent = hook_tool.to_string();
            let rules_tool = pane_tool.to_string();
            self.update_status_from_manifest(
                &session,
                metadata,
                &agent,
                &rules_tool,
                hook,
                is_dead,
            );
            return;
        }

        // Agents still on hand-written detectors: the hook file decides.
        let hook =
            hook.filter(|_| !tmux::status_rules::has_rules(&self.source_profile, &pane_tool));
        if let Some(hook) = hook {
            let stale_running = hook.status == Status::Running
                && hook
                    .age
                    .is_some_and(|age| age >= LEGACY_RUNNING_HOOK_MAX_AGE);
            if !stale_running {
                self.status = hook.status;
                // An Error keeps its explanation, as the manifest path does.
                if hook.status == Status::Error {
                    if self.last_error.is_none() {
                        let pane_content = session.capture_pane(20).unwrap_or_default();
                        self.last_error = Some(summarize_error_from_pane(&pane_content));
                    }
                } else {
                    self.last_error = None;
                }
                return;
            }
            tracing::debug!(target: "session.store",
                "status '{}': {} `running` hook write is {:?} old, falling back to the pane",
                self.title, hook_tool, hook.age);
        }

        let pane_content = session.capture_pane(50).unwrap_or_default();
        let detected =
            tmux::detect_status_from_content_in(&self.source_profile, &pane_content, &pane_tool);
        tracing::trace!(target: "session.store",
            "status '{}': detected={:?}, cmd_override={}, custom_cmd={}",
            self.title,
            detected,
            self.has_command_override(),
            self.has_custom_command(),
        );
        let has_command_override = self.has_command_override();
        let shell_stale = detected == Status::Idle
            && !has_command_override
            && !is_dead
            && self.pane_is_stale_shell(metadata, &session);
        self.status = resolve_detected_status(
            detected,
            is_dead,
            shell_stale,
            has_command_override,
            &pane_content,
            &self.tool,
        );

        tracing::trace!(target: "session.store", "status '{}': final={:?}", self.title, self.status);

        if self.status == Status::Error {
            if self.last_error.is_none() {
                self.last_error = Some(summarize_error_from_pane(&pane_content));
            }
        } else {
            self.last_error = None;
        }
    }

    /// Whether the pane is sitting on a bare shell rather than the agent it was launched with: the
    /// agent exited and left the pane's shell behind.
    fn pane_is_stale_shell(
        &self,
        metadata: Option<&tmux::PaneMetadata>,
        session: &tmux::Session,
    ) -> bool {
        if self.expects_shell() {
            return false;
        }
        metadata
            .and_then(|m| {
                m.pane_current_command.as_deref().map(|current_command| {
                    tmux::utils::is_pane_running_shell_command(
                        current_command,
                        m.pane_start_command_is_protected,
                    )
                })
            })
            .unwrap_or_else(|| session.is_pane_running_shell())
    }

    /// Resolve status from the agent's detection manifest.
    fn update_status_from_manifest(
        &mut self,
        session: &tmux::Session,
        metadata: Option<&tmux::PaneMetadata>,
        agent: &str,
        rules_tool: &str,
        hook: Option<tmux::detect::HookObservation>,
        is_dead: bool,
    ) {
        let activity = metadata.and_then(|m| m.window_activity);
        let osc_title = metadata
            .and_then(|m| m.pane_title.as_deref())
            .unwrap_or_default();
        let screen_unchanged = skip_capture(
            activity,
            self.detection.activity,
            self.detection.captured_at,
            hook.is_some(),
            self.detection.pending.is_some(),
        );

        if screen_unchanged {
            // Nothing to re-decide, and nothing to re-derive from: the checks
            // below read the capture we deliberately did not take.
            self.detection.rule = Some("screen_unchanged");
            return;
        }

        // Stamped before the capture, never after.
        let captured_at = Utc::now().timestamp();
        let pane_content = session.capture_pane(50).unwrap_or_default();
        let clean = tmux::utils::strip_ansi(&pane_content);
        let detection = tmux::detect_with_rules(
            &self.source_profile,
            rules_tool,
            agent,
            &clean,
            osc_title,
            hook,
        )
        .unwrap_or_else(tmux::detect::Detection::idle_by_default);

        let Some(candidate) = detection.status else {
            // The screen is an agent-owned viewer; the last known status
            // stands rather than being overwritten by what a pager shows.
            self.detection.activity = activity;
            self.detection.captured_at = Some(captured_at);
            self.detection.rule = Some(detection.rule);
            return;
        };

        let has_command_override = self.has_command_override();
        // A `waiting` hook write outlives an agent that exits from an open
        // prompt, so an inferred Waiting gets the same shell check as Idle.
        let shell_stale = (candidate == Status::Idle
            || (candidate == Status::Waiting && !detection.visible))
            && !has_command_override
            && !is_dead
            && self.pane_is_stale_shell(metadata, session);
        let candidate = resolve_detected_status(
            candidate,
            is_dead,
            shell_stale,
            has_command_override,
            &pane_content,
            &self.tool,
        );

        let confirmed = self.confirm_detection(candidate, detection.visible);
        if let Some(status) = confirmed {
            self.status = status;
            if status == Status::Error {
                if self.last_error.is_none() {
                    self.last_error = Some(summarize_error_from_pane(&pane_content));
                }
            } else {
                self.last_error = None;
            }
        }
        self.detection.activity = activity;
        self.detection.captured_at = Some(captured_at);
        self.detection.rule = Some(detection.rule);
        tracing::trace!(target: "session.store",
            "status '{}': manifest rule={} candidate={:?} visible={} -> {:?}",
            self.title, detection.rule, candidate, detection.visible, self.status);
    }

    /// Whether `candidate` may be published now.
    fn confirm_detection(&mut self, candidate: Status, visible: bool) -> Option<Status> {
        let needs_confirmation =
            self.status == Status::Running && candidate == Status::Idle && !visible;
        if !needs_confirmation {
            self.detection.pending = None;
            return Some(candidate);
        }
        match self.detection.pending {
            Some(pending) if pending == candidate => {
                self.detection.pending = None;
                Some(candidate)
            }
            _ => {
                self.detection.pending = Some(candidate);
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::instance::test_helpers::*;

    #[test]
    fn test_skip_capture_requires_a_resolved_proposal() {
        // The regression this guard reintroduced once: a turn ends, the poll that sees the final
        // frame proposes Idle and holds it for a confirming poll, and the pane then draws nothing.
        assert!(
            !skip_capture(Some(100), Some(100), Some(101), false, true),
            "a pending proposal must be resolved, not skipped past"
        );
        assert!(skip_capture(Some(100), Some(100), Some(101), false, false));

        // A hook write changes the verdict without the pane drawing anything.
        assert!(!skip_capture(Some(100), Some(100), Some(101), true, false));
        // Fresh output, or no stamp to compare against at all.
        assert!(!skip_capture(Some(101), Some(100), Some(102), false, false));
        assert!(!skip_capture(None, None, Some(101), false, false));
        assert!(!skip_capture(Some(100), None, Some(101), false, false));
    }

    #[test]
    fn skip_capture_waits_for_a_capture_past_the_activity_second() {
        // `#{window_activity}` is an epoch second and the poll runs twice a second, so two frames
        // can share one value.
        assert!(
            !skip_capture(Some(100), Some(100), Some(100), false, false),
            "a capture taken inside the activity second proves nothing about \
             what was drawn later in it"
        );
        assert!(
            !skip_capture(Some(100), Some(100), Some(99), false, false),
            "nor does one taken before it"
        );
        assert!(
            skip_capture(Some(100), Some(100), Some(101), false, false),
            "a capture past the second is the proof the stamp claims to be"
        );
        // No capture recorded yet: nothing to date the stamp against.
        assert!(!skip_capture(Some(100), Some(100), None, false, false));
    }

    #[test]
    fn archived_row_with_frozen_live_status_settles_to_idle_on_poll() {
        // A row archived by a build that predates archive()'s status degrade arrives here still
        // claiming Waiting.
        for status in [Status::Running, Status::Waiting, Status::Starting] {
            let mut inst = Instance::new("test", "/tmp/test");
            inst.status = status;
            inst.archived_at = Some(Utc::now());
            inst.update_status_with_metadata(None, None);
            assert_eq!(inst.status, Status::Idle, "{status:?} must settle");
        }
        // Resting statuses on an archived row stay put.
        for status in [
            Status::Idle,
            Status::Stopped,
            Status::Error,
            Status::Unknown,
        ] {
            let mut inst = Instance::new("test", "/tmp/test");
            inst.status = status;
            inst.archived_at = Some(Utc::now());
            inst.update_status_with_metadata(None, None);
            assert_eq!(inst.status, status, "{status:?} should survive the poll");
        }
    }

    #[test]
    fn test_confirm_detection_holds_only_unwitnessed_idle() {
        // The one change a mid-redraw frame produces is a running session reading as a plain Idle,
        // so that is the only one that waits.
        let mut inst = Instance::new("test", "/tmp/test");
        inst.status = Status::Running;

        assert_eq!(inst.confirm_detection(Status::Idle, false), None);
        assert_eq!(inst.status, Status::Running);
        assert_eq!(
            inst.confirm_detection(Status::Idle, false),
            Some(Status::Idle)
        );

        // An idle a rule read off the agent's own chrome does not wait.
        inst.status = Status::Running;
        assert_eq!(
            inst.confirm_detection(Status::Idle, true),
            Some(Status::Idle)
        );

        // Nor does any other direction: a turn starting or a prompt appearing
        // must not be a poll late.
        inst.status = Status::Idle;
        assert_eq!(
            inst.confirm_detection(Status::Running, false),
            Some(Status::Running)
        );
        inst.status = Status::Running;
        assert_eq!(
            inst.confirm_detection(Status::Waiting, false),
            Some(Status::Waiting)
        );

        // A proposal that changes before it is confirmed starts over.
        inst.status = Status::Running;
        assert_eq!(inst.confirm_detection(Status::Idle, false), None);
        assert_eq!(
            inst.confirm_detection(Status::Waiting, false),
            Some(Status::Waiting)
        );
        assert!(inst.detection.pending.is_none());
    }

    #[test]
    fn test_archived_session_not_marked_error_when_tmux_gone() {
        // archiving kills the session's tmux on purpose. A subsequent status poll must not flip the
        // archived row to Error for the missing tmux.
        let mut inst = Instance::new("test", "/tmp/test");
        inst.archive();
        inst.update_status_with_metadata(None, None);
        assert_ne!(inst.status, Status::Error);
        assert_eq!(inst.status, Status::Idle);
        assert_eq!(inst.last_error, None);
    }

    #[test]
    fn test_archived_session_preserves_genuine_error() {
        // #2206 regression guard (passes on both trees): the archived guard never mutates status,
        // so a genuinely errored session keeps its Error state while archived.
        let mut inst = Instance::new("test", "/tmp/test");
        inst.archive();
        inst.status = Status::Error;
        inst.last_error = Some("agent crashed".to_string());
        inst.update_status_with_metadata(None, None);
        assert_eq!(inst.status, Status::Error);
        assert_eq!(inst.last_error.as_deref(), Some("agent crashed"));
    }

    #[test]
    fn test_archived_unarchived_genuine_error_roundtrips() {
        // archive then unarchive must stay status-preserving for a real failure.
        let mut inst = Instance::new("test", "/tmp/test");
        inst.archive();
        inst.status = Status::Error;
        inst.last_error = Some("agent crashed".to_string());
        inst.update_status_with_metadata(None, None);
        inst.unarchive();
        inst.update_status_with_metadata(None, None);
        assert_eq!(inst.status, Status::Error);
        assert_eq!(inst.last_error.as_deref(), Some("agent crashed"));
    }

    /// Regression guard for the false-Error-latch bug.
    #[test]
    #[serial_test::serial]
    fn test_confirmed_absent_session_still_latches_error() {
        let mut inst = Instance::new("test-absent", "/tmp/test-absent");
        inst.status = Status::Running;
        inst.last_error = None;
        inst.last_error_check = None;

        let guard = crate::tmux::SessionCacheGuard::capture();
        // Fresh cache, server reachable, but this instance's tmux session
        // name is not in it: a confirmed-absent session.
        guard.force_present(&["some_other_session"]);

        inst.update_status_with_metadata_inner(None, None);

        assert_eq!(inst.status, Status::Error);
        assert_eq!(inst.last_error.as_deref(), Some(TMUX_SESSION_GONE_ERROR));
        assert!(inst.last_error_check.is_some());
    }

    /// the poller / serve / ps loops resolve the session's live tmux name once against the batch
    /// snapshot.
    #[test]
    #[serial_test::serial]
    fn update_status_probes_the_resolved_name_not_the_title() {
        let resolved = format!("{}live_elsewhere_00000000", crate::tmux::SESSION_PREFIX);

        let guard = crate::tmux::SessionCacheGuard::capture();

        // Force the snapshot immediately before each probe. `#[serial]` only excludes other serial
        // tests, and the resolved-name pass below spawns tmux for pane metadata and capture.
        let mut inst = Instance::new("resolve-r2", "/tmp/resolve-r2");
        inst.status = Status::Running;
        guard.force_present(&[resolved.as_str()]);
        inst.update_status_with_metadata_inner(None, Some(&resolved));
        assert!(
            inst.ever_confirmed_present,
            "the passed resolved name must be the one probed"
        );
        assert_ne!(inst.status, Status::Error);

        let mut untold = Instance::new("resolve-r2", "/tmp/resolve-r2");
        untold.status = Status::Running;
        guard.force_present(&[resolved.as_str()]);
        untold.update_status_with_metadata_inner(None, None);
        assert_eq!(
            untold.status,
            Status::Error,
            "without the resolved name the title-derived name is absent from the cache"
        );
        assert_eq!(untold.last_error.as_deref(), Some(TMUX_SESSION_GONE_ERROR));
    }

    /// A tmux-server-unreachable probe (`SessionExistence::Unknown`) must not touch status,
    /// last_error, or last_error_check at all.
    #[test]
    #[serial_test::serial]
    fn test_unreachable_tmux_server_retains_running_status() {
        let mut inst = Instance::new("test-unknown", "/tmp/test-unknown");
        inst.status = Status::Running;
        inst.last_error = None;
        inst.last_error_check = None;

        let guard = crate::tmux::SessionCacheGuard::capture();
        // Fresh cache with no data.
        guard.force_unreachable();

        inst.update_status_with_metadata_inner(None, None);

        assert_eq!(inst.status, Status::Running);
        assert_eq!(inst.last_error, None);
        assert_eq!(inst.last_error_check, None);
    }

    /// Same `Unknown` retain-behavior, but starting from an already-set genuine `Status::Error`: an
    /// unreachable tmux server must not clear or overwrite a real prior failure either.
    #[test]
    #[serial_test::serial]
    fn test_unreachable_tmux_server_does_not_clear_existing_error() {
        let mut inst = Instance::new("test-unknown-error", "/tmp/test-unknown-error");
        inst.status = Status::Error;
        inst.last_error = Some("agent crashed".to_string());
        // None (rather than a stale Instant) so the 30s Error-recheck throttle above this code path
        // doesn't short-circuit before the probe we're testing ever runs.
        inst.last_error_check = None;

        let guard = crate::tmux::SessionCacheGuard::capture();
        guard.force_unreachable();

        inst.update_status_with_metadata_inner(None, None);

        assert_eq!(inst.status, Status::Error);
        assert_eq!(inst.last_error.as_deref(), Some("agent crashed"));
        assert_eq!(inst.last_error_check, None);
    }

    /// A session that has never been confirmed alive (`ever_confirmed_present` still `false`, e.g.
    /// `aoe add` without `--launch`) has nothing to "blip" from, so `Unknown` escalates to `Error`
    /// well before the long confirmed-present window.
    #[test]
    #[serial_test::serial]
    fn test_never_confirmed_present_unknown_escalates_after_fast_window() {
        let mut inst = Instance::new("test-never-present", "/tmp/test-never-present");
        inst.status = Status::Idle;
        inst.last_error = None;
        inst.last_error_check = None;
        assert!(!inst.ever_confirmed_present);
        inst.unknown_since = Some(
            std::time::Instant::now()
                - UNKNOWN_ERROR_WINDOW_NEVER_PRESENT
                - std::time::Duration::from_millis(1),
        );

        let guard = crate::tmux::SessionCacheGuard::capture();
        guard.force_unreachable();

        inst.update_status_with_metadata_inner(None, None);

        assert_eq!(inst.status, Status::Error);
        assert_eq!(
            inst.last_error.as_deref(),
            Some(TMUX_SERVER_UNREACHABLE_ERROR)
        );
        assert!(inst.last_error_check.is_some());
    }

    /// The never-confirmed-present fast window must still absorb a fresh `Unknown` streak (elapsed
    /// just under the window), otherwise every freshly-added, not-yet-launched session would flap
    /// to `Error` on the very first couple of poll ticks before tmux even has a chance to answer.
    #[test]
    #[serial_test::serial]
    fn test_never_confirmed_present_unknown_retains_status_below_fast_window() {
        let mut inst = Instance::new("test-never-present-fresh", "/tmp/test-never-present-fresh");
        inst.status = Status::Idle;
        inst.last_error = None;
        inst.last_error_check = None;
        assert!(!inst.ever_confirmed_present);
        inst.unknown_since =
            Some(std::time::Instant::now() - std::time::Duration::from_millis(500));

        let guard = crate::tmux::SessionCacheGuard::capture();
        guard.force_unreachable();

        inst.update_status_with_metadata_inner(None, None);

        assert_eq!(inst.status, Status::Idle);
        assert_eq!(inst.last_error, None);
        assert_eq!(inst.last_error_check, None);
    }

    /// The real production blip case.
    #[test]
    #[serial_test::serial]
    fn test_confirmed_present_unknown_retains_status_below_long_window() {
        let mut inst = Instance::new("test-confirmed-present", "/tmp/test-confirmed-present");
        inst.status = Status::Running;
        inst.last_error = None;
        inst.last_error_check = None;
        inst.ever_confirmed_present = true;
        // 11s: the max blip duration observed in production. Must not latch.
        inst.unknown_since = Some(std::time::Instant::now() - std::time::Duration::from_secs(11));

        let guard = crate::tmux::SessionCacheGuard::capture();
        guard.force_unreachable();

        inst.update_status_with_metadata_inner(None, None);

        assert_eq!(inst.status, Status::Running);
        assert_eq!(inst.last_error, None);
        assert_eq!(inst.last_error_check, None);
    }

    /// A session confirmed alive must still eventually latch `Error` once the tmux server has been
    /// unreachable past the long bounded window.
    #[test]
    #[serial_test::serial]
    fn test_confirmed_present_unknown_escalates_after_long_window() {
        let mut inst = Instance::new(
            "test-confirmed-present-dead",
            "/tmp/test-confirmed-present-dead",
        );
        inst.status = Status::Running;
        inst.last_error = None;
        inst.last_error_check = None;
        inst.ever_confirmed_present = true;
        inst.unknown_since = Some(
            std::time::Instant::now()
                - UNKNOWN_ERROR_WINDOW_CONFIRMED_PRESENT
                - std::time::Duration::from_millis(1),
        );

        let guard = crate::tmux::SessionCacheGuard::capture();
        guard.force_unreachable();

        inst.update_status_with_metadata_inner(None, None);

        assert_eq!(inst.status, Status::Error);
        assert_eq!(
            inst.last_error.as_deref(),
            Some(TMUX_SERVER_UNREACHABLE_ERROR)
        );
        assert!(inst.last_error_check.is_some());
    }

    /// `Present` must clear a stale `unknown_since` and flip `ever_confirmed_present` on, so a
    /// session that recovers from a real outage is treated as confirmed-alive (long window) on its
    /// next `Unknown` streak rather than falling back to the never-confirmed-present fast window.
    #[test]
    #[serial_test::serial]
    fn test_present_clears_unknown_since_and_marks_ever_confirmed_present() {
        let mut inst = Instance::new("present-clears-unknown", "/tmp/present-clears-unknown");
        inst.status = Status::Idle;
        inst.unknown_since = Some(std::time::Instant::now() - std::time::Duration::from_secs(2));
        assert!(!inst.ever_confirmed_present);
        let name = tmux::Session::generate_name(&inst.id, &inst.title);

        let guard = crate::tmux::SessionCacheGuard::capture();
        guard.force_present(&[name.as_str()]);

        inst.update_status_with_metadata_inner(None, None);

        assert!(inst.ever_confirmed_present);
        assert_eq!(inst.unknown_since, None);
    }

    #[test]
    #[serial_test::serial]
    fn test_update_status_with_metadata_seeds_baseline_without_restamp() {
        // a session loaded fresh from disk (e.g. TUI relaunch, or every tick of the daemon's
        // status_poll_loop) has no live observation history yet: `live_status_baseline` is `None`.
        let mut inst = Instance::new("test", "/tmp/test");
        inst.live_status_baseline = None;
        inst.status = Status::Starting;
        let stale_idle_entered_at = Some(Utc::now() - chrono::Duration::hours(2));
        let stale_last_accessed_at = Some(Utc::now() - chrono::Duration::hours(2));
        inst.idle_entered_at = stale_idle_entered_at;
        inst.last_accessed_at = stale_last_accessed_at;

        // Force detection to resolve to `Absent` -> Error deterministically: a fresh cache snapshot
        // that lists some other session but not this instance's.
        let _cache = force_session_absent();

        inst.update_status_with_metadata(None, None);

        // Detection confirms the session Absent, resolving to Error, which differs from the stale
        // disk `Starting`.
        assert_eq!(inst.status, Status::Error);
        assert_eq!(
            inst.idle_entered_at, stale_idle_entered_at,
            "first check after a fresh load must not clobber a stale-but-real idle_entered_at"
        );
        assert_eq!(
            inst.last_accessed_at, stale_last_accessed_at,
            "first check after a fresh load must not clobber a stale-but-real last_accessed_at"
        );
        assert_eq!(
            inst.live_status_baseline,
            Some(Status::Error),
            "the first check must seed the baseline for subsequent comparisons"
        );
    }

    #[test]
    #[serial_test::serial]
    fn test_update_status_with_metadata_keeps_last_accessed_at_on_transition() {
        // Once a live baseline is established, a real status change still re-anchors
        // idle_entered_at bookkeeping, but must NOT restamp last_accessed_at.
        let mut inst = Instance::new("test", "/tmp/test");
        inst.live_status_baseline = Some(Status::Idle);
        inst.status = Status::Idle;
        inst.idle_entered_at = Some(Utc::now() - chrono::Duration::hours(2));
        let user_touch = Some(Utc::now() - chrono::Duration::hours(2));
        inst.last_accessed_at = user_touch;

        // Force detection to resolve to `Absent` -> Error deterministically
        // (see #2936; without this the outcome is schedule-dependent).
        let _cache = force_session_absent();

        inst.update_status_with_metadata(None, None);

        // Detection confirms the session Absent, resolving to Error: a
        // genuine transition away from the established Idle baseline.
        assert_eq!(inst.status, Status::Error);
        assert_eq!(inst.idle_entered_at, None);
        assert_eq!(
            inst.last_accessed_at, user_touch,
            "a passive transition must not fabricate a user-gesture stamp"
        );
        assert_eq!(inst.live_status_baseline, Some(Status::Error));
    }

    #[test]
    #[serial_test::serial]
    fn test_update_status_with_metadata_twice_same_status_never_restamps() {
        // Two consecutive calls that both detect the same status (session confirmed Absent, so
        // detection is deterministically Error) must neither restamp.
        let mut inst = Instance::new("test", "/tmp/test");
        inst.live_status_baseline = Some(Status::Error);
        inst.status = Status::Error;
        let sentinel_idle = Some(Utc::now() - chrono::Duration::hours(3));
        let sentinel_accessed = Some(Utc::now() - chrono::Duration::hours(3));
        inst.idle_entered_at = sentinel_idle;
        inst.last_accessed_at = sentinel_accessed;

        // Force detection to resolve to `Absent` -> Error deterministically
        // (see #2936; without this the outcome is schedule-dependent).
        let _cache = force_session_absent();

        inst.update_status_with_metadata(None, None);
        assert_eq!(inst.status, Status::Error);
        assert_eq!(
            inst.idle_entered_at, sentinel_idle,
            "first call must not restamp"
        );
        assert_eq!(
            inst.last_accessed_at, sentinel_accessed,
            "first call must not restamp"
        );

        inst.update_status_with_metadata(None, None);
        assert_eq!(inst.status, Status::Error);
        assert_eq!(
            inst.idle_entered_at, sentinel_idle,
            "second call must not restamp"
        );
        assert_eq!(
            inst.last_accessed_at, sentinel_accessed,
            "second call must not restamp"
        );
    }

    #[test]
    fn test_update_status_with_metadata_transitions_never_stamp_last_accessed_at() {
        // Two back-to-back genuine transitions update the idle_entered_at bookkeeping and re-seed
        // the baseline between calls, but neither may touch last_accessed_at.
        let mut inst = Instance::new("test", "/tmp/test");
        inst.archive();
        inst.live_status_baseline = Some(Status::Idle);
        inst.status = Status::Unknown;
        let user_touch = Some(Utc::now() - chrono::Duration::hours(2));
        inst.last_accessed_at = user_touch;

        inst.update_status_with_metadata(None, None);
        assert_eq!(
            inst.status,
            Status::Unknown,
            "archived guard preserves resting status"
        );
        assert_eq!(inst.idle_entered_at, None, "non-idle transition clears it");
        assert_eq!(inst.last_accessed_at, user_touch);
        assert_eq!(inst.live_status_baseline, Some(Status::Unknown));

        inst.status = Status::Idle;
        inst.update_status_with_metadata(None, None);
        assert_eq!(inst.status, Status::Idle);
        assert!(
            inst.idle_entered_at.is_some(),
            "entering Idle re-anchors idle_entered_at"
        );
        assert_eq!(inst.last_accessed_at, user_touch);
        assert_eq!(inst.live_status_baseline, Some(Status::Idle));
    }

    #[test]
    fn test_instance_new_seeds_live_status_baseline_none() {
        // #2690 follow-up. A freshly constructed Instance has no live observation yet.
        let inst = Instance::new("test", "/tmp/test");
        assert_eq!(inst.live_status_baseline, None);
    }

    #[test]
    fn test_first_poll_after_status_write_does_not_fabricate_last_accessed_at() {
        // #2690 follow-up regression lock. Reproduces the pre-fix bug.
        let mut inst = Instance::new("test", "/tmp/test");
        assert_eq!(inst.last_accessed_at, None, "fixture invariant");
        // Simulate any post-construction status writer, `finalize_launch`
        // being the canonical one (`src/session/instance/start.rs`).
        inst.status = Status::Starting;

        inst.update_status_with_metadata(None, None);

        assert_eq!(
            inst.last_accessed_at, None,
            "first poll must not fabricate a `last_accessed_at` on an untouched session"
        );
    }

    struct KillTmuxOnDrop(String);

    impl Drop for KillTmuxOnDrop {
        fn drop(&mut self) {
            let _ = crate::tmux::tmux_command()
                .args(["kill-session", "-t", &self.0])
                .output();
        }
    }

    fn tmux_available() -> bool {
        crate::tmux::tmux_command()
            .arg("-V")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// End-to-end regression for #1913 through the real status pipeline.
    #[test]
    #[serial_test::serial]
    fn update_status_reconciles_running_hook_to_waiting_on_claude_approval_prompt() {
        if !tmux_available() {
            eprintln!("skipping: tmux not available");
            return;
        }

        let mut inst = Instance::new("aoe_test_1913_wait", "/tmp");
        assert_eq!(inst.tool, "claude");

        // Pane shows the approval prompt with the live spinner still active below it, the exact
        // shape from the issue screenshot.
        let pane = "  Bash command\n    \
touch /tmp/aoe_test_1913/marker.txt\n    Create marker file\n  \
Do you want to proceed?\n  \u{276f} 1. Yes\n    \
2. Yes, and always allow access to this project\n    3. No\n  \
Esc to cancel \u{b7} Tab to amend \u{b7} ctrl+e to explain\n\
\u{2736} Herding\u{2026} (53s \u{b7} \u{2193} 7.0k tokens)\n";
        let pane_file = std::env::temp_dir().join(format!("aoe_test_1913_{}.txt", inst.id));
        std::fs::write(&pane_file, pane).expect("write pane fixture");

        let session_name = tmux::Session::generate_name(&inst.id, &inst.title);
        let _guard = KillTmuxOnDrop(session_name.clone());
        // Single-quote the path so a temp dir with spaces or shell metacharacters (e.g. macOS
        // `$TMPDIR`) can't break the launch command.
        let quoted_pane_file =
            format!("'{}'", pane_file.to_string_lossy().replace('\'', r#"'\''"#));
        let launch = format!("cat {quoted_pane_file}; sleep 300");
        let created = crate::tmux::tmux_command()
            .args([
                "new-session",
                "-d",
                "-s",
                &session_name,
                "-x",
                "120",
                "-y",
                "40",
                &launch,
                ";",
                "set-option",
                "-t",
                &session_name,
                "pane-base-index",
                "0",
            ])
            .output()
            .expect("spawn tmux");
        assert!(
            created.status.success(),
            "tmux new-session failed: {}",
            String::from_utf8_lossy(&created.stderr)
        );

        // The clobbered hook state that produced the green row.
        use std::os::unix::fs::PermissionsExt;
        let base = crate::hooks::hook_base_path();
        if !base.exists() {
            std::fs::create_dir_all(&base).expect("create hook base dir");
        }
        std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o700))
            .expect("set hook base mode 0700");
        let dir = crate::hooks::hook_status_dir(&inst.id).expect("hook dir");
        std::fs::create_dir_all(&dir).expect("create hook dir");
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
            .expect("set hook instance mode 0700");
        std::fs::write(dir.join("status"), "running").expect("write status");
        assert_eq!(
            crate::hooks::read_hook_status(&inst.id),
            Some(Status::Running),
            "precondition: the raw hook signal is the Running that showed green"
        );

        // Wait for the pane to actually paint the cat output before the
        // authoritative read; a fixed sleep is flaky under parallel test load.
        let mut painted = false;
        for _ in 0..50 {
            let cap = crate::tmux::tmux_command()
                .args(["capture-pane", "-p", "-t", &session_name])
                .output();
            if let Ok(out) = cap {
                if String::from_utf8_lossy(&out.stdout).contains("Do you want to proceed?") {
                    painted = true;
                    break;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        assert!(painted, "approval prompt never painted into the tmux pane");

        crate::tmux::refresh_session_cache();
        inst.update_status_with_metadata(None, None);

        std::fs::remove_file(&pane_file).ok();
        crate::hooks::cleanup_hook_status_dir(&inst.id);

        assert_eq!(
            inst.status,
            Status::Waiting,
            "Claude blocked on an approval prompt must reconcile Running -> Waiting (#1913)"
        );
    }

    /// Pane metadata for a live agent pane: not dead, running the agent itself, so neither the
    /// dead-pane branch nor the stale-shell check spawns tmux behind the test.
    fn agent_pane_metadata(command: &str, window_activity: Option<i64>) -> tmux::PaneMetadata {
        tmux::PaneMetadata {
            pane_dead: false,
            pane_current_command: Some(command.to_string()),
            pane_start_command_is_protected: false,
            pane_pid: None,
            pane_title: None,
            window_activity,
            window_size: None,
        }
    }

    fn write_hook_status(instance_id: &str, status: &str) {
        use std::os::unix::fs::PermissionsExt;
        let base = crate::hooks::hook_base_path();
        if !base.exists() {
            std::fs::create_dir_all(&base).expect("create hook base dir");
        }
        std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o700))
            .expect("set hook base mode 0700");
        let dir = crate::hooks::hook_status_dir(instance_id).expect("hook dir");
        std::fs::create_dir_all(&dir).expect("create hook dir");
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
            .expect("set hook instance mode 0700");
        std::fs::write(dir.join("status"), status).expect("write status");
    }

    /// Install `rules` for `agent` under `profile` and return the registry
    /// guard that restores the profile's prior entries on drop.
    fn install_status_rules(
        profile: &str,
        agent: &str,
        rules: Vec<crate::session::config::StatusRule>,
    ) -> crate::tmux::status_rules::ProfileRegistryGuard {
        let guard = crate::tmux::status_rules::ProfileRegistryGuard::take(profile);
        let mut config = crate::session::Config::default();
        config
            .agents
            .entry(agent.to_string())
            .or_default()
            .status_rules = rules;
        crate::tmux::status_rules::install_from_config(profile, &config);
        guard
    }

    /// the hookless path's precedence is that a profile's own `[[agents.<name>.status_rules]]`
    /// decide, and the manifest path applies it.
    #[test]
    #[serial_test::serial]
    fn configured_rules_outrank_a_fresh_legacy_hook() {
        const PROFILE: &str = "legacy-hook-rules-precedence-test";
        let _registry = install_status_rules(
            PROFILE,
            "settl",
            vec![crate::session::config::StatusRule {
                status: crate::agents::HookStatus::Waiting,
                contains: Some("approve this?".to_string()),
                regex: None,
            }],
        );

        let mut inst = Instance::new("legacy-rules", "/tmp/legacy-rules");
        inst.source_profile = PROFILE.to_string();
        inst.tool = "settl".to_string();
        inst.status = Status::Idle;
        assert!(
            !tmux::detect::has_manifest(&inst.tool),
            "fixture invariant: settl must still be on the legacy hook path"
        );
        write_hook_status(&inst.id, "running");

        let name = tmux::Session::generate_name(&inst.id, &inst.title);
        let guard = crate::tmux::SessionCacheGuard::capture();
        guard.force_present(&[name.as_str()]);

        let metadata = agent_pane_metadata("settl", None);
        inst.update_status_with_metadata_inner(Some(&metadata), None);
        crate::hooks::cleanup_hook_status_dir(&inst.id);

        assert_eq!(
            inst.status,
            Status::Idle,
            "configured rules must decide over a fresh `running` hook write"
        );
    }

    /// the legacy hook path cleared `last_error` unconditionally, so an `error` write landed a row
    /// on Error with no explanation to render and no `last_error` for the 30s error re-check
    /// throttle to key on.
    #[test]
    #[serial_test::serial]
    fn legacy_error_hook_keeps_an_explanation() {
        const PROFILE: &str = "legacy-hook-error-explanation-test";
        // An empty config under a profile of its own: no rules for settl, so
        // this exercises the hook short-circuit rather than the rules path.
        let _registry = install_status_rules(PROFILE, "settl", Vec::new());

        let mut inst = Instance::new("legacy-error", "/tmp/legacy-error");
        inst.source_profile = PROFILE.to_string();
        inst.tool = "settl".to_string();
        inst.status = Status::Running;
        inst.last_error = None;
        write_hook_status(&inst.id, "error");

        let name = tmux::Session::generate_name(&inst.id, &inst.title);
        let guard = crate::tmux::SessionCacheGuard::capture();
        guard.force_present(&[name.as_str()]);
        let metadata = agent_pane_metadata("settl", None);

        inst.update_status_with_metadata_inner(Some(&metadata), None);
        assert_eq!(inst.status, Status::Error);
        assert!(
            inst.last_error.is_some(),
            "an Error hook must leave an explanation, which the error re-check \
             throttle also keys on"
        );

        // A standing explanation is the better one: it came from whatever
        // first diagnosed the failure.
        inst.last_error = Some("agent crashed".to_string());
        inst.last_error_check = None;
        inst.update_status_with_metadata_inner(Some(&metadata), None);
        crate::hooks::cleanup_hook_status_dir(&inst.id);

        assert_eq!(inst.status, Status::Error);
        assert_eq!(inst.last_error.as_deref(), Some("agent crashed"));
    }

    /// `#{window_activity}` is an epoch second and the poller runs twice a second, so a turn's last
    /// running frame and the idle frame that follows it can share one value.
    #[test]
    #[serial_test::serial]
    fn idle_frame_sharing_an_activity_second_is_still_observed() {
        if !tmux_available() {
            eprintln!("skipping: tmux not available");
            return;
        }

        let mut inst = Instance::new("aoe_test_3624_activity", "/tmp");
        assert_eq!(inst.tool, "claude");

        // The running frame is Claude's `active_spinner` shape. The idle frame matches no rule at
        // all, which is the unwitnessed Idle that has to wait for a confirming poll.
        let dir = std::env::temp_dir().join(format!("aoe_test_3624_{}", inst.id));
        std::fs::create_dir_all(&dir).expect("create fixture dir");
        let running_file = dir.join("running.txt");
        let idle_file = dir.join("idle.txt");
        let marker = dir.join("marker");
        std::fs::write(&running_file, "\u{2736} Working\u{2026} (5s)\n").expect("write running");
        std::fs::write(&idle_file, format!("{}turn over\n", "\n".repeat(150))).expect("write idle");

        let session_name = tmux::Session::generate_name(&inst.id, &inst.title);
        let _guard = KillTmuxOnDrop(session_name.clone());
        let quote =
            |p: &std::path::Path| format!("'{}'", p.to_string_lossy().replace('\'', r#"'\''"#));
        // The marker file is the test's clock: the pane holds the running
        // frame until it appears, then draws the idle frame and stops.
        let launch = format!(
            "cat {}; until [ -f {} ]; do sleep 0.05; done; cat {}; sleep 300",
            quote(&running_file),
            quote(&marker),
            quote(&idle_file),
        );
        let created = crate::tmux::tmux_command()
            .args([
                "new-session",
                "-d",
                "-s",
                &session_name,
                "-x",
                "120",
                "-y",
                "40",
                &launch,
            ])
            .output()
            .expect("spawn tmux");
        assert!(
            created.status.success(),
            "tmux new-session failed: {}",
            String::from_utf8_lossy(&created.stderr)
        );

        let wait_for_pane = |needle: &str| {
            for _ in 0..100 {
                let cap = crate::tmux::tmux_command()
                    .args(["capture-pane", "-p", "-t", &session_name])
                    .output();
                if let Ok(out) = cap {
                    if String::from_utf8_lossy(&out.stdout).contains(needle) {
                        return true;
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            false
        };
        assert!(wait_for_pane("Working"), "running frame never painted");

        let cache = crate::tmux::SessionCacheGuard::capture();
        cache.force_present(&[session_name.as_str()]);

        let poll = |inst: &mut Instance, activity: Option<i64>| {
            let metadata = agent_pane_metadata("claude", activity);
            inst.update_status_with_metadata_inner(Some(&metadata), Some(&session_name));
        };

        // The activity value both frames share. tmux stamps it in whichever second the output
        // landed in, which is the second this first capture is taken in.
        poll(&mut inst, Some(0));
        assert_eq!(inst.status, Status::Running, "running frame must be seen");
        let shared = inst
            .detection
            .captured_at
            .expect("capture stamps its second");
        inst.detection.activity = Some(shared);

        std::fs::File::create(&marker).expect("touch marker");
        assert!(wait_for_pane("turn over"), "idle frame never painted");

        poll(&mut inst, Some(shared));
        assert_eq!(
            inst.status,
            Status::Running,
            "an unwitnessed Idle waits one poll before it publishes"
        );
        assert_eq!(
            inst.detection.pending,
            Some(Status::Idle),
            "the final frame must be captured, not skipped as unchanged (#3624)"
        );

        poll(&mut inst, Some(shared));
        std::fs::remove_dir_all(&dir).ok();

        assert_eq!(
            inst.status,
            Status::Idle,
            "the confirming poll must publish the idle the final frame showed"
        );
    }

    /// `aoe ps`, `aoe status` and the worktree-edit guards observe a session once and exit, so the
    /// confirming poll an unwitnessed `Running -> Idle` waits for never arrives.
    #[test]
    #[serial_test::serial]
    fn a_single_observation_publishes_an_unwitnessed_idle() {
        if !tmux_available() {
            eprintln!("skipping: tmux not available");
            return;
        }

        let mut polled = Instance::new("aoe_test_3712_polled", "/tmp");
        // Guard, not a constant assertion: the manifest path is only reached
        // for a tool that has one.
        assert_eq!(polled.tool, "claude");

        // A parked Claude prompt carrying half-typed text.
        let pane = "earlier output\n\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\n\u{276f} half typed prompt\n\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\n  \u{23f5}\u{23f5} auto mode on (shift+tab to cycle)\n";
        let pane_file = std::env::temp_dir().join(format!("aoe_test_3712_{}.txt", polled.id));
        std::fs::write(&pane_file, pane).expect("write pane fixture");

        let session_name = tmux::Session::generate_name(&polled.id, &polled.title);
        let _guard = KillTmuxOnDrop(session_name.clone());
        let quoted = format!("'{}'", pane_file.to_string_lossy().replace('\'', r#"'\''"#));
        let created = crate::tmux::tmux_command()
            .args([
                "new-session",
                "-d",
                "-s",
                &session_name,
                "-x",
                "120",
                "-y",
                "40",
                &format!("cat {quoted}; sleep 300"),
            ])
            .output()
            .expect("spawn tmux");
        assert!(
            created.status.success(),
            "tmux new-session failed: {}",
            String::from_utf8_lossy(&created.stderr)
        );

        let mut painted = false;
        for _ in 0..100 {
            let cap = crate::tmux::tmux_command()
                .args(["capture-pane", "-p", "-t", &session_name])
                .output();
            if let Ok(out) = cap {
                if String::from_utf8_lossy(&out.stdout).contains("half typed prompt") {
                    painted = true;
                    break;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        std::fs::remove_file(&pane_file).ok();
        assert!(painted, "parked prompt never painted into the tmux pane");

        let cache = crate::tmux::SessionCacheGuard::capture();
        cache.force_present(&[session_name.as_str()]);
        let metadata = agent_pane_metadata("claude", None);

        // Both rows come off disk on `Running`, which is what the CLI reads.
        polled.status = Status::Running;
        polled.update_status_with_metadata(Some(&metadata), Some(&session_name));
        assert_eq!(
            polled.status,
            Status::Running,
            "a repeating poller holds an unwitnessed Idle for the poll that agrees"
        );
        assert_eq!(polled.detection.pending, Some(Status::Idle));

        // A one-shot reader starts from a bare disk load: no proposal on
        // record, and none it can ever meet.
        let mut once = Instance::new("aoe_test_3712_once", "/tmp");
        once.status = Status::Running;
        once.update_status_once(Some(&metadata), Some(&session_name));
        assert_eq!(
            once.status,
            Status::Idle,
            "one observation is all this caller gets, so its proposal decides (#3712)"
        );
        assert_eq!(once.detection.pending, None);
    }
}
