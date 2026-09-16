import { describe, expect, it } from "vitest";

import {
  applyEvent,
  applyReducedState,
  deriveTurnActive,
  emptyAcpState,
  normaliseTurnState,
  type AcpEvent,
  type AcpFrame,
  type AcpState,
  type ReducedState,
} from "./acpTypes";

const ev = (state: AcpState, seq: number, event: AcpEvent) => applyEvent(state, { session_id: "s-1", seq, event });

function frame(seq: number, text: string): AcpFrame {
  return {
    session_id: "s-1",
    seq,
    event: { UserPromptSent: { text } },
  };
}

function withOptimisticPrompt(state: AcpState, text: string, id = "cmp-1"): AcpState {
  return {
    ...state,
    optimisticRows: state.optimisticRows.concat({
      id,
      kind: "user_prompt",
      text,
      at: new Date().toISOString(),
    }),
    inflightPromptIds: state.inflightPromptIds.concat(id),
    promptSeq: state.promptSeq + 1,
    turnActive: true,
  };
}

describe("applyEvent / UserPromptSent (control state)", () => {
  it("opens the turn and marks it active, adding no activity row", () => {
    const next = applyEvent(emptyAcpState(), frame(1, "hi"));
    expect(next.activity).toHaveLength(0);
    expect(next.serverTurnActive).toBe(true);
    expect(next.turnActive).toBe(true);
    expect(next.promptSeq).toBe(1);
    expect(next.lastSeq).toBe(1);
  });

  it("clears startup/error flags so the new turn starts clean", () => {
    const stale: AcpState = {
      ...emptyAcpState(),
      startupError: "old error",
      lastError: "old action error",
      rateLimitRetriesExhausted: true,
      turnActive: false,
    };
    const next = applyEvent(stale, frame(1, "new prompt"));
    expect(next.startupError).toBeNull();
    expect(next.lastError).toBeNull();
    expect(next.rateLimitRetriesExhausted).toBe(false);
    expect(next.turnActive).toBe(true);
  });

  it("is a no-op for a frame at or below lastSeq (returns the same ref)", () => {
    const seeded: AcpState = { ...emptyAcpState(), lastSeq: 3 };
    const next = applyEvent(seeded, frame(3, "dup"));
    expect(next).toBe(seeded);
  });
});

describe("applyEvent / UserDiffCommentsPrompt (#1123) (control state)", () => {
  function diffCommentsFrame(seq: number): AcpFrame {
    return {
      session_id: "s-1",
      seq,
      event: {
        UserDiffCommentsPrompt: {
          intro: "Take a look:",
          outro: "Please address these comments.",
          isMultiRepo: true,
          comments: [],
          assembledMarkdown: "Take a look:\n\n## Diff comments\n\n...\n",
        },
      },
    };
  }

  it("opens the turn (the typed row itself is server-owned)", () => {
    const next = applyEvent(emptyAcpState(), diffCommentsFrame(1));
    expect(next.activity).toHaveLength(0);
    expect(next.serverTurnActive).toBe(true);
    expect(next.turnActive).toBe(true);
    expect(next.promptSeq).toBe(1);
  });

  it("applies the same per-turn resets as a plain prompt", () => {
    const stale: AcpState = {
      ...emptyAcpState(),
      startupError: "old error",
      lastError: "old action error",
      workerStopped: true,
      workerRestarting: true,
      agentUnresponsive: true,
      turnActive: false,
    };
    const next = applyEvent(stale, diffCommentsFrame(1));
    expect(next.startupError).toBeNull();
    expect(next.lastError).toBeNull();
    expect(next.workerStopped).toBe(false);
    expect(next.workerRestarting).toBe(false);
    expect(next.agentUnresponsive).toBe(false);
    expect(next.turnActive).toBe(true);
  });

  it("counts as a prior user turn so a later SessionContextReset arms the primer (#1123)", () => {
    let state = applyEvent(emptyAcpState(), diffCommentsFrame(1));
    state = ev(state, 2, { SessionContextReset: { reason: "session/load failed: bad id" } });
    expect(state.contextPrimerAvailable).toEqual({
      resetSeq: 2,
      reason: "session/load failed: bad id",
    });
  });
});

describe("applyEvent / ACP session id lifecycle", () => {
  it("AcpSessionAssigned is a no-op for the conversation surface", () => {
    const before = emptyAcpState();
    const after = ev(before, 1, { AcpSessionAssigned: { acp_session_id: "uuid-1234" } });
    expect(after.lastSeq).toBe(1);
    expect(after.activity).toEqual([]);
    expect(after.sessionUsage).toBeNull();
  });

  it("SessionContextReset clears stale usage and arms the primer after a prior prompt", () => {
    let state = ev(emptyAcpState(), 1, { UsageUpdated: { usage: { used: 75000, size: 200000 } } });
    expect(state.sessionUsage?.used).toBe(75000);

    state = ev(state, 2, { UserPromptSent: { text: "hi" } });

    state = ev(state, 3, {
      SessionContextReset: { reason: "session/load failed: bad id" },
    });
    expect(state.sessionUsage).toBeNull();
    expect(state.contextPrimerAvailable).toEqual({
      resetSeq: 3,
      reason: "session/load failed: bad id",
    });
  });

  it("SessionContextReset uses a fallback primer reason when reason is empty", () => {
    let state = ev(emptyAcpState(), 1, { UserPromptSent: { text: "hi" } });
    state = ev(state, 2, { SessionContextReset: { reason: "" } });
    expect(state.contextPrimerAvailable?.reason.length).toBeGreaterThan(0);
  });

  it("SessionContextReset is silent on a session with no prior user prompt", () => {
    let state = ev(emptyAcpState(), 1, { UsageUpdated: { usage: { used: 100, size: 200000 } } });
    state = ev(state, 2, {
      SessionContextReset: { reason: "session/load failed: bad id" },
    });
    expect(state.sessionUsage).toBeNull();
    expect(state.contextPrimerAvailable).toBeNull();
    expect(state.lastSeq).toBe(2);
  });

  it("SessionContextReset that arrives BEFORE the first prompt stays hidden after later prompts", () => {
    let state = ev(emptyAcpState(), 1, { UsageUpdated: { usage: { used: 100, size: 200000 } } });
    state = ev(state, 2, { SessionContextReset: { reason: "session/load failed" } });
    state = ev(state, 3, { UserPromptSent: { text: "hi" } });
    expect(state.activity.some((r) => r.kind === "context_reset")).toBe(false);
  });

  it("SessionContextReset with prior prompt sets contextPrimerAvailable (#1004)", () => {
    let state = ev(emptyAcpState(), 1, { UserPromptSent: { text: "do a thing" } });
    expect(state.contextPrimerAvailable).toBeNull();
    state = ev(state, 2, { SessionContextReset: { reason: "load failed: bad id" } });
    expect(state.contextPrimerAvailable).toEqual({
      resetSeq: 2,
      reason: "load failed: bad id",
    });
  });

  it("SessionContextReset without prior prompt does not set contextPrimerAvailable", () => {
    const state = ev(emptyAcpState(), 1, { SessionContextReset: { reason: "load failed" } });
    expect(state.contextPrimerAvailable).toBeNull();
  });

  it("codex /new driven reset drops the context tracker to the post-reset baseline (#2979)", () => {
    let state = ev(emptyAcpState(), 1, { UsageUpdated: { usage: { used: 75000, size: 200000 } } });
    expect(state.sessionUsage?.used).toBe(75000);
    state = ev(state, 2, { UserPromptSent: { text: "/new" } });
    expect(state.turnActive).toBe(true);
    state = ev(state, 3, "SessionCleared");
    expect(state.sessionUsage).toBeNull();
    state = ev(state, 4, {
      SessionContextReset: { reason: "conversation cleared; the agent started a fresh session" },
    });
    expect(state.sessionUsage).toBeNull();
    expect(state.usageBaseline).toBeNull();
    state = ev(state, 5, { AcpSessionAssigned: { acp_session_id: "fresh-uuid" } });
    state = ev(state, 6, { Stopped: { reason: "session_reset" } });
    expect(state.turnActive).toBe(false);
    expect(state.activity.some((r) => r.kind === "empty_output")).toBe(false);
    state = ev(state, 7, { UsageUpdated: { usage: { used: 1200, size: 200000 } } });
    expect(state.sessionUsage?.used).toBe(1200);
  });

  it("UserPromptSent clears contextPrimerAvailable (one-shot affordance)", () => {
    let state = ev(emptyAcpState(), 1, { UserPromptSent: { text: "first" } });
    state = ev(state, 2, { SessionContextReset: { reason: "load failed" } });
    expect(state.contextPrimerAvailable).not.toBeNull();
    state = ev(state, 3, { UserPromptSent: { text: "second" } });
    expect(state.contextPrimerAvailable).toBeNull();
  });
});

