//! Retired start-cascade worker storage.
//!
//! TUI start/restart now submits `SessionMutation::Start` through the daemon
//! feed and the canonical snapshot drives the row, so no caller enqueues a
//! local cascade here. The struct stays (with its worker and test seams) so
//! unmigrated readers still compile; `request_restart` and
//! `try_recv_result` have no cut-over callers. See
//! `HomeView::apply_restart_results`.

use std::sync::mpsc::TryRecvError;

#[allow(dead_code)]
use crate::session::restart::perform_restart;
#[allow(dead_code)]
pub use crate::session::restart::{RestartRequest, RestartResult};
use crate::tui::worker::Worker;

pub struct RestartPoller {
    worker: Worker<RestartRequest, RestartResult>,
}

impl RestartPoller {
    pub fn new() -> Self {
        Self {
            worker: Worker::spawn("aoe-restart-poller", perform_restart),
        }
    }

    #[allow(dead_code)]
    pub fn request_restart(&self, request: RestartRequest) {
        self.worker.request(request);
    }

    /// Non-blocking poll for a completed restart. Surfaces `Disconnected`
    /// (returned forever once the worker thread is gone, e.g. after a panic in
    /// `perform_restart`) rather than collapsing it into `None`, so the caller
    /// can clear stuck in-flight state instead of leaving rows pinned on
    /// `Status::Starting` forever.
    #[allow(dead_code)]
    pub fn try_recv_result(&self) -> Result<RestartResult, TryRecvError> {
        self.worker.try_recv()
    }
}

impl Default for RestartPoller {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::Instance;
    use std::time::Duration;

    struct TestPoller(Option<RestartPoller>);

    impl Drop for TestPoller {
        fn drop(&mut self) {
            if let Some(poller) = self.0.take() {
                let _ = poller.worker.finish_for_test();
            }
        }
    }

    #[test]
    #[serial_test::serial]
    fn restart_poller_channel_communication() {
        if !crate::tui::isolated_test_process(
            "tui::restart_poller::tests::restart_poller_channel_communication",
            Duration::from_secs(5),
        ) {
            return;
        }
        let home = crate::session::test_support::isolate_app_dir();
        let mut fixture = TestPoller(Some(RestartPoller::new()));
        let poller = fixture.0.as_mut().unwrap();
        let mut instance = Instance::new("Test Session", home.path().to_str().unwrap());
        instance.tool = "bash".to_string();
        instance.command = "true".to_string();
        let session_id = instance.id.clone();

        poller.request_restart(RestartRequest {
            session_id: session_id.clone(),
            instance,
            size: None,
            wake_message: String::new(),
            skip_on_launch: false,
            bound_hooks: true,
            discard_sandbox_container: false,
        });

        let result = loop {
            match poller.try_recv_result() {
                Ok(result) => break result,
                Err(TryRecvError::Empty) => std::thread::sleep(Duration::from_millis(20)),
                Err(error) => panic!("restart worker disconnected: {error}"),
            }
        };
        assert_eq!(result.session_id, session_id);
        assert_eq!(result.instance.id, session_id);
    }

    #[test]
    fn restart_poller_try_recv_returns_empty_when_no_result() {
        let poller = RestartPoller::new();
        assert!(matches!(poller.try_recv_result(), Err(TryRecvError::Empty)));
    }
}
