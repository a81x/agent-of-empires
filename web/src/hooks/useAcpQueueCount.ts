import { useMemo, useSyncExternalStore } from "react";

import { getQueuedCount, subscribeAcpState } from "../lib/acpStateStorage";

export function useQueuedCountForSessions(sessionIds: readonly string[]): number {
  const ids = sessionIds.join("|");
  const subscribe = useMemo(() => {
    const filter = new Set(ids ? ids.split("|").filter(Boolean) : []);
    return (cb: () => void) => subscribeAcpState(cb, filter);
  }, [ids]);
  return useSyncExternalStore(
    subscribe,
    () => {
      let total = 0;
      for (const id of ids ? ids.split("|") : []) {
        if (id) total += getQueuedCount(id);
      }
      return total;
    },
    () => 0,
  );
}
