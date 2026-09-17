//! This TUI's claim on the size of the session it is showing.
//!
//! Selecting a row is a `view` claim (`crate::tmux::size_lock`): it lets this
//! TUI pre-size that pane to its preview, and tells every other client that
//! someone is reading it. Live mode claims separately, as `live`, from the
//! live-send worker. The claim is held on a worker thread because a claim and
//! its heartbeat are tmux forks, which must never run on the render thread.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::tmux::{SizeMode, SIZE_OWNER_HEARTBEAT, SIZE_OWNER_TTL};

/// Size-lock identity of this process as a viewer. Stable for the process, so
/// a re-claim of the same session is a refresh rather than a takeover.
pub(crate) fn viewer_id() -> String {
    format!("tui-view-{}", std::process::id())
}

/// What other clients call this machine in "size set by ..." messages.
pub(crate) fn viewer_label() -> String {
    match crate::util::hostname() {
        Some(host) => format!("{host} (aoe)"),
        None => "aoe".to_string(),
    }
}

/// The session this TUI watches, and who holds its size.
#[derive(Debug, Default, PartialEq, Eq)]
struct Watched {
    /// tmux session name, or `None` while nothing is selected.
    name: Option<String>,
    /// Label of the client holding the size, when it is not us.
    held_by: Option<String>,
}

pub struct ViewLock {
    watched: Arc<Mutex<Watched>>,
    stop: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl ViewLock {
    pub fn new() -> Self {
        let watched = Arc::new(Mutex::new(Watched::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let worker = std::thread::Builder::new()
            .name("aoe-view-lock".into())
            .spawn({
                let watched = Arc::clone(&watched);
                let stop = Arc::clone(&stop);
                move || run(&watched, &stop)
            })
            .ok();
        Self {
            watched,
            stop,
            worker,
        }
    }

    /// Watch `name` (a tmux session name), or nothing. Cheap and idempotent:
    /// the claim itself happens on the worker.
    pub(crate) fn watch(&self, name: Option<&str>) {
        let mut watched = self.watched.lock().unwrap_or_else(|e| e.into_inner());
        if watched.name.as_deref() == name {
            return;
        }
        watched.name = name.map(str::to_string);
        watched.held_by = None;
    }

    /// The other client holding the watched session's size, if any. The UI
    /// shows it so a pane that will not take this TUI's geometry explains
    /// itself.
    pub(crate) fn held_by(&self) -> Option<String> {
        self.watched
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .held_by
            .clone()
    }
}

impl Default for ViewLock {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for ViewLock {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn run(watched: &Mutex<Watched>, stop: &AtomicBool) {
    let who = viewer_id();
    let label = viewer_label();
    let mut held: Option<String> = None;
    while !stop.load(Ordering::Relaxed) {
        let want = watched
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .name
            .clone();
        if held != want {
            if let Some(old) = held.take() {
                crate::tmux::Session::from_name(&old).release_size_owner(&who);
            }
        }
        if let Some(name) = want {
            let session = crate::tmux::Session::from_name(&name);
            // Refreshing our own lock is one fork; a claim reads the state
            // first, so it only runs while we do not hold the session.
            let owned = if held.as_deref() == Some(name.as_str()) {
                session.refresh_size_owner(&who)
            } else {
                session.claim_size_lock(&who, &label, SizeMode::View)
            };
            held = owned.then(|| name.clone());
            let held_by = if owned {
                None
            } else {
                session
                    .size_state()
                    .active(crate::util::now_ms(), SIZE_OWNER_TTL)
                    .map(|lock| lock.label.clone())
            };
            let mut watched = watched.lock().unwrap_or_else(|e| e.into_inner());
            if watched.name.as_deref() == Some(name.as_str()) {
                watched.held_by = held_by;
            }
        }
        std::thread::sleep(SIZE_OWNER_HEARTBEAT);
    }
    if let Some(name) = held {
        crate::tmux::Session::from_name(&name).release_size_owner(&who);
    }
}
