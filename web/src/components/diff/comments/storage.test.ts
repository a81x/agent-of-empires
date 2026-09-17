import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { clearStoredComments, EMPTY_STORAGE, isEmptyState, loadComments, saveComments, storageKey } from "./storage";
import type { DiffComment, DiffCommentsStorageV1 } from "./types";

// The default node env has no localStorage.
let data: Map<string, string>;
beforeEach(() => {
  data = new Map();
  (globalThis as { localStorage: Storage }).localStorage = {
    get length() {
      return data.size;
    },
    key: (i) => Array.from(data.keys())[i] ?? null,
    getItem: (k) => data.get(k) ?? null,
    setItem: (k, v) => void data.set(k, String(v)),
    removeItem: (k) => void data.delete(k),
    clear: () => data.clear(),
  };
});
afterEach(() => {
  vi.restoreAllMocks();
});

function mkComment(overrides: Partial<DiffComment> = {}): DiffComment {
  return {
    id: "c1",
    filePath: "src/foo.rs",
    side: "new",
    startLine: 5,
    endLine: 5,
    body: "review",
    capturedSnippet: "snippet",
    createdAt: "2025-01-01T00:00:00Z",
    ...overrides,
  };
}

const withComment = (id = "c1") => ({ ...EMPTY_STORAGE, comments: [mkComment({ id })] });
const stored = (value: unknown) => localStorage.setItem(storageKey("sess-1"), JSON.stringify(value));

describe("loadComments", () => {
  it("round-trips per session under a versioned key", () => {
    const original: DiffCommentsStorageV1 = {
      version: 1,
      comments: [mkComment({ id: "a" }), mkComment({ id: "b" })],
      clearAfterSend: false,
      introDraft: "hi",
      outroDraft: "bye",
    };
    saveComments("sess-1", original);
    saveComments("sess-2", withComment("z"));
    expect(storageKey("abc")).toBe("aoe:diff-comments:v1:abc");
    expect(loadComments("sess-1")).toEqual(original);
    expect(loadComments("sess-2").comments.map((c) => c.id)).toEqual(["z"]);
  });

  it.each([
    ["an absent key", undefined],
    ["corrupt JSON", "not json"],
    ["an unknown version", JSON.stringify({ ...withComment(), version: 99 })],
  ])("returns the empty envelope for %s", (_, raw) => {
    if (raw !== undefined) localStorage.setItem(storageKey("sess-1"), raw);
    expect(loadComments("sess-1")).toEqual(EMPTY_STORAGE);
  });

  it("drops malformed comments and defaults missing fields", () => {
    stored({
      version: 1,
      comments: [
        mkComment({ id: "good" }),
        { ...mkComment({ id: "bad" }), filePath: 12 },
        { ...mkComment(), side: "left" },
      ],
    });
    expect(loadComments("sess-1")).toEqual({ ...EMPTY_STORAGE, comments: [mkComment({ id: "good" })] });
  });
});

describe("saveComments", () => {
  it("survives a throwing write", () => {
    const spy = vi.spyOn(localStorage, "setItem").mockImplementation(() => {
      throw new Error("QuotaExceeded");
    });
    expect(() => saveComments("sess-1", withComment())).not.toThrow();
    expect(spy).toHaveBeenCalled();
  });

  it("removes the key when state becomes empty, including a lone clearAfterSend toggle", () => {
    saveComments("sess-1", withComment());
    expect(data.has(storageKey("sess-1"))).toBe(true);
    saveComments("sess-1", { ...EMPTY_STORAGE, clearAfterSend: false });
    expect(data.has(storageKey("sess-1"))).toBe(false);
  });

  it.each<[Partial<DiffCommentsStorageV1>, boolean]>([
    [{}, true],
    [{ clearAfterSend: false }, true],
    [{ comments: [mkComment()] }, false],
    [{ introDraft: "hi" }, false],
    [{ outroDraft: "bye" }, false],
  ])("isEmptyState(%j) is %s", (over, empty) => {
    expect(isEmptyState({ ...EMPTY_STORAGE, ...over })).toBe(empty);
  });
});

it("clearStoredComments removes only that session", () => {
  saveComments("sess-1", withComment());
  saveComments("sess-2", withComment());
  clearStoredComments("sess-1");
  clearStoredComments("absent");
  expect([...data.keys()]).toEqual([storageKey("sess-2")]);
});
