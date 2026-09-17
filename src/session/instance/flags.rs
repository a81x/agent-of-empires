//! User-facing triage state: archive, trash, favorite, pin, snooze, unread,
//! color, and the idle bookkeeping they read.

use super::*;

/// The MVP palette for the per-session color label. Kept deliberately small and status-oriented.
pub const SESSION_COLORS: &[&str] = &["red", "amber", "green"];

/// True when `color` is a member of the [`SESSION_COLORS`] palette.
pub fn is_valid_session_color(color: &str) -> bool {
    SESSION_COLORS.contains(&color)
}

/// Mutually-exclusive lifecycle bucket a session belongs to, computed by
/// `Instance::effective_bucket()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionBucket {
    Active,
    Archived,
    Trashed,
}

impl Instance {
    /// Stamp `last_accessed_at` to the current time AND wake the session from any sink state.
    pub fn touch_last_accessed(&mut self) {
        self.last_accessed_at = Some(Utc::now());
        self.archived_at = None;
        self.snoozed_until = None;
        self.idle_dormant_since = None;
    }

    /// Whether this session's structured view worker was auto-stopped for inactivity and should not
    /// be respawned by the reconciler until the user wakes it.
    pub fn is_idle_dormant(&self) -> bool {
        self.idle_dormant_since.is_some()
    }

    /// Mark the session dormant after its structured view worker was auto-stopped
    /// for inactivity. Idempotent: re-marking refreshes the timestamp.
    pub fn mark_idle_dormant(&mut self) {
        self.idle_dormant_since = Some(Utc::now());
    }

    /// Whether this session should render as "dormant" (worker auto-stopped for inactivity,
    /// resumable) rather than with its raw `status`.
    pub fn is_shown_dormant(&self) -> bool {
        self.is_idle_dormant() && self.status != Status::Stopped
    }

    /// Mark the session archived. Archived sessions sink to the bottom of the Attention sort and
    /// render in italic+dim style, but remain visible.
    pub fn archive(&mut self) {
        self.archived_at = Some(Utc::now());
        self.favorited_at = None;
        self.snoozed_until = None;
        self.pinned_at = None;
        self.settle_archived_status();
    }

    /// Idle is the resting state an archived row can truthfully claim; see `archive`.
    pub(crate) fn settle_archived_status(&mut self) {
        if matches!(
            self.status,
            Status::Running | Status::Waiting | Status::Starting
        ) {
            self.status = Status::Idle;
        }
    }

    pub fn unarchive(&mut self) {
        self.archived_at = None;
        self.idle_dormant_since = None;
    }

    pub fn is_archived(&self) -> bool {
        self.archived_at.is_some()
    }

    /// Soft-delete the session into the trash bucket. Stops the live session (handled by the
    /// caller.
    pub fn trash(&mut self) {
        if self.trashed_at.is_none() {
            self.trashed_at = Some(Utc::now());
        }
    }

    /// Restore a trashed session back to its prior bucket (active or
    /// archived, depending on the preserved sibling flags). Idempotent.
    pub fn untrash(&mut self) {
        self.trashed_at = None;
    }

    pub fn is_trashed(&self) -> bool {
        self.trashed_at.is_some()
    }

    /// The mutually-exclusive lifecycle bucket a session renders in. Precedence is `Trashed >
    /// Archived > Active`.
    pub fn effective_bucket(&self) -> SessionBucket {
        if self.is_trashed() {
            SessionBucket::Trashed
        } else if self.is_archived() {
            SessionBucket::Archived
        } else {
            SessionBucket::Active
        }
    }

    /// Mark the session favorite. Sibling of `archive`, with opposite semantics.
    pub fn favorite(&mut self) {
        self.favorited_at = Some(Utc::now());
        self.archived_at = None;
        self.snoozed_until = None;
    }

    pub fn unfavorite(&mut self) {
        self.favorited_at = None;
    }

    pub fn is_favorited(&self) -> bool {
        self.favorited_at.is_some()
    }

