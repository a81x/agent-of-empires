//! E2E coverage for the Settings TUI.

use serial_test::parallel;

use crate::harness::{app_dir_in, require_tmux, TuiTestHarness};

/// The mouse-capture toggle (issue #1346) must be reachable from the Settings
/// search and editable, so the env-only `AOE_MOUSE_CAPTURE` escape hatch is no
/// longer the only knob. Search jumps to the field in the Interaction tab;
/// Space flips it from the default-on state to Disabled.
#[test]
#[parallel]
fn settings_exposes_editable_mouse_capture_toggle() {
    require_tmux!();

    let mut h = TuiTestHarness::new("settings_mouse_capture");
    h.spawn_tui();

    h.wait_for("No sessions yet");
    h.send_keys("s");
    h.wait_for("Settings");

    // Settings-wide search jumps straight to the field regardless of which
    // category it lives in.
    h.send_keys("/");
    h.type_text("mouse capture");
    h.wait_for("Mouse Capture");
    h.send_keys("Enter");
    h.assert_screen_contains("Mouse Capture");

    // Default is on; toggling lands on the Disabled state.
    h.send_keys("Space");
    h.wait_for("Disabled");
}

/// Save each sidebar position through Settings and verify it in a fresh TUI process.
#[test]
#[parallel]
fn settings_changes_sidebar_position_and_persists_it() {
    require_tmux!();

    let mut h = TuiTestHarness::new("settings_sidebar_position");
    h.spawn_tui();
    h.wait_for_ready();
    let title_column = |screen: &str| {
        let top = screen.lines().next().expect("home title row");
        top.find(" aoe ").expect("list title")
    };
    let left_column = title_column(&h.capture_screen());

    for (value, on_right) in [("right", true), ("left", false)] {
        h.send_keys("s");
        h.wait_for("Settings");
        h.send_keys("/");
        h.type_text("Sidebar Position");
        h.wait_for("Sidebar Position");
        h.send_keys("Enter");
        h.send_keys("Enter");
        h.send_keys("C-s");
        let config = std::fs::read_to_string(app_dir_in(h.home_path()).join("config.toml"))
            .expect("saved config");
        let config: toml::Value = toml::from_str(&config).unwrap();
        assert_eq!(config["session"]["sidebar_position"].as_str(), Some(value));
        h.send_keys("Escape");
        h.send_keys("Escape");
        h.wait_for_ready();
        let column = title_column(&h.capture_screen());
        assert_eq!(column > left_column, on_right);
        if !on_right {
            assert_eq!(column, left_column);
        }

        h.send_keys("q");
        h.wait_for("Quit Agent of Empires");
        h.send_keys("y");
        h.wait_for_exit(std::time::Duration::from_secs(10));
        h.spawn_tui();
        h.wait_for_ready();
        assert_eq!(title_column(&h.capture_screen()), column);
    }
}
