//! Live per-session control state, folded once at the publish choke point.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use super::state::{AcpState, Event};

/// A session's folded control state and how far it has been folded.
#[derive(Debug, Clone)]
struct Cached {
    state: AcpState,
    /// Highest seq folded in.
    last_seq: u64,
}

/// Per-session slot.
type Slot = Arc<Mutex<Option<Cached>>>;

#[derive(Debug, Default)]
pub struct ControlStateCache {
    sessions: Mutex<HashMap<String, Slot>>,
}

/// Recover a poisoned lock rather than propagating the panic: a poisoned
/// control-state mutex means some other thread panicked mid-fold, and the
/// worst case here is a stale projection, which the seq guard below evicts.
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

impl ControlStateCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// The slot for `session_id`, creating an empty one if absent.
    fn slot(&self, session_id: &str) -> Slot {
        let mut map = lock(&self.sessions);
        Arc::clone(
            map.entry(session_id.to_string())
                .or_insert_with(|| Arc::new(Mutex::new(None))),
        )
    }

    /// Fold `event` into the session's cached state, if it has one.
    pub fn apply_if_cached(&self, session_id: &str, seq: u64, event: &Event) {
        let slot = self.slot(session_id);
        let mut guard = lock(&slot);
        let Some(cached) = guard.as_mut() else {
            return;
        };
        if seq == cached.last_seq {
            // Exact repeat of the seq we already folded: a benign publish
            // retry (the store reports these as primary-key collisions).
            return;
        }
        if seq != cached.last_seq + 1 {
            *guard = None;
            return;
        }
        if cached.state.apply_event(event.clone()).is_err() {
            *guard = None;
            return;
        }
        cached.last_seq = seq;
    }

    /// Drop a session's fold.
    pub fn forget(&self, session_id: &str) {
        let mut map = lock(&self.sessions);
        map.remove(session_id);
    }

    /// The session's control state, running `hydrate` on a miss.
    pub fn get_or_hydrate(
        &self,
        session_id: &str,
        hydrate: impl FnOnce() -> (AcpState, u64),
    ) -> AcpState {
        let slot = self.slot(session_id);
        let mut guard = lock(&slot);
        if let Some(cached) = guard.as_ref() {
            return cached.state.clone();
        }
        let (state, last_seq) = hydrate();
        *guard = Some(Cached {
            state: state.clone(),
            last_seq,
        });
        state
    }

    #[cfg(test)]
    fn is_cached(&self, session_id: &str) -> bool {
        let slot = self.slot(session_id);
        let guard = lock(&slot);
        guard.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::state::{AcpSessionId, AgentName};

    fn seed() -> AcpState {
        AcpState::new(AcpSessionId("s-1".into()), AgentName("claude".into()), None)
    }

    fn prompt() -> Event {
        Event::UserPromptSent {
            text: "go".into(),
            attachments: Vec::new(),
            prompt_id: None,
        }
    }

    fn stopped() -> Event {
        Event::Stopped {
            reason: "end_turn".into(),
        }
    }

    #[test]
    fn a_session_nothing_hydrated_stays_uncached() {
        let cache = ControlStateCache::new();
        cache.apply_if_cached("s-1", 42, &prompt());
        assert!(!cache.is_cached("s-1"));

        let mut hydrated = 0;
        let state = cache.get_or_hydrate("s-1", || {
            hydrated += 1;
            (seed(), 0)
        });
        assert!(!state.turn_active);
        assert_eq!(hydrated, 1);
    }

    #[test]
    fn a_hydrated_session_folds_live_without_rehydrating() {
        let cache = ControlStateCache::new();
        let mut hydrates = 0;
        let mut hydrate_count = || {
            hydrates += 1;
        };
        cache.get_or_hydrate("s-1", || {
            hydrate_count();
            (seed(), 0)
        });
        cache.apply_if_cached("s-1", 1, &prompt());
        let state = cache.get_or_hydrate("s-1", || {
            hydrate_count();
            (seed(), 0)
        });
        assert!(state.turn_active, "the live fold reached the reader");

        cache.apply_if_cached("s-1", 2, &stopped());
        let state = cache.get_or_hydrate("s-1", || {
            hydrate_count();
            (seed(), 0)
        });
        assert!(!state.turn_active);
        assert_eq!(hydrates, 1, "one hydrate for the session's whole life");
    }

    /// Anything that does not continue the sequence evicts rather than folds.
    #[test]
    fn a_break_in_the_sequence_evicts_instead_of_folding_wrong() {
        let cases: [(&str, u64, u64, bool); 4] = [
            // (name, first seq, second seq, still cached after)
            ("consecutive seqs fold", 1, 2, true),
            ("an exact repeat is a benign publish retry", 1, 1, true),
            ("a forward gap means events were missed", 1, 3, false),
            (
                "a backward jump means the seq counter was reset",
                5,
                1,
                false,
            ),
        ];
        for (name, first, second, still_cached) in cases {
            let cache = ControlStateCache::new();
            cache.get_or_hydrate("s-1", || (seed(), first - 1));
            cache.apply_if_cached("s-1", first, &prompt());
            cache.apply_if_cached("s-1", second, &stopped());
            assert_eq!(cache.is_cached("s-1"), still_cached, "{name}");
        }
    }

    /// A repeat must not double-apply.
    #[test]
    fn a_repeated_seq_is_not_folded_twice() {
        let approval = |nonce: &str| crate::acp::approvals::Approval {
            nonce: crate::acp::approvals::Nonce(nonce.to_string()),
            tool_call: crate::acp::state::ToolCall {
                id: "tc-1".into(),
                name: "Edit".into(),
                kind: "edit".into(),
                args_preview: String::new(),
                started_at: chrono::Utc::now(),
                diffs: Vec::new(),
                memory_recall: None,
                parent_tool_call_id: None,
            },
            destructive: false,
            options: Vec::new(),
            choice: false,
            requested_at: chrono::Utc::now(),
            resolved: None,
        };
        let cache = ControlStateCache::new();
        cache.get_or_hydrate("s-1", || (seed(), 0));
        let event = Event::ApprovalRequested {
            approval: approval("n-1"),
        };
        cache.apply_if_cached("s-1", 1, &event);
        cache.apply_if_cached("s-1", 1, &event);
        let state = cache.get_or_hydrate("s-1", || (seed(), 0));
        assert_eq!(state.pending_approvals.len(), 1);
    }

    #[test]
    fn forget_drops_the_fold_so_a_reused_id_starts_clean() {
        let cache = ControlStateCache::new();
        cache.get_or_hydrate("s-1", || (seed(), 0));
        cache.apply_if_cached("s-1", 1, &prompt());
        assert!(cache.get_or_hydrate("s-1", || (seed(), 0)).turn_active);

        cache.forget("s-1");
        assert!(!cache.is_cached("s-1"));
        assert!(!cache.get_or_hydrate("s-1", || (seed(), 0)).turn_active);
    }
}
