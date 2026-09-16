// @vitest-environment jsdom

import { describe, expect, it, vi } from "vitest";
import { renderHook } from "@testing-library/react";
import { useKeyboardShortcuts } from "./useKeyboardShortcuts";

function dispatch(target: EventTarget, init: KeyboardEventInit) {
  const event = new KeyboardEvent("keydown", {
    bubbles: true,
    cancelable: true,
    ...init,
  });
  target.dispatchEvent(event);
  return event;
}

function makeActions() {
  return {
    onNew: vi.fn(),
    onNewScratch: vi.fn(),
    onDiff: vi.fn(),
    onEscape: vi.fn(),
    onHelp: vi.fn(),
    onSettings: vi.fn(),
    onPalette: vi.fn(),
    onToggleSidebar: vi.fn(),
    onToggleRightPanel: vi.fn(),
    onToggleTerminalFocus: vi.fn(),
  };
}

describe("useKeyboardShortcuts", () => {
  it("fires onPalette for Ctrl+K dispatched on a nested target", () => {
    const actions = makeActions();
    renderHook(() => useKeyboardShortcuts(() => actions));

    dispatch(document.body, { key: "k", ctrlKey: true });

    expect(actions.onPalette).toHaveBeenCalledTimes(1);
  });

  it("still fires when a child element calls stopPropagation in bubble phase", () => {
    const actions = makeActions();
    renderHook(() => useKeyboardShortcuts(() => actions));

    const child = document.createElement("textarea");
    document.body.appendChild(child);
    child.addEventListener("keydown", (e) => e.stopPropagation());

    dispatch(child, { key: "k", ctrlKey: true });

    expect(actions.onPalette).toHaveBeenCalledTimes(1);
    child.remove();
  });

  it("routes Ctrl+Alt+B (KeyB) to onToggleRightPanel", () => {
    const actions = makeActions();
    renderHook(() => useKeyboardShortcuts(() => actions));

    dispatch(document.body, {
      key: "b",
      code: "KeyB",
      ctrlKey: true,
      altKey: true,
    });

    expect(actions.onToggleRightPanel).toHaveBeenCalledTimes(1);
    expect(actions.onToggleSidebar).not.toHaveBeenCalled();
  });

  it("routes Cmd/Ctrl+Shift+N to onNewScratch (fast-create shortcut)", () => {
    const actions = makeActions();
    renderHook(() => useKeyboardShortcuts(() => actions));

    dispatch(document.body, {
      key: "N",
      code: "KeyN",
      ctrlKey: true,
      shiftKey: true,
    });

    expect(actions.onNewScratch).toHaveBeenCalledTimes(1);
    expect(actions.onNew).not.toHaveBeenCalled();
  });

  it("does NOT fire onNewScratch for plain Shift+N (no modifier)", () => {
    const actions = makeActions();
    renderHook(() => useKeyboardShortcuts(() => actions));

    dispatch(document.body, {
      key: "N",
      code: "KeyN",
      shiftKey: true,
    });

    expect(actions.onNewScratch).not.toHaveBeenCalled();
  });

  it("detaches the listener on unmount", () => {
    const actions = makeActions();
    const { unmount } = renderHook(() => useKeyboardShortcuts(() => actions));

    unmount();
    dispatch(document.body, { key: "k", ctrlKey: true });

    expect(actions.onPalette).not.toHaveBeenCalled();
  });
});
