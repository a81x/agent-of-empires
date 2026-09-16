import { useCallback, useMemo, useState } from "react";
import type { ProjectInfo, Workspace, RepoGroup } from "../lib/types";
import { mergeRegisteredProjects, unpinnedSavedProjects } from "../lib/registeredProjects";
import { safeGetItem, safeRemoveItem, safeSetItem } from "../lib/safeStorage";
import {
  applyRepoAppearanceUpdate,
  loadRepoAppearances,
  persistRepoAppearances,
  type RepoAppearanceUpdate,
} from "../lib/repoAppearance";
import { loadRepoGroupOrder, persistRepoGroupOrder } from "../lib/repoGroupOrder";
import { compareSortValues, type PluginSortValue } from "../lib/pluginUi";
import {
  compareWorkspacesByAttention,
  compareWorkspacesByLastActivityDesc,
  compareWorkspacesByPluginSort,
  type PluginSortContext,
  repoGroupAttentionRank,
  repoGroupIsFavorited,
  repoGroupIsUrgent,
  repoGroupLastActivityMs,
  repoGroupPluginSortValue,
  workspaceTriageTier,
  type SidebarSortMode,
} from "../lib/sidebarSort";

const COLLAPSED_KEY_PREFIX = "aoe-repo-collapsed-";
export const MULTI_REPO_GROUP_ID = "__multi_repo__";
export const SCRATCH_GROUP_ID = "__scratch__";

function loadCollapsed(id: string): boolean {
  return safeGetItem(`${COLLAPSED_KEY_PREFIX}${id}`) === "1";
}

function isMultiRepoWorkspace(ws: Workspace): boolean {
  return ws.sessions.some((s) => (s.workspace_repos?.length ?? 0) > 1);
}

function isScratchWorkspace(ws: Workspace): boolean {
  return ws.sessions.some((s) => s.scratch);
}