describe("applyEvent / Stopped user_stopped", () => {
  it("sets workerStopped on reason=user_stopped and clears turnActive", () => {
    let state = ev(emptyAcpState(), 1, { UserPromptSent: { text: "long task" } });
    expect(state.turnActive).toBe(true);
    expect(state.workerStopped).toBe(false);
    state = ev(state, 2, { Stopped: { reason: "user_stopped" } });
    expect(state.workerStopped).toBe(true);
    expect(state.turnActive).toBe(false);
  });

  it("does NOT set workerStopped on reason=prompt_complete", () => {
    let state = ev(emptyAcpState(), 1, { UserPromptSent: { text: "hi" } });
    state = ev(state, 2, { Stopped: { reason: "prompt_complete" } });
    expect(state.workerStopped).toBe(false);
  });

  it("clears workerStopped on the next UserPromptSent", () => {
    let state = ev(emptyAcpState(), 1, { Stopped: { reason: "user_stopped" } });
    expect(state.workerStopped).toBe(true);
    state = ev(state, 2, { UserPromptSent: { text: "back online" } });
    expect(state.workerStopped).toBe(false);
  });

  it("clears workerStopped on AcpSessionAssigned (manual reconnect succeeded)", () => {
    let state = ev(emptyAcpState(), 1, { Stopped: { reason: "user_stopped" } });
    expect(state.workerStopped).toBe(true);
    state = ev(state, 2, { AcpSessionAssigned: { acp_session_id: "abc-123" } });
    expect(state.workerStopped).toBe(false);
  });
});

describe("applyEvent / Stopped rate_limit_exhausted_retries", () => {
  it("sets the exhausted retry notice", () => {
    const next = ev(emptyAcpState(), 1, { Stopped: { reason: "rate_limit_exhausted_retries" } });
    expect(next.rateLimitRetriesExhausted).toBe(true);
  });
});

describe("applyEvent / RateLimitAutoResumed", () => {
  it("clears the exhausted retry notice", () => {
    const seeded: AcpState = {
      ...emptyAcpState(),
      rateLimitRetriesExhausted: true,
    };
    const next = ev(seeded, 1, { RateLimitAutoResumed: { resets_at: "2026-09-02T00:00:00Z" } });
    expect(next.rateLimitRetriesExhausted).toBe(false);
  });
});

describe("applyEvent / Stopped restart_pending", () => {
  it("sets workerRestarting (not workerStopped) on reason=restart_pending", () => {
    const state = ev(emptyAcpState(), 1, { Stopped: { reason: "restart_pending" } });
    expect(state.workerRestarting).toBe(true);
    expect(state.workerStopped).toBe(false);
    expect(state.turnActive).toBe(false);
  });

  it("clears workerRestarting on AcpSessionAssigned (reconciler auto-respawn finished)", () => {
    let state = ev(emptyAcpState(), 1, { Stopped: { reason: "restart_pending" } });
    expect(state.workerRestarting).toBe(true);
    state = ev(state, 2, { AcpSessionAssigned: { acp_session_id: "fresh-id" } });
    expect(state.workerRestarting).toBe(false);
  });

  it("user_stopped → restart_pending transitions cleanly", () => {
    let state = ev(emptyAcpState(), 1, { Stopped: { reason: "user_stopped" } });
    expect(state.workerStopped).toBe(true);
    state = ev(state, 2, { Stopped: { reason: "restart_pending" } });
    expect(state.workerStopped).toBe(false);
    expect(state.workerRestarting).toBe(true);
  });
});

describe("applyEvent / Stopped idle_auto_stop (#1689)", () => {
  it("sets workerIdleStopped (not workerStopped) on reason=idle_auto_stop", () => {
    const state = ev(emptyAcpState(), 1, { Stopped: { reason: "idle_auto_stop" } });
    expect(state.workerIdleStopped).toBe(true);
    expect(state.workerStopped).toBe(false);
    expect(state.workerRestarting).toBe(false);
    expect(state.turnActive).toBe(false);
  });

  it("clears workerIdleStopped on the next UserPromptSent (the prompt woke it)", () => {
    let state = ev(emptyAcpState(), 1, { Stopped: { reason: "idle_auto_stop" } });
    expect(state.workerIdleStopped).toBe(true);
    state = ev(state, 2, { UserPromptSent: { text: "wake up" } });
    expect(state.workerIdleStopped).toBe(false);
  });

  it("clears workerIdleStopped on AcpSessionAssigned (respawn handshake landed)", () => {
    let state = ev(emptyAcpState(), 1, { Stopped: { reason: "idle_auto_stop" } });
    expect(state.workerIdleStopped).toBe(true);
    state = ev(state, 2, { AcpSessionAssigned: { acp_session_id: "fresh-id" } });
    expect(state.workerIdleStopped).toBe(false);
  });
});