    /// Set (or clear, with `None`) the per-session color label. Only a value in the
    /// [`SESSION_COLORS`] palette is accepted.
    pub fn set_color(&mut self, color: Option<String>) -> Result<(), String> {
        match color {
            None => self.color = None,
            Some(c) => {
                if !is_valid_session_color(&c) {
                    return Err(format!(
                        "invalid color {:?}; expected one of: {}, or none",
                        c,
                        SESSION_COLORS.join(", ")
                    ));
                }
                self.color = Some(c);
            }
        }
        Ok(())
    }

    /// Read the agent-raised urgent flag from `attention.json`.
    pub fn is_urgent(&self) -> bool {
        if self.is_archived() || self.is_snoozed() {
            return false;
        }
        crate::hooks::read_hook_urgent(&self.id)
    }

    /// Temporarily defer this session for `minutes`; sets `snoozed_until` to `Utc::now() +
    /// minutes`.
    pub fn snooze(&mut self, minutes: u32) {
        self.snoozed_until = Some(Utc::now() + chrono::Duration::minutes(minutes as i64));
        self.pinned_at = None;
    }

    pub fn unsnooze(&mut self) {
        self.snoozed_until = None;
    }

    /// True if the session carries the unread marker.
    pub fn is_unread(&self) -> bool {
        self.unread
    }

    /// Mark the session unread. Used both by the auto-mark on a finished turn (`Running -> Idle`)
    /// and the manual "Mark as unread" action.
    pub fn mark_unread(&mut self) {
        self.unread = true;
    }

    /// Clear the unread marker. Used whenever the user engages with the session (open/attach,
    /// live-send, click, dwell) and by the explicit "Mark as read" action.
    pub fn mark_read(&mut self) {
        self.unread = false;
    }

    /// Manual toggle (`U`): read -> unread; unread -> read.
    pub fn toggle_unread(&mut self) {
        self.unread = !self.unread;
    }

    /// True if `snoozed_until` is set AND in the future. Expired snoozes return false so the row
    /// naturally rejoins the main sort on the next render.
    pub fn is_snoozed(&self) -> bool {
        self.snoozed_until.map(|t| t > Utc::now()).unwrap_or(false)
    }

    /// Combined "don't bother me" sink-state check: trashed, snoozed, or archived.
    pub fn is_dismissed(&self) -> bool {
        self.is_trashed() || self.is_snoozed() || self.is_archived()
    }

    /// Remaining snooze duration as a `chrono::Duration`, or `None` if the
    /// session isn't snoozed (or the timestamp has already expired).
    pub fn snooze_remaining(&self) -> Option<chrono::Duration> {
        self.snoozed_until.and_then(|t| {
            let delta = t - Utc::now();
            if delta > chrono::Duration::zero() {
                Some(delta)
            } else {
                None
            }
        })
    }

    /// Mark this session pinned. Pin is a web-only surfacing primitive.
    pub fn pin(&mut self) {
        self.pinned_at = Some(Utc::now());
        self.archived_at = None;
        self.snoozed_until = None;
    }

    pub fn unpin(&mut self) {
        self.pinned_at = None;
    }

    pub fn is_pinned(&self) -> bool {
        self.pinned_at.is_some()
    }

    /// Time elapsed since this session most recently transitioned into `Idle`.
    pub fn idle_age(&self) -> Option<std::time::Duration> {
        if self.status != Status::Idle {
            return None;
        }
        let since = self.idle_entered_at?;
        (Utc::now() - since).to_std().ok()
    }

