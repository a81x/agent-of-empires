//! Handlers for ACP `terminal/*` requests.

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;

use thiserror::Error;
use tokio::process::Command;
use tokio::sync::Mutex;
use tracing::info;

use crate::containers::container_interface::{docker_env_args, EnvEntry};

/// Routing target for a terminal command.
#[derive(Debug, Clone)]
pub struct TerminalSandbox {
    pub container_name: String,
    /// Resolved env entries to forward into the container for this command.
    pub env_entries: Vec<EnvEntry>,
}

#[derive(Debug, Error)]
pub enum TerminalError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("terminal {0} does not exist")]
    UnknownTerminal(String),
}

/// Identifier returned to the agent on `terminal/create`.
pub type TerminalId = String;

/// One terminal's captured output and exit status.
#[derive(Debug, Clone)]
pub struct TerminalOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
}

/// Per-session terminal manager.
#[derive(Debug, Clone, Default)]
pub struct TerminalManager {
    inner: Arc<Mutex<TerminalManagerInner>>,
}

#[derive(Debug, Default)]
struct TerminalManagerInner {
    outputs: std::collections::HashMap<TerminalId, TerminalOutput>,
}

/// Build the `docker exec` argv and inherit-env pairs for a sandboxed
/// `terminal/create` request.
pub(crate) fn build_sandbox_exec_args(
    sandbox: &TerminalSandbox,
    cwd: &std::path::Path,
    command: &str,
    args: &[String],
) -> (Vec<String>, Vec<(String, String)>) {
    let (env_argv, inherit_pairs) = docker_env_args(&sandbox.env_entries);
    let mut full_args: Vec<String> = vec![
        "exec".into(),
        "-w".into(),
        cwd.to_string_lossy().into_owned(),
    ];
    full_args.extend(env_argv);
    full_args.push(sandbox.container_name.clone());
    full_args.push(command.to_string());
    full_args.extend(args.iter().cloned());
    (full_args, inherit_pairs)
}