describe("applyEvent / WakeupScheduled lifecycle", () => {
  it("user-typed prompt mid-wait keeps the pending wakeup", () => {
    const future = new Date(Date.now() + 95_000).toISOString();
    let state = ev(emptyAcpState(), 1, { WakeupScheduled: { at: future, reason: "test wake" } });
    expect(state.nextWakeupAt).toBe(future);
    state = ev(state, 2, { UserPromptSent: { text: "btw, ping me when you wake" } });
    expect(state.nextWakeupAt).toBe(future);
    expect(state.nextWakeupReason).toBe("test wake");
  });

  it("prompt after wakeup `at` clears the pending wakeup", () => {
    const past = new Date(Date.now() - 5_000).toISOString();
    let state = ev(emptyAcpState(), 1, { WakeupScheduled: { at: past, reason: "test wake" } });
    expect(state.nextWakeupAt).toBe(past);
    state = ev(state, 2, { UserPromptSent: { text: "Wake-up fired. Confirm." } });
    expect(state.nextWakeupAt).toBeNull();
    expect(state.nextWakeupReason).toBeNull();
  });
});

describe("applyEvent / MonitorArmed lifecycle", () => {
  it("MonitorArmed sets the monitoring badge with its description", () => {
    const state = ev(emptyAcpState(), 1, { MonitorArmed: { description: "clippy passes" } });
    expect(state.monitorArmed).toBe(true);
    expect(state.monitorDescription).toBe("clippy passes");
  });

  it("persists across agent activity with no user prompt", () => {
    let state = ev(emptyAcpState(), 1, { MonitorArmed: { description: "build" } });
    state = ev(state, 2, "ThinkingStarted");
    state = ev(state, 3, { AgentMessageChunk: { text: "resuming" } });
    expect(state.monitorArmed).toBe(true);
  });

  it("clears on the next user prompt (the user takes over)", () => {
    let state = ev(emptyAcpState(), 1, { MonitorArmed: { description: "watch" } });
    state = ev(state, 2, { UserPromptSent: { text: "stop watching" } });
    expect(state.monitorArmed).toBe(false);
    expect(state.monitorDescription).toBeNull();
  });

  it("persists on the arming turn's Stopped while the monitor is still pending", () => {
    let state = ev(emptyAcpState(), 1, { MonitorArmed: { description: "watch" } });
    state = ev(state, 2, { Stopped: { reason: "prompt_complete" } });
    expect(state.monitorArmed).toBe(true);
  });

  it.each(["prompt_complete", "agent_idle"])(
    "clears once the monitor fired (tool started after arm) and the turn ends with %s",
    (reason) => {
      let state = ev(emptyAcpState(), 1, { MonitorArmed: { description: "watch" } });
      state = ev(state, 2, {
        ToolCallStarted: {
          tool_call: {
            id: "tc-1",
            name: "Read File",
            kind: "read",
            args_preview: "{}",
            started_at: new Date().toISOString(),
          },
        },
      });
      expect(state.monitorArmed).toBe(true);
      state = ev(state, 3, { Stopped: { reason } });
      expect(state.monitorArmed).toBe(false);
      expect(state.monitorDescription).toBeNull();
    },
  );
});

describe("applyEvent / SessionCleared", () => {
  it("snapshots the cost baseline and leaves the rest to the server", () => {
    const seeded: AcpState = {
      ...emptyAcpState(),
      sessionUsage: { used: 10, size: 200_000, cost: { amount: 1.5, currency: "USD" } },
      usageBaseline: { cost: 2 },
    };
    const next = ev(seeded, 7, "SessionCleared");
    expect(next.usageBaseline).toEqual({ cost: 3.5 });
    expect(next.sessionUsage).toBeNull();
  });
});

describe("applyEvent / ConversationCompacted", () => {
  it("drops the stale usage snapshot (the compacted divider row is server-owned)", () => {
    const seeded: AcpState = {
      ...emptyAcpState(),
      sessionUsage: { used: 100, size: 200_000 },
    };
    const next = ev(seeded, 9, "ConversationCompacted");
    expect(next.activity).toHaveLength(0);
    expect(next.sessionUsage).toBeNull();
  });

  it("does not arm the primer banner", () => {
    const next = ev(emptyAcpState(), 3, "ConversationCompacted");
    expect(next.contextPrimerAvailable).toBeNull();
  });
});

