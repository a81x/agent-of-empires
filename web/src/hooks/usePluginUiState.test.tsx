// @vitest-environment jsdom

import { renderHook, act } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { PluginUiState } from "../lib/api";
import { fetchPluginUiState } from "../lib/api";
import { reportError, reportInfo, reportOpenLink } from "../lib/toastBus";
import { usePluginUiState } from "./usePluginUiState";

vi.mock("../lib/api", () => ({ fetchPluginUiState: vi.fn() }));
vi.mock("../lib/toastBus", () => ({
  reportError: vi.fn(),
  reportInfo: vi.fn(),
  reportOpenLink: vi.fn(),
}));

const fetchMock = vi.mocked(fetchPluginUiState);

function snapshot(notifications: PluginUiState["notifications"]): PluginUiState {
  return { entries: [], notifications };
}

beforeEach(() => {
  vi.useFakeTimers();
  fetchMock.mockReset();
  vi.mocked(reportError).mockReset();
  vi.mocked(reportInfo).mockReset();
  vi.mocked(reportOpenLink).mockReset();
});
afterEach(() => {
  vi.useRealTimers();
});

describe("usePluginUiState notifications", () => {
  it("adopts the first backlog silently, then toasts only newer seqs once", async () => {
    fetchMock
      .mockResolvedValueOnce(snapshot([{ seq: 1, plugin_id: "acme.kit", tone: "info", title: "old" }]))
      .mockResolvedValueOnce(
        snapshot([
          { seq: 1, plugin_id: "acme.kit", tone: "info", title: "old" },
          { seq: 2, plugin_id: "acme.kit", tone: "danger", title: "Build failed", body: "tests" },
        ]),
      )
      .mockResolvedValue(
        snapshot([
          { seq: 1, plugin_id: "acme.kit", tone: "info", title: "old" },
          { seq: 2, plugin_id: "acme.kit", tone: "danger", title: "Build failed", body: "tests" },
        ]),
      );

    renderHook(() => usePluginUiState());

    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(reportError).not.toHaveBeenCalled();
    expect(reportInfo).not.toHaveBeenCalled();

    await act(async () => {
      await vi.advanceTimersByTimeAsync(3000);
    });
    expect(reportError).toHaveBeenCalledTimes(1);
    expect(reportError).toHaveBeenCalledWith("Build failed: tests");

    await act(async () => {
      await vi.advanceTimersByTimeAsync(3000);
    });
    expect(reportError).toHaveBeenCalledTimes(1);
  });

  it("re-seeds when the ring resets (daemon restart) so new toasts fire", async () => {
    fetchMock
      .mockResolvedValueOnce(snapshot([{ seq: 5, plugin_id: "acme.kit", tone: "info", title: "old" }]))
      .mockResolvedValueOnce(snapshot([{ seq: 1, plugin_id: "acme.kit", tone: "info", title: "after restart" }]))
      .mockResolvedValue(
        snapshot([
          { seq: 1, plugin_id: "acme.kit", tone: "info", title: "after restart" },
          { seq: 2, plugin_id: "acme.kit", tone: "info", title: "fresh" },
        ]),
      );

    renderHook(() => usePluginUiState());

    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(reportInfo).not.toHaveBeenCalled();

    await act(async () => {
      await vi.advanceTimersByTimeAsync(3000);
    });
    expect(reportInfo).not.toHaveBeenCalled();

    await act(async () => {
      await vi.advanceTimersByTimeAsync(3000);
    });
    expect(reportInfo).toHaveBeenCalledTimes(1);
    expect(reportInfo).toHaveBeenCalledWith("fresh");
  });

  it("routes a notification carrying an href to the click-to-open toast", async () => {
    fetchMock
      .mockResolvedValueOnce(snapshot([{ seq: 1, plugin_id: "acme.kit", tone: "info", title: "seed" }]))
      .mockResolvedValue(
        snapshot([
          { seq: 1, plugin_id: "acme.kit", tone: "info", title: "seed" },
          {
            seq: 2,
            plugin_id: "acme.kit",
            tone: "info",
            title: "Open link",
            href: "https://example.com/pr/1",
          },
        ]),
      );

    renderHook(() => usePluginUiState());
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(3000);
    });
    expect(reportOpenLink).toHaveBeenCalledWith("Open link", "https://example.com/pr/1");
    expect(reportInfo).not.toHaveBeenCalled();
  });
});

describe("usePluginUiState refresh indicator", () => {
  it("does not flip isRefreshing for a poll that settles before the delay", async () => {
    fetchMock.mockResolvedValue(snapshot([]));
    const { result } = renderHook(() => usePluginUiState());

    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(result.current.isRefreshing).toBe(false);

    await act(async () => {
      await vi.advanceTimersByTimeAsync(300);
    });
    expect(result.current.isRefreshing).toBe(false);
  });

  it("flips isRefreshing on once a poll outlasts the delay, off when it settles", async () => {
    let resolve!: (v: PluginUiState | null) => void;
    fetchMock.mockReturnValueOnce(new Promise((r) => (resolve = r)));
    fetchMock.mockResolvedValue(snapshot([]));
    const { result } = renderHook(() => usePluginUiState());

    await act(async () => {
      await vi.advanceTimersByTimeAsync(100);
    });
    expect(result.current.isRefreshing).toBe(false);

    await act(async () => {
      await vi.advanceTimersByTimeAsync(200);
    });
    expect(result.current.isRefreshing).toBe(true);

    await act(async () => {
      resolve(snapshot([]));
    });
    expect(result.current.isRefreshing).toBe(false);
  });

  it("clears isRefreshing even when a slow poll fails (returns null)", async () => {
    let resolve!: (v: PluginUiState | null) => void;
    fetchMock.mockReturnValueOnce(new Promise((r) => (resolve = r)));
    fetchMock.mockResolvedValue(snapshot([]));
    const { result } = renderHook(() => usePluginUiState());

    await act(async () => {
      await vi.advanceTimersByTimeAsync(300);
    });
    expect(result.current.isRefreshing).toBe(true);

    await act(async () => {
      resolve(null);
    });
    expect(result.current.isRefreshing).toBe(false);
  });
});