export function useRepoGroups(
  workspaces: Workspace[],
  workspaceOrdering: readonly string[] = [],
  sortMode: SidebarSortMode = "manual",
  projects: readonly ProjectInfo[] = [],
  pluginSort?: PluginSortContext,
): {
  groups: RepoGroup[];
  savedProjects: RepoGroup[];
  toggleRepoCollapsed: (repoId: string) => void;
  updateRepoAppearance: (repoId: string, update: RepoAppearanceUpdate) => void;
  reorderRepoGroups: (orderedGroupIds: string[]) => void;
} {
  const [collapsedMap, setCollapsedMap] = useState<Record<string, boolean>>({});
  const [appearanceMap, setAppearanceMap] = useState(loadRepoAppearances);
  const [groupOrder, setGroupOrder] = useState<string[]>(loadRepoGroupOrder);

  const { groups, savedProjects } = useMemo(() => {
    const rank = new Map(workspaceOrdering.map((id, i) => [id, i] as const));
    const rankOf = (id: string) => rank.get(id) ?? Infinity;
    const groupRank = new Map(groupOrder.map((id, i) => [id, i] as const));
    const sortByRank = (list: Workspace[]) =>
      [...list].sort((a, b) => {
        const aTier = workspaceTriageTier(a);
        const bTier = workspaceTriageTier(b);
        if (aTier !== bTier) return aTier - bTier;
        // `Infinity - Infinity` is NaN, so compare explicitly and tie-break on id.
        const ar = rankOf(a.id);
        const br = rankOf(b.id);
        if (ar < br) return -1;
        if (ar > br) return 1;
        return a.id.localeCompare(b.id);
      });
    const pluginCompare = pluginSort ? compareWorkspacesByPluginSort(pluginSort) : null;
    const sortWorkspaces = (list: Workspace[]) => {
      if (pluginCompare) {
        return [...list].sort(pluginCompare);
      }
      if (sortMode === "attention") {
        return [...list].sort(compareWorkspacesByAttention);
      }
      if (sortMode === "lastActivity") {
        return [...list].sort(compareWorkspacesByLastActivityDesc);
      }
      return sortByRank(list);
    };

    const byRepo = new Map<string, Workspace[]>();
    const multiRepo: Workspace[] = [];
    const scratch: Workspace[] = [];

    for (const ws of workspaces) {
      if (isScratchWorkspace(ws)) {
        scratch.push(ws);
        continue;
      }
      if (isMultiRepoWorkspace(ws)) {
        multiRepo.push(ws);
        continue;
      }
      const existing = byRepo.get(ws.projectPath);
      if (existing) existing.push(ws);
      else byRepo.set(ws.projectPath, [ws]);
    }

    const repoGroups: RepoGroup[] = [];

    for (const [repoPath, repoWorkspaces] of byRepo) {
      const sorted = sortWorkspaces(repoWorkspaces);
      const hasActive = sorted.some((ws) => ws.status === "active");
      const collapsed = collapsedMap[repoPath] ?? loadCollapsed(repoPath);
      const remoteOwner = sorted[0]?.sessions[0]?.remote_owner ?? null;
      const remoteOwnerKey = sorted[0]?.sessions[0]?.remote_owner_key ?? null;
      const appearance = appearanceMap[repoPath];
      const defaultDisplayName = repoPath.split("/").pop() ?? repoPath;

      repoGroups.push({
        id: repoPath,
        repoPath,
        displayName: appearance?.alias ?? defaultDisplayName,
        defaultDisplayName,
        alias: appearance?.alias ?? null,
        color: appearance?.color ?? null,
        remoteOwner,
        remoteOwnerKey,
        workspaces: sorted,
        status: hasActive ? "active" : "idle",
        collapsed,
        registeredProjects: [],
      });
    }

    if (multiRepo.length > 0) {
      const sorted = sortWorkspaces(multiRepo);
      const hasActive = sorted.some((ws) => ws.status === "active");
      const collapsed = collapsedMap[MULTI_REPO_GROUP_ID] ?? loadCollapsed(MULTI_REPO_GROUP_ID);
      const appearance = appearanceMap[MULTI_REPO_GROUP_ID];
      const defaultDisplayName = "Multi-repo";
      repoGroups.push({
        id: MULTI_REPO_GROUP_ID,
        repoPath: MULTI_REPO_GROUP_ID,
        displayName: appearance?.alias ?? defaultDisplayName,
        defaultDisplayName,
        alias: appearance?.alias ?? null,
        color: appearance?.color ?? null,
        remoteOwner: null,
        remoteOwnerKey: null,
        workspaces: sorted,
        status: hasActive ? "active" : "idle",
        collapsed,
        registeredProjects: [],
      });
    }

    if (scratch.length > 0) {
      const sorted = sortWorkspaces(scratch);
      const hasActive = sorted.some((ws) => ws.status === "active");
      const collapsed = collapsedMap[SCRATCH_GROUP_ID] ?? loadCollapsed(SCRATCH_GROUP_ID);
      const appearance = appearanceMap[SCRATCH_GROUP_ID];
      const defaultDisplayName = "Scratch";
      repoGroups.push({
        id: SCRATCH_GROUP_ID,
        repoPath: SCRATCH_GROUP_ID,
        displayName: appearance?.alias ?? defaultDisplayName,
        defaultDisplayName,
        alias: appearance?.alias ?? null,
        color: appearance?.color ?? null,
        remoteOwner: null,
        remoteOwnerKey: null,
        workspaces: sorted,
        status: hasActive ? "active" : "idle",
        collapsed,
        registeredProjects: [],
      });
    }

    const merged = mergeRegisteredProjects(repoGroups, [...projects], {
      alias: (repoPath) => appearanceMap[repoPath]?.alias ?? null,
      color: (repoPath) => appearanceMap[repoPath]?.color ?? null,
      collapsed: (repoPath) => collapsedMap[repoPath] ?? loadCollapsed(repoPath),
    });

    const isSyntheticGroup = (id: string) => id === MULTI_REPO_GROUP_ID || id === SCRATCH_GROUP_ID;
    const isRegisteredEmpty = (g: RepoGroup) => g.workspaces.length === 0 && g.registeredProjects.length > 0;

    merged.sort((a, b) => {
      if (pluginSort) {
        if (a.id === SCRATCH_GROUP_ID) return 1;
        if (b.id === SCRATCH_GROUP_ID) return -1;
        if (a.id === MULTI_REPO_GROUP_ID) return 1;
        if (b.id === MULTI_REPO_GROUP_ID) return -1;
        const ae = isRegisteredEmpty(a);
        const be = isRegisteredEmpty(b);
        if (ae !== be) return ae ? 1 : -1;
        const av: PluginSortValue | undefined = repoGroupPluginSortValue(a.workspaces, pluginSort);
        const bv: PluginSortValue | undefined = repoGroupPluginSortValue(b.workspaces, pluginSort);
        const cmp = compareSortValues(av, bv, pluginSort.direction);
        if (cmp !== 0) return cmp;
        const ak = repoGroupLastActivityMs(a.workspaces);
        const bk = repoGroupLastActivityMs(b.workspaces);
        if (ak !== bk) return bk - ak;
        return a.repoPath.localeCompare(b.repoPath);
      }
      if (sortMode === "attention") {
        if (a.id === SCRATCH_GROUP_ID) return 1;
        if (b.id === SCRATCH_GROUP_ID) return -1;
        if (a.id === MULTI_REPO_GROUP_ID) return 1;
        if (b.id === MULTI_REPO_GROUP_ID) return -1;
        const ae = isRegisteredEmpty(a);
        const be = isRegisteredEmpty(b);
        if (ae !== be) return ae ? 1 : -1;
        const au = repoGroupIsUrgent(a.workspaces);
        const bu = repoGroupIsUrgent(b.workspaces);
        if (au !== bu) return au ? -1 : 1;
        const ar = repoGroupAttentionRank(a.workspaces);
        const br = repoGroupAttentionRank(b.workspaces);
        if (ar !== br) return ar - br;
        const af = repoGroupIsFavorited(a.workspaces);
        const bf = repoGroupIsFavorited(b.workspaces);
        if (af !== bf) return af ? -1 : 1;
        const ak = repoGroupLastActivityMs(a.workspaces);
        const bk = repoGroupLastActivityMs(b.workspaces);
        if (ak !== bk) return bk - ak;
        return a.repoPath.localeCompare(b.repoPath);
      }
      if (sortMode === "lastActivity") {
        if (a.id === SCRATCH_GROUP_ID) return 1;
        if (b.id === SCRATCH_GROUP_ID) return -1;
        if (a.id === MULTI_REPO_GROUP_ID) return 1;
        if (b.id === MULTI_REPO_GROUP_ID) return -1;
        const ae = isRegisteredEmpty(a);
        const be = isRegisteredEmpty(b);
        if (ae !== be) return ae ? 1 : -1;
        const ak = repoGroupLastActivityMs(a.workspaces);
        const bk = repoGroupLastActivityMs(b.workspaces);
        if (ak !== bk) return bk - ak;
        return a.repoPath.localeCompare(b.repoPath);
      }
      const ag = groupRank.get(a.id);
      const bg = groupRank.get(b.id);
      const SYNTHETIC_BOTTOM = Number.MAX_SAFE_INTEGER;
      const fallbackRank = (g: RepoGroup) =>
        isSyntheticGroup(g.id) ? SYNTHETIC_BOTTOM : isRegisteredEmpty(g) ? SYNTHETIC_BOTTOM - 1 : -1;
      const keyOf = (g: RepoGroup, rank: number | undefined) => (rank != null ? rank : fallbackRank(g));
      const ka = keyOf(a, ag);
      const kb = keyOf(b, bg);
      if (ka !== kb) return ka - kb;
      if (ka === SYNTHETIC_BOTTOM) {
        if (a.id === MULTI_REPO_GROUP_ID) return -1;
        if (b.id === MULTI_REPO_GROUP_ID) return 1;
        return 0;
      }
      const am = Math.min(...a.workspaces.map((w) => rankOf(w.id)));
      const bm = Math.min(...b.workspaces.map((w) => rankOf(w.id)));
      if (am !== bm) return am - bm;
      return a.repoPath.localeCompare(b.repoPath);
    });

    const savedProjects = unpinnedSavedProjects(repoGroups, [...projects], {
      alias: (repoPath) => appearanceMap[repoPath]?.alias ?? null,
      color: (repoPath) => appearanceMap[repoPath]?.color ?? null,
    });

    return { groups: merged, savedProjects };
  }, [workspaces, workspaceOrdering, sortMode, pluginSort, projects, collapsedMap, appearanceMap, groupOrder]);

  const toggleRepoCollapsed = useCallback((repoId: string) => {
    setCollapsedMap((prev) => {
      const current = prev[repoId] ?? loadCollapsed(repoId);
      const next = !current;
      if (next) {
        safeSetItem(`${COLLAPSED_KEY_PREFIX}${repoId}`, "1");
      } else {
        safeRemoveItem(`${COLLAPSED_KEY_PREFIX}${repoId}`);
      }
      return { ...prev, [repoId]: next };
    });
  }, []);

  const updateRepoAppearance = useCallback((repoId: string, update: RepoAppearanceUpdate) => {
    setAppearanceMap((prev) => {
      const next = applyRepoAppearanceUpdate(prev, repoId, update);
      persistRepoAppearances(next);
      return next;
    });
  }, []);

  const reorderRepoGroups = useCallback((orderedGroupIds: string[]) => {
    setGroupOrder(orderedGroupIds);
    persistRepoGroupOrder(orderedGroupIds);
  }, []);

  return {
    groups,
    savedProjects,
    toggleRepoCollapsed,
    updateRepoAppearance,
    reorderRepoGroups,
  };
}
