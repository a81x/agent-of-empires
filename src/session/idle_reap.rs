//! Pure decision logic for auto-stopping idle plain TUI/tmux sessions
//! (`session.auto_stop_idle_secs`).

use std::collections::HashSet;
use std::sync::Arc;

use chrono::{DateTime, Utc};

use super::{Instance, Status, Storage};
use crate::file_watch::FileWatchService;

/// A plain session the reaper intends to auto-stop, with the inputs the caller needs to claim it
/// (`profile` to open the right storage, the resolved `threshold_secs` for the in-lock re-check).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdleReapCandidate {
    pub session_id: String,
    pub profile: String,
    pub threshold_secs: u32,
}

/// Select the plain (non-structured view) sessions eligible for idle auto-stop.
pub fn idle_reap_candidates(
    instances: &[Instance],
    now: DateTime<Utc>,
    attached: &HashSet<String>,
    resolve_threshold: impl Fn(&str) -> u32,
) -> Vec<IdleReapCandidate> {
    let mut candidates = Vec::new();
    for inst in instances {
        if inst.is_structured() {
            continue;
        }
        let profile = inst.effective_profile();
        let threshold_secs = resolve_threshold(&profile);
        if threshold_secs == 0 {
            continue;
        }
        let is_attached = inst
            .tmux_session()
            .ok()
            .is_some_and(|s| attached.contains(s.name()));
        if should_auto_stop_session(
            now,
            inst.status,
            inst.idle_entered_at,
            inst.last_accessed_at,
            is_attached,
            threshold_secs,
        ) {
            candidates.push(IdleReapCandidate {
                session_id: inst.id.clone(),
                profile,
                threshold_secs,
            });
        }
    }
    candidates
}

/// Decide whether a plain (non-structured view) session should be auto-stopped for inactivity.
pub fn should_auto_stop_session(
    now: DateTime<Utc>,
    status: Status,
    idle_entered_at: Option<DateTime<Utc>>,
    last_accessed_at: Option<DateTime<Utc>>,
    is_attached: bool,
    threshold_secs: u32,
) -> bool {
    if threshold_secs == 0 {
        return false;
    }
    if status != Status::Idle {
        return false;
    }
    if is_attached {
        return false;
    }
    let Some(entered) = idle_entered_at else {
        return false;
    };
    let anchor = match last_accessed_at {
        Some(accessed) if accessed > entered => accessed,
        _ => entered,
    };
    match (now - anchor).to_std() {
        Ok(elapsed) => elapsed.as_secs() >= threshold_secs as u64,
        // Negative duration (anchor in the future / clock skew): not eligible.
        Err(_) => false,
    }
}

/// Atomically claim an idle session for auto-stop, under the per-profile storage file lock so
/// concurrent reapers (a standalone TUI and an `aoe serve` daemon against the same on-disk state)
/// cannot double-stop it.
pub fn claim_idle_stop(
    profile: &str,
    file_watch: Arc<FileWatchService>,
    session_id: &str,
    now: DateTime<Utc>,
    threshold_secs: u32,
) -> anyhow::Result<Option<Instance>> {
    let storage = Storage::new(profile, file_watch)?;
    storage.update(|instances, _groups| {
        let Some(inst) = instances.iter_mut().find(|i| i.id == session_id) else {
            return Ok(None);
        };
        // Defense in depth: never stop a structured view row through the plain-session path, even
        // if a caller reached here without going through `idle_reap_candidates` (which already
        // excludes structured view sessions).
        if inst.is_structured() {
            return Ok(None);
        }
        let eligible = should_auto_stop_session(
            now,
            inst.status,
            inst.idle_entered_at,
            inst.last_accessed_at,
            false,
            threshold_secs,
        );
        if !eligible {
            return Ok(None);
        }
        inst.status = Status::Stopped;
        Ok(Some(inst.clone()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn now() -> DateTime<Utc> {
        Utc::now()
    }

    #[test]
    fn should_auto_stop_session_cases() {
        let n = now();
        // (case, status, idle entered ago, last accessed ago, attached, threshold, expected)
        let cases: &[(&str, Status, Option<i64>, Option<i64>, bool, u32, bool)] = &[
            ("disabled threshold", Status::Idle, Some(36000), None, false, 0, false),
            ("running", Status::Running, Some(36000), None, false, 60, false),
            ("waiting", Status::Waiting, Some(36000), None, false, 60, false),
            ("error", Status::Error, Some(36000), None, false, 60, false),
            ("attached", Status::Idle, Some(36000), None, true, 60, false),
            ("never entered idle", Status::Idle, None, None, false, 60, false),
            ("past threshold", Status::Idle, Some(120), None, false, 60, true),
            ("within threshold", Status::Idle, Some(30), None, false, 60, false),
            ("exactly at threshold", Status::Idle, Some(60), None, false, 60, true),
            ("access after idle entry re-anchors", Status::Idle, Some(7200), Some(10), false, 60, false),
            ("access before idle entry does not", Status::Idle, Some(120), Some(18000), false, 60, true),
            ("anchor in the future (clock skew)", Status::Idle, Some(-60), None, false, 60, false),
        ];
        for &(case, status, entered, accessed, attached, threshold, expected) in cases {
            let ago = |secs: i64| n - Duration::seconds(secs);
            assert_eq!(
                should_auto_stop_session(
                    n,
                    status,
                    entered.map(ago),
                    accessed.map(ago),
                    attached,
                    threshold,
                ),
                expected,
                "{case}"
            );
        }
    }

    fn idle_instance(title: &str) -> Instance {
        let mut inst = Instance::new(title, "/tmp/idle-reap-test");
        inst.status = Status::Idle;
        inst.idle_entered_at = Some(Utc::now() - Duration::seconds(120));
        inst
    }

    #[test]
    fn candidates_select_only_reapable_plain_sessions() {
        let n = now();
        let idle = idle_instance("a");
        let attached = HashSet::from([idle.tmux_session().unwrap().name().to_string()]);
        let mut running = idle.clone();
        running.status = Status::Running;
        let one = std::slice::from_ref(&idle);

        let got = idle_reap_candidates(one, n, &HashSet::new(), |_| 60);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].session_id, idle.id);
        assert_eq!(got[0].threshold_secs, 60);

        assert!(
            idle_reap_candidates(one, n, &HashSet::new(), |_| 0).is_empty(),
            "threshold 0 disables the reaper"
        );
        assert!(
            idle_reap_candidates(&[running], n, &HashSet::new(), |_| 60).is_empty(),
            "a running session is never a candidate"
        );
        assert!(
            idle_reap_candidates(one, n, &attached, |_| 60).is_empty(),
            "an attached session is never a candidate"
        );
    }

    #[test]
    #[serial_test::serial]
    fn claim_is_single_shot_under_storage_lock() {
        let temp = tempfile::tempdir().unwrap();
        let _env = crate::session::test_support::isolate_home(temp.path());

        let inst = idle_instance("claimable");
        let id = inst.id.clone();
        let storage = Storage::new_unwatched("test-profile").unwrap();
        storage
            .update(|instances, _groups| {
                instances.push(inst);
                Ok(())
            })
            .unwrap();

        let now = Utc::now();
        let first =
            claim_idle_stop("test-profile", FileWatchService::noop(), &id, now, 60).unwrap();
        assert!(first.is_some(), "first claim should win");

        let second =
            claim_idle_stop("test-profile", FileWatchService::noop(), &id, now, 60).unwrap();
        assert!(
            second.is_none(),
            "second claim must not re-stop the session"
        );

        let stored = storage.load().unwrap();
        assert_eq!(stored[0].status, Status::Stopped);
    }
}