describe("applyEvent / usageBaseline (#1354)", () => {
  it("SessionCleared captures the cumulative cost as the baseline", () => {
    let state = ev(emptyAcpState(), 1, {
      UsageUpdated: {
        usage: {
          used: 10_000,
          size: 200_000,
          cost: { amount: 0.42, currency: "USD" },
        },
      },
    });
    expect(state.sessionUsage?.cost?.amount).toBeCloseTo(0.42, 6);
    expect(state.usageBaseline).toBeNull();

    state = ev(state, 2, "SessionCleared");
    expect(state.sessionUsage).toBeNull();
    expect(state.usageBaseline?.cost).toBeCloseTo(0.42, 6);
  });

  it("UsageUpdated after /clear subtracts the baseline from cumulative cost", () => {
    let state = ev(emptyAcpState(), 1, {
      UsageUpdated: {
        usage: {
          used: 10_000,
          size: 200_000,
          cost: { amount: 0.42, currency: "USD" },
        },
      },
    });
    state = ev(state, 2, "SessionCleared");
    state = ev(state, 3, {
      UsageUpdated: {
        usage: {
          used: 5_000,
          size: 200_000,
          cost: { amount: 0.49, currency: "USD" },
        },
      },
    });
    expect(state.sessionUsage?.cost?.amount).toBeCloseTo(0.07, 6);
    expect(state.sessionUsage?.cost?.currency).toBe("USD");
    expect(state.sessionUsage?.used).toBe(5_000);
    expect(state.sessionUsage?.size).toBe(200_000);
  });

  it("/clear with no prior usage leaves the next UsageUpdate untouched", () => {
    let state = ev(emptyAcpState(), 1, "SessionCleared");
    expect(state.usageBaseline?.cost).toBe(0);
    state = ev(state, 2, {
      UsageUpdated: {
        usage: {
          used: 1_000,
          size: 200_000,
          cost: { amount: 0.05, currency: "USD" },
        },
      },
    });
    expect(state.sessionUsage?.cost?.amount).toBeCloseTo(0.05, 6);
  });

  it("repeated /clear accumulates the baseline to the true cumulative", () => {
    let state = ev(emptyAcpState(), 1, {
      UsageUpdated: {
        usage: {
          used: 10_000,
          size: 200_000,
          cost: { amount: 0.1, currency: "USD" },
        },
      },
    });
    state = ev(state, 2, "SessionCleared");
    state = ev(state, 3, {
      UsageUpdated: {
        usage: {
          used: 4_000,
          size: 200_000,
          cost: { amount: 0.15, currency: "USD" },
        },
      },
    });
    expect(state.sessionUsage?.cost?.amount).toBeCloseTo(0.05, 6);
    state = ev(state, 4, "SessionCleared");
    expect(state.usageBaseline?.cost).toBeCloseTo(0.15, 6);
    state = ev(state, 5, {
      UsageUpdated: {
        usage: {
          used: 2_000,
          size: 200_000,
          cost: { amount: 0.18, currency: "USD" },
        },
      },
    });
    expect(state.sessionUsage?.cost?.amount).toBeCloseTo(0.03, 6);
  });

  it("ConversationCompacted captures the baseline the same way as /clear", () => {
    let state = ev(emptyAcpState(), 1, {
      UsageUpdated: {
        usage: {
          used: 20_000,
          size: 200_000,
          cost: { amount: 0.3, currency: "USD" },
        },
      },
    });
    state = ev(state, 2, "ConversationCompacted");
    expect(state.usageBaseline?.cost).toBeCloseTo(0.3, 6);
    state = ev(state, 3, {
      UsageUpdated: {
        usage: {
          used: 1_000,
          size: 200_000,
          cost: { amount: 0.32, currency: "USD" },
        },
      },
    });
    expect(state.sessionUsage?.cost?.amount).toBeCloseTo(0.02, 6);
  });

  it("AgentSwitched clears the baseline so the new backend starts at zero", () => {
    let state = ev(emptyAcpState(), 1, {
      UsageUpdated: {
        usage: {
          used: 10_000,
          size: 200_000,
          cost: { amount: 0.42, currency: "USD" },
        },
      },
    });
    state = ev(state, 2, "SessionCleared");
    expect(state.usageBaseline?.cost).toBeCloseTo(0.42, 6);
    state = ev(state, 3, {
      AgentSwitched: { from: "claude", to: "codex", reason: "rate_limited" },
    });
    expect(state.usageBaseline).toBeNull();
    state = ev(state, 4, {
      UsageUpdated: {
        usage: {
          used: 500,
          size: 200_000,
          cost: { amount: 0.01, currency: "USD" },
        },
      },
    });
    expect(state.sessionUsage?.cost?.amount).toBeCloseTo(0.01, 6);
  });

  it("SessionContextReset clears the baseline (new ACP session starts at zero)", () => {
    let state = ev(emptyAcpState(), 1, {
      UsageUpdated: {
        usage: {
          used: 10_000,
          size: 200_000,
          cost: { amount: 0.2, currency: "USD" },
        },
      },
    });
    state = ev(state, 2, "SessionCleared");
    expect(state.usageBaseline?.cost).toBeCloseTo(0.2, 6);
    state = ev(state, 3, { SessionContextReset: { reason: "session/load failed" } });
    expect(state.usageBaseline).toBeNull();
  });

  it("UsageUpdated with no cost field is a no-op for the baseline", () => {
    let state = ev(emptyAcpState(), 1, "SessionCleared");
    state = ev(state, 2, {
      UsageUpdated: { usage: { used: 100, size: 200_000 } },
    });
    expect(state.sessionUsage?.cost ?? null).toBeNull();
    expect(state.sessionUsage?.used).toBe(100);
  });

  it("compact after /clear stacks the baseline onto the prior cumulative", () => {
    let state = ev(emptyAcpState(), 1, {
      UsageUpdated: {
        usage: {
          used: 10_000,
          size: 200_000,
          cost: { amount: 0.1, currency: "USD" },
        },
      },
    });
    state = ev(state, 2, "SessionCleared");
    expect(state.usageBaseline?.cost).toBeCloseTo(0.1, 6);
    state = ev(state, 3, {
      UsageUpdated: {
        usage: {
          used: 5_000,
          size: 200_000,
          cost: { amount: 0.15, currency: "USD" },
        },
      },
    });
    expect(state.sessionUsage?.cost?.amount).toBeCloseTo(0.05, 6);
    state = ev(state, 4, "ConversationCompacted");
    expect(state.usageBaseline?.cost).toBeCloseTo(0.15, 6);
    state = ev(state, 5, {
      UsageUpdated: {
        usage: {
          used: 2_000,
          size: 200_000,
          cost: { amount: 0.17, currency: "USD" },
        },
      },
    });
    expect(state.sessionUsage?.cost?.amount).toBeCloseTo(0.02, 6);
  });

  it("UsageUpdated with baseline set but no incoming cost passes the usage through raw", () => {
    let state = ev(emptyAcpState(), 1, {
      UsageUpdated: {
        usage: {
          used: 10_000,
          size: 200_000,
          cost: { amount: 0.1, currency: "USD" },
        },
      },
    });
    state = ev(state, 2, "SessionCleared");
    expect(state.usageBaseline?.cost).toBeCloseTo(0.1, 6);
    state = ev(state, 3, {
      UsageUpdated: { usage: { used: 1_000, size: 200_000 } },
    });
    expect(state.sessionUsage?.used).toBe(1_000);
    expect(state.sessionUsage?.cost ?? null).toBeNull();
    state = ev(state, 4, {
      UsageUpdated: {
        usage: {
          used: 1_500,
          size: 200_000,
          cost: { amount: 0.12, currency: "USD" },
        },
      },
    });
    expect(state.sessionUsage?.cost?.amount).toBeCloseTo(0.02, 6);
  });

  it("clamps cost to zero if the agent ever reports a smaller cumulative than the baseline", () => {
    let state = ev(emptyAcpState(), 1, {
      UsageUpdated: {
        usage: {
          used: 10_000,
          size: 200_000,
          cost: { amount: 0.5, currency: "USD" },
        },
      },
    });
    state = ev(state, 2, "SessionCleared");
    state = ev(state, 3, {
      UsageUpdated: {
        usage: {
          used: 100,
          size: 200_000,
          cost: { amount: 0.1, currency: "USD" },
        },
      },
    });
    expect(state.sessionUsage?.cost?.amount).toBe(0);
  });
});

describe("applyEvent / AgentSwitched", () => {
  it("records the handoff and resets the cost baseline", () => {
    const seeded: AcpState = {
      ...emptyAcpState(),
      sessionUsage: { used: 100, size: 200_000 },
      usageBaseline: { cost: 4 },
    };
    const next = ev(seeded, 11, {
      AgentSwitched: { from: "claude", to: "codex", reason: "rate_limited" },
    });
    expect(next.sessionUsage).toBeNull();
    expect(next.usageBaseline).toBeNull();
    expect(next.lastAgentSwitch).toMatchObject({
      from: "claude",
      to: "codex",
      reason: "rate_limited",
    });
    expect(next.activity).toHaveLength(0);
  });

  it("does not double-apply on replay", () => {
    const first = ev(emptyAcpState(), 5, {
      AgentSwitched: { from: "claude", to: "codex", reason: "rate_limited" },
    });
    const second = applyEvent(first, {
      session_id: "s-1",
      seq: 5, // same seq; reducer must drop.
      event: {
        AgentSwitched: { from: "claude", to: "codex", reason: "rate_limited" },
      },
    });
    expect(second).toBe(first);
  });

  it("clears stale worker-stopped flags from the prior backend shutdown", () => {
    const seeded: AcpState = {
      ...emptyAcpState(),
      agent: "claude",
      workerStopped: true,
      workerRestarting: true,
      agentUnresponsive: true,
    };
    const next = ev(seeded, 13, {
      AgentSwitched: { from: "claude", to: "codex", reason: "rate_limited" },
    });
    expect(next.workerStopped).toBe(false);
    expect(next.workerRestarting).toBe(false);
    expect(next.agentUnresponsive).toBe(false);
  });

  it("clears the exhausted retry notice from the prior backend", () => {
    const seeded: AcpState = {
      ...emptyAcpState(),
      rateLimitRetriesExhausted: true,
    };
    const next = ev(seeded, 13, {
      AgentSwitched: { from: "claude", to: "codex", reason: "rate_limited" },
    });
    expect(next.rateLimitRetriesExhausted).toBe(false);
  });
});