impl TerminalManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Spawn a one-shot terminal: run a command, wait for exit, capture
    /// stdout/stderr.
    pub async fn create_and_run(
        &self,
        session_id: &str,
        command: &str,
        args: Vec<String>,
        cwd: PathBuf,
        sandbox: Option<&TerminalSandbox>,
    ) -> Result<TerminalId, TerminalError> {
        let id = format!("term-{}", uuid::Uuid::new_v4().simple());
        info!(
            target: "acp.terminal",
            session = %session_id,
            terminal = %id,
            command = %command,
            cwd = %cwd.display(),
            sandboxed = sandbox.is_some(),
            "terminal/create"
        );

        let child = match sandbox {
            Some(s) => {
                let runtime = crate::containers::get_container_runtime();
                let binary = runtime.base.binary;
                let (full_args, inherit_pairs) = build_sandbox_exec_args(s, &cwd, command, &args);
                let mut cmd = Command::new(binary);
                cmd.args(&full_args)
                    .stdin(Stdio::null())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .kill_on_drop(true);
                for (k, v) in inherit_pairs {
                    cmd.env(k, v);
                }
                cmd.spawn()?
            }
            None => Command::new(command)
                .args(&args)
                .current_dir(&cwd)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                // Kill the child if the prompt task is dropped on a
                // force-stop / cancel-escalation worker teardown.
                .kill_on_drop(true)
                .spawn()?,
        };

        // Drain stdout, stderr, and wait() concurrently; sequential reads deadlock on a full pipe.
        let raw = child.wait_with_output().await?;
        let output = TerminalOutput {
            stdout: String::from_utf8_lossy(&raw.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&raw.stderr).into_owned(),
            exit_code: raw.status.code(),
        };

        self.inner.lock().await.outputs.insert(id.clone(), output);
        Ok(id)
    }

    /// Returns the captured output of a terminal.
    pub async fn output(&self, terminal_id: &str) -> Result<TerminalOutput, TerminalError> {
        let inner = self.inner.lock().await;
        inner
            .outputs
            .get(terminal_id)
            .cloned()
            .ok_or_else(|| TerminalError::UnknownTerminal(terminal_id.into()))
    }

    /// Drop captured output.
    pub async fn release(&self, terminal_id: &str) {
        self.inner.lock().await.outputs.remove(terminal_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn create_runs_and_captures_output() {
        let _env = crate::session::test_support::EnvGuard::read_lock();
        let mgr = TerminalManager::new();
        let cwd = std::env::temp_dir();
        let id = mgr
            .create_and_run("s-1", "echo", vec!["hello".into()], cwd, None)
            .await
            .unwrap();
        let out = mgr.output(&id).await.unwrap();
        assert!(out.stdout.contains("hello"));
        assert_eq!(out.exit_code, Some(0));
    }

    #[tokio::test]
    async fn release_is_idempotent_cleanup() {
        let _env = crate::session::test_support::EnvGuard::read_lock();
        let mgr = TerminalManager::new();
        let id = mgr
            .create_and_run("s-1", "true", vec![], std::env::temp_dir(), None)
            .await
            .unwrap();
        mgr.release(&id).await;
        mgr.release(&id).await;
        mgr.release("never-existed").await;
        assert!(matches!(
            mgr.output(&id).await,
            Err(TerminalError::UnknownTerminal(_))
        ));
    }

    #[tokio::test]
    async fn large_stderr_does_not_deadlock() {
        let _env = crate::session::test_support::EnvGuard::read_lock();
        let mgr = TerminalManager::new();
        let cwd = std::env::temp_dir();
        let script = "head -c 204800 /dev/zero | tr '\\0' 'x' >&2; echo done";
        let run = mgr.create_and_run(
            "s-large-stderr",
            "sh",
            vec!["-c".into(), script.into()],
            cwd,
            None,
        );
        let id = tokio::time::timeout(std::time::Duration::from_secs(5), run)
            .await
            .expect("create_and_run hung; pipe deadlock regressed")
            .expect("create_and_run failed");
        let out = mgr.output(&id).await.unwrap();
        assert_eq!(out.stderr.len(), 204_800);
        assert!(out.stdout.contains("done"));
        assert_eq!(out.exit_code, Some(0));
    }

    #[tokio::test]
    async fn large_stdout_and_stderr_drain_concurrently() {
        let _env = crate::session::test_support::EnvGuard::read_lock();
        let mgr = TerminalManager::new();
        let cwd = std::env::temp_dir();
        let script = "head -c 204800 /dev/zero | tr '\\0' 'o' \
                      & head -c 204800 /dev/zero | tr '\\0' 'e' >&2 \
                      & wait";
        let run = mgr.create_and_run(
            "s-both-pipes",
            "sh",
            vec!["-c".into(), script.into()],
            cwd,
            None,
        );
        let id = tokio::time::timeout(std::time::Duration::from_secs(5), run)
            .await
            .expect("create_and_run hung; pipe deadlock regressed")
            .expect("create_and_run failed");
        let out = mgr.output(&id).await.unwrap();
        assert_eq!(out.stdout.len(), 204_800);
        assert_eq!(out.stderr.len(), 204_800);
        assert_eq!(out.exit_code, Some(0));
    }

    #[test]
    fn sandbox_exec_args_emit_e_flags_for_each_entry() {
        let sandbox = TerminalSandbox {
            container_name: "aoe-sandbox-test".into(),
            env_entries: vec![
                EnvEntry::Inherit {
                    key: "GH_TOKEN".into(),
                    value: "ghp_secret".into(),
                },
                EnvEntry::Literal {
                    key: "TERM".into(),
                    value: "xterm".into(),
                },
            ],
        };
        let (argv, inherit) = build_sandbox_exec_args(
            &sandbox,
            std::path::Path::new("/workspace"),
            "gh",
            &["pr".into(), "list".into()],
        );

        // exec -w /workspace [-e flags...] container cmd [args...]
        assert_eq!(argv[0], "exec");
        assert_eq!(argv[1], "-w");
        assert_eq!(argv[2], "/workspace");
        // Both -e flags must appear before the container name.
        let container_idx = argv
            .iter()
            .position(|a| a == "aoe-sandbox-test")
            .expect("container name in argv");
        let e_positions: Vec<usize> = argv
            .iter()
            .enumerate()
            .filter_map(|(i, a)| (a == "-e").then_some(i))
            .collect();
        assert_eq!(e_positions.len(), 2, "one -e flag per env entry");
        for pos in &e_positions {
            assert!(
                *pos < container_idx,
                "-e flags must precede the container name"
            );
        }
        // Inherit value must NOT leak into argv.
        assert!(
            !argv.iter().any(|a| a.contains("ghp_secret")),
            "secret leaked into argv: {:?}",
            argv
        );
        // Inherit pairs carry the actual value for cmd.env(k, v).
        assert_eq!(
            inherit,
            vec![("GH_TOKEN".to_string(), "ghp_secret".to_string())]
        );
        // Command + args appear after the container name.
        assert_eq!(argv[container_idx + 1], "gh");
        assert_eq!(argv[container_idx + 2], "pr");
        assert_eq!(argv[container_idx + 3], "list");
    }

    #[test]
    fn sandbox_exec_args_no_env_entries() {
        let sandbox = TerminalSandbox {
            container_name: "aoe-sandbox-test".into(),
            env_entries: vec![],
        };
        let (argv, inherit) = build_sandbox_exec_args(
            &sandbox,
            std::path::Path::new("/workspace"),
            "echo",
            &["hi".into()],
        );
        assert_eq!(
            argv,
            vec![
                "exec".to_string(),
                "-w".to_string(),
                "/workspace".to_string(),
                "aoe-sandbox-test".to_string(),
                "echo".to_string(),
                "hi".to_string(),
            ]
        );
        assert!(inherit.is_empty());
    }
}
