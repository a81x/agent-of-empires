//! Remote daemon sessions listed inline, and "group by remote".

use super::*;
use crate::session::config::GroupByMode;
use crate::tui::remote_feed::RemoteSnapshot;

fn wire(id: &str, title: &str) -> crate::daemon::SessionResponse {
    serde_json::from_value(serde_json::json!({"id": id, "title": title})).unwrap()
}

fn with_remote(env: &mut TestEnv, rows: Vec<crate::daemon::SessionResponse>) {
    env.view.remote_snapshots = vec![RemoteSnapshot {
        name: "mini".into(),
        sessions: Some(Ok(rows)),
        meta: None,
    }];
    env.view.remotes_configured = true;
    env.view.rebuild_flat_items();
}

#[test]
#[serial]
fn a_defaulted_remote_grouping_renders_the_pre_remote_default_until_a_remote_exists() {
    let mut env = create_test_env_with_sessions(2);
    env.view.group_by = GroupByMode::Remote;
    env.view.group_by_is_default = true;
    env.view.fallback_group_by = GroupByMode::Manual;
    env.view.remotes_configured = false;
    assert_eq!(env.view.effective_group_by(), GroupByMode::Manual);

    env.view.remotes_configured = true;
    assert_eq!(env.view.effective_group_by(), GroupByMode::Remote);

    // An explicit choice is honored even with nothing configured.
    env.view.remotes_configured = false;
    env.view.group_by_is_default = false;
    assert_eq!(env.view.effective_group_by(), GroupByMode::Remote);
}

#[test]
#[serial]
fn grouping_by_remote_lists_local_first_then_each_remote_one_indent_deep() {
    let mut env = create_test_env_with_sessions(2);
    env.view.group_by = GroupByMode::Remote;
    env.view.group_by_is_default = false;
    with_remote(&mut env, vec![wire("r1", "remote one")]);

    let shape: Vec<(&str, usize)> = env
        .view
        .flat_items
        .iter()
        .map(|item| {
            let kind = match item {
                Item::LocalGroup { .. } => "local",
                Item::Session { .. } => "session",
                Item::RemoteGroup { .. } => "remote",
                Item::RemoteSession { .. } => "remote-session",
                Item::Group { .. } => "group",
            };
            (kind, item.depth())
        })
        .collect();
    assert_eq!(
        shape,
        [
            ("local", 0),
            ("session", 1),
            ("session", 1),
            ("remote", 0),
            ("remote-session", 1),
        ]
    );
}

#[test]
#[serial]
fn collapsing_a_machine_header_hides_only_that_machine() {
    let mut env = create_test_env_with_sessions(2);
    env.view.group_by = GroupByMode::Remote;
    env.view.group_by_is_default = false;
    with_remote(&mut env, vec![wire("r1", "remote one")]);

    env.view.cursor = 0;
    env.view.update_selected();
    assert!(env.view.toggle_machine_header_at_cursor());
    assert!(!env
        .view
        .flat_items
        .iter()
        .any(|item| matches!(item, Item::Session { .. })));
    assert!(env
        .view
        .flat_items
        .iter()
        .any(|item| matches!(item, Item::RemoteSession { .. })));
}

fn register_mini() {
    let mut registry = crate::daemon::remotes::Registry::default();
    registry.upsert(crate::daemon::remotes::Remote {
        name: "mini".into(),
        // Unroutable: the worker's connect fails in the background, which these
        // tests never drain; they assert the view's own state.
        url: "https://127.0.0.1:9".into(),
        enabled: true,
        token: Some("tok".into()),
        session: None,
        binding: None,
    });
    crate::daemon::remotes::save(&registry).unwrap();
}