describe("turnActive: daemon truth plus an optimistic overlay (#3417)", () => {
  function reduced(turn_active: boolean): ReducedState {
    return {
      agent: "claude",
      model: null,
      mode: "Default",
      current_plan: null,
      in_flight_tool: null,
      pending_approvals: [],
      pending_elicitations: [],
      thinking: null,
      rate_limit: null,
      available_commands: [],
      available_modes: [],
      current_mode_id: null,
      turn_active,
      cancelling: false,
      compacting: false,
    };
  }

  it("deriveTurnActive ORs daemon truth with an unacknowledged prompt", () => {
    const cases: Array<[boolean, string[], boolean]> = [
      [true, [], true],
      [false, [], false],
      [false, ["p1"], true], // POST sent, echo not yet applied
      [true, ["p1"], true],
    ];
    for (const [serverTurnActive, inflightPromptIds, expected] of cases) {
      expect(deriveTurnActive({ serverTurnActive, inflightPromptIds })).toBe(expected);
    }
  });

  it("N prompts steered into one turn are all closed by its single Stopped", async () => {
    const { acpHookReducer } = await import("../hooks/useAcpSession");
    let state = ev(emptyAcpState(), 1, {
      PromptCapabilities: { image: false, audio: false, embedded_context: false, steering: true },
    });
    let seq = 1;
    for (const id of ["p1", "p2", "p3", "p4", "p5"]) {
      state = acpHookReducer(state, { kind: "user_prompt", id, text: id });
      expect(state.turnActive).toBe(true);
      seq += 1;
      state = ev(state, seq, { UserPromptSent: { text: id, prompt_id: id } });
      state = applyReducedState(state, reduced(true));
      expect(state.turnActive).toBe(true);
    }
    expect(state.inflightPromptIds).toEqual([]);
    expect(state.promptSeq).toBe(5);

    state = ev(state, seq + 1, { Stopped: { reason: "prompt_complete" } });
    state = applyReducedState(state, reduced(false));
    expect(state.turnActive).toBe(false);
    expect(state.serverTurnActive).toBe(false);
  });

  it("a Stopped ends the turn on the happy single-prompt path", () => {
    let state = ev(emptyAcpState(), 1, { UserPromptSent: { text: "hi" } });
    expect(state.turnActive).toBe(true);
    state = ev(state, 2, { Stopped: { reason: "prompt_complete" } });
    expect(state.turnActive).toBe(false);
  });

  it("late Stopped from a prior turn does NOT clobber a fresh follow-up", async () => {
    const { acpHookReducer } = await import("../hooks/useAcpSession");
    let state = ev(emptyAcpState(), 1, { UserPromptSent: { text: "first turn" } });
    expect(state.turnActive).toBe(true);
    state = acpHookReducer(state, { kind: "user_prompt", id: "cmp-fu", text: "follow-up" });
    expect(state.inflightPromptIds).toEqual(["cmp-fu"]);

    state = ev(state, 2, { Stopped: { reason: "prompt_complete" } });
    expect(state.serverTurnActive).toBe(false);
    expect(state.turnActive).toBe(true);
    state = applyReducedState(state, reduced(false));
    expect(state.turnActive).toBe(true);

    state = ev(state, 3, { UserPromptSent: { text: "follow-up", prompt_id: "cmp-fu" } });
    expect(state.inflightPromptIds).toEqual([]);
    expect(state.turnActive).toBe(true);
    expect(state.promptSeq).toBe(2);

    state = ev(state, 4, { Stopped: { reason: "prompt_complete" } });
    expect(state.turnActive).toBe(false);
  });

  it("a spurious Stopped on an idle session does not poison the next prompt", () => {
    let state = ev(emptyAcpState(), 1, { UserPromptSent: { text: "hi" } });
    state = ev(state, 2, { Stopped: { reason: "prompt_complete" } });
    expect(state.turnActive).toBe(false);
    state = ev(state, 3, { Stopped: { reason: "prompt_complete" } });
    expect(state.turnActive).toBe(false);
    state = ev(state, 4, { UserPromptSent: { text: "second" } });
    expect(state.turnActive).toBe(true);
    expect(state.promptSeq).toBe(2);
  });

  it("optimistic user_prompt plus its matching echo settles exactly that id", async () => {
    const { acpHookReducer } = await import("../hooks/useAcpSession");
    let state = acpHookReducer(emptyAcpState(), { kind: "user_prompt", id: "cmp-echo", text: "echo me" });
    expect(state.inflightPromptIds).toEqual(["cmp-echo"]);
    state = ev(state, 5, { UserPromptSent: { text: "echo me", prompt_id: "cmp-echo" } });
    expect(state.inflightPromptIds).toEqual([]);
    expect(state.turnActive).toBe(true);
    expect(state.promptSeq).toBe(1);
  });

  it("AgentStartupError closes the turn", () => {
    let state = ev(emptyAcpState(), 1, { UserPromptSent: { text: "first" } });
    state = ev(state, 2, { AgentStartupError: { message: "boom" } });
    expect(state.turnActive).toBe(false);
    expect(state.startupError).toBe("boom");
  });

  it("an idle session's own first prompt is not a steered continuation", async () => {
    const { acpHookReducer } = await import("../hooks/useAcpSession");
    let state = ev(emptyAcpState(), 1, {
      PromptCapabilities: { image: false, audio: false, embedded_context: false, steering: true },
    });
    state = { ...state, workerStopped: true, workerRestarting: true };
    state = acpHookReducer(state, { kind: "user_prompt", id: "cmp-first", text: "first" });
    expect(state.turnActive).toBe(true);
    expect(state.serverTurnActive).toBe(false);

    state = ev(state, 2, { UserPromptSent: { text: "first", prompt_id: "cmp-first" } });
    expect(state.workerStopped).toBe(false);
    expect(state.workerRestarting).toBe(false);
  });

  it("a genuinely steered mid-turn prompt still skips the per-turn resets", () => {
    const running: AcpState = {
      ...emptyAcpState(),
      promptCapabilities: { image: false, audio: false, embeddedContext: false, steering: true },
      serverTurnActive: true,
      turnActive: true,
      cancelEscalatesAt: new Date(Date.now() + 10_000).toISOString(),
    };
    const next = ev(running, 2, { UserPromptSent: { text: "steered" } });
    expect(next.cancelEscalatesAt).not.toBeNull();
  });

  it("optimistic-match UserPromptSent still applies the per-turn resets", () => {
    const stale: AcpState = {
      ...withOptimisticPrompt(emptyAcpState(), "follow-up", "cmp-fu"),
      workerStopped: true,
      workerRestarting: true,
      nextWakeupAt: new Date(Date.now() - 1_000).toISOString(),
      nextWakeupReason: "tick",
    };
    const next = ev(stale, 9, { UserPromptSent: { text: "follow-up", prompt_id: "cmp-fu" } });
    expect(next.workerStopped).toBe(false);
    expect(next.workerRestarting).toBe(false);
    expect(next.nextWakeupAt).toBeNull();
    expect(next.nextWakeupReason).toBeNull();
    expect(next.inflightPromptIds).toEqual([]);
    expect(next.turnActive).toBe(true);
  });
});

