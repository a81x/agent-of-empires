// @vitest-environment jsdom

import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ACP_MAX_RETRIES_EXPORT, ACP_WS_STALE_MS, acpRetryDelayMs, useAcpSession } from "./useAcpSession";

describe("acpRetryDelayMs", () => {
  it("returns 1s for the first attempt", () => {
    expect(acpRetryDelayMs(1)).toBe(1000);
  });

  it("doubles for each attempt up to the 30s cap", () => {
    expect(acpRetryDelayMs(2)).toBe(2000);
    expect(acpRetryDelayMs(3)).toBe(4000);
    expect(acpRetryDelayMs(4)).toBe(8000);
    expect(acpRetryDelayMs(5)).toBe(16000);
    expect(acpRetryDelayMs(6)).toBe(30000);
    expect(acpRetryDelayMs(7)).toBe(30000);
    expect(acpRetryDelayMs(100)).toBe(30000);
  });

  it("clamps non-positive inputs to the 1s base", () => {
    expect(acpRetryDelayMs(0)).toBe(1000);
    expect(acpRetryDelayMs(-5)).toBe(1000);
  });
});

interface FakeSocket {
  url: string;
  protocols: string[] | string | undefined;
  readyState: number;
  onopen: ((ev: Event) => void) | null;
  onclose: ((ev: CloseEvent) => void) | null;
  onerror: ((ev: Event) => void) | null;
  onmessage: ((ev: MessageEvent) => void) | null;
  close: () => void;
  send: (data: string | ArrayBufferLike | Blob | ArrayBufferView) => void;
}

const sockets: FakeSocket[] = [];
let originalWebSocket: typeof WebSocket;

class FakeWebSocket implements FakeSocket {
  url: string;
  protocols: string[] | string | undefined;
  readyState: number = 0;
  onopen: ((ev: Event) => void) | null = null;
  onclose: ((ev: CloseEvent) => void) | null = null;
  onerror: ((ev: Event) => void) | null = null;
  onmessage: ((ev: MessageEvent) => void) | null = null;
  static CONNECTING = 0;
  static OPEN = 1;
  static CLOSING = 2;
  static CLOSED = 3;
  constructor(url: string, protocols?: string | string[]) {
    this.url = url;
    this.protocols = protocols;
    sockets.push(this);
  }
  close(): void {
    this.readyState = FakeWebSocket.CLOSED;
    if (this.onclose) {
      this.onclose({
        code: 1000,
        reason: "test close",
        wasClean: true,
      } as CloseEvent);
    }
  }
  send(): void {}
}

beforeEach(() => {
  vi.useFakeTimers();
  sockets.length = 0;
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      const url = typeof input === "string" ? input : input.toString();
      if (url.includes("/api/login/status")) {
        return new Response(
          JSON.stringify({
            required: false,
            authenticated: true,
            elevated: true,
            elevated_until_secs: null,
          }),
          { status: 200 },
        );
      }
      return new Response(JSON.stringify({ frames: [], lost: false, highest_seq: 0 }), { status: 200 });
    }),
  );
  originalWebSocket = global.WebSocket;
  global.WebSocket = FakeWebSocket as unknown as typeof WebSocket;
});

