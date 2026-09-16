import {
  lazy,
  Suspense,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { Puzzle } from "lucide-react";
import { useMatch, useNavigate, useSearchParams } from "react-router-dom";
import { IDLE_DECAY_WINDOW_MS } from "./lib/session";
import { diffSelectionStale } from "./lib/diffSelection";
import { useSessions } from "./hooks/useSessions";
import { useDashboardPresence } from "./hooks/useDashboardPresence";
import { clearAcpCache } from "./hooks/useAcpSession";
import { clearDraft, sweepOrphanDrafts } from "./lib/acpDrafts";
import { AcpPrefsProvider } from "./lib/acpPrefs";
import { safeGetItem, safeRemoveItem } from "./lib/safeStorage";
import { isAutomatedSession } from "./lib/onboarding";
import { useWorkspaces } from "./hooks/useWorkspaces";
import { useLastSessionRestore } from "./hooks/useLastSessionRestore";
import { useRepoGroups } from "./hooks/useRepoGroups";
import { useSessionGroups } from "./hooks/useSessionGroups";
import { useNestedSidebarGroups } from "./hooks/useNestedSidebarGroups";
import { useOrgGroups } from "./hooks/useOrgGroups";
import { PluginUiProvider, usePluginUiEntries } from "./lib/pluginUiContext";
import { buildSortValueMap, pluginSortSpecs } from "./lib/pluginUi";
import type { PluginSortContext, SidebarSortMode } from "./lib/sidebarSort";
import { nextAttentionSessionId, sessionNeedsAttention, workspaceIsTrashed } from "./lib/sidebarSort";
import { useSidebarSortMode } from "./hooks/useSidebarSortMode";
import { useSidebarAxis } from "./hooks/useSidebarAxis";
import { repoGroupToSidebarGroup, type SidebarGroup } from "./lib/sidebarGroups";
import { useProjects } from "./hooks/useProjects";
import { useKeyboardShortcuts } from "./hooks/useKeyboardShortcuts";
import { useResolvedTheme } from "./hooks/useResolvedTheme";
import { useWebSettings } from "./hooks/useWebSettings";
import { useDiffFiles } from "./hooks/useDiffFiles";
import { useDiffComments } from "./hooks/useDiffComments";
import { clearStoredComments, sweepOrphanComments } from "./components/diff/comments/storage";
import { SendCommentsDialog } from "./components/diff/comments/SendCommentsDialog";
import { useCommandActions, buildConversationActions, type SessionStateAction } from "./hooks/useCommandActions";
import { usePluginCommands } from "./hooks/usePluginCommands";
import { useSettingsCommands } from "./hooks/useSettingsCommands";
import { useEdgeSwipe } from "./hooks/useEdgeSwipe";
import { useIsCoarsePointer } from "./hooks/useIsCoarsePointer";
import { useMobileViewportLock } from "./hooks/useMobileViewportLock";
import { useIsWideViewport } from "./hooks/useIsWideViewport";
import type { RightPanelView } from "./lib/rightPanelView";
import { usePaneLayout, dockTabs, dockGroups, dockOf, isActiveTab, isDockCollapsed } from "./lib/paneLayout";
import { isPluginPaneId, resolvePaneIcon, usePluginPanes, type PluginPane } from "./lib/pluginPanes";
import { PluginPaneBody } from "./components/plugin/PluginSlots";
import { TOUR_ANCHORS, tourAnchor } from "./lib/tourSteps";
import {
  deleteWorkspaceSessions,
  type Notifier,
  restoreSessions,
  trashedWorkspaceRestoreIds,
  trashSessions,
  workspaceCleanupDefaults,
} from "./lib/trashActions";
import {
  loginStatus,
  logout,
  stopSession,
  startSession,
  acpEnable,
  acpDisable,
  fetchAbout,
  fetchSettings,
  fetchTelemetryStatus,
  setTelemetryConsent,
  reportTelemetrySeen,
  isDebugBuild,
  markWebTourSeen,
  updateWorkspaceOrdering,
  createProject,
  setProjectPinned,
  deleteProject,
  setSessionUnread,
  killTerminal,
  setSessionPin,
  setSessionArchive,
  setSessionSnooze,
  trashSession,
  restoreSession,
  fetchPlugins,
} from "./lib/api";
import type { DeleteSessionOptions, ServerAbout } from "./lib/api";
import { getClientCapabilities } from "./lib/clientCapabilities";
import { normalizeProjectPathKey } from "./lib/registeredProjects";
import { IdleDecayWindowContext, parseIdleDecayWindowMs } from "./lib/idleDecay";
import { parseUnreadIndicatorEnabled, UnreadIndicatorContext, useUnreadIndicatorEnabled } from "./lib/unreadIndicator";
import { parseSessionRowTagMode, SessionRowTagContext, type SessionRowTagMode } from "./lib/sessionRowTag";
import { parseSessionColorsEnabled, SessionColorsContext } from "./lib/sessionColors";
import { toastBus, reportError } from "./lib/toastBus";
import { isAbsolutePath, resolveToRepoRelative, type FileRef } from "./lib/fileRef";
import { OPEN_SESSION_EVENT } from "./lib/sessionRoute";
import { dispatchFocusTerminal, requestSessionInputFocus, setPendingTerminalFocus } from "./lib/terminalFocus";
import {
  clearMobileKeyboardProxyInput,
  deliverMobileKeyboardProxyInput,
  forwardTerminalBeforeInput,
} from "./lib/mobileKeyboardProxy";
import { hydrateWebUiStateFromServer, initWebUiSync } from "./lib/webUiSync";
import { WorkspaceSidebar, SnoozeModal } from "./components/WorkspaceSidebar";
import { DeleteSessionDialog } from "./components/DeleteSessionDialog";
import { StopSessionDialog } from "./components/StopSessionDialog";
import { SwitchViewDialog } from "./components/SwitchViewDialog";
import { TopBar } from "./components/TopBar";
import { AppShellSkeleton, MainPaneSkeleton } from "./components/AppShellSkeleton";
import { ContentSplit } from "./components/ContentSplit";
import { TerminalSessionStack } from "./components/TerminalSessionStack";
const StructuredView = lazy(() =>
  import("./components/acp/StructuredView").then((m) => ({
    default: m.StructuredView,
  })),
);
import { type PaneDisplay } from "./components/Dock";
import { DockGroups, type DockGroupView } from "./components/DockGroups";
import { BottomDock } from "./components/BottomDock";
import { PaneDndController } from "./components/PaneDndController";
import { visibleToFullIndex, type DropTarget } from "./components/paneDnd";
import { BackgroundAgentsPanel } from "./components/acp/BackgroundAgentsPanel";
import { DiffPane } from "./components/DiffPane";
import { FilesPane } from "./components/FilesPane";
import { FileContentViewer } from "./components/diff/FileContentViewer";
import { PairedShellPane } from "./components/PairedTerminal";
import { BUILTIN_PANES, isTerminalTabId, terminalIndexOf, terminalTabId, type DockLocation } from "./lib/panes";
import { MobileRightPanelPicker } from "./components/MobileRightPanelPicker";
import { MobileMainPane } from "./components/MobileMainPane";
import { ChromeCollapseHandle, CollapsibleRegion } from "./components/CollapsibleChrome";
import { DiffFileViewer } from "./components/diff/DiffFileViewer";
import { SettingsView } from "./components/SettingsView";
import { ProjectFormModal } from "./components/ProjectFormModal";
import { HelpOverlay } from "./components/HelpOverlay";
import { useTour } from "./hooks/useTour";
import { useWelcomePhase } from "./hooks/useWelcomePhase";
import { ThemeIntro } from "./components/onboarding/ThemeIntro";
import type { TourScope } from "./lib/tourSteps";
import { SessionWizard } from "./components/session-wizard/SessionWizard";
import type { WizardPrefill } from "./components/session-wizard/SessionWizard";
import type { ProjectInfo, RepoGroup, SessionResponse } from "./lib/types";
import { Dashboard } from "./components/Dashboard";
import { LoginPage } from "./components/LoginPage";
import { TokenEntryPage } from "./components/TokenEntryPage";
import { LOGIN_REQUIRED_EVENT, TOKEN_EXPIRED_EVENT, resetTokenExpired } from "./lib/fetchInterceptor";
import { AboutModal } from "./components/AboutModal";
import { TelemetryConsentModal } from "./components/TelemetryConsentModal";
import { TipsModal } from "./components/TipsModal";
import { useTips, shouldAutoPopTips } from "./hooks/useTips";
import { CommandPalette } from "./components/command-palette/CommandPalette";
import { useConversationSearch } from "./hooks/useConversationSearch";
import { DisconnectBanner } from "./components/DisconnectBanner";
import { ElevationPrompt } from "./components/ElevationPrompt";
import { UpdateBanner } from "./components/UpdateBanner";
import { DashboardUpdateBanner } from "./components/DashboardUpdateBanner";

const LEGACY_TOUR_SEEN_KEY = "aoe-tour-seen";

export default function App() {
  useMobileViewportLock();
  useResolvedTheme();
  const [loginRequired, setLoginRequired] = useState<boolean | null>(null);
  const [loginAuthenticated, setLoginAuthenticated] = useState(true);
  const [tokenExpired, setTokenExpired] = useState(false);
  const [idleDecayWindowMs, setIdleDecayWindowMs] = useState(IDLE_DECAY_WINDOW_MS);
  const [unreadIndicatorEnabled, setUnreadIndicatorEnabled] = useState(true);
  const [sessionRowTagMode, setSessionRowTagMode] = useState<SessionRowTagMode>("branch");
  const [sessionColorsEnabled, setSessionColorsEnabled] = useState(true);

  const applyAppSettings = useCallback((settings: Record<string, unknown> | null | undefined) => {
    setIdleDecayWindowMs(parseIdleDecayWindowMs(settings));
    setUnreadIndicatorEnabled(parseUnreadIndicatorEnabled(settings));
    setSessionRowTagMode(parseSessionRowTagMode(settings));
    setSessionColorsEnabled(parseSessionColorsEnabled(settings));
  }, []);

  const refreshAppSettings = useCallback(async () => {
    applyAppSettings(await fetchSettings());
  }, [applyAppSettings]);

  useEffect(() => {
    const onTokenExpired = () => setTokenExpired(true);
    window.addEventListener(TOKEN_EXPIRED_EVENT, onTokenExpired);
    return () => window.removeEventListener(TOKEN_EXPIRED_EVENT, onTokenExpired);
  }, []);

  useEffect(() => {
    const onLoginRequired = () => {
      setTokenExpired(false);
      setLoginRequired(true);
      setLoginAuthenticated(false);
    };
    window.addEventListener(LOGIN_REQUIRED_EVENT, onLoginRequired);
    return () => window.removeEventListener(LOGIN_REQUIRED_EVENT, onLoginRequired);
  }, []);

  useEffect(() => {
    loginStatus().then(({ required, authenticated }) => {
      setLoginRequired(required);
      setLoginAuthenticated(authenticated);
    });
  }, []);

  useEffect(() => {
    fetchSettings().then(applyAppSettings);
  }, [applyAppSettings]);

  const handleTokenSuccess = () => {
    setTokenExpired(false);
    loginStatus().then(({ required, authenticated }) => {
      setLoginRequired(required);
      setLoginAuthenticated(authenticated);
    });
  };

  const handleLoginSuccess = () => {
    setLoginAuthenticated(true);
    resetTokenExpired();
  };

  const handleLogout = async () => {
    await logout();
    setLoginAuthenticated(false);
  };

  if (tokenExpired) {
    return <TokenEntryPage onSuccess={handleTokenSuccess} />;
  }

  if (loginRequired && !loginAuthenticated) {
    return <LoginPage onSuccess={handleLoginSuccess} />;
  }

  if (loginRequired === null) {
    return <AppShellSkeleton />;
  }

  return (
    <IdleDecayWindowContext.Provider value={idleDecayWindowMs}>
      <UnreadIndicatorContext.Provider value={unreadIndicatorEnabled}>
        <SessionRowTagContext.Provider value={sessionRowTagMode}>
          <SessionColorsContext.Provider value={sessionColorsEnabled}>
            <PluginUiProvider>
              <AppContent
                loginRequired={loginRequired}
                onLogout={handleLogout}
                onSettingsRefresh={refreshAppSettings}
              />
            </PluginUiProvider>
            <ElevationPrompt />
          </SessionColorsContext.Provider>
        </SessionRowTagContext.Provider>
      </UnreadIndicatorContext.Provider>
    </IdleDecayWindowContext.Provider>
  );
}

function isInsideEditable(target: EventTarget | null): boolean {
  let el: HTMLElement | null = target instanceof HTMLElement ? target : null;
  while (el) {
    const tag = el.tagName;
    if (tag === "INPUT" || tag === "TEXTAREA" || el.isContentEditable) {
      return true;
    }
    el = el.parentElement;
  }
  return false;
}

function AppContent({
  loginRequired,
  onLogout,
  onSettingsRefresh,
}: {
  loginRequired: boolean;
  onLogout: () => void;
  onSettingsRefresh: () => Promise<void> | void;
}) {
  useDashboardPresence();
  useEffect(() => {
    initWebUiSync();
    void hydrateWebUiStateFromServer();
  }, []);

  const navigate = useNavigate();
  const [searchParams, setSearchParams] = useSearchParams();
  const { settings: webSettings } = useWebSettings();
  const sessionMatch = useMatch("/session/:sessionId");
  const settingsRootMatch = useMatch("/settings");
  const settingsTabMatch = useMatch("/settings/:tab");
  const profilesMatch = useMatch("/profiles");
  const activeSessionId = sessionMatch?.params.sessionId ?? null;
  const showSettings = settingsRootMatch !== null || settingsTabMatch !== null;
  const settingsTab = settingsTabMatch?.params.tab ?? null;

  const {
    sessions,
    workspaceOrdering,
    setWorkspaceOrdering,
    markLocalOrderingUpdate,
    error,
    loaded: sessionsLoaded,
    injectSession,
    setSessionStatus,
    applySession,
  } = useSessions();
  const workspaces = useWorkspaces(sessions);
  const trashedWorkspaces = useMemo(() => workspaces.filter(workspaceIsTrashed), [workspaces]);

  useLastSessionRestore({ activeSessionId, sessions, sessionsLoaded });

  const sweptDraftsRef = useRef(false);
  useEffect(() => {
    if (sweptDraftsRef.current) return;
    if (!sessionsLoaded) return;
    sweptDraftsRef.current = true;
    sweepOrphanDrafts(new Set(sessions.map((s) => s.id)));
  }, [sessionsLoaded, sessions]);

  const sweptCommentsRef = useRef(false);
  useEffect(() => {
    if (sweptCommentsRef.current) return;
    if (!sessionsLoaded) return;
    sweptCommentsRef.current = true;
    sweepOrphanComments(new Set(sessions.map((s) => s.id)));
  }, [sessionsLoaded, sessions]);

  const [sidebarSortMode, setSidebarSortMode] = useSidebarSortMode();
  const [sidebarAxis, setSidebarAxis] = useSidebarAxis();

  const pluginUiEntries = usePluginUiEntries();
  const [pluginSortRef, setPluginSortRef] = useState<{ pluginId: string; entryId: string } | null>(null);
  const activePluginSort = useMemo(() => {
    if (!pluginSortRef) return null;
    return (
      pluginSortSpecs(pluginUiEntries).find(
        (s) => s.pluginId === pluginSortRef.pluginId && s.entryId === pluginSortRef.entryId,
      ) ?? null
    );
  }, [pluginUiEntries, pluginSortRef]);
  const pluginSort = useMemo<PluginSortContext | undefined>(
    () =>
      activePluginSort
        ? {
            direction: activePluginSort.direction,
            values: buildSortValueMap(pluginUiEntries, activePluginSort.pluginId, activePluginSort.column),
          }
        : undefined,
    [activePluginSort, pluginUiEntries],
  );
  const selectSidebarSortMode = useCallback(
    (mode: SidebarSortMode) => {
      setPluginSortRef(null);
      setSidebarSortMode(mode);
    },
    [setSidebarSortMode],
  );

  const { projects, refresh: refreshProjects } = useProjects();
  const {
    groups: repoGroups,
    savedProjects,
    toggleRepoCollapsed,
    updateRepoAppearance,
    reorderRepoGroups,
  } = useRepoGroups(workspaces, workspaceOrdering, sidebarSortMode, projects, pluginSort);
  const { groups: sessionGroups, toggleGroupCollapsed } = useSessionGroups(workspaces, sidebarSortMode, pluginSort);
  const { groups: nestedGroups, toggleSubgroupCollapsed } = useNestedSidebarGroups(
    repoGroups,
    sidebarSortMode,
    pluginSort,
  );
  const {
    groups: orgGroups,
    toggleOrgCollapsed,
    toggleRepoCollapsed: toggleOrgRepoCollapsed,
  } = useOrgGroups(repoGroups);

  const sidebarGroups = useMemo(
    () => (sidebarAxis === "group" ? sessionGroups : repoGroups.map(repoGroupToSidebarGroup)),
    [sidebarAxis, sessionGroups, repoGroups],
  );
  const toggleSidebarGroup = sidebarAxis === "group" ? toggleGroupCollapsed : toggleRepoCollapsed;

  const handleReorderWorkspaces = useCallback(
    (newOrder: string[]) => {
      setWorkspaceOrdering(newOrder);
      markLocalOrderingUpdate();
      void updateWorkspaceOrdering(newOrder);
    },
    [setWorkspaceOrdering, markLocalOrderingUpdate],
  );

  const [selectedFile, setSelectedFile] = useState<{
    path: string;
    repoName?: string;
    line?: number;
    cited?: boolean;
    external?: boolean;
  } | null>(null);
  const selectedFilePath = selectedFile?.path ?? null;
  const selectedFileExternal = selectedFile?.external ?? false;
  const selectedRepoName = selectedFile?.repoName;
  const selectedFileLine = selectedFile?.line;
  const {
    layout: paneLayout,
    openTab,
    addTerminal,
    closeTab,
    activateTab,
    moveTab,
    placeTab,
    toggleKind,
    togglePlugin,
    syncPlugins,
    setDockCollapsed,
  } = usePaneLayout(activeSessionId);
  const pluginPanes = usePluginPanes(activeSessionId);
  const pluginPaneById = useMemo(() => {
    const m = new Map<string, PluginPane>();
    for (const p of pluginPanes) m.set(p.id, p);
    return m;
  }, [pluginPanes]);

  useEffect(() => {
    syncPlugins(
      webSettings.autoOpenPluginPanes ? pluginPanes.map((p) => ({ id: p.id, defaultDock: p.defaultDock })) : [],
    );
  }, [pluginPanes, syncPlugins, webSettings.autoOpenPluginPanes]);

  const [pluginIdentityById, setPluginIdentityById] = useState<
    Record<string, { icon?: string; iconAssetUrl?: string }>
  >({});
  useEffect(() => {
    void fetchPlugins().then((res) => {
      if (!res) return;
      setPluginIdentityById(
        Object.fromEntries(
          res.plugins.map((p) => [p.id, { icon: p.icon ?? undefined, iconAssetUrl: p.icon_asset_url ?? undefined }]),
        ),
      );
    });
  }, []);

  const paneDescriptor = useCallback(
    (id: string): PaneDisplay => {
      const plugin = pluginPaneById.get(id);
      if (plugin) {
        const identity = pluginIdentityById[plugin.entry.plugin_id];
        const icon = resolvePaneIcon(plugin.icon, identity?.icon) ?? Puzzle;
        return { title: plugin.title, icon, iconAssetUrl: identity?.iconAssetUrl };
      }
      if (isTerminalTabId(id)) {
        const idx = terminalIndexOf(id);
        const term = BUILTIN_PANES.find((p) => p.id === "terminal")!;
        return { title: idx === 0 ? term.title : `${term.title} ${idx + 1}`, icon: term.icon };
      }
      const d = BUILTIN_PANES.find((p) => p.id === id)!;
      return { title: d.title, icon: d.icon };
    },
    [pluginPaneById, pluginIdentityById],
  );

  const tabAvailable = useCallback(
    (id: string) => !id.startsWith("plugin:") || pluginPaneById.has(id),
    [pluginPaneById],
  );
  const renderGroups = useCallback(
    (dock: DockLocation): DockGroupView[] =>
      dockGroups(paneLayout, dock)
        .map((g, group) => {
          const tabs = g.tabs.filter(tabAvailable);
          const active = g.active && tabs.includes(g.active) ? g.active : (tabs[0] ?? null);
          return { group, tabs, active };
        })
        .filter((g) => g.tabs.length > 0),
    [paneLayout, tabAvailable],
  );

  const availableRightGroups = useMemo(() => renderGroups("right"), [renderGroups]);
  const rightDockExplicitlyCollapsed = isDockCollapsed(paneLayout, "right");
  const rightDockCollapsed = rightDockExplicitlyCollapsed || availableRightGroups.length === 0;
  const rightGroups = useMemo(
    () => (rightDockExplicitlyCollapsed ? [] : availableRightGroups),
    [rightDockExplicitlyCollapsed, availableRightGroups],
  );
  const bottomGroups = useMemo(() => renderGroups("bottom"), [renderGroups]);
  const groupsByDock = useMemo(
    () => ({
      right: rightGroups.map((g) => ({ group: g.group, tabs: g.tabs })),
      bottom: bottomGroups.map((g) => ({ group: g.group, tabs: g.tabs })),
    }),
    [rightGroups, bottomGroups],
  );
  const terminalOpen = (["right", "bottom"] as DockLocation[]).some(
    (d) => !isDockCollapsed(paneLayout, d) && dockTabs(paneLayout, d).some(isTerminalTabId),
  );

  const isPaneOpen = (kind: string): boolean => {
    if (kind === "terminal") return terminalOpen;
    const dock = dockOf(paneLayout, kind);
    return dock !== null && !isDockCollapsed(paneLayout, dock);
  };
  const togglePaneAny = useCallback(
    (kind: string) => {
      const defaultDock: DockLocation =
        pluginPaneById.get(kind)?.defaultDock ?? BUILTIN_PANES.find((p) => p.id === kind)?.defaultDock ?? "right";
      if (isPluginPaneId(kind)) togglePlugin(kind, defaultDock);
      else toggleKind(kind as "diff" | "terminal" | "agents" | "files", defaultDock);
    },
    [toggleKind, togglePlugin, pluginPaneById],
  );
  const openAgentsPane = useCallback(() => {
    const dock = dockOf(paneLayout, "agents");
    if (dock) activateTab(dock, "agents");
    else toggleKind("agents", "right");
  }, [paneLayout, activateTab, toggleKind]);
  const closePaneAny = useCallback(
    (id: string) => {
      if (isTerminalTabId(id)) {
        const idx = terminalIndexOf(id);
        if (idx >= 1) {
          if (activeSessionId) {
            void killTerminal(activeSessionId, idx).then((ok) => {
              if (ok) closeTab(id);
            });
          }
          return;
        }
      }
      closeTab(id);
    },
    [closeTab, activeSessionId],
  );
  const movePaneAny = useCallback((id: string, dock: DockLocation) => moveTab(id, dock), [moveTab]);
  const placeVisibleTab = useCallback(
    (id: string, target: DropTarget) => {
      if (target.newGroup) {
        placeTab(id, { dock: target.dock, group: target.group, newGroup: true });
        return;
      }
      const fullBase = (dockGroups(paneLayout, target.dock)[target.group]?.tabs ?? []).filter((tab) => tab !== id);
      const index = visibleToFullIndex(fullBase, target.index ?? fullBase.length, tabAvailable);
      placeTab(id, { dock: target.dock, group: target.group, index });
    },
    [paneLayout, placeTab, tabAvailable],
  );
  const isMdUp = useIsWideViewport();
  const singlePane = !isMdUp;
  const [rightPanelView, setRightPanelView] = useState<RightPanelView>("agent");
  const [pickerOpen, setPickerOpen] = useState(false);
  const [headerCollapsed, setHeaderCollapsed] = useState(false);
  const [pairedMounted, setPairedMounted] = useState(false);
  const [showSessionWizard, setShowSessionWizard] = useState(false);
  const [showHelp, setShowHelp] = useState(false);
  const tipsAutoPoppedRef = useRef(false);
  const tipsAutoPopFrameRef = useRef<number | null>(null);
  const tourSeenAtLoadRef = useRef<boolean | null>(null);
  const tips = useTips();
  const [showPalette, setShowPalette] = useState(false);
  const [paletteQuery, setPaletteQuery] = useState("");
  const [snoozeTargetId, setSnoozeTargetId] = useState<string | null>(null);
  const [showAbout, setShowAbout] = useState(false);
  const [telemetryConsentNeeded, setTelemetryConsentNeeded] = useState(false);
  const [telemetryConsentKnown, setTelemetryConsentKnown] = useState(false);
  const [sidebarOpen, setSidebarOpen] = useState(() => window.innerWidth >= 768);
  const keyboardProxyRef = useRef<HTMLTextAreaElement>(null);
  const [keyboardProxy, setKeyboardProxy] = useState<HTMLTextAreaElement | null>(null);
  const setKeyboardProxyRef = useCallback((element: HTMLTextAreaElement | null) => {
    keyboardProxyRef.current = element;
    setKeyboardProxy(element);
  }, []);

  const [serverAbout, setServerAbout] = useState<ServerAbout | null>(null);
  const caps = useMemo(() => getClientCapabilities(serverAbout), [serverAbout]);

  const activeWorkspace = useMemo(() => {
    if (!activeSessionId) return undefined;
    return workspaces.find((w) => w.sessions.some((s) => s.id === activeSessionId));
  }, [workspaces, activeSessionId]);
  const activeSession = activeWorkspace?.sessions.find((s) => s.id === activeSessionId);
  const allPaneIds: string[] = [
    ...(caps.canUseDiff ? ["diff", "files"] : []),
    ...(caps.canUseTerminal ? ["terminal"] : []),
    ...(activeSession?.view === "structured" ? ["agents"] : []),
    ...(caps.cityhall ? [] : pluginPanes.map((p) => p.id)),
  ];

  const diffPanelActive = isMdUp ? dockOf(paneLayout, "diff") !== null : rightPanelView === "diff";
  const {
    files: diffFiles,
    perRepoBases,
    warning,
    loading: diffFilesLoading,
    revision,
    refresh: refreshDiffFiles,
  } = useDiffFiles(activeSessionId, diffPanelActive);

  const diffComments = useDiffComments(activeSessionId);
  const commentsEnabled = activeSession?.view === "structured";
  const commentSendEnabled = commentsEnabled && !activeSession?.trashed_at;
  const commentSendDisabledReason = !commentsEnabled
    ? "Diff comments can only be sent from the agent view. Switch this session to the agent view first."
    : "This session is in the trash. Restore it to send comments to the agent.";
  const commentsIsMultiRepo = (activeSession?.workspace_repos.length ?? 0) > 0;
  const [sendDialogOpen, setSendDialogOpen] = useState(false);

  useEffect(() => {
    if (!commentSendEnabled) return;
    const onKey = (e: KeyboardEvent) => {
      if (!((e.metaKey || e.ctrlKey) && e.shiftKey && e.key.toLowerCase() === "s")) {
        return;
      }
      if (isInsideEditable(e.target)) return;
      if (diffComments.count === 0) return;
      e.preventDefault();
      setSendDialogOpen(true);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [commentSendEnabled, diffComments.count]);

  const unreadIndicatorEnabled = useUnreadIndicatorEnabled();
  useEffect(() => {
    if (unreadIndicatorEnabled && activeSessionId && activeSession?.unread) {
      void setSessionUnread(activeSessionId, false);
    }
  }, [unreadIndicatorEnabled, activeSessionId, activeSession?.unread]);

  const prevActiveSessionIdRef = useRef(activeSessionId);
  if (activeSessionId !== prevActiveSessionIdRef.current) {
    prevActiveSessionIdRef.current = activeSessionId;
    setRightPanelView("agent");
    setPickerOpen(false);
    setPairedMounted(false);
    setSelectedFile(null);
  }

  if (activeSessionId && diffSelectionStale(selectedFile, diffFilesLoading, diffFiles)) {
    setSelectedFile(null);
  }

  if (rightPanelView === "paired" && !pairedMounted) {
    setPairedMounted(true);
  }

  if (isPluginPaneId(rightPanelView) && !pluginPanes.some((p) => p.id === rightPanelView)) {
    setRightPanelView("agent");
  }

  useEffect(() => {
    if (!singlePane) return;
    const id = requestAnimationFrame(() => window.dispatchEvent(new Event("resize")));
    return () => cancelAnimationFrame(id);
  }, [singlePane, rightPanelView]);

  const focusKeyboardProxy = () => {
    if (window.innerWidth < 768 && navigator.maxTouchPoints > 0) {
      keyboardProxyRef.current?.focus();
    }
  };
  const closeKeyboardProxy = () => {
    if (window.innerWidth < 768 && navigator.maxTouchPoints > 0) {
      keyboardProxyRef.current?.blur();
      if (document.activeElement instanceof HTMLTextAreaElement) document.activeElement.blur();
    }
  };

  const keyboardProxySessionIdRef = useRef(activeSessionId);
  const keyboardProxyViewRef = useRef<RightPanelView>(singlePane ? rightPanelView : "agent");
  const transitionKeyboardProxy = useCallback((nextSessionId: string | null, nextView: RightPanelView) => {
    if (keyboardProxySessionIdRef.current === nextSessionId && keyboardProxyViewRef.current === nextView) return;
    keyboardProxySessionIdRef.current = nextSessionId;
    keyboardProxyViewRef.current = nextView;
    if (keyboardProxyRef.current) keyboardProxyRef.current.value = "";
    clearMobileKeyboardProxyInput();
  }, []);

  useLayoutEffect(() => {
    transitionKeyboardProxy(activeSessionId, singlePane ? rightPanelView : "agent");
  }, [activeSessionId, singlePane, rightPanelView, transitionKeyboardProxy]);

  useEffect(() => {
    const proxy = keyboardProxy;
    if (!proxy) return;
    const onBeforeInput = (e: InputEvent) => forwardTerminalBeforeInput(e, deliverMobileKeyboardProxyInput);
    proxy.addEventListener("beforeinput", onBeforeInput);
    return () => proxy.removeEventListener("beforeinput", onBeforeInput);
  }, [keyboardProxy]);

  const isCoarse = useIsCoarsePointer();
  const focusAgentInput = useCallback(
    (session: SessionResponse | undefined) => requestSessionInputFocus(session, isCoarse),
    [isCoarse],
  );

  const handleSelectSession = useCallback(
    (sessionId: string) => {
      const ws = workspaces.find((w) => w.sessions.some((s) => s.id === sessionId));
      if (ws) {
        const picked = ws.sessions.find((s) => s.id === sessionId);
        transitionKeyboardProxy(sessionId, sessionId === activeSessionId && singlePane ? rightPanelView : "agent");
        navigate(`/session/${encodeURIComponent(sessionId)}`);
        if (isCoarse) {
          if (picked?.tool === "claude" && picked.view !== "structured") {
            closeKeyboardProxy();
          } else if (webSettings.autoOpenKeyboard) {
            focusKeyboardProxy();
            if (picked?.view === "structured") setPendingTerminalFocus("composer");
          }
        } else {
          focusKeyboardProxy();
          focusAgentInput(picked);
        }
        if (window.innerWidth < 768) setSidebarOpen(false);
      }
    },
    [
      navigate,
      workspaces,
      focusAgentInput,
      isCoarse,
      transitionKeyboardProxy,
      webSettings.autoOpenKeyboard,
      activeSessionId,
      singlePane,
      rightPanelView,
    ],
  );

  const handleSelectWorkspace = (workspaceId: string, sessionId: string | null) => {
    const ws = workspaces.find((w) => w.id === workspaceId);
    if (ws) {
      const picked = ws.sessions.find((s) => s.id === sessionId);
      if (picked) {
        transitionKeyboardProxy(picked.id, picked.id === activeSessionId && singlePane ? rightPanelView : "agent");
        navigate(`/session/${encodeURIComponent(picked.id)}`);
        if (isCoarse) {
          if (picked.tool === "claude" && picked.view !== "structured") {
            closeKeyboardProxy();
          } else if (webSettings.autoOpenKeyboard) {
            focusKeyboardProxy();
            if (picked.view === "structured") setPendingTerminalFocus("composer");
          }
        } else {
          focusKeyboardProxy();
          focusAgentInput(picked);
        }
      } else {
        transitionKeyboardProxy(null, "agent");
        navigate("/");
      }
    }
    if (window.innerWidth < 768) {
      setSidebarOpen(false);
    }
  };

  useEffect(() => {
    const onOpen = (e: Event) => {
      const detail = (e as CustomEvent).detail as { sessionId?: string } | undefined;
      if (detail?.sessionId) {
        handleSelectSession(detail.sessionId);
      }
    };
    window.addEventListener(OPEN_SESSION_EVENT, onOpen);
    return () => window.removeEventListener(OPEN_SESSION_EVENT, onOpen);
  }, [handleSelectSession]);

  const [wizardPrefill, setWizardPrefill] = useState<WizardPrefill | undefined>(undefined);
  const [deletingWorkspaceId, setDeletingWorkspaceId] = useState<string | null>(null);
  const [stoppingWorkspaceId, setStoppingWorkspaceId] = useState<string | null>(null);
  const [switchViewTarget, setSwitchViewTarget] = useState<{ sessionId: string; toStructured: boolean } | null>(null);
  const [serverAboutLoaded, setServerAboutLoaded] = useState(false);

  const refreshServerAbout = useCallback(async () => {
    try {
      const about = await fetchAbout();
      if (about) setServerAbout(about);
    } finally {
      setServerAboutLoaded(true);
    }
  }, []);

  useEffect(() => {
    let active = true;
    void fetchAbout()
      .then((about) => {
        if (!active) return;
        if (about) setServerAbout(about);
        if (about && !about.read_only) reportTelemetrySeen("web");
      })
      .finally(() => {
        if (active) setServerAboutLoaded(true);
      });
    void fetchTelemetryStatus()
      .then((status) => {
        if (!active || !status) return;
        if (!status.responded && !status.do_not_track) {
          setTelemetryConsentNeeded(true);
        }
      })
      .finally(() => {
        if (active) setTelemetryConsentKnown(true);
      });
    return () => {
      active = false;
    };
  }, []);

  useEffect(() => {
    if (!serverAboutLoaded || serverAbout?.read_only) return;
    if (activeSession?.view !== "structured") return;
    reportTelemetrySeen("structured_view");
  }, [serverAboutLoaded, serverAbout?.read_only, activeSession?.view]);

  const handleTelemetryConsent = useCallback((enabled: boolean) => {
    setTelemetryConsentNeeded(false);
    void setTelemetryConsent(enabled);
  }, []);

  const deletingWorkspace = deletingWorkspaceId ? workspaces.find((w) => w.id === deletingWorkspaceId) : null;
  const deletingSessions = deletingWorkspace?.sessions ?? [];
  const liveDeletingSessions = deletingSessions.filter((session) => !session.trashed_at);
  const deletingSession = deletingWorkspace?.sessions[0] ?? null;
  const deletingDefaultToTrash = liveDeletingSessions.some((session) => session.cleanup_defaults.delete_to_trash);
  const deletingCleanupDefaults = deletingSession
    ? {
        delete_to_trash: deletingDefaultToTrash,
        ...workspaceCleanupDefaults(deletingSessions),
      }
    : null;
  const deletingBranchName =
    deletingSessions.find((session) => session.branch)?.branch ?? deletingSession?.branch ?? null;

  const handleDeleteSession = useCallback((workspaceId: string) => {
    setDeletingWorkspaceId(workspaceId);
  }, []);

  const handleConfirmDelete = async (options: DeleteSessionOptions) => {
    if (!deletingWorkspace) return;
    const sessions = deletingWorkspace.sessions;
    setDeletingWorkspaceId(null);
    await deleteWorkspaceSessions(sessions, options, activeSessionId, {
      setStatus: setSessionStatus,
      purgeLocal: (id) => {
        clearAcpCache(id);
        clearDraft(id);
        clearStoredComments(id);
      },
      navigateHome: () => navigate("/"),
      notify: toastBus.handler,
    });
  };

  const emptyingTrashRef = useRef(false);
  const handleEmptyTrash = useCallback(async () => {
    if (trashedWorkspaces.length === 0 || emptyingTrashRef.current) return;
    emptyingTrashRef.current = true;
    let anyFailed = false;
    const notify: Notifier = {
      error: () => {
        anyFailed = true;
      },
      info: () => {},
    };
    try {
      for (const ws of trashedWorkspaces) {
        await deleteWorkspaceSessions(
          ws.sessions,
          {
            ...workspaceCleanupDefaults(ws.sessions),
            force_delete: true,
          },
          activeSessionId,
          {
            setStatus: setSessionStatus,
            purgeLocal: (id) => {
              clearAcpCache(id);
              clearDraft(id);
              clearStoredComments(id);
            },
            navigateHome: () => navigate("/"),
            notify,
          },
        );
      }
    } finally {
      emptyingTrashRef.current = false;
    }
    toastBus.handler?.[anyFailed ? "error" : "info"](
      anyFailed ? "Some trashed sessions could not be deleted" : "Emptied trash",
    );
  }, [trashedWorkspaces, activeSessionId, setSessionStatus, navigate]);

  const handleConfirmTrash = async () => {
    if (!deletingWorkspace) return;
    const ids = deletingWorkspace.sessions.map((s) => s.id);
    if (ids.length === 0) return;
    const wasActive = activeSessionId != null && ids.includes(activeSessionId);

    setDeletingWorkspaceId(null);
    for (const id of ids) setSessionStatus(id, "Stopped");
    if (wasActive) {
      navigate("/");
    }

    await trashSessions(ids, {
      applySession,
      onError: (id) => setSessionStatus(id, "Error"),
      notify: toastBus.handler,
    });
  };

  const handleRestoreSession = useCallback(
    (sessionIds: string[]) => restoreSessions(sessionIds, { applySession, notify: toastBus.handler }),
    [applySession],
  );

  const stoppingWorkspace = stoppingWorkspaceId ? workspaces.find((w) => w.id === stoppingWorkspaceId) : null;
  const stoppingSession = stoppingWorkspace?.sessions[0] ?? null;

  const handleStopSession = useCallback((workspaceId: string) => {
    setStoppingWorkspaceId(workspaceId);
  }, []);

  const handleConfirmStop = useCallback(async () => {
    if (!stoppingSession) return;
    const sessionId = stoppingSession.id;

    setStoppingWorkspaceId(null);
    setSessionStatus(sessionId, "Stopped");

    const result = await stopSession(sessionId);
    if (!result) {
      setSessionStatus(sessionId, "Error");
      toastBus.handler?.error("Failed to stop session");
      return;
    }
    toastBus.handler?.info("Session stopped");
  }, [stoppingSession, setSessionStatus]);

  const switchViewSession = switchViewTarget
    ? (workspaces.flatMap((w) => w.sessions).find((s) => s.id === switchViewTarget.sessionId) ?? null)
    : null;

  const handleSwitchView = useCallback((sessionId: string, toStructured: boolean) => {
    setSwitchViewTarget({ sessionId, toStructured });
  }, []);

  const handleConfirmSwitchView = useCallback(async () => {
    if (!switchViewTarget) return;
    const { sessionId, toStructured } = switchViewTarget;
    const result = toStructured ? await acpEnable(sessionId) : await acpDisable(sessionId);
    setSwitchViewTarget(null);
    if (!result) {
      toastBus.handler?.error(`Failed to switch to ${toStructured ? "structured view" : "terminal"}`);
      return;
    }
    toastBus.handler?.info(`Switched to ${toStructured ? "structured view" : "terminal"}`);
  }, [switchViewTarget]);

  const handleStartSession = useCallback(
    async (workspaceId: string) => {
      const ws = workspaces.find((w) => w.id === workspaceId);
      const session = ws?.sessions[0];
      if (!session) return;

      setSessionStatus(session.id, "Starting");
      const result = await startSession(session.id);
      if (!result) {
        setSessionStatus(session.id, "Error");
        toastBus.handler?.error("Failed to start session");
        return;
      }
      toastBus.handler?.info("Session started");
    },
    [workspaces, setSessionStatus],
  );

  const handleCreateSession = useCallback(
    (repoPath: string) => {
      const projectSessions = sessions
        .filter((s) => (s.main_repo_path || s.project_path) === repoPath)
        .sort((a, b) => (b.last_accessed_at ?? "").localeCompare(a.last_accessed_at ?? ""));
      const latest = projectSessions[0];

      setWizardPrefill({
        path: repoPath,
        tool: latest?.tool ?? "claude",
        yoloMode: latest?.yolo_mode ?? false,
        sandboxEnabled: latest?.is_sandboxed ?? false,
        profile: latest?.profile || undefined,
        group: latest?.group_path || undefined,
      });
      setShowSessionWizard(true);
    },
    [sessions],
  );

  const handlePinProject = useCallback(
    async (repoPath: string) => {
      const key = normalizeProjectPathKey(repoPath);
      const existing = projects.filter((p) => normalizeProjectPathKey(p.path) === key);
      let failed: { error?: string } | undefined;
      if (existing.length > 0) {
        const results = await Promise.all(existing.map((p) => setProjectPinned(p.name, p.scope, true)));
        failed = results.find((r) => !r.ok);
      } else {
        const res = await createProject({ path: repoPath, scope: "global", pinned: true });
        if (!res.ok) failed = res;
      }
      if (failed) {
        toastBus.handler?.error(failed.error ?? "Failed to pin project");
        return;
      }
      await refreshProjects();
    },
    [projects, refreshProjects],
  );

  const handleUnpinProject = useCallback(
    async (group: SidebarGroup) => {
      const pinned = group.registeredProjects.filter((p) => p.pinned);
      const results = await Promise.all(pinned.map((p) => setProjectPinned(p.name, p.scope, false)));
      const failed = results.find((r) => !r.ok);
      if (failed) {
        toastBus.handler?.error(failed.error ?? "Failed to unpin project");
      }
      await refreshProjects();
    },
    [refreshProjects],
  );

  const [projectForm, setProjectForm] = useState<{ editProject: ProjectInfo | null } | null>(null);
  const handleAddProject = useCallback(() => setProjectForm({ editProject: null }), []);
  const handleEditProject = useCallback((project: ProjectInfo) => setProjectForm({ editProject: project }), []);

  const handleRemoveProject = useCallback(
    async (group: RepoGroup) => {
      if (!confirm(`Remove project '${group.displayName}' from the sidebar?`)) return;
      const results = await Promise.all(group.registeredProjects.map((p) => deleteProject(p.name, p.scope)));
      const failed = results.find((r) => !r.ok);
      if (failed) {
        toastBus.handler?.error(failed.error ?? "Failed to remove project");
      }
      await refreshProjects();
    },
    [refreshProjects],
  );

  const toggleDiff = useCallback(() => {
    if (isMdUp) {
      toggleKind("diff", "right");
    } else {
      setPickerOpen((o) => !o);
    }
  }, [isMdUp, toggleKind]);

  const toggleRightDock = useCallback(() => {
    if (!isMdUp) {
      setPickerOpen((o) => !o);
      return;
    }
    if (rightDockCollapsed) {
      setDockCollapsed("right", false);
      if (availableRightGroups.length === 0) {
        openTab("diff", "right");
        openTab(terminalTabId(0), "right");
      }
    } else {
      setDockCollapsed("right", true);
    }
  }, [isMdUp, rightDockCollapsed, setDockCollapsed, availableRightGroups.length, openTab]);

  const handlePickView = useCallback(
    (view: RightPanelView) => {
      transitionKeyboardProxy(activeSessionId, view);
      setRightPanelView(view);
      setPickerOpen(false);
    },
    [activeSessionId, transitionKeyboardProxy],
  );

  const handleSelectFile = useCallback((path: string, repoName?: string, line?: number) => {
    setSelectedFile({ path, repoName, line });
  }, []);

  const handleOpenFileRef = useCallback(
    (ref: FileRef) => {
      if (!activeSession) return;
      const resolved = resolveToRepoRelative(ref.path, activeSession);
      if (resolved && !activeSession.scratch) {
        setSelectedFile({
          path: resolved.relativePath,
          repoName: resolved.repoName,
          line: ref.line,
          cited: true,
        });
        return;
      }
      if (resolved || isAbsolutePath(ref.path)) {
        setSelectedFile({
          path: resolved?.relativePath ?? ref.path,
          line: ref.line,
          cited: true,
          external: true,
        });
        return;
      }
      toastBus.handler?.error(`Could not open ${ref.path}: not inside this session's repo`);
    },
    [activeSession],
  );

  const handleCloseFile = useCallback(() => {
    setSelectedFile(null);
  }, []);

  const handleGoDashboard = useCallback(() => {
    navigate("/");
    setSelectedFile(null);
  }, [navigate]);

  const handleOpenSettings = useCallback(() => {
    navigate("/settings");
    if (window.innerWidth < 768) setSidebarOpen(false);
  }, [navigate]);

  useEffect(() => {
    if (profilesMatch) navigate(`/settings/profiles${window.location.search}`, { replace: true });
  }, [profilesMatch, navigate]);

  const handleCloseSettings = useCallback(() => {
    if (activeSessionId) {
      navigate(`/session/${encodeURIComponent(activeSessionId)}`);
    } else {
      navigate("/");
    }
  }, [navigate, activeSessionId]);

  const handleOpenHelp = useCallback(() => {
    setShowHelp(true);
  }, []);

  const handleOpenAbout = useCallback(() => {
    setShowAbout(true);
  }, []);

  const handleToggleSidebar = useCallback(() => {
    setSidebarOpen((o) => !o);
  }, []);

  const openSidebar = useCallback(() => setSidebarOpen(true), []);
  const openDiff = useCallback(() => {
    if (isMdUp) {
      openTab("diff", "right");
    } else {
      setPickerOpen(true);
    }
  }, [isMdUp, openTab]);
  useEdgeSwipe({
    edge: "left",
    enabled: !sidebarOpen && webSettings.sidebarSide !== "right",
    onSwipe: openSidebar,
    blurOnSwipe: true,
    anywhere: true,
  });
  useEdgeSwipe({
    edge: "right",
    enabled: rightDockCollapsed && !!activeSessionId,
    onSwipe: openDiff,
  });

  const handleNewSession = useCallback(() => {
    if (serverAbout?.read_only) return;
    setWizardPrefill(undefined);
    setShowSessionWizard(true);
  }, [serverAbout?.read_only]);

  const handleNewScratch = useCallback(() => {
    if (serverAbout?.read_only) return;
    setWizardPrefill({ scratch: true });
    setShowSessionWizard(true);
  }, [serverAbout?.read_only]);

  const handleCloneFromUrl = useCallback(() => {
    setWizardPrefill({ initialTab: "clone" });
    setShowSessionWizard(true);
  }, []);

  const handleToggleTerminalFocus = useCallback(() => {
    if (!activeSessionId) return;
    const active = document.activeElement;
    const pairedPanels = document.querySelectorAll<HTMLElement>('[data-term="paired"]');
    let inPaired = false;
    if (active) {
      for (const p of pairedPanels) {
        if (p.contains(active)) {
          inPaired = true;
          break;
        }
      }
    }
    const target = inPaired ? "agent" : "paired";

    if (singlePane) {
      setRightPanelView(target);
      if (target === "paired") setPendingTerminalFocus("paired");
      requestAnimationFrame(() => dispatchFocusTerminal(target));
      return;
    }

    if (target === "paired") {
      const terminalTabs = (["right", "bottom"] as DockLocation[])
        .flatMap((d) => dockTabs(paneLayout, d))
        .filter(isTerminalTabId);
      const termTab = terminalTabs.find((id) => isActiveTab(paneLayout, id)) ?? terminalTabs[0] ?? terminalTabId(0);
      const termDock = dockOf(paneLayout, termTab);
      if (termDock && isActiveTab(paneLayout, termTab)) {
        dispatchFocusTerminal("paired");
        return;
      }
      setPendingTerminalFocus("paired");
      if (termDock) activateTab(termDock, termTab);
      else openTab(termTab, "right");
      return;
    }
    if (target === "agent" && selectedFilePath) {
      setSelectedFile(null);
      requestAnimationFrame(() => dispatchFocusTerminal("agent"));
      return;
    }
    dispatchFocusTerminal(target);
  }, [activeSessionId, singlePane, paneLayout, openTab, activateTab, selectedFilePath]);

  const attentionJump = useMemo(() => {
    const orderedIds: string[] = [];
    const attention = new Set<string>();
    for (const g of sidebarGroups) {
      for (const v of g.workspaces) {
        for (const s of v.workspace.sessions) {
          orderedIds.push(s.id);
          if (sessionNeedsAttention(s)) attention.add(s.id);
        }
      }
    }
    return { orderedIds, attention };
  }, [sidebarGroups]);

  const handleJumpToAttention = useCallback(() => {
    const next = nextAttentionSessionId(attentionJump.orderedIds, attentionJump.attention, activeSessionId);
    if (next) handleSelectSession(next);
  }, [attentionJump, activeSessionId, handleSelectSession]);

  useKeyboardShortcuts(
    useCallback(
      () => ({
        onNew: handleNewSession,
        onJumpToAttention: handleJumpToAttention,
        onNewScratch: handleNewScratch,
        onDiff: () => toggleDiff(),
        onEscape: () => {
          if (deletingWorkspaceId) {
            setDeletingWorkspaceId(null);
            return;
          }
          if (stoppingWorkspaceId) {
            setStoppingWorkspaceId(null);
            return;
          }
          if (showPalette) {
            setShowPalette(false);
            setPaletteQuery("");
            return;
          }
          setShowSessionWizard(false);
          setShowHelp(false);
          if (showSettings) handleCloseSettings();
          setShowAbout(false);
          setSelectedFile(null);
        },
        onHelp: () => setShowHelp((h) => !h),
        onSettings: () => (showSettings ? handleCloseSettings() : navigate("/settings")),
        onPalette: () => {
          setPaletteQuery("");
          setShowPalette((p) => !p);
        },
        onToggleSidebar: () => setSidebarOpen((o) => !o),
        onToggleRightPanel: () => toggleRightDock(),
        onToggleTerminalFocus: handleToggleTerminalFocus,
      }),
      [
        toggleDiff,
        toggleRightDock,
        showPalette,
        deletingWorkspaceId,
        stoppingWorkspaceId,
        showSettings,
        handleCloseSettings,
        navigate,
        handleToggleTerminalFocus,
        handleNewSession,
        handleNewScratch,
        handleJumpToAttention,
      ],
    ),
  );

  const handleSessionStateAction = useCallback(
    async (id: string, action: SessionStateAction) => {
      if (action === "snooze") {
        setSnoozeTargetId(id);
        return;
      }
      const run: Record<Exclude<SessionStateAction, "snooze">, () => Promise<SessionResponse | null>> = {
        pin: () => setSessionPin(id, true),
        unpin: () => setSessionPin(id, false),
        archive: () => setSessionArchive(id, true),
        unarchive: () => setSessionArchive(id, false),
        unsnooze: () => setSessionSnooze(id, null),
        trash: () => trashSession(id),
        untrash: () => restoreSession(id),
      };
      const result = await run[action]();
      if (result) applySession(result);
      else reportError(`Failed to ${action} session`);
    },
    [applySession],
  );

  const commandActions = useCommandActions({
    sessions,
    activeSessionId,
    activeSession: activeSession ?? null,
    loginRequired,
    hasActiveSession: !!activeSession,
    readOnly: !!serverAbout?.read_only,
    onNewSession: handleNewSession,
    onNewScratch: handleNewScratch,
    onSelectSession: handleSelectSession,
    onJumpToAttention: handleJumpToAttention,
    hasAttentionSession: attentionJump.attention.size > 0,
    onSessionStateAction: handleSessionStateAction,
    onToggleDiff: toggleDiff,
    onOpenSettings: handleOpenSettings,
    onOpenHelp: handleOpenHelp,
    onOpenAbout: handleOpenAbout,
    onGoDashboard: handleGoDashboard,
    onToggleSidebar: handleToggleSidebar,
    onLogout,
  });

  const openSettingsTab = useCallback((tab: string) => navigate(`/settings/${tab}`), [navigate]);
  const settingsCommands = useSettingsCommands({
    open: showPalette,
    readOnly: !!serverAbout?.read_only,
    onOpenSettingsTab: openSettingsTab,
  });
  const { actions: pluginCommandActions, overlay: pluginLinkPicker } = usePluginCommands(
    pluginUiEntries,
    activeSessionId,
  );

  const { results: conversationHits, loading: conversationSearching } = useConversationSearch(paletteQuery);
  const conversationActions = useMemo(
    () =>
      buildConversationActions(conversationHits, sessions, activeSessionId).map(({ sessionId, ...rest }) => ({
        ...rest,
        perform: () => handleSelectSession(sessionId),
      })),
    [conversationHits, sessions, activeSessionId, handleSelectSession],
  );

  const renderContent = () => {
    if (showSettings) {
      return (
        <SettingsView
          tab={settingsTab}
          onClose={handleCloseSettings}
          onSelectTab={(t) => {
            const p = searchParams.get("profile");
            navigate(`/settings/${t}${p ? `?profile=${encodeURIComponent(p)}` : ""}`);
          }}
          onServerAboutRefresh={refreshServerAbout}
          onSettingsRefresh={onSettingsRefresh}
          profile={searchParams.get("profile")}
          onSelectProfile={(p) => {
            const next = new URLSearchParams(searchParams);
            next.set("profile", p);
            setSearchParams(next, { replace: true });
          }}
          readOnly={serverAbout?.read_only}
          cityhall={caps.cityhall}
        />
      );
    }

    if (activeSessionId && !sessionsLoaded) {
      return <MainPaneSkeleton />;
    }

    if (!activeWorkspace || !activeSession) {
      return (
        <Dashboard
          sessions={sessions}
          onSelectSession={handleSelectSession}
          onNewSession={handleNewSession}
          onCloneFromUrl={handleCloneFromUrl}
          onToggleSidebar={handleToggleSidebar}
          readOnly={serverAbout?.read_only}
          canManageProjects={caps.canManageProjects}
        />
      );
    }

    if (singlePane) {
      return (
        <MobileMainPane
          view={rightPanelView}
          pluginPanes={pluginPanes}
          onBackToAgent={() => handlePickView("agent")}
          pairedMounted={pairedMounted}
          activeSession={activeSession ?? null}
          activeSessionId={activeSessionId}
          sessions={sessions}
          webSettings={webSettings}
          selectedFilePath={selectedFilePath}
          selectedRepoName={selectedRepoName}
          selectedFileLine={selectedFileLine}
          revision={revision}
          diffFiles={diffFiles}
          perRepoBases={perRepoBases}
          warning={warning}
          diffFilesLoading={diffFilesLoading}
          onSelectFile={handleSelectFile}
          onOpenFileRef={handleOpenFileRef}
          onCloseFile={handleCloseFile}
          onDiffRefresh={refreshDiffFiles}
          commentsEnabled={commentsEnabled}
          commentSendEnabled={commentSendEnabled}
          commentSendDisabledReason={commentSendDisabledReason}
          diffComments={diffComments}
          commentsIsMultiRepo={commentsIsMultiRepo}
          sendDialogOpen={sendDialogOpen}
          onOpenSendDialog={() => setSendDialogOpen(true)}
          onCloseSendDialog={() => setSendDialogOpen(false)}
          onClearSelectedFile={() => setSelectedFile(null)}
        />
      );
    }

    const renderPaneBody = (id: string): ReactNode => {
      const plugin = pluginPaneById.get(id);
      if (plugin) return <PluginPaneBody entry={plugin.entry} />;
      if (id === "agents") {
        return <BackgroundAgentsPanel sessionId={activeSessionId} />;
      }
      if (id === "files") {
        return <FilesPane key={activeSessionId ?? "none"} sessionId={activeSessionId} />;
      }
      if (id === "diff") {
        return (
          <DiffPane
            session={activeSession ?? null}
            sessionId={activeSessionId}
            files={diffFiles}
            perRepoBases={perRepoBases}
            warning={warning}
            filesLoading={diffFilesLoading}
            selectedFilePath={selectedFilePath}
            selectedRepoName={selectedRepoName}
            onSelectFile={handleSelectFile}
            onDiffRefresh={refreshDiffFiles}
            commentsEnabled={commentsEnabled}
            commentsCount={diffComments.count}
            commentsSendEnabled={commentSendEnabled}
            commentsSendDisabledReason={commentSendDisabledReason}
            onOpenSendDialog={() => setSendDialogOpen(true)}
            onDiscardAllComments={diffComments.clearComments}
          />
        );
      }
      return (
        <PairedShellPane
          session={activeSession ?? null}
          sessionId={activeSessionId}
          terminalIndex={isTerminalTabId(id) ? terminalIndexOf(id) : 0}
        />
      );
    };
    return (
      <div className="flex-1 flex flex-col min-h-0">
        <PaneDndController groupsByDock={groupsByDock} descriptorFor={paneDescriptor} onPlaceTab={placeVisibleTab}>
          <ContentSplit
            collapsed={rightDockCollapsed}
            onToggleCollapse={toggleRightDock}
            left={
              <div className="flex-1 flex flex-col min-h-0 overflow-hidden relative">
                <div className={selectedFilePath ? "hidden" : "flex-1 flex flex-col min-h-0 overflow-hidden"}>
                  {activeSession?.view === "structured" ? (
                    <Suspense fallback={<AcpLoadingFallback />}>
                      <StructuredView
                        key={activeSessionId}
                        sessionId={activeSessionId!}
                        acpWorkerState={activeSession.acp_worker_state ?? "absent"}
                        rateLimitAutoResume={activeSession.rate_limit_auto_resume}
                        tool={activeSession.tool}
                        acpAgent={activeSession.acp_agent ?? null}
                        clearAliases={activeSession.clear_aliases}
                        archivedAt={activeSession.archived_at ?? null}
                        snoozedUntil={activeSession.snoozed_until ?? null}
                        trashedAt={activeSession.trashed_at ?? null}
                        onRestore={
                          activeSession.trashed_at
                            ? () => handleRestoreSession(trashedWorkspaceRestoreIds(workspaces, activeSessionId!))
                            : undefined
                        }
                        onOpenFileRef={handleOpenFileRef}
                        fileRefSession={activeSession}
                        onOpenAgentsPane={openAgentsPane}
                        isSandboxed={activeSession.is_sandboxed}
                      />
                    </Suspense>
                  ) : (
                    <TerminalSessionStack
                      activeSessionId={activeSessionId!}
                      sessions={sessions.filter((session) => session.view !== "structured")}
                      persistent={webSettings.persistentTerminals}
                      maxPersistentTerminals={webSettings.maxPersistentTerminals}
                    />
                  )}
                </div>

                {selectedFilePath &&
                  activeSessionId &&
                  (selectedFileExternal ? (
                    <FileContentViewer
                      sessionId={activeSessionId}
                      filePath={selectedFilePath}
                      onBack={handleCloseFile}
                    />
                  ) : (
                    <DiffFileViewer
                      sessionId={activeSessionId}
                      filePath={selectedFilePath}
                      repoName={selectedRepoName}
                      targetLine={selectedFileLine}
                      revision={revision}
                      onClose={handleCloseFile}
                      commentsEnabled={commentsEnabled}
                      commentsStore={diffComments}
                    />
                  ))}
              </div>
            }
            right={
              <div {...tourAnchor(TOUR_ANCHORS.rightPanel)} className="flex min-h-0 min-w-0 flex-1">
                <DockGroups
                  location="right"
                  groups={rightGroups}
                  descriptorFor={paneDescriptor}
                  renderBody={renderPaneBody}
                  onActivate={(id) => activateTab("right", id)}
                  onMove={movePaneAny}
                  onClose={closePaneAny}
                  onNewTerminal={serverAbout?.read_only ? undefined : () => addTerminal("right")}
                />
              </div>
            }
          />
          {bottomGroups.length > 0 && (
            <BottomDock
              groups={bottomGroups}
              descriptorFor={paneDescriptor}
              renderBody={renderPaneBody}
              onActivate={(id) => activateTab("bottom", id)}
              onMove={movePaneAny}
              onClose={closePaneAny}
              onNewTerminal={serverAbout?.read_only ? undefined : () => addTerminal("bottom")}
            />
          )}
        </PaneDndController>
        {sendDialogOpen && commentsEnabled && activeSessionId && (
          <SendCommentsDialog
            sessionId={activeSessionId}
            comments={diffComments.comments}
            isMultiRepo={commentsIsMultiRepo}
            sendEnabled={commentSendEnabled}
            sendDisabledReason={commentSendDisabledReason}
            introDraft={diffComments.introDraft}
            outroDraft={diffComments.outroDraft}
            clearAfterSend={diffComments.clearAfterSend}
            onChangeIntro={diffComments.setIntroDraft}
            onChangeOutro={diffComments.setOutroDraft}
            onChangeClearAfterSend={diffComments.setClearAfterSend}
            onClose={() => setSendDialogOpen(false)}
            onSent={() => {
              if (diffComments.clearAfterSend) {
                diffComments.clearComments();
                diffComments.setIntroDraft("");
                diffComments.setOutroDraft("");
              }
              setSendDialogOpen(false);
              setSelectedFile(null);
              toastBus.handler?.info("Comments sent to agent");
            }}
          />
        )}
      </div>
    );
  };

  const acpPrefs = useMemo(
    () => ({
      showToolDurations: serverAbout?.acp_show_tool_durations ?? true,
      replayEvents: serverAbout?.acp_replay_events ?? 0,
      compactionReminder: serverAbout?.acp_compaction_reminder ?? false,
      compactionReminderPercent: serverAbout?.acp_compaction_reminder_percent ?? 75,
    }),
    [
      serverAbout?.acp_show_tool_durations,
      serverAbout?.acp_replay_events,
      serverAbout?.acp_compaction_reminder,
      serverAbout?.acp_compaction_reminder_percent,
    ],
  );

  const tourScope: TourScope =
    !activeWorkspace || !activeSession
      ? "dashboard"
      : activeSession.view === "structured"
        ? "structured-view"
        : "session";
  const [tourSeen, setTourSeen] = useState(false);
  const [tourSeenKnown, setTourSeenKnown] = useState(false);

  useEffect(() => {
    fetchSettings().then((settings) => {
      if (!settings) return;
      const backendSeen = settings.app_state?.has_seen_web_tour === true;
      const legacySeen = safeGetItem(LEGACY_TOUR_SEEN_KEY) === "1";
      const seenAtLoad = backendSeen || legacySeen;
      setTourSeen(seenAtLoad);
      setTourSeenKnown(true);
      tourSeenAtLoadRef.current = seenAtLoad;
      if (legacySeen && !backendSeen) {
        void markWebTourSeen().then((ok) => {
          if (ok) safeRemoveItem(LEGACY_TOUR_SEEN_KEY);
        });
      }
    });
  }, []);

  const handleTourSeen = useCallback(() => {
    setTourSeen(true);
    void markWebTourSeen();
  }, []);

  const tourAutoLaunchReady =
    serverAboutLoaded &&
    sessionsLoaded &&
    !activeSessionId &&
    !showSettings &&
    !showSessionWizard &&
    !showHelp &&
    !showAbout &&
    !showPalette &&
    !projectForm;
  const welcome = useWelcomePhase({
    scope: tourScope,
    readOnly: !!serverAbout?.read_only,
    autoLaunchReady: tourAutoLaunchReady,
    tourSeen,
    tourSeenKnown,
  });
  const tour = useTour({
    scope: tourScope,
    readOnly: !!serverAbout?.read_only,
    cityhall: caps.cityhall,
    isDesktop: !isCoarse,
    autoLaunchReady: tourAutoLaunchReady && welcome.resolved,
    seen: tourSeen,
    seenKnown: tourSeenKnown,
    onSeen: handleTourSeen,
    onNavigate: (tab) => (tab ? navigate(`/settings/${tab}`) : handleCloseSettings()),
  });

  useEffect(() => {
    if (tipsAutoPoppedRef.current) return;
    const gate = shouldAutoPopTips({
      loaded: tips.loaded,
      hasUnseen: tips.hasUnseen,
      tourSeenAtLoad: tourSeenAtLoadRef.current,
      onboardingReady: tourAutoLaunchReady && welcome.resolved,
      telemetryPending: !telemetryConsentKnown || telemetryConsentNeeded,
      tourActive: tour.isTourActive,
      automated: isAutomatedSession(),
    });
    if (!gate) return;
    tipsAutoPoppedRef.current = true;
    tipsAutoPopFrameRef.current = requestAnimationFrame(() => tips.open());
  }, [
    tips,
    tourSeenKnown,
    tourAutoLaunchReady,
    welcome.resolved,
    telemetryConsentKnown,
    telemetryConsentNeeded,
    tour.isTourActive,
  ]);

  useEffect(
    () => () => {
      if (tipsAutoPopFrameRef.current !== null) {
        cancelAnimationFrame(tipsAutoPopFrameRef.current);
      }
    },
    [],
  );

  if (!serverAboutLoaded) {
    return <div className="h-dvh bg-surface-900 safe-area-inset" />;
  }

  const headerCollapsible =
    singlePane &&
    !showSettings &&
    !!activeWorkspace &&
    activeSession?.view === "structured" &&
    rightPanelView === "agent";

  return (
    <AcpPrefsProvider value={acpPrefs}>
      <div className="h-dvh flex flex-col bg-surface-900 text-text-primary overflow-hidden safe-area-inset">
        <CollapsibleRegion id="conversation-header" collapsed={headerCollapsible && headerCollapsed}>
          <TopBar
            activeWorkspace={activeWorkspace}
            activeSession={activeSession ?? null}
            onToggleSidebar={handleToggleSidebar}
            onOpenPalette={() => setShowPalette(true)}
            onToggleDiff={toggleDiff}
            paneIds={allPaneIds}
            paneDescriptor={paneDescriptor}
            isPaneOpen={isPaneOpen}
            onTogglePane={togglePaneAny}
            onOpenHelp={handleOpenHelp}
            onOpenAbout={handleOpenAbout}
            onStartTutorial={tour.startTour}
            onLogout={onLogout}
            loginRequired={loginRequired}
            isOffline={!!error}
            isDevBuild={isDebugBuild(serverAbout)}
            onOpenTips={tips.open}
            onGoDashboard={handleGoDashboard}
            sidebarColumnVisible={!showSettings && sidebarOpen}
            rightColumnVisible={isMdUp && !showSettings && !!activeWorkspace && !!activeSession && !rightDockCollapsed}
          />
        </CollapsibleRegion>

        <DisconnectBanner />
        <UpdateBanner />
        <DashboardUpdateBanner />

        {headerCollapsible && (
          <ChromeCollapseHandle
            edge="top"
            collapsed={headerCollapsed}
            onToggle={() => setHeaderCollapsed((v) => !v)}
            collapseLabel="Collapse conversation header"
            expandLabel="Expand conversation header"
            controlsId="conversation-header"
            testId="header-collapse-toggle"
          />
        )}

        <div className="flex flex-1 min-h-0">
          {!showSettings && (
            <WorkspaceSidebar
              groups={sidebarGroups}
              nestedGroups={nestedGroups}
              orgGroups={orgGroups}
              trashedWorkspaces={trashedWorkspaces}
              onToggleSubgroup={toggleSubgroupCollapsed}
              onToggleOrg={toggleOrgCollapsed}
              onToggleOrgRepo={toggleOrgRepoCollapsed}
              onReorderWorkspaces={handleReorderWorkspaces}
              onReorderGroups={reorderRepoGroups}
              activeId={activeWorkspace?.id ?? null}
              open={sidebarOpen}
              onToggle={() => setSidebarOpen(false)}
              onSelect={handleSelectWorkspace}
              onToggleGroup={toggleSidebarGroup}
              onUpdateRepoAppearance={updateRepoAppearance}
              onNew={() => {
                setWizardPrefill(undefined);
                setShowSessionWizard(true);
              }}
              onCreateSession={handleCreateSession}
              onPinProject={handlePinProject}
              onUnpinProject={handleUnpinProject}
              savedProjects={savedProjects}
              onAddProject={handleAddProject}
              onEditProject={handleEditProject}
              onRemoveProject={handleRemoveProject}
              onSettings={handleOpenSettings}
              onDeleteSession={handleDeleteSession}
              onRestoreSession={handleRestoreSession}
              onEmptyTrash={handleEmptyTrash}
              onStopSession={handleStopSession}
              onStartSession={handleStartSession}
              onSwitchView={handleSwitchView}
              readOnly={serverAbout?.read_only}
              canManageProjects={caps.canManageProjects}
              sortMode={sidebarSortMode}
              onSortModeChange={selectSidebarSortMode}
              pluginSortRef={pluginSortRef}
              onPluginSortChange={setPluginSortRef}
              axis={sidebarAxis}
              onAxisChange={setSidebarAxis}
            />
          )}

          <div className="flex-1 flex flex-col min-h-0 min-w-0">{renderContent()}</div>
        </div>

        {showSessionWizard && (
          <SessionWizard
            onClose={() => {
              setShowSessionWizard(false);
              setWizardPrefill(undefined);
            }}
            onCreated={(session?: SessionResponse) => {
              if (session) {
                injectSession(session);
                navigate(`/session/${encodeURIComponent(session.id)}`);
                if (window.innerWidth < 768) setSidebarOpen(false);
              }
              setShowSessionWizard(false);
              setWizardPrefill(undefined);
            }}
            prefill={wizardPrefill}
            nameOnly={caps.nameOnlyWizard}
          />
        )}

        {projectForm && (
          <ProjectFormModal
            initial={projectForm.editProject}
            onClose={() => setProjectForm(null)}
            onSaved={() => refreshProjects()}
          />
        )}

        {welcome.showWelcome && <ThemeIntro onDone={welcome.dismissWelcome} />}

        {tour.tourElement}

        {showHelp && <HelpOverlay onClose={() => setShowHelp(false)} />}

        {tips.isOpen && (
          <TipsModal
            tips={tips.tips}
            startIndex={tips.startIndex}
            enabled={tips.enabled}
            onMarkSeen={tips.markSeen}
            onSetEnabled={tips.setEnabled}
            onClose={tips.close}
          />
        )}

        {showAbout && <AboutModal onClose={() => setShowAbout(false)} sessionId={activeSessionId} />}
        {telemetryConsentNeeded && <TelemetryConsentModal onChoose={handleTelemetryConsent} />}

        {deletingSession && deletingCleanupDefaults && (
          <DeleteSessionDialog
            sessionTitle={deletingSession.title}
            branchName={deletingBranchName}
            hasManagedWorktree={deletingSessions.some((session) => session.has_cleanable_worktree ?? false)}
            isSandboxed={deletingSessions.some((session) => session.is_sandboxed)}
            isScratch={deletingSessions.some((session) => session.scratch)}
            cleanupDefaults={deletingCleanupDefaults}
            defaultToTrash={deletingDefaultToTrash}
            affectedSessions={deletingSessions.map((session) => ({
              id: session.id,
              title: session.title,
              isSandboxed: session.is_sandboxed,
            }))}
            onConfirm={handleConfirmDelete}
            onTrash={handleConfirmTrash}
            onCancel={() => setDeletingWorkspaceId(null)}
          />
        )}

        {stoppingSession && (
          <StopSessionDialog
            sessionTitle={stoppingSession.title}
            onConfirm={handleConfirmStop}
            onCancel={() => setStoppingWorkspaceId(null)}
          />
        )}

        {switchViewTarget && switchViewSession && (
          <SwitchViewDialog
            sessionTitle={switchViewSession.title}
            toStructured={switchViewTarget.toStructured}
            keepsContext={switchViewSession.keeps_context ?? false}
            onConfirm={handleConfirmSwitchView}
            onCancel={() => setSwitchViewTarget(null)}
          />
        )}

        <CommandPalette
          open={showPalette}
          onClose={() => {
            setShowPalette(false);
            setPaletteQuery("");
          }}
          actions={[...commandActions, ...conversationActions, ...settingsCommands, ...pluginCommandActions]}
          onSearchChange={setPaletteQuery}
          searching={conversationSearching}
        />

        {pluginLinkPicker}

        {snoozeTargetId && (
          <SnoozeModal
            title="Snooze session"
            onCancel={() => setSnoozeTargetId(null)}
            onPick={(minutes) => {
              const id = snoozeTargetId;
              setSnoozeTargetId(null);
              void setSessionSnooze(id, minutes).then((result) => {
                if (result) applySession(result);
                else reportError("Failed to snooze session");
              });
            }}
          />
        )}

        {activeWorkspace && activeSession && (
          <MobileRightPanelPicker
            open={pickerOpen && singlePane}
            active={rightPanelView}
            pluginPanes={pluginPanes}
            onSelect={handlePickView}
            onClose={() => setPickerOpen(false)}
          />
        )}

        <textarea
          ref={setKeyboardProxyRef}
          data-keyboard-proxy
          aria-hidden="true"
          tabIndex={-1}
          className="fixed bottom-0 left-0 w-px h-px opacity-0 pointer-events-none"
          style={{ caretColor: "transparent", color: "transparent" }}
          autoCapitalize="off"
          autoCorrect="off"
          autoComplete="off"
          spellCheck={false}
        />
      </div>
    </AcpPrefsProvider>
  );
}

function AcpLoadingFallback() {
  return (
    <div className="flex h-full items-center justify-center bg-surface-900 text-text-dim">
      <div className="text-xs font-mono uppercase tracking-wide">Loading acp…</div>
    </div>
  );
}
