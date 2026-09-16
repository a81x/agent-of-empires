import { describe, expect, it } from "vitest";

import type { ActivityRow } from "./acpTypes";
import {
  DEFAULT_HISTORY_WINDOW,
  historyWindow,
  historyWindowStart,
  initialHistoryWindow,
  lastUserBoundaryIndex,
} from "./acpHistoryWindow";

function row(kind: ActivityRow["kind"], i: number): ActivityRow {
  return { id: `${kind}-${i}`, kind, text: `${kind} ${i}` };
}

function transcript(turns: number, perTurn: number): ActivityRow[] {
  const rows: ActivityRow[] = [];
  for (let t = 0; t < turns; t += 1) {
    rows.push(row("user_prompt", t));
    for (let r = 0; r < perTurn; r += 1) rows.push(row("message", t * 100 + r));
  }
  return rows;
}

function toolRow(id: string, parentId?: string): ActivityRow {
  return {
    id,
    kind: "tool_start",
    text: id,
    tool: {
      id,
      name: id,
      kind: "other",
      args_preview: "{}",
      started_at: "",
      parent_tool_call_id: parentId,
    },
  };
}

function subagentTranscript(lead: number, children: number, parentChain: string[] = ["task1"]): ActivityRow[] {
  const rows: ActivityRow[] = [row("user_prompt", 0)];
  for (let i = 0; i < lead; i += 1) rows.push(row("tool_complete", i));
  for (let p = 0; p < parentChain.length; p += 1) {
    rows.push(toolRow(parentChain[p]!, p === 0 ? undefined : parentChain[p - 1]!));
  }
  const leafParent = parentChain[parentChain.length - 1]!;
  for (let c = 0; c < children; c += 1) rows.push(toolRow(`child-${c}`, leafParent));
  return rows;
}

describe("historyWindowStart", () => {
  it("returns 0 when everything fits", () => {
    const rows = transcript(2, 3); // 8 rows
    expect(historyWindowStart(rows, DEFAULT_HISTORY_WINDOW)).toBe(0);
    expect(historyWindowStart(rows, 8)).toBe(0);
  });

  it("snaps the cap cut forward to the nearest user turn boundary", () => {
    const rows = transcript(10, 10);
    const start = historyWindowStart(rows, 30);
    expect(rows[start]!.kind).toBe("user_prompt");
    expect(start).toBe(88);
    expect(rows.length - start).toBeLessThanOrEqual(30);
  });

  it("hard-cuts at the cap when one huge turn has no boundary after it", () => {
    const rows: ActivityRow[] = [row("user_prompt", 0)];
    for (let i = 0; i < 500; i += 1) rows.push(row("tool_complete", i));
    const start = historyWindowStart(rows, 150);
    expect(start).toBe(rows.length - 150); // 351
    expect(rows.length - start).toBe(150);
  });

  it("counts user_diff_comments as a turn boundary", () => {
    const rows: ActivityRow[] = [];
    for (let i = 0; i < 40; i += 1) rows.push(row("message", i));
    rows[35] = row("user_diff_comments", 35);
    expect(historyWindowStart(rows, 10)).toBe(35);
  });

  it("walks down to 0 as the window grows past the transcript", () => {
    const rows = transcript(5, 5); // 30 rows
    expect(historyWindowStart(rows, 30)).toBe(0);
    expect(historyWindowStart(rows, 1000)).toBe(0);
  });

  it("treats a non-positive window as show-all", () => {
    const rows = transcript(10, 10);
    expect(historyWindowStart(rows, 0)).toBe(0);
    expect(historyWindowStart(rows, -5)).toBe(0);
  });

  it("pulls the cut back to the Task parent when it lands among sub-agent children (#2313)", () => {
    const rows = subagentTranscript(100, 50);
    const parentIdx = rows.findIndex((r) => r.kind === "tool_start" && r.tool?.id === "task1");
    expect(parentIdx).toBe(101);
    const start = historyWindowStart(rows, 40);
    expect(start).toBe(parentIdx);
    expect(rows[start]!.tool?.parent_tool_call_id).toBeUndefined();
  });

  it("leaves the cut alone when it lands exactly on the Task parent", () => {
    const rows = subagentTranscript(100, 50); // 152 rows, parent at 101.
    expect(historyWindowStart(rows, 51)).toBe(101);
  });

  it("walks the whole parent chain back for nested sub-agents", () => {
    const rows = subagentTranscript(100, 49, ["task1", "task2"]);
    expect(historyWindowStart(rows, 40)).toBe(101);
  });

  it("does not pull back a clean user-boundary start", () => {
    const rows = transcript(10, 10);
    const start = historyWindowStart(rows, 30);
    expect(rows[start]!.kind).toBe("user_prompt");
    expect(start).toBe(88);
  });
});