describe("normaliseTurnState (#3417 persisted-state backfill)", () => {
  it("seeds serverTurnActive from a cached turnActive=true", () => {
    const cached = {
      ...emptyAcpState(),
      turnActive: true,
    } as AcpState & { serverTurnActive?: boolean };
    delete cached.serverTurnActive;
    const normalised = normaliseTurnState(cached);
    expect(normalised.serverTurnActive).toBe(true);
    expect(normalised.turnActive).toBe(true);
  });

  it("seeds serverTurnActive from a cached turnActive=false", () => {
    const cached = {
      ...emptyAcpState(),
      turnActive: false,
    } as AcpState & { serverTurnActive?: boolean };
    delete cached.serverTurnActive;
    const normalised = normaliseTurnState(cached);
    expect(normalised.serverTurnActive).toBe(false);
    expect(normalised.turnActive).toBe(false);
  });

  it("never restores in-flight prompt ids: no POST survives a reload", () => {
    const cached = {
      ...emptyAcpState(),
      serverTurnActive: false,
      turnActive: true,
      inflightPromptIds: ["cmp-stale"],
    } as AcpState;
    const normalised = normaliseTurnState(cached);
    expect(normalised.inflightPromptIds).toEqual([]);
    expect(normalised.turnActive).toBe(false);
  });

  it("backfills promptSeq from the persisted prompt rows on a pre-#3417 entry", () => {
    const cached = {
      ...emptyAcpState(),
      activity: [
        { id: "a", kind: "user_prompt", text: "one", at: "" },
        { id: "b", kind: "message", text: "hi", at: "" },
        { id: "c", kind: "user_prompt", text: "two", at: "" },
      ],
    } as AcpState & { promptSeq?: number };
    delete cached.promptSeq;
    expect(normaliseTurnState(cached).promptSeq).toBe(2);
  });
});

describe("compaction reminder dismissal", () => {
  const usageFrame = (seq: number, used: number, size = 200_000): AcpFrame => ({
    session_id: "s-1",
    seq,
    event: { UsageUpdated: { usage: { used, size } } },
  });

  it("survives usage climbing further, and re-arms after a context boundary", async () => {
    const { acpHookReducer } = await import("../hooks/useAcpSession");
    let state = applyEvent(emptyAcpState(), usageFrame(1, 160_000));

    state = acpHookReducer(state, { kind: "dismiss_compaction_reminder" });
    expect(state.compactionReminderDismissed?.used).toBe(160_000);

    state = applyEvent(state, usageFrame(2, 180_000));
    expect(state.compactionReminderDismissed?.used).toBe(160_000);

    state = ev(state, 3, "ConversationCompacted");
    expect(state.sessionUsage).toBeNull();
    state = applyEvent(state, usageFrame(4, 20_000));
    expect(state.compactionReminderDismissed).toBeNull();

    state = acpHookReducer(state, { kind: "dismiss_compaction_reminder" });
    state = applyEvent(state, usageFrame(5, 30_000));
    expect(state.compactionReminderDismissed?.used).toBe(20_000);
    state = ev(state, 6, "SessionCleared");
    state = applyEvent(state, usageFrame(7, 40_000));
    expect(state.compactionReminderDismissed).toBeNull();
  });

  it("re-arms on every boundary that nulls the usage snapshot", async () => {
    const { acpHookReducer } = await import("../hooks/useAcpSession");
    const boundaries: AcpFrame["event"][] = [
      "ConversationCompacted",
      "SessionCleared",
      { SessionContextReset: { reason: "session/load failed: bad id" } },
      { AgentSwitched: { from: "claude", to: "codex", reason: "rate_limit" } },
    ];
    for (const event of boundaries) {
      let state = applyEvent(emptyAcpState(), usageFrame(1, 160_000));
      state = acpHookReducer(state, { kind: "dismiss_compaction_reminder" });
      state = ev(state, 2, event);
      state = applyEvent(state, usageFrame(3, 170_000));
      expect(state.compactionReminderDismissed, JSON.stringify(event)).toBeNull();
    }
  });

  it("backfills the dismissal on entries persisted before it existed", () => {
    const persisted = { ...emptyAcpState() } as AcpState & {
      compactionReminderDismissed?: AcpState["compactionReminderDismissed"];
    };
    delete persisted.compactionReminderDismissed;
    expect(normaliseTurnState(persisted).compactionReminderDismissed).toBeNull();
  });
});

describe("acpHookReducer / dismiss_primer", () => {
  it("clears contextPrimerAvailable", async () => {
    const { acpHookReducer } = await import("../hooks/useAcpSession");
    const seeded: AcpState = {
      ...emptyAcpState(),
      contextPrimerAvailable: {
        resetSeq: 12,
        reason: "Conversation context reset; agent transcript was unavailable.",
      },
    };
    const next = acpHookReducer(seeded, { kind: "dismiss_primer" });
    expect(next.contextPrimerAvailable).toBeNull();
  });
});

describe("applyEvent / ModeSwitchFailed", () => {
  it("captures the rejected mode + reason", () => {
    const next = ev(emptyAcpState(), 1, {
      ModeSwitchFailed: {
        mode_id: "bypassPermissions",
        reason: "Mode bypassPermissions is not available.",
      },
    });
    expect(next.modeSwitchFailed).not.toBeNull();
    expect(next.modeSwitchFailed?.modeId).toBe("bypassPermissions");
    expect(next.modeSwitchFailed?.reason).toBe("Mode bypassPermissions is not available.");
  });

  it("clears when a subsequent CurrentModeChanged lands", () => {
    let state = ev(emptyAcpState(), 1, {
      ModeSwitchFailed: { mode_id: "bypassPermissions", reason: "denied" },
    });
    expect(state.modeSwitchFailed).not.toBeNull();
    state = ev(state, 2, { CurrentModeChanged: { current_mode_id: "acceptEdits" } });
    expect(state.modeSwitchFailed).toBeNull();
  });
});