    /// True iff this session should keep the machine awake: it is active (`Running`, `Waiting`,
    /// `Starting`, or `Creating`), or it went idle less than `window` ago.
    pub fn has_recent_activity(&self, window: std::time::Duration) -> bool {
        matches!(
            self.status,
            Status::Running | Status::Waiting | Status::Starting | Status::Creating
        ) || matches!(self.idle_age(), Some(age) if age < window)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_color_accepts_palette_and_clears_with_none() {
        let mut inst = Instance::new("color-test", "/tmp");
        assert_eq!(inst.color, None);

        for c in SESSION_COLORS {
            inst.set_color(Some((*c).to_string())).unwrap();
            assert_eq!(inst.color.as_deref(), Some(*c));
        }

        inst.set_color(None).unwrap();
        assert_eq!(inst.color, None);
    }

    #[test]
    fn set_color_rejects_unknown_color_and_leaves_prior_value() {
        let mut inst = Instance::new("color-test", "/tmp");
        inst.set_color(Some("green".to_string())).unwrap();

        let err = inst
            .set_color(Some("chartreuse".to_string()))
            .expect_err("unknown color must be rejected");
        assert!(
            err.contains("chartreuse"),
            "error should name the value: {err}"
        );
        // A rejected write must not clobber the previously stored color.
        assert_eq!(inst.color.as_deref(), Some("green"));
    }

    #[test]
    fn is_valid_session_color_matches_palette() {
        assert!(is_valid_session_color("red"));
        assert!(is_valid_session_color("amber"));
        assert!(is_valid_session_color("green"));
        assert!(!is_valid_session_color("blue"));
        assert!(!is_valid_session_color(""));
        assert!(!is_valid_session_color("Red"));
    }

    /// `touch_last_accessed` is what `aoe send` and the TUI dispatch path call when the user
    /// interacts with a session.
    #[test]
    fn test_touch_last_accessed_clears_archived() {
        let mut inst = Instance::new("test", "/tmp/test");
        inst.archive();
        assert!(inst.is_archived());
        inst.touch_last_accessed();
        assert!(!inst.is_archived());
        assert!(inst.last_accessed_at.is_some());
    }

    #[test]
    fn test_touch_last_accessed_clears_snooze() {
        let mut inst = Instance::new("test", "/tmp/test");
        inst.snooze(30);
        assert!(inst.is_snoozed());
        inst.touch_last_accessed();
        assert!(!inst.is_snoozed());
    }

    #[test]
    fn test_touch_last_accessed_clears_idle_dormant() {
        let mut inst = Instance::new("test", "/tmp/test");
        inst.mark_idle_dormant();
        assert!(inst.is_idle_dormant());
        inst.touch_last_accessed();
        assert!(!inst.is_idle_dormant());
    }

    #[test]
    fn test_unarchive_clears_idle_dormant() {
        let mut inst = Instance::new("test", "/tmp/test");
        inst.archive();
        inst.mark_idle_dormant();
        assert!(inst.is_archived());
        assert!(inst.is_idle_dormant());

        inst.unarchive();

        assert!(!inst.is_archived());
        assert!(
            !inst.is_idle_dormant(),
            "unarchive should wake sessions blocked by idle auto-stop"
        );
    }

    #[test]
    fn test_mark_unread_and_mark_read_are_idempotent() {
        let mut inst = Instance::new("test", "/tmp/test");
        assert!(!inst.is_unread());
        // read -> unread
        inst.mark_unread();
        assert!(inst.is_unread());
        // unread -> unread (idempotent)
        inst.mark_unread();
        assert!(inst.is_unread());
        // unread -> read
        inst.mark_read();
        assert!(!inst.is_unread());
        // read -> read (idempotent)
        inst.mark_read();
        assert!(!inst.is_unread());
    }

    #[test]
    fn test_toggle_unread_round_trips() {
        let mut inst = Instance::new("test", "/tmp/test");
        // read -> unread
        inst.toggle_unread();
        assert!(inst.is_unread());
        // unread -> read
        inst.toggle_unread();
        assert!(!inst.is_unread());
    }

    #[test]
    fn test_unread_serde_round_trip() {
        // Absent field deserializes to false (older sessions.json).
        let inst: Instance = serde_json::from_value(serde_json::json!({
            "id": "abc",
            "title": "t",
            "project_path": "/tmp",
            "tool": "claude",
            "status": "idle",
            "created_at": "2026-01-01T00:00:00Z",
        }))
        .expect("deserialize without unread");
        assert!(!inst.unread);

        // Round-trips when set, and is omitted when false.
        let mut set = Instance::new("t", "/tmp");
        set.unread = true;
        let json = serde_json::to_value(&set).unwrap();
        assert_eq!(json["unread"], serde_json::json!(true));
        let back: Instance = serde_json::from_value(json).unwrap();
        assert!(back.unread);

        let read = Instance::new("t", "/tmp");
        let json = serde_json::to_value(&read).unwrap();
        assert!(
            json.get("unread").is_none(),
            "false must skip serialization"
        );
    }

    #[test]
    fn test_mark_idle_dormant_sets_marker() {
        let mut inst = Instance::new("test", "/tmp/test");
        assert!(!inst.is_idle_dormant());
        inst.mark_idle_dormant();
        assert!(inst.is_idle_dormant());
        assert!(inst.idle_dormant_since.is_some());
    }

    #[test]
    fn test_is_shown_dormant_precedence() {
        // Idle + dormant marker: the idle-reaper's output, presents dormant.
        let mut idle_reaped = Instance::new("test", "/tmp/test");
        idle_reaped.status = Status::Idle;
        idle_reaped.mark_idle_dormant();
        assert!(idle_reaped.is_shown_dormant());

        // Stopped + dormant marker: a deliberate Stop (which also marks dormant).
        let mut deliberate_stop = Instance::new("test", "/tmp/test");
        deliberate_stop.status = Status::Stopped;
        deliberate_stop.mark_idle_dormant();
        assert!(!deliberate_stop.is_shown_dormant());

        // Idle, no marker: a live idle session, unaffected.
        let mut live_idle = Instance::new("test", "/tmp/test");
        live_idle.status = Status::Idle;
        assert!(!live_idle.is_shown_dormant());

        // Running, no marker: live, unaffected.
        let mut running = Instance::new("test", "/tmp/test");
        running.status = Status::Running;
        assert!(!running.is_shown_dormant());
    }

    #[test]
    fn test_touch_last_accessed_preserves_favorite() {
        let mut inst = Instance::new("test", "/tmp/test");
        inst.favorite();
        assert!(inst.is_favorited());
        inst.touch_last_accessed();
        // Favorite is orthogonal to sink states; user interaction must not
        // clear it.
        assert!(inst.is_favorited());
    }

    #[test]
    fn test_archive_clears_snooze() {
        // Direct mutator test (no merge): the data-layer contract is that archive is mutually
        // exclusive with every other triage flag.
        let mut inst = Instance::new("s", "/tmp/x");
        inst.snooze(15);
        assert!(inst.is_snoozed());
        inst.archive();
        assert!(inst.is_archived());
        assert!(!inst.is_snoozed());
    }

    #[test]
    fn test_pin_clears_archive_and_snooze() {
        let mut inst = Instance::new("s", "/tmp/x");
        inst.archive();
        assert!(inst.is_archived());
        inst.pin();
        assert!(inst.is_pinned());
        assert!(!inst.is_archived());
        assert!(!inst.is_snoozed());

        inst.snooze(15);
        assert!(inst.is_snoozed());
        inst.pin();
        assert!(inst.is_pinned());
        assert!(!inst.is_snoozed());
    }

    #[test]
    fn test_archive_clears_pin() {
        let mut inst = Instance::new("s", "/tmp/x");
        inst.pin();
        assert!(inst.is_pinned());
        inst.archive();
        assert!(inst.is_archived());
        assert!(!inst.is_pinned());
    }

    #[test]
    fn test_trash_untrash_roundtrip() {
        let mut inst = Instance::new("s", "/tmp/x");
        assert!(!inst.is_trashed());
        assert_eq!(inst.effective_bucket(), SessionBucket::Active);

        inst.trash();
        assert!(inst.is_trashed());
        assert_eq!(inst.effective_bucket(), SessionBucket::Trashed);

        inst.untrash();
        assert!(!inst.is_trashed());
        assert_eq!(inst.effective_bucket(), SessionBucket::Active);
    }

    #[test]
    fn test_trash_preserves_sibling_triage_flags() {
        let mut inst = Instance::new("s", "/tmp/x");
        inst.favorite();
        inst.pin();
        assert!(inst.is_favorited());
        assert!(inst.is_pinned());

        inst.trash();
        // Trash wins the bucket but leaves the decorations intact so
        // restore is faithful (a trashed favorite comes back a favorite).
        assert_eq!(inst.effective_bucket(), SessionBucket::Trashed);
        assert!(inst.is_favorited(), "favorite preserved across trash");
        assert!(inst.is_pinned(), "pin preserved across trash");

        inst.untrash();
        assert!(inst.is_favorited());
        assert!(inst.is_pinned());
    }

    #[test]
    fn test_effective_bucket_trash_beats_archive() {
        let mut inst = Instance::new("s", "/tmp/x");
        inst.archive();
        assert_eq!(inst.effective_bucket(), SessionBucket::Archived);
        inst.trash();
        assert_eq!(
            inst.effective_bucket(),
            SessionBucket::Trashed,
            "trash takes precedence over archive in bucketing"
        );
        // archived_at is preserved, so restore returns to the archived bucket.
        assert!(inst.is_archived());
        inst.untrash();
        assert_eq!(inst.effective_bucket(), SessionBucket::Archived);
    }

    #[test]
    fn test_trashed_at_serde_roundtrip_and_default() {
        // A non-trashed instance omits trashed_at on the wire (skip_serializing_if), so
        // deserializing it exercises the missing-field path that legacy rows hit.
        let fresh = Instance::new("s", "/tmp/x");
        let fresh_json = serde_json::to_string(&fresh).expect("serialize fresh");
        assert!(
            !fresh_json.contains("trashed_at"),
            "None trashed_at must not be serialized"
        );
        let parsed: Instance = serde_json::from_str(&fresh_json).expect("parse fresh");
        assert!(!parsed.is_trashed(), "missing trashed_at => None");

        let mut inst = Instance::new("s", "/tmp/x");
        inst.trash();
        let json = serde_json::to_string(&inst).expect("serialize");
        let back: Instance = serde_json::from_str(&json).expect("round-trip");
        assert!(back.is_trashed());
    }

    #[test]
    fn test_snooze_clears_pin() {
        let mut inst = Instance::new("s", "/tmp/x");
        inst.pin();
        assert!(inst.is_pinned());
        inst.snooze(30);
        assert!(inst.is_snoozed());
        assert!(!inst.is_pinned());
    }

    #[test]
    fn test_touch_last_accessed_preserves_pin() {
        let mut inst = Instance::new("s", "/tmp/x");
        inst.pin();
        assert!(inst.is_pinned());
        inst.touch_last_accessed();
        // Pin is an explicit user surfacing signal, not a sink state.
        // User interaction (send, attach) must NOT clear it.
        assert!(inst.is_pinned());
    }

    #[test]
    fn test_pin_and_favorite_coexist() {
        let mut inst = Instance::new("s", "/tmp/x");
        inst.favorite();
        assert!(inst.is_favorited());
        inst.pin();
        // Pin and favorite drive different surfaces (TUI Attention vs web
        // sidebar). They must coexist; pinning does NOT clear favorite.
        assert!(inst.is_pinned());
        assert!(inst.is_favorited());

        let mut inst2 = Instance::new("s2", "/tmp/x");
        inst2.pin();
        inst2.favorite();
        // Same in reverse: favoriting does NOT clear pin.
        assert!(inst2.is_pinned());
        assert!(inst2.is_favorited());
    }

    #[test]
    fn test_idle_age_returns_none_for_non_idle() {
        let mut inst = Instance::new("test", "/tmp/test");
        inst.status = Status::Running;
        inst.idle_entered_at = Some(Utc::now() - chrono::Duration::seconds(60));
        assert_eq!(inst.idle_age(), None);
    }

    #[test]
    fn test_idle_age_returns_none_when_no_timestamp() {
        let mut inst = Instance::new("test", "/tmp/test");
        inst.status = Status::Idle;
        inst.idle_entered_at = None;
        assert_eq!(inst.idle_age(), None);
    }

    #[test]
    fn test_idle_age_returns_positive_duration() {
        let mut inst = Instance::new("test", "/tmp/test");
        inst.status = Status::Idle;
        inst.idle_entered_at = Some(Utc::now() - chrono::Duration::seconds(5));
        let age = inst.idle_age().expect("idle age should be present");
        // Allow generous slack so the test isn't flaky on slow CI.
        assert!(age.as_secs() >= 4 && age.as_secs() <= 30);
    }

    #[test]
    fn test_idle_age_clamps_negative_to_none() {
        let mut inst = Instance::new("test", "/tmp/test");
        inst.status = Status::Idle;
        // Future timestamp (clock skew, hand-crafted state).
        inst.idle_entered_at = Some(Utc::now() + chrono::Duration::seconds(60));
        assert_eq!(inst.idle_age(), None);
    }

    #[test]
    fn test_has_recent_activity_active_statuses_are_true() {
        let window = std::time::Duration::from_secs(15 * 60);
        for status in [
            Status::Running,
            Status::Waiting,
            Status::Starting,
            Status::Creating,
        ] {
            let mut inst = Instance::new("test", "/tmp/test");
            inst.status = status;
            assert!(
                inst.has_recent_activity(window),
                "{status:?} should keep the machine awake"
            );
        }
    }

    #[test]
    fn test_has_recent_activity_inactive_statuses_are_false() {
        let window = std::time::Duration::from_secs(15 * 60);
        for status in [
            Status::Stopped,
            Status::Error,
            Status::Unknown,
            Status::Deleting,
        ] {
            let mut inst = Instance::new("test", "/tmp/test");
            inst.status = status;
            assert!(
                !inst.has_recent_activity(window),
                "{status:?} must not hold the sleep-inhibit assertion"
            );
        }
    }

    #[test]
    fn test_has_recent_activity_idle_within_window_is_true() {
        let window = std::time::Duration::from_secs(15 * 60);
        let mut inst = Instance::new("test", "/tmp/test");
        inst.status = Status::Idle;
        inst.idle_entered_at = Some(Utc::now() - chrono::Duration::seconds(60));
        assert!(inst.has_recent_activity(window));
    }

    #[test]
    fn test_has_recent_activity_idle_past_window_is_false() {
        let window = std::time::Duration::from_secs(15 * 60);
        let mut inst = Instance::new("test", "/tmp/test");
        inst.status = Status::Idle;
        inst.idle_entered_at = Some(Utc::now() - chrono::Duration::minutes(30));
        assert!(!inst.has_recent_activity(window));
    }

    #[test]
    fn test_has_recent_activity_idle_without_timestamp_is_false() {
        let window = std::time::Duration::from_secs(15 * 60);
        let mut inst = Instance::new("test", "/tmp/test");
        inst.status = Status::Idle;
        inst.idle_entered_at = None;
        assert!(!inst.has_recent_activity(window));
    }
    #[test]
    fn archive_settles_live_interaction_status_to_idle() {
        for status in [Status::Running, Status::Waiting, Status::Starting] {
            let mut inst = Instance::new("test", "/tmp/test");
            inst.status = status;
            inst.archive();
            assert!(inst.is_archived());
            assert_eq!(
                inst.status,
                Status::Idle,
                "{status:?} cannot be true of a row whose tmux archive tore down"
            );
        }
    }

    #[test]
    fn archive_leaves_resting_statuses_alone() {
        for status in [
            Status::Idle,
            Status::Stopped,
            Status::Error,
            Status::Unknown,
        ] {
            let mut inst = Instance::new("test", "/tmp/test");
            inst.status = status;
            inst.archive();
            assert_eq!(inst.status, status, "{status:?} should survive archive");
        }
    }
}
