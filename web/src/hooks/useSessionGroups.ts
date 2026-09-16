import { useCallback, useEffect, useMemo, useState } from "react";
import type { Workspace } from "../lib/types";
import { safeGetItem, safeRemoveItem, safeSetItem } from "../lib/safeStorage";
import { buildSessionGroups, type SidebarGroup } from "../lib/sidebarGroups";
import type { PluginSortContext, SidebarSortMode } from "../lib/sidebarSort";
import { useIdleDecayWindowMs } from "../lib/idleDecay";

const COLLAPSED_KEY_PREFIX = "aoe-group-collapsed-";

function loadCollapsed(id: string): boolean {
  return safeGetItem(`${COLLAPSED_KEY_PREFIX}${id}`) === "1";
}

export function useSessionGroups(
  workspaces: Workspace[],
  sortMode: SidebarSortMode,
  pluginSort?: PluginSortContext,
): {
  groups: SidebarGroup[];
  toggleGroupCollapsed: (groupId: string) => void;
} {
  const idleDecayWindowMs = useIdleDecayWindowMs();
  const [collapsedMap, setCollapsedMap] = useState<Record<string, boolean>>({});

  const groups = useMemo(
    () =>
      buildSessionGroups(workspaces, {
        idleDecayWindowMs,
        sortMode,
        pluginSort,
        isCollapsed: (id) => collapsedMap[id] ?? loadCollapsed(id),
      }),
    [workspaces, idleDecayWindowMs, sortMode, pluginSort, collapsedMap],
  );

  // Keep the updater pure: StrictMode double-invokes it, so persist in an effect.
  const toggleGroupCollapsed = useCallback((groupId: string) => {
    setCollapsedMap((prev) => {
      const current = prev[groupId] ?? loadCollapsed(groupId);
      return { ...prev, [groupId]: !current };
    });
  }, []);

  useEffect(() => {
    for (const [id, collapsed] of Object.entries(collapsedMap)) {
      if (collapsed) {
        safeSetItem(`${COLLAPSED_KEY_PREFIX}${id}`, "1");
      } else {
        safeRemoveItem(`${COLLAPSED_KEY_PREFIX}${id}`);
      }
    }
  }, [collapsedMap]);

  return { groups, toggleGroupCollapsed };
}