describe("acpHookReducer / dismiss_mode_switch_failed", () => {
  it("clears the notice", async () => {
    const { acpHookReducer } = await import("../hooks/useAcpSession");
    const seeded: AcpState = {
      ...emptyAcpState(),
      modeSwitchFailed: {
        modeId: "bypassPermissions",
        reason: "denied",
        at: new Date().toISOString(),
      },
    };
    const next = acpHookReducer(seeded, {
      kind: "dismiss_mode_switch_failed",
    });
    expect(next.modeSwitchFailed).toBeNull();
  });
});

function stoppedFrame(reason: string, seq: number): AcpFrame {
  return {
    session_id: "s-orphan",
    seq,
    event: { Stopped: { reason } },
  };
}

describe("AcpState reducer / silent-orphan watchdog (#1240)", () => {
  it("sets agentOrphaned and workerRestarting on prompt_orphaned", () => {
    let state: AcpState = {
      ...emptyAcpState(),
      serverTurnActive: true,
      turnActive: true,
      promptSeq: 1,
    };
    state = applyEvent(state, stoppedFrame("prompt_orphaned", 1));
    expect(state.agentOrphaned).toBe(true);
    expect(state.workerRestarting).toBe(true);
    expect(state.workerStopped).toBe(false);
    expect(state.agentUnresponsive).toBe(false);
  });

  it("clears agentUnresponsive when prompt_orphaned arrives after it", () => {
    let state: AcpState = {
      ...emptyAcpState(),
      serverTurnActive: true,
      turnActive: true,
      promptSeq: 2,
    };
    state = applyEvent(state, stoppedFrame("agent_unresponsive", 1));
    expect(state.agentUnresponsive).toBe(true);
    expect(state.agentOrphaned).toBe(false);
    state = applyEvent(state, stoppedFrame("prompt_orphaned", 2));
    expect(state.agentUnresponsive).toBe(false);
    expect(state.agentOrphaned).toBe(true);
  });

  it("clears agentOrphaned on AcpSessionAssigned (respawn completed)", () => {
    let state: AcpState = {
      ...emptyAcpState(),
      serverTurnActive: true,
      turnActive: true,
      promptSeq: 1,
    };
    state = applyEvent(state, stoppedFrame("prompt_orphaned", 1));
    expect(state.agentOrphaned).toBe(true);
    state = applyEvent(state, {
      session_id: "s-orphan",
      seq: 2,
      event: { AcpSessionAssigned: { acp_session_id: "sess-abc" } },
    });
    expect(state.agentOrphaned).toBe(false);
    expect(state.workerRestarting).toBe(false);
  });

  it("clears agentOrphaned on UserPromptSent (user moving on)", () => {
    let state: AcpState = {
      ...emptyAcpState(),
      serverTurnActive: true,
      turnActive: true,
      promptSeq: 1,
    };
    state = applyEvent(state, stoppedFrame("prompt_orphaned", 1));
    expect(state.agentOrphaned).toBe(true);
    state = applyEvent(state, {
      session_id: "s-orphan",
      seq: 2,
      event: { UserPromptSent: { text: "next prompt" } },
    });
    expect(state.agentOrphaned).toBe(false);
  });

  it("clears agentOrphaned on user_stopped", () => {
    let state: AcpState = {
      ...emptyAcpState(),
      serverTurnActive: true,
      turnActive: true,
      promptSeq: 1,
    };
    state = applyEvent(state, stoppedFrame("prompt_orphaned", 1));
    expect(state.agentOrphaned).toBe(true);
    state = applyEvent(state, stoppedFrame("user_stopped", 2));
    expect(state.agentOrphaned).toBe(false);
  });

  it("backfills agentOrphaned=false on pre-#1240 persisted state", () => {
    const stale = {
      ...emptyAcpState(),
      promptSeq: 0,
    } as AcpState & { agentOrphaned?: boolean };
    delete stale.agentOrphaned;
    const normalised = normaliseTurnState(stale);
    expect(normalised.agentOrphaned).toBe(false);
  });

  it("backfills usageBaseline=null on pre-#1354 persisted state", () => {
    const stale = {
      ...emptyAcpState(),
      promptSeq: 0,
    } as AcpState & { usageBaseline?: { cost: number } | null };
    delete stale.usageBaseline;
    const normalised = normaliseTurnState(stale);
    expect(normalised.usageBaseline).toBeNull();
  });

  it("preserves a non-null usageBaseline through normaliseTurnState", () => {
    const cached: AcpState = {
      ...emptyAcpState(),
      promptSeq: 3,
      usageBaseline: { cost: 0.42 },
    };
    const normalised = normaliseTurnState(cached);
    expect(normalised.usageBaseline?.cost).toBeCloseTo(0.42, 6);
  });

  it("clears agentOrphaned on restart_pending", () => {
    let state: AcpState = {
      ...emptyAcpState(),
      serverTurnActive: true,
      turnActive: true,
      promptSeq: 1,
    };
    state = applyEvent(state, stoppedFrame("prompt_orphaned", 1));
    expect(state.agentOrphaned).toBe(true);
    state = applyEvent(state, stoppedFrame("restart_pending", 2));
    expect(state.agentOrphaned).toBe(false);
    expect(state.workerRestarting).toBe(true);
  });

  it("clears agentOrphaned when agent_unresponsive arrives next", () => {
    let state: AcpState = {
      ...emptyAcpState(),
      serverTurnActive: true,
      turnActive: true,
      promptSeq: 2,
    };
    state = applyEvent(state, stoppedFrame("prompt_orphaned", 1));
    expect(state.agentOrphaned).toBe(true);
    state = applyEvent(state, stoppedFrame("agent_unresponsive", 2));
    expect(state.agentOrphaned).toBe(false);
    expect(state.agentUnresponsive).toBe(true);
    expect(state.workerRestarting).toBe(true);
  });
});

describe("applyEvent / IncompatibleAgent (claude-agent-acp v0.39.0)", () => {
  it("sets state.incompatibleAgent from the structured detail", () => {
    const next = ev(emptyAcpState(), 1, {
      IncompatibleAgent: {
        detail: {
          kind: "incompatible_agent_version",
          package_name: "@agentclientprotocol/claude-agent-acp",
          installed: "0.32.0",
          required: "0.39.0",
          install_command: "npm install -g @agentclientprotocol/claude-agent-acp@latest",
        },
      },
    });
    expect(next.incompatibleAgent).not.toBeNull();
    expect(next.incompatibleAgent?.kind).toBe("incompatible_agent_version");
    if (next.incompatibleAgent?.kind === "incompatible_agent_version") {
      expect(next.incompatibleAgent.installed).toBe("0.32.0");
      expect(next.incompatibleAgent.required).toBe("0.39.0");
    }
  });

  it("clears incompatibleAgent on AcpSessionAssigned (respawn healed)", () => {
    let state: AcpState = ev(emptyAcpState(), 1, {
      IncompatibleAgent: {
        detail: {
          kind: "incompatible_agent_version",
          package_name: "@agentclientprotocol/claude-agent-acp",
          installed: "0.32.0",
          required: "0.39.0",
          install_command: "npm install -g @agentclientprotocol/claude-agent-acp@latest",
        },
      },
    });
    expect(state.incompatibleAgent).not.toBeNull();
    state = ev(state, 2, { AcpSessionAssigned: { acp_session_id: "acp-1" } });
    expect(state.incompatibleAgent).toBeNull();
  });
});

