//! Create sessions on a remote daemon from the new-session dialog.
//!
//! Runs on a worker: the daemon answers only after it has cloned a worktree,
//! started a container or run hooks, which must not stall the TUI.

use std::sync::mpsc::TryRecvError;

use reqwest::StatusCode;

use crate::daemon::{CreateSessionBody, DaemonClient, DaemonClientError};
use crate::tui::dialogs::NewSessionData;
use crate::tui::worker::Worker;

pub(crate) struct CreateRequest {
    pub remote: String,
    pub client: DaemonClient,
    pub body: CreateSessionBody,
}

/// `(remote, new session id or error)`.
pub(crate) type CreateOutcome = (String, Result<String, String>);

pub struct RemoteCreate {
    worker: Worker<CreateRequest, CreateOutcome>,
}

impl RemoteCreate {
    pub fn new() -> Self {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
        Self {
            worker: Worker::spawn("aoe-remote-create", move |request: CreateRequest| {
                let outcome = match runtime.as_ref() {
                    Ok(rt) => rt
                        .block_on(request.client.create_session_unpinned(&request.body))
                        .map(|created| created.id)
                        .map_err(|e| create_failure_message(&e)),
                    Err(e) => Err(format!("no runtime: {e}")),
                };
                (request.remote, outcome)
            }),
        }
    }

    pub(crate) fn request(&self, request: CreateRequest) {
        self.worker.request(request);
    }

    pub(crate) fn try_recv(&self) -> Result<CreateOutcome, TryRecvError> {
        self.worker.try_recv()
    }
}

impl Default for RemoteCreate {
    fn default() -> Self {
        Self::new()
    }
}

/// Why a create failed, from the status alone: the daemon's error body is
/// never read. A bare 403 is most often a repo whose hooks the remote has not
/// trusted, since this dialog has no remote trust prompt to approve them.
fn create_failure_message(error: &DaemonClientError) -> String {
    match error {
        DaemonClientError::Status {
            status: StatusCode::FORBIDDEN,
            code: None,
            ..
        } => "the daemon refused the create (HTTP 403); the repo's hooks may need trusting \
              on that machine (`aoe add --trust-hooks` there)"
            .to_string(),
        DaemonClientError::Status {
            status: StatusCode::BAD_REQUEST,
            code: None,
            ..
        } => "the daemon rejected the create (HTTP 400); check the path, branch and profile \
              on that machine"
            .to_string(),
        other => other.summary(),
    }
}

/// The daemon's `POST /api/sessions` body for a dialog submit. Sent without
/// the runtime epoch the local feed pins, since a remote has its own runtime;
/// empty title and profile defer to that daemon's defaults.
pub(crate) fn create_body(data: &NewSessionData) -> CreateSessionBody {
    // `trust_hooks: None` refuses unapproved repo hooks; `false` would skip
    // them instead, and nothing here may approve them.
    let mut body = crate::tui::home::wizard_create_body(data, None);
    body.title = body.title.filter(|title| !title.is_empty());
    body.profile = body.profile.filter(|profile| !profile.is_empty());
    body
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_optional_fields_are_omitted_and_view_follows_the_structured_choice() {
        let data = NewSessionData {
            remote: Some("mini".into()),
            profile: String::new(),
            title: String::new(),
            path: "/Users/remote/app".into(),
            group: "work".into(),
            tool: "codex".into(),
            worktree_enabled: true,
            worktree_branch: Some("feat".into()),
            create_new_branch: true,
            base_branch: Some("main".into()),
            extra_repo_paths: Vec::new(),
            sandbox: false,
            sandbox_image: String::new(),
            yolo_mode: false,
            extra_env: Vec::new(),
            extra_args: String::new(),
            command_override: String::new(),
            scratch: false,
            fork_seed: None,
            structured: false,
        };
        let body = create_body(&data);
        assert_eq!(body.title, None);
        assert_eq!(body.profile, None);
        assert_eq!(body.sandbox_image, None);
        assert_eq!(body.path, "/Users/remote/app");
        assert_eq!(body.tool, "codex");
        assert_eq!(body.worktree_branch.as_deref(), Some("feat"));
        assert_eq!(body.view, crate::session::View::Terminal);
        assert_eq!(body.trust_hooks, None);
    }

    #[test]
    fn a_failed_create_is_described_from_its_status_not_its_body() {
        let status = |status, code| DaemonClientError::Status {
            status,
            code,
            body: "server text".into(),
            truncated: false,
        };
        for (error, expected) in [
            (
                status(StatusCode::FORBIDDEN, None),
                "hooks may need trusting",
            ),
            (status(StatusCode::BAD_REQUEST, None), "check the path"),
            (
                status(
                    StatusCode::FORBIDDEN,
                    Some(crate::daemon::ApiErrorCode::ReadOnly),
                ),
                "read-only",
            ),
            (status(StatusCode::NOT_FOUND, None), "HTTP 404"),
        ] {
            let message = create_failure_message(&error);
            assert!(message.contains(expected), "{message}");
            assert!(!message.contains("server text"), "{message}");
        }
    }
}
