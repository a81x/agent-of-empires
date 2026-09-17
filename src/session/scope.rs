//! State filter shared by the CLI (`aoe list --state`) and the daemon REST API (`GET
//! /api/sessions?state=`) so the two vocabularies cannot drift.

use serde::{Deserialize, Serialize};

use super::Instance;

/// Which "state" of session a caller wants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionScope {
    /// The default in every current caller: sessions that are neither archived nor trashed.
    Live,
    /// Sessions currently in the trash (`remove` but not yet purged).
    Trashed,
    /// Every persisted session, regardless of state.
    All,
}

impl SessionScope {
    /// Does `inst` belong in a listing filtered by `scope`?
    pub fn matches(scope: Option<SessionScope>, inst: &Instance) -> bool {
        match scope {
            None | Some(SessionScope::All) => true,
            Some(SessionScope::Live) => !inst.is_archived() && !inst.is_trashed(),
            Some(SessionScope::Trashed) => inst.is_trashed(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserializes_lowercase_wire_names() {
        assert_eq!(
            serde_json::from_str::<SessionScope>("\"live\"").unwrap(),
            SessionScope::Live
        );
        assert_eq!(
            serde_json::from_str::<SessionScope>("\"trashed\"").unwrap(),
            SessionScope::Trashed
        );
        assert_eq!(
            serde_json::from_str::<SessionScope>("\"all\"").unwrap(),
            SessionScope::All
        );
    }

    #[test]
    fn rejects_unrecognized_value() {
        assert!(serde_json::from_str::<SessionScope>("\"archived\"").is_err());
        assert!(serde_json::from_str::<SessionScope>("\"LIVE\"").is_err());
        assert!(serde_json::from_str::<SessionScope>("\"\"").is_err());
    }

    #[test]
    fn matches_no_scope_returns_everything() {
        let mut live = Instance::new("live", "/repo");
        assert!(SessionScope::matches(None, &live));
        live.archive();
        assert!(SessionScope::matches(None, &live));
    }

    #[test]
    fn matches_live_excludes_archived_and_trashed() {
        let live = Instance::new("live", "/repo");
        let mut archived = Instance::new("arch", "/repo");
        archived.archive();
        let mut trashed = Instance::new("trash", "/repo");
        trashed.trash();

        assert!(SessionScope::matches(Some(SessionScope::Live), &live));
        assert!(!SessionScope::matches(Some(SessionScope::Live), &archived));
        assert!(!SessionScope::matches(Some(SessionScope::Live), &trashed));
    }

    #[test]
    fn matches_trashed_is_trashed_only() {
        let mut trashed = Instance::new("t", "/repo");
        trashed.trash();
        let mut archived = Instance::new("a", "/repo");
        archived.archive();

        assert!(SessionScope::matches(Some(SessionScope::Trashed), &trashed));
        assert!(!SessionScope::matches(
            Some(SessionScope::Trashed),
            &archived
        ));
    }
}
