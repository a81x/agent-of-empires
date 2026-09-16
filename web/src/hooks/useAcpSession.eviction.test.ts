// @vitest-environment jsdom

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { emptyAcpState } from "../lib/acpTypes";
import { __test } from "./useAcpSession";

const { persistState, loadPersistedState, evictOldestPersistedAcpState, STORAGE_KEY_PREFIX } = __test;

const DRAFT_KEY_PREFIX = "acp:draft:";

function quotaError(): DOMException {
  return new DOMException("The quota has been exceeded.", "QuotaExceededError");
}

beforeEach(() => {
  window.localStorage.clear();
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("loadPersistedState schema backfill", () => {
  it("backfills fields added after an entry was persisted (pendingElicitations)", () => {
    const legacy = { ...emptyAcpState() } as Record<string, unknown>;
    delete legacy.pendingElicitations;
    window.localStorage.setItem(
      `${STORAGE_KEY_PREFIX}sess-legacy`,
      JSON.stringify({ savedAt: Date.now(), state: legacy }),
    );

    const loaded = loadPersistedState("sess-legacy");
    expect(loaded).toBeDefined();
    expect(loaded?.pendingElicitations).toEqual([]);
    expect(loaded?.pendingApprovals).toEqual([]);
  });

  it("backfills oldestSeq to 0 for a pre-#2236 entry and preserves a stored value", () => {
    const legacy = { ...emptyAcpState() } as Record<string, unknown>;
    delete legacy.oldestSeq;
    window.localStorage.setItem(
      `${STORAGE_KEY_PREFIX}sess-pre2236`,
      JSON.stringify({ savedAt: Date.now(), state: legacy }),
    );
    expect(loadPersistedState("sess-pre2236")?.oldestSeq).toBe(0);

    const withSeq = { ...emptyAcpState(), oldestSeq: 42 };
    window.localStorage.setItem(
      `${STORAGE_KEY_PREFIX}sess-seq`,
      JSON.stringify({ savedAt: Date.now(), state: withSeq }),
    );
    expect(loadPersistedState("sess-seq")?.oldestSeq).toBe(42);
  });

  it("preserves a persisted pendingElicitations list", () => {
    const state = {
      ...emptyAcpState(),
      pendingElicitations: [
        {
          nonce: "e-1",
          message: "Pick",
          tool_call_id: null,
          questions: [],
          requested_at: "2026-06-10T00:00:00Z",
          resolved: null,
        },
      ],
    };
    window.localStorage.setItem(`${STORAGE_KEY_PREFIX}sess-keep`, JSON.stringify({ savedAt: Date.now(), state }));
    const loaded = loadPersistedState("sess-keep");
    expect(loaded?.pendingElicitations.map((e) => e.nonce)).toEqual(["e-1"]);
  });
});

describe("structured view cache eviction (#1345)", () => {
  it("evicts the oldest acp-state entry when the write hits quota and retries", () => {
    const now = Date.now();
    window.localStorage.setItem(
      `${STORAGE_KEY_PREFIX}sess-old`,
      JSON.stringify({ savedAt: now - 86_400_000, state: emptyAcpState() }),
    );
    window.localStorage.setItem(
      `${STORAGE_KEY_PREFIX}sess-new`,
      JSON.stringify({ savedAt: now, state: emptyAcpState() }),
    );

    const setItem = vi.spyOn(Storage.prototype, "setItem").mockImplementationOnce(() => {
      throw quotaError();
    });

    persistState("sess-current", emptyAcpState());

    expect(setItem).toHaveBeenCalled();
    expect(window.localStorage.getItem(`${STORAGE_KEY_PREFIX}sess-old`)).toBeNull();
    expect(window.localStorage.getItem(`${STORAGE_KEY_PREFIX}sess-new`)).not.toBeNull();
    expect(window.localStorage.getItem(`${STORAGE_KEY_PREFIX}sess-current`)).not.toBeNull();
  });

  it("never evicts acp:draft:* entries even when older than acp-state entries", () => {
    const now = Date.now();
    window.localStorage.setItem(`${DRAFT_KEY_PREFIX}sess-old`, "draft body");
    window.localStorage.setItem(
      `${STORAGE_KEY_PREFIX}sess-old`,
      JSON.stringify({ savedAt: now - 86_400_000, state: emptyAcpState() }),
    );

    const removed = evictOldestPersistedAcpState(`${STORAGE_KEY_PREFIX}sess-current`);
    expect(removed).toBe(true);
    expect(window.localStorage.getItem(`${STORAGE_KEY_PREFIX}sess-old`)).toBeNull();
    expect(window.localStorage.getItem(`${DRAFT_KEY_PREFIX}sess-old`)).toBe("draft body");
  });

  it("never evicts unrelated keys (e.g. theme cache, settings)", () => {
    const now = Date.now();
    window.localStorage.setItem("aoe-resolved-theme", "themedata");
    window.localStorage.setItem("aoe-web-settings", "{}");
    window.localStorage.setItem(
      `${STORAGE_KEY_PREFIX}sess-old`,
      JSON.stringify({ savedAt: now - 86_400_000, state: emptyAcpState() }),
    );

    evictOldestPersistedAcpState(`${STORAGE_KEY_PREFIX}sess-current`);

    expect(window.localStorage.getItem("aoe-resolved-theme")).toBe("themedata");
    expect(window.localStorage.getItem("aoe-web-settings")).toBe("{}");
    expect(window.localStorage.getItem(`${STORAGE_KEY_PREFIX}sess-old`)).toBeNull();
  });

  it("prefers corrupt acp-state entries over older valid ones", () => {
    const now = Date.now();
    window.localStorage.setItem(
      `${STORAGE_KEY_PREFIX}sess-valid-old`,
      JSON.stringify({ savedAt: now - 86_400_000, state: emptyAcpState() }),
    );
    window.localStorage.setItem(`${STORAGE_KEY_PREFIX}sess-corrupt`, "not valid json{{{");

    evictOldestPersistedAcpState(`${STORAGE_KEY_PREFIX}sess-current`);

    expect(window.localStorage.getItem(`${STORAGE_KEY_PREFIX}sess-corrupt`)).toBeNull();
    expect(window.localStorage.getItem(`${STORAGE_KEY_PREFIX}sess-valid-old`)).not.toBeNull();
  });

  it("does not evict the current session's key (the one being written)", () => {
    const now = Date.now();
    window.localStorage.setItem(
      `${STORAGE_KEY_PREFIX}sess-current`,
      JSON.stringify({ savedAt: now - 86_400_000, state: emptyAcpState() }),
    );

    const removed = evictOldestPersistedAcpState(`${STORAGE_KEY_PREFIX}sess-current`);
    expect(removed).toBe(false);
    expect(window.localStorage.getItem(`${STORAGE_KEY_PREFIX}sess-current`)).not.toBeNull();
  });

  it("retry depth is exactly 1: second failure stays silent and does not loop", () => {
    const now = Date.now();
    window.localStorage.setItem(
      `${STORAGE_KEY_PREFIX}sess-old`,
      JSON.stringify({ savedAt: now - 86_400_000, state: emptyAcpState() }),
    );

    const setItem = vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw quotaError();
    });

    expect(() => persistState("sess-current", emptyAcpState())).not.toThrow();
    expect(setItem).toHaveBeenCalledTimes(2);
  });

  it("returns false silently when no acp-state candidate exists", () => {
    window.localStorage.setItem(`${DRAFT_KEY_PREFIX}sess-a`, "draft");
    window.localStorage.setItem("aoe-resolved-theme", "{}");

    const removed = evictOldestPersistedAcpState(`${STORAGE_KEY_PREFIX}sess-current`);
    expect(removed).toBe(false);
    expect(window.localStorage.getItem(`${DRAFT_KEY_PREFIX}sess-a`)).toBe("draft");
  });
});