describe("historyWindow", () => {
  it("can load earlier when rows are windowed out and there is no clear", () => {
    const rows = transcript(10, 10); // 110 rows
    const w = historyWindow(rows, 30, false);
    expect(w.start).toBeGreaterThan(0);
    expect(w.canLoadEarlier).toBe(true);
  });

  it("cannot load earlier when everything fits", () => {
    const rows = transcript(2, 3); // 8 rows
    expect(historyWindow(rows, DEFAULT_HISTORY_WINDOW, false)).toEqual({ start: 0, canLoadEarlier: false });
  });

  it("suppresses load-earlier when the only hidden rows are pre-clear", () => {
    const rows: ActivityRow[] = [];
    for (let t = 0; t < 100; t += 1) {
      rows.push(row("user_prompt", t));
      rows.push(row("message", t));
    }
    rows.push(row("session_cleared", 999));
    for (let t = 0; t < 2; t += 1) {
      rows.push(row("user_prompt", 1000 + t));
      rows.push(row("message", 1000 + t));
    }
    const w = historyWindow(rows, DEFAULT_HISTORY_WINDOW, false);
    expect(w.start).toBeLessThan(rows.length - 5);
    expect(w.canLoadEarlier).toBe(false);
  });

  it("can load earlier post-clear rows, and ignores the clear when cleared turns are shown", () => {
    const rows: ActivityRow[] = [row("session_cleared", 0)];
    for (let i = 0; i < 200; i += 1) rows.push(row("message", i));
    expect(historyWindow(rows, 30, false).canLoadEarlier).toBe(true);
    expect(historyWindow(rows, 30, true).canLoadEarlier).toBe(true);
  });
});

describe("initialHistoryWindow", () => {
  it("keeps the default when the last turn fits inside it", () => {
    const rows = transcript(100, 1); // 200 rows, last turn = 2 rows
    expect(initialHistoryWindow(rows)).toBe(DEFAULT_HISTORY_WINDOW);
  });

  it("widens to the whole last turn when that turn alone is longer than the default", () => {
    const rows = transcript(3, 2);
    const promptIdx = rows.length;
    rows.push(row("user_prompt", 99));
    for (let i = 0; i < 400; i += 1) rows.push(row("tool_complete", i));
    expect(lastUserBoundaryIndex(rows)).toBe(promptIdx);
    const visible = initialHistoryWindow(rows);
    expect(visible).toBe(401);
    expect(historyWindowStart(rows, visible)).toBe(promptIdx);
  });

  it("counts typed diff comments as the last turn's boundary", () => {
    const rows: ActivityRow[] = transcript(2, 1);
    rows.push(row("user_diff_comments", 7));
    for (let i = 0; i < 200; i += 1) rows.push(row("message", i));
    expect(initialHistoryWindow(rows)).toBe(201);
  });

  it("falls back to the default when the transcript has no user turn", () => {
    const rows: ActivityRow[] = [];
    for (let i = 0; i < 500; i += 1) rows.push(row("message", i));
    expect(lastUserBoundaryIndex(rows)).toBe(-1);
    expect(initialHistoryWindow(rows)).toBe(DEFAULT_HISTORY_WINDOW);
    expect(initialHistoryWindow([])).toBe(DEFAULT_HISTORY_WINDOW);
  });
});