#[test]
#[serial]
fn a_remote_row_never_selects_a_local_session_and_enter_live_sends_in_the_pane() {
    let mut env = create_test_env_with_sessions(1);
    register_mini();
    env.view.group_by = GroupByMode::Remote;
    env.view.group_by_is_default = false;
    with_remote(&mut env, vec![wire("r1", "remote one")]);

    let idx = env
        .view
        .flat_items
        .iter()
        .position(|item| matches!(item, Item::RemoteSession { .. }))
        .expect("remote row listed");
    env.view.cursor = idx;
    env.view.update_selected();
    assert_eq!(env.view.selected_session, None);
    assert_eq!(env.view.selected_group, None);
    assert_eq!(
        env.view.remote_preview_key,
        Some(("mini".to_string(), "r1".to_string())),
        "selecting a remote row watches it for the preview"
    );

    assert!(env.view.activate_selected_session().is_none());
    let live = env
        .view
        .remote_live
        .as_ref()
        .expect("live-send in the pane");
    assert_eq!(
        (live.remote.as_str(), live.session_id.as_str()),
        ("mini", "r1")
    );
}

#[test]
#[serial]
fn the_exit_chord_leaves_remote_live_send_and_moving_off_the_row_stops_watching() {
    let mut env = create_test_env_with_sessions(1);
    register_mini();
    env.view.group_by = GroupByMode::Remote;
    env.view.group_by_is_default = false;
    with_remote(&mut env, vec![wire("r1", "remote one")]);
    env.view.cursor = env
        .view
        .flat_items
        .iter()
        .position(|item| matches!(item, Item::RemoteSession { .. }))
        .unwrap();
    env.view.update_selected();
    env.view.activate_selected_session();
    let chord = env.view.remote_live.as_ref().unwrap().exit_chords[0];

    env.view.handle_key(KeyEvent::new(chord.0, chord.1), None);
    assert!(env.view.remote_live.is_none());
    assert!(
        env.view.remote_preview_key.is_some(),
        "still previewing after live-send ends"
    );

    env.view.cursor = 0;
    env.view.update_selected();
    assert_eq!(env.view.remote_preview_key, None);
}

#[test]
#[serial]
fn remote_sections_sit_above_the_archived_shelf_in_other_groupings() {
    let mut env = create_test_env_with_sessions(2);
    env.view.group_by = GroupByMode::Manual;
    env.view.group_by_is_default = false;
    env.view.archived_section_collapsed = false;
    env.view.cursor = 0;
    env.view.update_selected();
    with_canonical_archive(&mut env, |env| {
        env.view.toggle_archive_at_cursor().unwrap();
    });
    with_remote(&mut env, vec![wire("r1", "remote one")]);

    let shelf = env.view.shelf_start().expect("archived shelf present");
    let remote = env
        .view
        .flat_items
        .iter()
        .position(|item| matches!(item, Item::RemoteGroup { .. }))
        .expect("remote header listed");
    assert!(remote < shelf, "remote section must stay above the shelf");
}

fn shelved(id: &str, archived: bool, trashed: bool) -> crate::daemon::SessionResponse {
    serde_json::from_value(serde_json::json!({
        "id": id,
        "title": id,
        "archived_at": archived.then_some("2026-01-01T00:00:00Z"),
        "trashed_at": trashed.then_some("2026-02-01T00:00:00Z"),
    }))
    .unwrap()
}

fn position_of_remote_row(env: &TestEnv, want: &str) -> Option<usize> {
    env.view
        .flat_items
        .iter()
        .position(|item| matches!(item, Item::RemoteSession { id, .. } if id == want))
}

fn section_count(env: &TestEnv, section: &str) -> Option<usize> {
    env.view.flat_items.iter().find_map(|item| match item {
        Item::Group {
            path,
            session_count,
            ..
        } if path == section => Some(*session_count),
        _ => None,
    })
}

#[test]
#[serial]
fn remote_archived_and_trashed_rows_go_to_the_shelf_not_the_machine_section() {
    let mut env = create_test_env_with_sessions(1);
    env.view.group_by = GroupByMode::Remote;
    env.view.group_by_is_default = false;
    env.view.archived_section_collapsed = false;
    env.view.trashed_section_collapsed = false;
    with_remote(
        &mut env,
        vec![
            wire("live", "live"),
            shelved("old", true, false),
            shelved("gone", false, true),
        ],
    );

    let shelf = env
        .view
        .shelf_start()
        .expect("shelf created for remote rows");
    assert!(position_of_remote_row(&env, "live").unwrap() < shelf);
    assert!(position_of_remote_row(&env, "old").unwrap() > shelf);
    assert!(position_of_remote_row(&env, "gone").unwrap() > shelf);
    assert!(matches!(
        env.view
            .flat_items
            .iter()
            .find(|i| matches!(i, Item::RemoteGroup { depth: 0, .. })),
        Some(Item::RemoteGroup {
            session_count: 1,
            ..
        })
    ));
    assert_eq!(
        section_count(&env, crate::session::ARCHIVED_SECTION_PATH),
        Some(1)
    );
    assert_eq!(
        section_count(&env, crate::session::TRASH_SECTION_PATH),
        Some(1)
    );
}

