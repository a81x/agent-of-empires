// @vitest-environment jsdom

import { describe, expect, it } from "vitest";

import { emptyAcpState, type AcpFrame, type ActivityRow } from "../lib/acpTypes";
import { reducer } from "./useAcpSession";

function prompt(seq: number, text: string): AcpFrame {
  return { session_id: "s", seq, event: { UserPromptSent: { text } } };
}

function promptRow(seq: number, text: string): ActivityRow {
  return { id: `user-seq-${seq}`, kind: "user_prompt", text, at: "2026-01-01T00:00:00Z" };
}

describe("useAcpSession recent-first paging", () => {
  it("frames tail seeds oldestSeq once; live appends don't lower it", () => {
    let s = reducer(emptyAcpState(), {
      kind: "frames",
      frames: [prompt(5, "e"), prompt(6, "f")],
      oldestSeq: 5,
    });
    expect(s.oldestSeq).toBe(5);
    s = reducer(s, { kind: "frames", frames: [prompt(7, "g")] });
    expect(s.oldestSeq).toBe(5);
  });

  it("prepend adds older rows ahead of the loaded tail and lowers oldestSeq", () => {
    let s = reducer(emptyAcpState(), {
      kind: "frames",
      frames: [prompt(5, "e"), prompt(6, "f")],
      rows: [promptRow(5, "e"), promptRow(6, "f")],
      oldestSeq: 5,
    });
    s = reducer(s, { kind: "prepend", rows: [promptRow(2, "b"), promptRow(3, "c")], oldestSeq: 2 });
    expect(s.oldestSeq).toBe(2);
    expect(s.activity.map((r) => r.id)).toEqual(["user-seq-2", "user-seq-3", "user-seq-5", "user-seq-6"]);
  });

  it("prepend preserves optimistic / queue / approval state", () => {
    let s = reducer(emptyAcpState(), {
      kind: "frames",
      frames: [prompt(5, "e")],
      rows: [promptRow(5, "e")],
      oldestSeq: 5,
    });
    s = reducer(s, { kind: "enqueue_prompt", id: "q-1", text: "queued" });
    const approvals = [{ nonce: "n1" } as unknown as (typeof s.pendingApprovals)[number]];
    s = { ...s, pendingApprovals: approvals };

    s = reducer(s, { kind: "prepend", rows: [promptRow(2, "b")], oldestSeq: 2 });

    expect(s.queuedPrompts).toHaveLength(1);
    expect(s.queuedPrompts[0]!.text).toBe("queued");
    expect(s.pendingApprovals).toBe(approvals);
    expect(s.activity[0]!.id).toBe("user-seq-2");
  });

  it("handshake backfills empty fields but never overwrites loaded values", () => {
    const caps: AcpFrame = {
      session_id: "s",
      seq: 1,
      event: { PromptCapabilities: { image: true, audio: false, embedded_context: true } },
    };
    const withCaps = {
      ...emptyAcpState(),
      promptCapabilities: { image: false, audio: false, embeddedContext: false, steering: false },
    };
    const kept = reducer(withCaps, { kind: "handshake", frames: [caps] });
    expect(kept.promptCapabilities).toEqual({ image: false, audio: false, embeddedContext: false, steering: false });

    const filled = reducer(emptyAcpState(), { kind: "handshake", frames: [caps] });
    expect(filled.promptCapabilities).toEqual({ image: true, audio: false, embeddedContext: true, steering: false });
    expect(filled.activity).toHaveLength(0);
  });
});