afterEach(() => {
  global.WebSocket = originalWebSocket;
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

async function flushAsync(): Promise<void> {
  await act(async () => {
    for (let i = 0; i < 6; i++) {
      await Promise.resolve();
    }
  });
}

describe("useAcpSession reconnect (#1130)", () => {
  it("schedules a backoff retry on WS close and exposes the countdown", async () => {
    const { result } = renderHook(() => useAcpSession("sess-1"));
    await flushAsync();
    expect(sockets).toHaveLength(1);
    const first = sockets[0]!;

    act(() => {
      first.readyState = FakeWebSocket.CLOSED;
      first.onclose?.({
        code: 1006,
        reason: "",
        wasClean: false,
      } as CloseEvent);
    });
    expect(result.current.reconnecting).toBe(true);
    expect(result.current.retryCount).toBe(1);
    expect(result.current.retryCountdown).toBeGreaterThanOrEqual(1);

    await act(async () => {
      await vi.advanceTimersByTimeAsync(acpRetryDelayMs(1));
    });
    await flushAsync();
    expect(sockets).toHaveLength(2);
  });

  it("stops retrying after MAX_RETRIES and exposes manualReconnect", async () => {
    const { result } = renderHook(() => useAcpSession("sess-2"));
    await flushAsync();

    for (let attempt = 1; attempt <= ACP_MAX_RETRIES_EXPORT; attempt++) {
      const sock = sockets[sockets.length - 1]!;
      act(() => {
        sock.readyState = FakeWebSocket.CLOSED;
        sock.onclose?.({
          code: 1006,
          reason: "",
          wasClean: false,
        } as CloseEvent);
      });
      if (attempt < ACP_MAX_RETRIES_EXPORT) {
        await act(async () => {
          await vi.advanceTimersByTimeAsync(acpRetryDelayMs(attempt));
        });
        await flushAsync();
      }
    }
    const lastSock = sockets[sockets.length - 1]!;
    act(() => {
      lastSock.readyState = FakeWebSocket.CLOSED;
      lastSock.onclose?.({
        code: 1006,
        reason: "",
        wasClean: false,
      } as CloseEvent);
    });
    expect(result.current.reconnecting).toBe(false);
    expect(result.current.retryCount).toBe(ACP_MAX_RETRIES_EXPORT);
    expect(typeof result.current.manualReconnect).toBe("function");

    const socketsBefore = sockets.length;
    act(() => {
      result.current.manualReconnect();
    });
    await flushAsync();
    expect(sockets.length).toBe(socketsBefore + 1);
    expect(result.current.retryCount).toBe(0);
  });

  it("does not call /api/login/status before dialing (structured view WS is not elevation-gated)", async () => {
    const fetchSpy = vi.fn(async (input: RequestInfo | URL) => {
      const url = typeof input === "string" ? input : input.toString();
      if (url.includes("/api/login/status")) {
        throw new Error(
          "structured view dial should not preflight /api/login/status; the WS is no longer elevation-gated",
        );
      }
      return new Response(JSON.stringify({ frames: [], lost: false, highest_seq: 0 }), { status: 200 });
    });
    vi.stubGlobal("fetch", fetchSpy);

    renderHook(() => useAcpSession("sess-no-preflight"));
    await flushAsync();
    expect(sockets).toHaveLength(1);
    for (const call of fetchSpy.mock.calls) {
      const url = String(call[0]);
      expect(url).not.toContain("/api/login/status");
    }
  });

  it("resets the retry counter once the socket opens successfully", async () => {
    const { result } = renderHook(() => useAcpSession("sess-3"));
    await flushAsync();
    const first = sockets[0]!;
    act(() => {
      first.readyState = FakeWebSocket.CLOSED;
      first.onclose?.({
        code: 1006,
        reason: "",
        wasClean: false,
      } as CloseEvent);
    });
    expect(result.current.retryCount).toBe(1);

    await act(async () => {
      await vi.advanceTimersByTimeAsync(acpRetryDelayMs(1));
    });
    await flushAsync();
    const second = sockets[1]!;
    act(() => {
      second.readyState = FakeWebSocket.OPEN;
      second.onopen?.({} as Event);
    });
    expect(result.current.retryCount).toBe(0);
    expect(result.current.reconnecting).toBe(false);
  });
});

describe("useAcpSession liveness watchdog (#2287)", () => {
  function heartbeat(sock: FakeSocket): void {
    sock.onmessage?.({ data: JSON.stringify({ kind: "heartbeat" }) } as MessageEvent);
  }

  it("force-redials a stale OPEN socket the browser still reports as alive (zombie)", async () => {
    renderHook(() => useAcpSession("sess-zombie"));
    await flushAsync();
    expect(sockets).toHaveLength(1);
    const first = sockets[0]!;
    act(() => {
      first.readyState = FakeWebSocket.OPEN;
      first.onopen?.({} as Event);
    });

    await act(async () => {
      await vi.advanceTimersByTimeAsync(ACP_WS_STALE_MS + 15000);
    });
    await flushAsync();

    expect(first.readyState).toBe(FakeWebSocket.CLOSED);
    expect(sockets.length).toBeGreaterThanOrEqual(2);
  });

  it("does not reconnect while heartbeats keep the socket fresh", async () => {
    renderHook(() => useAcpSession("sess-alive"));
    await flushAsync();
    expect(sockets).toHaveLength(1);
    const first = sockets[0]!;
    act(() => {
      first.readyState = FakeWebSocket.OPEN;
      first.onopen?.({} as Event);
    });

    for (let i = 0; i < 4; i++) {
      await act(async () => {
        await vi.advanceTimersByTimeAsync(30000);
      });
      act(() => heartbeat(first));
    }
    await flushAsync();

    expect(sockets).toHaveLength(1);
    expect(first.readyState).toBe(FakeWebSocket.OPEN);
  });

  it("watchdog never dials a socket that is not OPEN (no resurrection)", async () => {
    renderHook(() => useAcpSession("sess-connecting"));
    await flushAsync();
    expect(sockets).toHaveLength(1);
    expect(sockets[0]!.readyState).toBe(FakeWebSocket.CONNECTING);

    await act(async () => {
      await vi.advanceTimersByTimeAsync(ACP_WS_STALE_MS + 30000);
    });
    await flushAsync();
    expect(sockets).toHaveLength(1);
  });
});
