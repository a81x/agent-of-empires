//! Claude Code transcript lookup.

use super::canonicalize_or_raw;

/// Claude's per-project directory name: every char other than ASCII alphanumerics and `-` becomes `-`.
pub(crate) fn encode_claude_project_path(project_path: &str) -> String {
    project_path
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// True only when Claude's home resolves and `<config>/projects/<cwd>/<id>.jsonl`
/// is missing, so a never-prompted pinned id can launch fresh instead of failing
/// `--resume`. The config dir follows the session's `host_env` like the launch
/// does; probing the wrong tree would downgrade real conversations. Existence
/// only, so an idle conversation still counts as present.
pub(crate) fn claude_host_transcript_confirmed_absent(
    project_path: &str,
    session_id: &str,
    host_env: &[String],
) -> bool {
    let claude_home = match crate::hooks::resolve_config_dir_override("CLAUDE_CONFIG_DIR", host_env)
    {
        Some(dir) => std::path::PathBuf::from(dir),
        None => match dirs::home_dir() {
            Some(home) => home.join(".claude"),
            None => return false,
        },
    };
    let canonical = canonicalize_or_raw(project_path);
    let transcript = claude_home
        .join("projects")
        .join(encode_claude_project_path(&canonical.to_string_lossy()))
        .join(format!("{session_id}.jsonl"));
    !transcript.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn encode_claude_project_path_replaces_non_alphanumerics() {
        for (input, expected) in [
            ("/Users/foo/bar", "-Users-foo-bar"),
            ("my-project-123", "my-project-123"),
            (
                "/home/user/my project (copy)",
                "-home-user-my-project--copy-",
            ),
        ] {
            assert_eq!(encode_claude_project_path(input), expected);
        }
    }

    #[test]
    fn transcript_absence_is_existence_only() {
        let tmp = tempfile::tempdir().unwrap();
        let project_dir = tmp.path().join("projects").join("-tmp-myproject");
        std::fs::create_dir_all(&project_dir).unwrap();
        let present = "11111111-2222-3333-4444-555555555555";
        let missing = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
        let file = project_dir.join(format!("{present}.jsonl"));
        std::fs::write(&file, "data\n").unwrap();
        // An old transcript must still count as present.
        let hour_ago = std::time::SystemTime::now() - Duration::from_secs(3600);
        std::fs::File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(hour_ago))
            .unwrap();
        let _env =
            crate::session::test_support::EnvGuard::set(&[("CLAUDE_CONFIG_DIR", tmp.path())]);

        assert!(!claude_host_transcript_confirmed_absent(
            "/tmp/myproject",
            present,
            &[]
        ));
        assert!(claude_host_transcript_confirmed_absent(
            "/tmp/myproject",
            missing,
            &[]
        ));
        assert!(claude_host_transcript_confirmed_absent(
            "/tmp/never-opened-project",
            present,
            &[]
        ));
    }
}
