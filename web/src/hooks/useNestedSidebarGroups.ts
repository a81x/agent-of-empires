import { useCallback, useEffect, useMemo, useState } from "react";
import type { RepoGroup } from "../lib/types";
import { safeGetItem, safeRemoveItem, safeSetItem } from "../lib/safeStorage";
import { buildNestedSidebarGroups, type NestedSidebarGroup } from "../lib/sidebarGroups";
import type { PluginSortContext, SidebarSortMode } from "../lib/sidebarSort";
import { useIdleDecayWindowMs } from "../lib/idleDecay";

const COLLAPSED_KEY_PREFIX = "aoe-nested-group-collapsed-";

// Encode both halves so a `::` in a path cannot collide two pairs.
function subgroupKey(repoId: string, groupPath: string): string {
  return `${encodeURIComponent(repoId)}::${encodeURIComponent(groupPath)}`;
}

function loadCollapsed(key: string): boolean {
  return safeGetItem(`${COLLAPSED_KEY_PREFIX}${key}`) === "1";
}

export function useNestedSidebarGroups(
  repoGroups: RepoGroup[],
  sortMode: SidebarSortMode,
  pluginSort?: PluginSortContext,
): {
  groups: NestedSidebarGroup[];
  toggleSubgroupCollapsed: (repoId: string, groupPath: string) => void;
} {
  const idleDecayWindowMs = useIdleDecayWindowMs();
  const [collapsedMap, setCollapsedMap] = useState<Record<string, boolean>>({});

  const groups = useMemo(
    () =>
      buildNestedSidebarGroups(repoGroups, {
        idleDecayWindowMs,
        sortMode,
        pluginSort,
        isSubgroupCollapsed: (repoId, groupPath) => {
          const key = subgroupKey(repoId, groupPath);
          return collapsedMap[key] ?? loadCollapsed(key);
        },
      }),
    [repoGroups, idleDecayWindowMs, sortMode, pluginSort, collapsedMap],
  );

  const toggleSubgroupCollapsed = useCallback((repoId: string, groupPath: string) => {
    const key = subgroupKey(repoId, groupPath);
    setCollapsedMap((prev) => {
      const current = prev[key] ?? loadCollapsed(key);
      return { ...prev, [key]: !current };
    });
  }, []);

  useEffect(() => {
    for (const [key, collapsed] of Object.entries(collapsedMap)) {
      if (collapsed) {
        safeSetItem(`${COLLAPSED_KEY_PREFIX}${key}`, "1");
      } else {
        safeRemoveItem(`${COLLAPSED_KEY_PREFIX}${key}`);
      }
    }
  }, [collapsedMap]);

  return { groups, toggleSubgroupCollapsed };
}