describe("applyEvent / ConfigOptions (#1403)", () => {
  function sampleOptions() {
    return [
      {
        id: "model",
        name: "Model",
        category: "model" as const,
        current_value: "claude-opus-4-7",
        options: [
          { value: "claude-opus-4-7", name: "Claude Opus 4.7" },
          { value: "claude-sonnet-4-6", name: "Claude Sonnet 4.6" },
        ],
      },
      {
        id: "effort",
        name: "Reasoning Effort",
        category: "thought_level" as const,
        current_value: "default",
        options: [
          { value: "default", name: "Default" },
          { value: "high", name: "High" },
        ],
      },
    ];
  }

  it("applies ConfigOptionsUpdated as a full snapshot replacement", () => {
    let state = ev(emptyAcpState(), 1, { ConfigOptionsUpdated: { options: sampleOptions() } });
    expect(state.configOptions).toHaveLength(2);
    state = ev(state, 2, {
      ConfigOptionsUpdated: {
        options: [
          {
            id: "model",
            name: "Model",
            category: "model",
            current_value: "claude-sonnet-4-6",
            options: [],
          },
        ],
      },
    });
    expect(state.configOptions).toHaveLength(1);
    expect(state.configOptions[0].current_value).toBe("claude-sonnet-4-6");
  });

  it("populates configOptionSwitchFailed without mutating configOptions", () => {
    let state = ev(emptyAcpState(), 1, { ConfigOptionsUpdated: { options: sampleOptions() } });
    const before = state.configOptions;
    state = ev(state, 2, {
      ConfigOptionSwitchFailed: {
        config_id: "model",
        value: "claude-sonnet-4-6",
        reason: "rate limited",
      },
    });
    expect(state.configOptions).toBe(before);
    expect(state.configOptionSwitchFailed).toEqual({
      configId: "model",
      value: "claude-sonnet-4-6",
      reason: "rate limited",
      at: expect.any(String),
    });
  });

  it("clears pending and auto-dismisses matching failure on confirming snapshot", () => {
    let state: AcpState = {
      ...emptyAcpState(),
      pendingConfigOption: { configId: "model", value: "claude-sonnet-4-6" },
    };
    state = ev(state, 1, {
      ConfigOptionSwitchFailed: {
        config_id: "model",
        value: "claude-sonnet-4-6",
        reason: "transient",
      },
    });
    expect(state.pendingConfigOption).toBeNull();
    expect(state.configOptionSwitchFailed).not.toBeNull();

    const confirming = sampleOptions();
    confirming[0].current_value = "claude-sonnet-4-6";
    state = ev(state, 2, { ConfigOptionsUpdated: { options: confirming } });
    expect(state.configOptionSwitchFailed).toBeNull();
    expect(state.pendingConfigOption).toBeNull();
  });

  it("preserves a non-matching failure notice across snapshots", () => {
    let state = ev(emptyAcpState(), 1, { ConfigOptionsUpdated: { options: sampleOptions() } });
    state = ev(state, 2, {
      ConfigOptionSwitchFailed: {
        config_id: "model",
        value: "claude-sonnet-4-6",
        reason: "transient",
      },
    });
    state = ev(state, 3, { ConfigOptionsUpdated: { options: sampleOptions() } });
    expect(state.configOptionSwitchFailed).not.toBeNull();
  });

  it("AgentSwitched clears configOptions and the failure notice", () => {
    let state = ev(emptyAcpState(), 1, { ConfigOptionsUpdated: { options: sampleOptions() } });
    state = ev(state, 2, {
      ConfigOptionSwitchFailed: {
        config_id: "effort",
        value: "high",
        reason: "unsupported",
      },
    });
    state = ev(state, 3, {
      AgentSwitched: { from: "claude", to: "codex", reason: "rate_limit" },
    });
    expect(state.configOptions).toEqual([]);
    expect(state.configOptionSwitchFailed).toBeNull();
    expect(state.pendingConfigOption).toBeNull();
  });

  it("SessionCleared preserves configOptions (adapter capabilities outlive /clear)", () => {
    let state = ev(emptyAcpState(), 1, { ConfigOptionsUpdated: { options: sampleOptions() } });
    state = ev(state, 2, "SessionCleared");
    expect(state.configOptions).toHaveLength(2);
  });
});

describe("applyEvent / UsageUpdated context-window latch (upstream #596 bandaid)", () => {
  function usageFrame(seq: number, used: number, size: number): AcpFrame {
    return {
      session_id: "s-1",
      seq,
      event: { UsageUpdated: { usage: { used, size } } },
    };
  }
  function modelFrame(seq: number, currentValue: string): AcpFrame {
    return {
      session_id: "s-1",
      seq,
      event: {
        ConfigOptionsUpdated: {
          options: [
            {
              id: "model",
              name: "Model",
              category: "model",
              current_value: currentValue,
              options: [],
            },
          ],
        },
      },
    };
  }

  it("latches the largest window and ignores the mid-turn 200k downgrade", () => {
    let state = applyEvent(emptyAcpState(), usageFrame(1, 10_000, 200_000));
    expect(state.sessionUsage?.size).toBe(200_000);
    state = applyEvent(state, usageFrame(2, 20_000, 1_000_000));
    expect(state.sessionUsage?.size).toBe(1_000_000);
    state = applyEvent(state, usageFrame(3, 30_000, 200_000));
    expect(state.sessionUsage?.size).toBe(1_000_000);
    expect(state.sessionUsage?.used).toBe(30_000);
  });

  it("resets the latch on a context boundary (SessionCleared)", () => {
    let state = applyEvent(emptyAcpState(), usageFrame(1, 20_000, 1_000_000));
    expect(state.sessionUsage?.size).toBe(1_000_000);
    state = ev(state, 2, "SessionCleared");
    expect(state.sessionUsage).toBeNull();
    state = applyEvent(state, usageFrame(3, 5_000, 200_000));
    expect(state.sessionUsage?.size).toBe(200_000);
  });

  it("resets the latch when the model changes", () => {
    let state = applyEvent(emptyAcpState(), modelFrame(1, "sonnet"));
    state = applyEvent(state, usageFrame(2, 20_000, 1_000_000));
    expect(state.sessionUsage?.size).toBe(1_000_000);
    state = applyEvent(state, modelFrame(3, "haiku"));
    expect(state.sessionUsage).toBeNull();
    state = applyEvent(state, usageFrame(4, 5_000, 200_000));
    expect(state.sessionUsage?.size).toBe(200_000);
  });
});