describe("persistState strips attachment-bearing queued rows (#1833)", () => {
  it("drops queued rows that carry attachments and writes no base64 bytes", () => {
    const state = {
      ...emptyAcpState(),
      queuedPrompts: [
        { id: "q1", text: "plain text", queuedAt: "2026-01-01T00:00:00.000Z" },
        {
          id: "q2",
          text: "with image",
          queuedAt: "2026-01-01T00:00:01.000Z",
          attachments: [
            {
              kind: "image" as const,
              mimeType: "image/png",
              dataB64: "QUJDREVG",
              name: "shot.png",
            },
          ],
        },
      ],
    };

    persistState("sess-strip", state);

    const raw = window.localStorage.getItem(`${STORAGE_KEY_PREFIX}sess-strip`);
    expect(raw).not.toBeNull();
    expect(raw).not.toContain("QUJDREVG");

    const parsed = JSON.parse(raw!) as {
      state: { queuedPrompts: Array<{ id: string; attachments?: unknown }> };
    };
    expect(parsed.state.queuedPrompts).toHaveLength(1);
    expect(parsed.state.queuedPrompts[0]?.id).toBe("q1");
  });

  it("leaves a fully text-only queue untouched (no needless clone)", () => {
    const state = {
      ...emptyAcpState(),
      queuedPrompts: [{ id: "q1", text: "a", queuedAt: "2026-01-01T00:00:00.000Z" }],
    };
    persistState("sess-textonly", state);
    const raw = window.localStorage.getItem(`${STORAGE_KEY_PREFIX}sess-textonly`);
    const parsed = JSON.parse(raw!) as {
      state: { queuedPrompts: Array<{ id: string }> };
    };
    expect(parsed.state.queuedPrompts).toHaveLength(1);
    expect(parsed.state.queuedPrompts[0]?.id).toBe("q1");
  });
});
