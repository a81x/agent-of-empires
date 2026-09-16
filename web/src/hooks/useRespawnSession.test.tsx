// @vitest-environment jsdom

import { act, cleanup, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { useRespawnSession } from "./useRespawnSession";

beforeEach(() => {
  vi.stubGlobal(
    "fetch",
    vi.fn().mockResolvedValue({
      ok: true,
      status: 200,
      text: () => Promise.resolve(""),
    }),
  );
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe("useRespawnSession resetKey", () => {
  it("starts fresh for a second incident after a successful respawn", async () => {
    const { result, rerender } = renderHook(({ resetKey }: { resetKey: string }) => useRespawnSession("s1", resetKey), {
      initialProps: { resetKey: "reset-1" },
    });

    await act(async () => {
      await result.current.respawn();
    });
    expect(result.current.state).toBe("ok");

    rerender({ resetKey: "reset-2" });

    expect(result.current.state).toBe("idle");
    expect(result.current.error).toBeNull();
  });

  it("starts fresh for a second incident after a failed respawn", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue({
        ok: false,
        status: 500,
        text: () => Promise.resolve("boom"),
      }),
    );
    const { result, rerender } = renderHook(
      ({ resetKey }: { resetKey: string | null }) => useRespawnSession("s1", resetKey),
      {
        initialProps: { resetKey: "reset-1" },
      },
    );

    await act(async () => {
      await result.current.respawn();
    });
    expect(result.current.state).toBe("failed");
    expect(result.current.error).toContain("boom");

    rerender({ resetKey: null });
    expect(result.current.state).toBe("idle");
    expect(result.current.error).toBeNull();

    rerender({ resetKey: "reset-2" });
    expect(result.current.state).toBe("idle");
    expect(result.current.error).toBeNull();
  });

  it("starts fresh for a second incident that reuses the same key", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue({
        ok: false,
        status: 409,
        text: () => Promise.resolve("worker already running"),
      }),
    );
    const { result, rerender } = renderHook(
      ({ resetKey }: { resetKey: string | null }) => useRespawnSession("s1", resetKey),
      {
        initialProps: { resetKey: "unknown" as string | null },
      },
    );

    await act(async () => {
      await result.current.respawn();
    });
    expect(result.current.state).toBe("failed");

    rerender({ resetKey: null });
    rerender({ resetKey: "unknown" });

    expect(result.current.state).toBe("idle");
    expect(result.current.error).toBeNull();
  });

  it.each([
    ["a null in between", [null, "unknown"] as (string | null)[]],
    ["another reset in between", ["2099-01-01T09:30:00Z", "unknown"] as (string | null)[]],
  ])("ignores a request that completed after its incident ended, %s", async (_label, keysAfter) => {
    let release: (() => void) | undefined;
    vi.stubGlobal(
      "fetch",
      vi.fn().mockImplementation(
        () =>
          new Promise((resolve) => {
            release = () => resolve({ ok: true, status: 200, text: () => Promise.resolve("") });
          }),
      ),
    );
    const { result, rerender } = renderHook(
      ({ resetKey }: { resetKey: string | null }) => useRespawnSession("s1", resetKey),
      {
        initialProps: { resetKey: "unknown" as string | null },
      },
    );

    let pending: Promise<boolean> | undefined;
    await act(async () => {
      pending = result.current.respawn();
    });
    expect(result.current.state).toBe("retrying");

    for (const resetKey of keysAfter) rerender({ resetKey });
    expect(result.current.state).toBe("idle");

    await act(async () => {
      release?.();
      await pending;
    });

    expect(result.current.state).toBe("idle");
    expect(result.current.error).toBeNull();
  });
});
