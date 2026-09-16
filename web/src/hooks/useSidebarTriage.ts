import { useCallback, useState } from "react";

import { setSessionArchive, setSessionPin, setSessionSnooze, setSessionUnread } from "../lib/api";
import { reportError } from "../lib/toastBus";
import {
  EMPTY_OPTIMISTIC,
  makeOptimisticSnoozedUntil,
  reconcileOptimistic,
  withOverride,
  type OptimisticTriage,
} from "../lib/sidebarOptimistic";
import type { Workspace } from "../lib/types";

export interface TriageResult {
  workspaceId: string;
  ok: boolean;
  skipped?: boolean;
}

export function useSidebarTriage(workspaces: readonly Workspace[]) {
  const [overlay, setOverlay] = useState<Map<string, OptimisticTriage>>(() => new Map());
  const [trackedWorkspaces, setTrackedWorkspaces] = useState(workspaces);
  if (workspaces !== trackedWorkspaces) {
    setTrackedWorkspaces(workspaces);
    setOverlay((prev) => reconcileOptimistic(prev, workspaces));
  }

  const setOverride = useCallback((workspaceId: string, patch: Partial<OptimisticTriage>) => {
    setOverlay((prev) => {
      const next = new Map(prev);
      next.set(workspaceId, withOverride(prev.get(workspaceId), patch));
      return next;
    });
  }, []);

  const optimisticFor = useCallback(
    (workspaceId: string): OptimisticTriage => overlay.get(workspaceId) ?? EMPTY_OPTIMISTIC,
    [overlay],
  );

  const pin = useCallback(
    async (ws: Workspace, pinned: boolean): Promise<TriageResult> => {
      const sessionId = ws.sessions[0]?.id;
      if (!sessionId) return { workspaceId: ws.id, ok: false, skipped: true };
      setOverride(ws.id, { pinned });
      const result = await setSessionPin(sessionId, pinned);
      if (!result) {
        setOverride(ws.id, { pinned: null });
        return { workspaceId: ws.id, ok: false };
      }
      return { workspaceId: ws.id, ok: true };
    },
    [setOverride],
  );

  const archive = useCallback(
    async (ws: Workspace, archived: boolean): Promise<TriageResult> => {
      const sessionId = ws.sessions[0]?.id;
      if (!sessionId) return { workspaceId: ws.id, ok: false, skipped: true };
      setOverride(ws.id, { archived });
      const result = await setSessionArchive(sessionId, archived);
      if (!result) {
        setOverride(ws.id, { archived: null });
        return { workspaceId: ws.id, ok: false };
      }
      return { workspaceId: ws.id, ok: true };
    },
    [setOverride],
  );

  const snooze = useCallback(
    async (ws: Workspace, minutes: number | null): Promise<TriageResult> => {
      const sessionId = ws.sessions[0]?.id;
      if (!sessionId) return { workspaceId: ws.id, ok: false, skipped: true };
      const optimisticUntil = minutes == null ? null : makeOptimisticSnoozedUntil(minutes);
      setOverride(ws.id, { snoozedUntil: optimisticUntil });
      const result = await setSessionSnooze(sessionId, minutes);
      if (!result) {
        setOverride(ws.id, { snoozedUntil: undefined });
        return { workspaceId: ws.id, ok: false };
      }
      return { workspaceId: ws.id, ok: true };
    },
    [setOverride],
  );

  const unread = useCallback(
    async (ws: Workspace, markUnread: boolean): Promise<TriageResult> => {
      const sessionId = ws.sessions[0]?.id;
      if (!sessionId) return { workspaceId: ws.id, ok: false, skipped: true };
      setOverride(ws.id, { unread: markUnread });
      const result = await setSessionUnread(sessionId, markUnread);
      if (!result) {
        setOverride(ws.id, { unread: null });
        return { workspaceId: ws.id, ok: false };
      }
      return { workspaceId: ws.id, ok: true };
    },
    [setOverride],
  );

  const pinToggle = useCallback(
    (ws: Workspace, pinned: boolean) => {
      void pin(ws, pinned).then((r) => {
        if (!r.ok && !r.skipped) {
          reportError(pinned ? "Failed to pin session" : "Failed to unpin session");
        }
      });
    },
    [pin],
  );

  const archiveToggle = useCallback(
    (ws: Workspace, archived: boolean) => {
      void archive(ws, archived).then((r) => {
        if (!r.ok && !r.skipped) {
          reportError(archived ? "Failed to archive session" : "Failed to unarchive session");
        }
      });
    },
    [archive],
  );

  const snoozeOne = useCallback(
    (ws: Workspace, minutes: number | null) => {
      void snooze(ws, minutes).then((r) => {
        if (!r.ok && !r.skipped) {
          reportError(minutes == null ? "Failed to unsnooze session" : "Failed to snooze session");
        }
      });
    },
    [snooze],
  );

  const unreadToggle = useCallback(
    (ws: Workspace, markUnread: boolean) => {
      void unread(ws, markUnread).then((r) => {
        if (!r.ok && !r.skipped) {
          reportError(markUnread ? "Failed to mark unread" : "Failed to mark read");
        }
      });
    },
    [unread],
  );

  // Serial: each PATCH rewrites the profile's session list, so concurrent calls would race.
  const runBulk = useCallback(
    async (
      workspaces: readonly Workspace[],
      action: (ws: Workspace) => Promise<TriageResult>,
    ): Promise<TriageResult[]> => {
      const results: TriageResult[] = [];
      for (const ws of workspaces) {
        results.push(await action(ws));
      }
      return results;
    },
    [],
  );

  const bulkPin = useCallback(
    (wss: readonly Workspace[], pinned: boolean) => runBulk(wss, (ws) => pin(ws, pinned)),
    [runBulk, pin],
  );
  const bulkArchive = useCallback(
    (wss: readonly Workspace[], archived: boolean) => runBulk(wss, (ws) => archive(ws, archived)),
    [runBulk, archive],
  );
  const bulkSnooze = useCallback(
    (wss: readonly Workspace[], minutes: number | null) => runBulk(wss, (ws) => snooze(ws, minutes)),
    [runBulk, snooze],
  );

  return {
    optimisticFor,
    pinToggle,
    archiveToggle,
    snooze: snoozeOne,
    unreadToggle,
    bulkPin,
    bulkArchive,
    bulkSnooze,
  };
}
