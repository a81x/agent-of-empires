// @vitest-environment jsdom

import { afterEach, describe, expect, it } from "vitest";
import { renderHook } from "@testing-library/react";
import { useRef } from "react";

import { useFocusTerminalTarget } from "../useFocusTerminalTarget";
import { consumePendingTerminalFocus, dispatchFocusTerminal, setPendingTerminalFocus } from "../../lib/terminalFocus";

afterEach(() => {
  consumePendingTerminalFocus("composer");
  consumePendingTerminalFocus("agent");
});

function renderWithElement(target: "composer" | "agent", el: HTMLElement | null) {
  return renderHook(() => {
    const ref = useRef<HTMLElement | null>(el);
    useFocusTerminalTarget(target, ref);
    return ref;
  });
}

describe("useFocusTerminalTarget", () => {
  it("focuses the ref when a matching focus event is dispatched", () => {
    const el = document.createElement("textarea");
    document.body.appendChild(el);
    try {
      renderWithElement("composer", el);
      expect(document.activeElement).not.toBe(el);
      dispatchFocusTerminal("composer");
      expect(document.activeElement).toBe(el);
    } finally {
      el.remove();
    }
  });

  it("ignores focus events for other targets", () => {
    const el = document.createElement("textarea");
    document.body.appendChild(el);
    try {
      renderWithElement("composer", el);
      dispatchFocusTerminal("agent");
      expect(document.activeElement).not.toBe(el);
    } finally {
      el.remove();
    }
  });

  it("ignores a focus event with no detail", () => {
    const el = document.createElement("textarea");
    document.body.appendChild(el);
    try {
      renderWithElement("composer", el);
      window.dispatchEvent(new CustomEvent("aoe:focus-terminal"));
      expect(document.activeElement).not.toBe(el);
    } finally {
      el.remove();
    }
  });

  it("consuming a latch with no element present is a no-op", () => {
    setPendingTerminalFocus("composer");
    renderWithElement("composer", null);
    expect(consumePendingTerminalFocus("composer")).toBe(false);
  });

  it("stashes the latch when the element is not present at event time", () => {
    renderWithElement("composer", null);
    dispatchFocusTerminal("composer");
    expect(consumePendingTerminalFocus("composer")).toBe(true);
  });

  it("consumes a pending latch on mount", () => {
    const el = document.createElement("textarea");
    document.body.appendChild(el);
    try {
      setPendingTerminalFocus("composer");
      renderWithElement("composer", el);
      expect(document.activeElement).toBe(el);
      expect(consumePendingTerminalFocus("composer")).toBe(false);
    } finally {
      el.remove();
    }
  });

  it("removes its listener on unmount", () => {
    const el = document.createElement("textarea");
    document.body.appendChild(el);
    try {
      const { unmount } = renderWithElement("composer", el);
      unmount();
      el.blur();
      dispatchFocusTerminal("composer");
      expect(document.activeElement).not.toBe(el);
    } finally {
      el.remove();
    }
  });
});