#[test]
#[serial]
fn a_collapsed_shelf_section_counts_remote_rows_without_listing_them() {
    let mut env = create_test_env_with_sessions(1);
    env.view.group_by = GroupByMode::Remote;
    env.view.group_by_is_default = false;
    env.view.trashed_section_collapsed = true;
    with_remote(&mut env, vec![shelved("gone", false, true)]);

    assert_eq!(
        section_count(&env, crate::session::TRASH_SECTION_PATH),
        Some(1)
    );
    assert_eq!(position_of_remote_row(&env, "gone"), None);
}

#[test]
#[serial]
fn enter_on_a_trashed_remote_row_says_where_to_restore_it_instead_of_opening() {
    let mut env = create_test_env_with_sessions(1);
    env.view.group_by = GroupByMode::Remote;
    env.view.group_by_is_default = false;
    env.view.trashed_section_collapsed = false;
    with_remote(&mut env, vec![shelved("gone", false, true)]);

    env.view.cursor = position_of_remote_row(&env, "gone").expect("listed in the shelf");
    env.view.update_selected();
    match env.view.activate_selected_session() {
        Some(Action::SetTransientStatus(message)) => assert!(message.contains("mini"), "{message}"),
        other => panic!("expected a restore hint, got {other:?}"),
    }
}

#[test]
#[serial]
fn a_remote_submit_closes_the_dialog_and_hands_off_to_the_remote() {
    let mut env = create_test_env_with_sessions(1);
    register_mini();
    let target = crate::tui::dialogs::RemoteTarget {
        name: "mini".into(),
        home: Some("/Users/remote".into()),
        profiles: vec!["default".into()],
        tools: vec!["claude".into()],
        docker_available: false,
        client: crate::daemon::DaemonClient::new("https://127.0.0.1:9", Some("tok")).unwrap(),
    };
    let mut dialog = NewSessionDialog::new(
        AvailableTools::with_tools(&["claude"]),
        Vec::new(),
        "default",
        vec!["default".to_string()],
    )
    .with_remotes(vec![target]);
    dialog.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    env.view.new_dialog = Some(dialog);

    env.view
        .handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), None);
    assert!(env.view.new_dialog.is_none());
    let flash = env.view.status_flash_text().unwrap_or_default().to_string();
    assert!(flash.contains("Creating session on mini"), "{flash}");
    assert_eq!(
        env.view.instances().count(),
        1,
        "nothing is created locally"
    );
}

#[test]
#[serial]
fn enter_on_a_remote_structured_row_opens_it_and_tab_explains_there_is_no_pane() {
    let mut env = create_test_env_with_sessions(1);
    env.view.group_by = GroupByMode::Remote;
    env.view.group_by_is_default = false;
    let structured: crate::daemon::SessionResponse = serde_json::from_value(
        serde_json::json!({"id": "s1", "title": "structured", "view": "structured"}),
    )
    .unwrap();
    with_remote(&mut env, vec![structured]);

    env.view.cursor = position_of_remote_row(&env, "s1").expect("listed");
    env.view.update_selected();
    match env.view.activate_selected_session() {
        Some(Action::OpenRemoteStructuredView { remote, id }) => {
            assert_eq!((remote.as_str(), id.as_str()), ("mini", "s1"));
        }
        other => panic!("expected a remote structured open, got {other:?}"),
    }
    match env
        .view
        .handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE), None)
    {
        Some(Action::SetTransientStatus(message)) => {
            assert!(message.contains("press Enter"), "{message}")
        }
        other => panic!("expected a hint, got {other:?}"),
    }
    assert!(env.view.remote_live.is_none());
}
