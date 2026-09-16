// @vitest-environment jsdom

import { renderHook, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { useSessions } from "./useSessions";
import * as api from "../lib/api";

describe("useSessions / loaded sentinel", () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("starts loaded=false before the first fetch resolves", () => {
    let resolveFetch: (value: api.SessionsEnvelope | null) => void = () => {};
    vi.spyOn(api, "fetchSessions").mockImplementation(() => new Promise((r) => (resolveFetch = r)));

    const { result } = renderHook(() => useSessions());

    expect(result.current.loaded).toBe(false);
    expect(result.current.sessions).toEqual([]);
    resolveFetch(null);
  });

  it("flips loaded=true after the first successful fetch", async () => {
    vi.spyOn(api, "fetchSessions").mockResolvedValue({
      sessions: [],
      workspace_ordering: [],
    });

    const { result } = renderHook(() => useSessions());

    await waitFor(() => expect(result.current.loaded).toBe(true));
    expect(result.current.error).toBe(false);
  });

  it("flips loaded=true even when the first fetch returns null", async () => {
    vi.spyOn(api, "fetchSessions").mockResolvedValue(null);

    const { result } = renderHook(() => useSessions());

    await waitFor(() => expect(result.current.loaded).toBe(true));
    expect(result.current.error).toBe(true);
  });
});
