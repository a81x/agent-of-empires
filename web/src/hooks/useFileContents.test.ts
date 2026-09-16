// @vitest-environment jsdom

import { act, renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { useFileContents, __resetFileContentsCache } from "./useFileContents";
import * as api from "../lib/api";
import type { RichFileContentsResponse } from "../lib/types";

function makeContents(
  path: string,
  body: string,
  status: RichFileContentsResponse["file"]["status"] = "modified",
): RichFileContentsResponse {
  return {
    file: { path, old_path: null, status, additions: 1, deletions: 0 },
    old_content: "",
    new_content: body,
    patch: `@@ -0,0 +1 @@\n+${body}`,
    is_binary: false,
    truncated: false,
  };
}

describe("useFileContents", () => {
  beforeEach(() => {
    __resetFileContentsCache();
  });
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("fetches on first open, then serves a revisit from cache without re-fetching", async () => {
    const spy = vi.spyOn(api, "getSessionFileContents").mockResolvedValue(makeContents("a.ts", "alpha"));

    const { result, rerender } = renderHook(({ path }) => useFileContents("s1", path, undefined), {
      initialProps: { path: "a.ts" },
    });

    await waitFor(() => expect(result.current.contents).not.toBeNull());
    expect(result.current.contents?.new_content).toBe("alpha");
    expect(spy).toHaveBeenCalledTimes(1);

    spy.mockResolvedValueOnce(makeContents("b.ts", "beta"));
    rerender({ path: "b.ts" });
    await waitFor(() => expect(result.current.contents?.new_content).toBe("beta"));
    expect(spy).toHaveBeenCalledTimes(2);

    rerender({ path: "a.ts" });
    expect(result.current.contents?.new_content).toBe("alpha");
    expect(result.current.loading).toBe(false);
    expect(spy).toHaveBeenCalledTimes(2);
  });

  it("keeps stale contents under a loading flag while switching to an uncached file", async () => {
    let resolve: (v: RichFileContentsResponse | null) => void = () => {};
    const spy = vi.spyOn(api, "getSessionFileContents");
    spy.mockResolvedValueOnce(makeContents("a.ts", "alpha"));

    const { result, rerender } = renderHook(({ path }) => useFileContents("s1", path, undefined), {
      initialProps: { path: "a.ts" },
    });
    await waitFor(() => expect(result.current.contents?.new_content).toBe("alpha"));

    spy.mockImplementationOnce(() => new Promise((r) => (resolve = r)));
    rerender({ path: "b.ts" });
    expect(result.current.contents?.new_content).toBe("alpha");
    expect(result.current.loading).toBe(true);

    await waitFor(() => expect(spy).toHaveBeenCalledTimes(2));
    resolve(makeContents("b.ts", "beta"));
    await waitFor(() => expect(result.current.contents?.new_content).toBe("beta"));
    expect(result.current.loading).toBe(false);
  });

  it("invalidates the cache when the revision is bumped", async () => {
    const spy = vi.spyOn(api, "getSessionFileContents");
    spy.mockResolvedValueOnce(makeContents("a.ts", "v1"));

    const { result, rerender } = renderHook(({ rev }) => useFileContents("s1", "a.ts", undefined, rev), {
      initialProps: { rev: 1 },
    });
    await waitFor(() => expect(result.current.contents?.new_content).toBe("v1"));
    expect(spy).toHaveBeenCalledTimes(1);

    spy.mockResolvedValueOnce(makeContents("a.ts", "v2"));
    rerender({ rev: 2 });
    await waitFor(() => expect(result.current.contents?.new_content).toBe("v2"));
    expect(spy).toHaveBeenCalledTimes(2);
  });

  it("evicts the oldest entry once the byte budget is exceeded", async () => {
    const big = "x".repeat(33 * 1024 * 1024);
    const spy = vi.spyOn(api, "getSessionFileContents");
    spy.mockResolvedValueOnce(makeContents("a.ts", "alpha"));

    const { result, rerender } = renderHook(({ path }) => useFileContents("s1", path, undefined), {
      initialProps: { path: "a.ts" },
    });
    await waitFor(() => expect(result.current.contents?.new_content).toBe("alpha"));

    spy.mockResolvedValueOnce(makeContents("big.ts", big));
    rerender({ path: "big.ts" });
    await waitFor(() => expect(result.current.contents?.new_content).toBe(big));

    spy.mockResolvedValueOnce(makeContents("a.ts", "alpha2"));
    rerender({ path: "a.ts" });
    await waitFor(() => expect(result.current.contents?.new_content).toBe("alpha2"));
    expect(spy).toHaveBeenCalledTimes(3);
  });

  it("refresh() forces a re-fetch even when the file is cached", async () => {
    const spy = vi.spyOn(api, "getSessionFileContents").mockResolvedValue(makeContents("a.ts", "alpha"));

    const { result } = renderHook(() => useFileContents("s1", "a.ts", undefined));
    await waitFor(() => expect(result.current.contents?.new_content).toBe("alpha"));
    expect(spy).toHaveBeenCalledTimes(1);

    spy.mockResolvedValueOnce(makeContents("a.ts", "alpha-refreshed"));
    await act(async () => {
      result.current.refresh();
    });
    await waitFor(() => expect(result.current.contents?.new_content).toBe("alpha-refreshed"));
    expect(spy).toHaveBeenCalledTimes(2);
  });

  it("surfaces an error when the fetch returns no contents", async () => {
    vi.spyOn(api, "getSessionFileContents").mockResolvedValue(null);

    const { result } = renderHook(() => useFileContents("s1", "a.ts", undefined));
    await waitFor(() => expect(result.current.error).toBe("Failed to load file contents"));
    expect(result.current.contents).toBeNull();
    expect(result.current.loading).toBe(false);
  });

  it("does not fetch and clears state while filePath is null", async () => {
    const spy = vi.spyOn(api, "getSessionFileContents").mockResolvedValue(makeContents("a.ts", "alpha"));

    const { result, rerender } = renderHook(
      ({ path }: { path: string | null }) => useFileContents("s1", path, undefined),
      {
        initialProps: { path: null as string | null },
      },
    );

    await act(async () => {
      await new Promise((r) => setTimeout(r, 5));
    });
    expect(spy).not.toHaveBeenCalled();
    expect(result.current.contents).toBeNull();
    expect(result.current.loading).toBe(false);

    rerender({ path: "a.ts" });
    await waitFor(() => expect(result.current.contents?.new_content).toBe("alpha"));
    expect(spy).toHaveBeenCalledTimes(1);
  });

  it("drops a superseded in-flight response when switching files rapidly", async () => {
    let resolveA: (v: RichFileContentsResponse | null) => void = () => {};
    const spy = vi.spyOn(api, "getSessionFileContents");
    spy.mockImplementationOnce(() => new Promise((r) => (resolveA = r)));

    const { result, rerender } = renderHook(({ path }) => useFileContents("s1", path, undefined), {
      initialProps: { path: "a.ts" },
    });
    await waitFor(() => expect(spy).toHaveBeenCalledTimes(1));

    spy.mockResolvedValueOnce(makeContents("b.ts", "beta"));
    rerender({ path: "b.ts" });
    await waitFor(() => expect(result.current.contents?.new_content).toBe("beta"));

    await act(async () => {
      resolveA(makeContents("a.ts", "alpha-late"));
      await new Promise((r) => setTimeout(r, 5));
    });
    expect(result.current.contents?.new_content).toBe("beta");
  });
});
