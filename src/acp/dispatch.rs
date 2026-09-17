//! Server-owned prompt dispatch: whether an incoming prompt is sent now,
//! steered into the running turn, or parked on the server queue.

use super::state::AcpState;

/// What the daemon decided to do with a prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case", tag = "disposition")]
pub enum PromptDispatch {
    /// No turn in flight: start one.
    Sent,
    /// A steerable turn is running and will take this mid-turn
    /// (`_session/steering`) rather than refusing it.
    Steered,
    /// Park it on the server-owned queue; the turn-end drain delivers it.
    Queued { reason: QueueReason },
}

/// Why a prompt was parked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QueueReason {
    /// A non-steerable turn is running.
    TurnActive,
    /// A cancel is pending on the running turn.
    Cancelling,
    /// A `/compact` is running.
    Compacting,
    /// No live worker, and not the idle-dormant case this POST would wake.
    WorkerDown,
}

/// Worker-liveness inputs the endpoint already computes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkerLiveness {
    /// The supervisor holds a live (or mid-respawn) worker for this session.
    pub running: bool,
    /// The session was auto-stopped for inactivity.
    pub idle_dormant: bool,
    /// The session is parked on the rate-limit redelivery cap.
    pub rate_limit_exhausted: bool,
}

/// Decide what to do with a prompt arriving for `state`.
pub fn decide(state: &AcpState, worker: WorkerLiveness) -> PromptDispatch {
    if !worker.running && !worker.idle_dormant && !worker.rate_limit_exhausted {
        return PromptDispatch::Queued {
            reason: QueueReason::WorkerDown,
        };
    }
    if !state.turn_active {
        return PromptDispatch::Sent;
    }
    if state.cancelling {
        return PromptDispatch::Queued {
            reason: QueueReason::Cancelling,
        };
    }
    if state.compacting {
        return PromptDispatch::Queued {
            reason: QueueReason::Compacting,
        };
    }
    if state.steering {
        return PromptDispatch::Steered;
    }
    PromptDispatch::Queued {
        reason: QueueReason::TurnActive,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn live() -> WorkerLiveness {
        WorkerLiveness {
            running: true,
            idle_dormant: false,
            rate_limit_exhausted: false,
        }
    }

    fn state(turn_active: bool, steering: bool, cancelling: bool, compacting: bool) -> AcpState {
        let mut s = AcpState::new(
            crate::acp::state::AcpSessionId("sess-1".into()),
            crate::acp::state::AgentName("claude".into()),
            None,
        );
        s.turn_active = turn_active;
        s.steering = steering;
        s.cancelling = cancelling;
        s.compacting = compacting;
        s
    }

    /// The decision table, keyed by the incident each row exists for.
    #[test]
    fn dispatch_table_covers_every_incident_by_name() {
        let queued = |r| PromptDispatch::Queued { reason: r };
        let cases: [(&str, AcpState, WorkerLiveness, PromptDispatch); 12] = [
            (
                "idle turn, live worker: ordinary send",
                state(false, false, false, false),
                live(),
                PromptDispatch::Sent,
            ),
            (
                "#2805 steerable turn takes a mid-turn prompt instead of queueing after it",
                state(true, true, false, false),
                live(),
                PromptDispatch::Steered,
            ),
            (
                "#2805 a non-steerable turn still parks",
                state(true, false, false, false),
                live(),
                queued(QueueReason::TurnActive),
            ),
            (
                "#1727 steerable but cancelling: parking is what keeps a \
                 Stop-then-type from restarting the runner",
                state(true, true, true, false),
                live(),
                queued(QueueReason::Cancelling),
            ),
            (
                "#1727 cancelling outranks compacting, so the reason names the \
                 gate that would have restarted the worker",
                state(true, true, true, true),
                live(),
                queued(QueueReason::Cancelling),
            ),
            (
                "#3219 steerable but compacting: the adapter would swallow the \
                 message into a turn that never answers it",
                state(true, true, false, true),
                live(),
                queued(QueueReason::Compacting),
            ),
            (
                "#1689 idle-dormant worker: the POST is the wake path, so a \
                 fresh prompt sends rather than parking on 'not running'",
                state(false, false, false, false),
                WorkerLiveness {
                    running: false,
                    idle_dormant: true,
                    rate_limit_exhausted: false,
                },
                PromptDispatch::Sent,
            ),
            (
                "#1689 a genuinely cold worker (mid-resume, not dormant) parks",
                state(false, false, false, false),
                WorkerLiveness {
                    running: false,
                    idle_dormant: false,
                    rate_limit_exhausted: false,
                },
                queued(QueueReason::WorkerDown),
            ),
            (
                "worker liveness is checked before the turn flags: no worker \
                 means no turn can be steered into",
                state(true, true, false, false),
                WorkerLiveness {
                    running: false,
                    idle_dormant: false,
                    rate_limit_exhausted: false,
                },
                queued(QueueReason::WorkerDown),
            ),
            (
                "an idle-dormant session with a stale turn_active latch parks \
                 rather than sending into a turn nothing is running",
                state(true, false, false, false),
                WorkerLiveness {
                    running: false,
                    idle_dormant: true,
                    rate_limit_exhausted: false,
                },
                queued(QueueReason::TurnActive),
            ),
            (
                "#3688 a session parked on the redelivery cap sends: nothing \
                 un-parks it on a timer, so queueing strands the prompt the \
                 banner asked for",
                state(false, false, false, false),
                WorkerLiveness {
                    running: false,
                    idle_dormant: false,
                    rate_limit_exhausted: true,
                },
                PromptDispatch::Sent,
            ),
            (
                "#3688 the cap park does not override the turn gates either, \
                 so a stale turn_active latch still parks",
                state(true, false, false, false),
                WorkerLiveness {
                    running: false,
                    idle_dormant: false,
                    rate_limit_exhausted: true,
                },
                queued(QueueReason::TurnActive),
            ),
        ];
        for (name, st, worker, expected) in cases {
            assert_eq!(decide(&st, worker), expected, "{name}");
        }
    }

    /// The wire shape the clients switch on.
    #[test]
    fn dispatch_serializes_to_the_documented_wire_shape() {
        let cases = [
            (PromptDispatch::Sent, r#"{"disposition":"sent"}"#),
            (PromptDispatch::Steered, r#"{"disposition":"steered"}"#),
            (
                PromptDispatch::Queued {
                    reason: QueueReason::Cancelling,
                },
                r#"{"disposition":"queued","reason":"cancelling"}"#,
            ),
        ];
        for (dispatch, expected) in cases {
            assert_eq!(serde_json::to_string(&dispatch).unwrap(), expected);
        }
    }
}
