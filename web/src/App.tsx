import { lazy, Suspense, useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { useMatch, useNavigate, useSearchParams } from "react-router-dom";
import { IDLE_DECAY_WINDOW_MS } from "./lib/session";
import { diffSelectionStale } from "./lib/diffSelection";
import { useSessions } from "./hooks/useSessions";
import { useDashboardPresence } from "./hooks/useDashboardPresence";
import { sweepOrphanDrafts } from "./lib/acpDrafts";
import { AcpPrefsProvider } from "./lib/acpPrefs";
import { useWorkspaces } from "./hooks/useWorkspaces";
import { useLastSessionRestore } from "./hooks/useLastSessionRestore";
import { PluginUiProvider, usePluginUiEntries } from "./lib/pluginUiContext";
import { nextAttentionSessionId, workspaceIsTrashed } from "./lib/sidebarSort";
import { useProjects } from "./hooks/useProjects";
import { useKeyboardShortcuts } from "./hooks/useKeyboardShortcuts";
import { useResolvedTheme } from "./hooks/useResolvedTheme";
import { useWebSettings } from "./hooks/useWebSettings";
import { useDiffFiles } from "./hooks/useDiffFiles";
import { useDiffComments } from "./hooks/useDiffComments";
import { sweepOrphanComments } from "./components/diff/comments/storage";
import { SendCommentsDialog } from "./components/diff/comments/SendCommentsDialog";
import { useCommandActions, buildConversationActions, type SessionStateAction } from "./hooks/useCommandActions";
import { usePluginCommands } from "./hooks/usePluginCommands";
import { useSettingsCommands } from "./hooks/useSettingsCommands";
import { useEdgeSwipe } from "./hooks/useEdgeSwipe";
import { useIsCoarsePointer } from "./hooks/useIsCoarsePointer";
import { useMobileViewportLock } from "./hooks/useMobileViewportLock";
import { useIsWideViewport } from "./hooks/useIsWideViewport";
import { listen } from "./hooks/domEvents";
import { useAppPanes } from "./hooks/app/useAppPanes";
import { useKeyboardProxy } from "./hooks/app/useKeyboardProxy";
import { useOnboarding } from "./hooks/app/useOnboarding";
import { useProjectActions } from "./hooks/app/useProjectActions";
import { useServerAbout } from "./hooks/app/useServerAbout";
import { useSessionLifecycle } from "./hooks/app/useSessionLifecycle";
import { useSidebarModel } from "./hooks/app/useSidebarModel";
import type { RightPanelView } from "./lib/rightPanelView";
import { dockTabs, dockOf, isActiveTab } from "./lib/paneLayout";
import { isPluginPaneId } from "./lib/pluginPanes";
import { PluginPaneBody } from "./components/plugin/PluginSlots";
import { TOUR_ANCHORS, tourAnchor } from "./lib/tourSteps";
import { trashedWorkspaceRestoreIds } from "./lib/trashActions";
import {
  loginStatus,
  logout,
  fetchSettings,
  isDebugBuild,
  reportTelemetrySeen,
  setSessionUnread,
  setSessionPin,
  setSessionArchive,
  setSessionSnooze,
  trashSession,
  restoreSession,
} from "./lib/api";
import { getClientCapabilities } from "./lib/clientCapabilities";
import { IdleDecayWindowContext, parseIdleDecayWindowMs } from "./lib/idleDecay";
import { parseUnreadIndicatorEnabled, UnreadIndicatorContext, useUnreadIndicatorEnabled } from "./lib/unreadIndicator";
import { parseSessionRowTagMode, SessionRowTagContext, type SessionRowTagMode } from "./lib/sessionRowTag";
import { parseSessionColorsEnabled, SessionColorsContext } from "./lib/sessionColors";
import { toastBus, reportError } from "./lib/toastBus";
import { isAbsolutePath, resolveToRepoRelative, type FileRef } from "./lib/fileRef";
import { OPEN_SESSION_EVENT } from "./lib/sessionRoute";
import { dispatchFocusTerminal, requestSessionInputFocus, setPendingTerminalFocus } from "./lib/terminalFocus";
import { hydrateWebUiStateFromServer, initWebUiSync } from "./lib/webUiSync";
import { WorkspaceSidebar, SnoozeModal } from "./components/WorkspaceSidebar";
import { DeleteSessionDialog } from "./components/DeleteSessionDialog";
import { StopSessionDialog } from "./components/StopSessionDialog";
import { SwitchViewDialog } from "./components/SwitchViewDialog";
import { TopBar } from "./components/TopBar";
import { AppShellSkeleton, MainPaneSkeleton } from "./components/AppShellSkeleton";
import { ContentSplit } from "./components/ContentSplit";
import { TerminalSessionStack } from "./components/TerminalSessionStack";
import { DockGroups } from "./components/DockGroups";
import { BottomDock } from "./components/BottomDock";
import { PaneDndController } from "./components/PaneDndController";
import { BackgroundAgentsPanel } from "./components/acp/BackgroundAgentsPanel";
import { DiffPane } from "./components/DiffPane";
import { FilesPane } from "./components/FilesPane";
import { FileContentViewer } from "./components/diff/FileContentViewer";
import { PairedShellPane } from "./components/PairedTerminal";
import { isTerminalTabId, terminalIndexOf, terminalTabId, type DockLocation } from "./lib/panes";
import { MobileRightPanelPicker } from "./components/MobileRightPanelPicker";
import { MobileMainPane } from "./components/MobileMainPane";
import { ChromeCollapseHandle, CollapsibleRegion } from "./components/CollapsibleChrome";
import { DiffFileViewer } from "./components/diff/DiffFileViewer";
import { SettingsView } from "./components/SettingsView";
import { ProjectFormModal } from "./components/ProjectFormModal";
import { HelpOverlay } from "./components/HelpOverlay";
import { ThemeIntro } from "./components/onboarding/ThemeIntro";
import type { TourScope } from "./lib/tourSteps";
import { SessionWizard } from "./components/session-wizard/SessionWizard";
import type { WizardPrefill } from "./components/session-wizard/SessionWizard";
import type { SessionResponse } from "./lib/types";
import { Dashboard } from "./components/Dashboard";
import { LoginPage } from "./components/LoginPage";
import { TokenEntryPage } from "./components/TokenEntryPage";
import { LOGIN_REQUIRED_EVENT, TOKEN_EXPIRED_EVENT, resetTokenExpired } from "./lib/fetchInterceptor";
import { AboutModal } from "./components/AboutModal";
import { TelemetryConsentModal } from "./components/TelemetryConsentModal";
import { TipsModal } from "./components/TipsModal";
import { useTips } from "./hooks/useTips";
import { CommandPalette } from "./components/command-palette/CommandPalette";
import { useConversationSearch } from "./hooks/useConversationSearch";
import { DisconnectBanner } from "./components/DisconnectBanner";
import { ElevationPrompt } from "./components/ElevationPrompt";
import { UpdateBanner } from "./components/UpdateBanner";
import { DashboardUpdateBanner } from "./components/DashboardUpdateBanner";

const StructuredView = lazy(() =>
  import("./components/acp/StructuredView").then((m) => ({
    default: m.StructuredView,
  })),
);

const isPhoneWidth = () => window.innerWidth < 768;

interface AppSettings {
  idleDecayWindowMs: number;
  unreadIndicatorEnabled: boolean;
  sessionRowTagMode: SessionRowTagMode;
  sessionColorsEnabled: boolean;
}

const DEFAULT_APP_SETTINGS: AppSettings = {
  idleDecayWindowMs: IDLE_DECAY_WINDOW_MS,
  unreadIndicatorEnabled: true,
  sessionRowTagMode: "branch",
  sessionColorsEnabled: true,
};

function parseAppSettings(settings: Record<string, unknown> | null | undefined): AppSettings {
  return {
    idleDecayWindowMs: parseIdleDecayWindowMs(settings),
    unreadIndicatorEnabled: parseUnreadIndicatorEnabled(settings),
    sessionRowTagMode: parseSessionRowTagMode(settings),
    sessionColorsEnabled: parseSessionColorsEnabled(settings),
  };
}

export default function App() {
  useMobileViewportLock();
  useResolvedTheme();
  const [loginRequired, setLoginRequired] = useState<boolean | null>(null);
  const [loginAuthenticated, setLoginAuthenticated] = useState(true);
  const [tokenExpired, setTokenExpired] = useState(false);
  const [appSettings, setAppSettings] = useState(DEFAULT_APP_SETTINGS);

  const refreshAppSettings = useCallback(async () => {
    setAppSettings(parseAppSettings(await fetchSettings()));
  }, []);

  const refreshLoginStatus = useCallback(() => {
    loginStatus().then(({ required, authenticated }) => {
      setLoginRequired(required);
      setLoginAuthenticated(authenticated);
    });
  }, []);

  useEffect(() => {
    return listen(() => setTokenExpired(true), [window, TOKEN_EXPIRED_EVENT]);
  }, []);

  useEffect(() => {
    const onLoginRequired = () => {
      setTokenExpired(false);
      setLoginRequired(true);
      setLoginAuthenticated(false);
    };
    return listen(onLoginRequired, [window, LOGIN_REQUIRED_EVENT]);
  }, []);

  useEffect(refreshLoginStatus, [refreshLoginStatus]);

  useEffect(() => {
    fetchSettings().then((settings) => setAppSettings(parseAppSettings(settings)));
  }, []);

  if (tokenExpired) {
    return (
      <TokenEntryPage
        onSuccess={() => {
          setTokenExpired(false);
          refreshLoginStatus();
        }}
      />
    );
  }

  if (loginRequired && !loginAuthenticated) {
    return (
      <LoginPage
        onSuccess={() => {
          setLoginAuthenticated(true);
          resetTokenExpired();
        }}
      />
    );
  }

  if (loginRequired === null) {
    return <AppShellSkeleton />;
  }

  return (
    <IdleDecayWindowContext.Provider value={appSettings.idleDecayWindowMs}>
      <UnreadIndicatorContext.Provider value={appSettings.unreadIndicatorEnabled}>
        <SessionRowTagContext.Provider value={appSettings.sessionRowTagMode}>
          <SessionColorsContext.Provider value={appSettings.sessionColorsEnabled}>
            <PluginUiProvider>
              <AppContent
                loginRequired={loginRequired}
                onLogout={async () => {
                  await logout();
                  setLoginAuthenticated(false);
                }}
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
  for (let el = target instanceof HTMLElement ? target : null; el; el = el.parentElement) {
    if (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.isContentEditable) return true;
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

  // Once per page load, drop locally stored drafts and comments for sessions that no longer exist.
  const sweptOrphansRef = useRef(false);
  useEffect(() => {
    if (sweptOrphansRef.current || !sessionsLoaded) return;
    sweptOrphansRef.current = true;
    const liveIds = new Set(sessions.map((s) => s.id));
    sweepOrphanDrafts(liveIds);
    sweepOrphanComments(liveIds);
  }, [sessionsLoaded, sessions]);

  const { projects, refresh: refreshProjects } = useProjects();
  const sidebar = useSidebarModel({
    workspaces,
    workspaceOrdering,
    setWorkspaceOrdering,
    markLocalOrderingUpdate,
    projects,
  });
  const pluginUiEntries = usePluginUiEntries();

  const [selectedFile, setSelectedFile] = useState<{
    path: string;
    repoName?: string;
    line?: number;
    cited?: boolean;
    external?: boolean;
  } | null>(null);
  const selectedFilePath = selectedFile?.path ?? null;
  const selectedRepoName = selectedFile?.repoName;
  const selectedFileLine = selectedFile?.line;
  const panes = useAppPanes(activeSessionId, webSettings.autoOpenPluginPanes);
  const {
    layout: paneLayout,
    openTab,
    activateTab,
    toggleKind,
    setDockCollapsed,
    pluginPanes,
    availableRightGroups,
    rightDockCollapsed,
  } = panes;
  const isMdUp = useIsWideViewport();
  const singlePane = !isMdUp;
  const [rightPanelView, setRightPanelView] = useState<RightPanelView>("agent");
  const [pickerOpen, setPickerOpen] = useState(false);
  const [headerCollapsed, setHeaderCollapsed] = useState(false);
  const [pairedMounted, setPairedMounted] = useState(false);
  const [showSessionWizard, setShowSessionWizard] = useState(false);
  const [wizardPrefill, setWizardPrefill] = useState<WizardPrefill | undefined>(undefined);
  const [showHelp, setShowHelp] = useState(false);
  const tips = useTips();
  const [showPalette, setShowPalette] = useState(false);
  const [paletteQuery, setPaletteQuery] = useState("");
  const [snoozeTargetId, setSnoozeTargetId] = useState<string | null>(null);
  const [showAbout, setShowAbout] = useState(false);
  const [sidebarOpen, setSidebarOpen] = useState(() => !isPhoneWidth());

  const activeWorkspace = useMemo(() => {
    if (!activeSessionId) return undefined;
    return workspaces.find((w) => w.sessions.some((s) => s.id === activeSessionId));
  }, [workspaces, activeSessionId]);
  const activeSession = activeWorkspace?.sessions.find((s) => s.id === activeSessionId);

  const about = useServerAbout();
  const { serverAbout } = about;
  useEffect(() => {
    if (!about.serverAboutLoaded || serverAbout?.read_only) return;
    if (activeSession?.view !== "structured") return;
    reportTelemetrySeen("structured_view");
  }, [about.serverAboutLoaded, serverAbout?.read_only, activeSession?.view]);
  const readOnly = !!serverAbout?.read_only;
  const caps = useMemo(() => getClientCapabilities(serverAbout), [serverAbout]);
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
      if (!((e.metaKey || e.ctrlKey) && e.shiftKey && e.key.toLowerCase() === "s")) return;
      if (isInsideEditable(e.target) || diffComments.count === 0) return;
      e.preventDefault();
      setSendDialogOpen(true);
    };
    return listen(onKey as EventListener, [window, "keydown"]);
  }, [commentSendEnabled, diffComments.count]);

  const unreadIndicatorEnabled = useUnreadIndicatorEnabled();
  useEffect(() => {
    if (unreadIndicatorEnabled && activeSessionId && activeSession?.unread) {
      void setSessionUnread(activeSessionId, false);
    }
  }, [unreadIndicatorEnabled, activeSessionId, activeSession?.unread]);

  const [trackedSessionId, setTrackedSessionId] = useState(activeSessionId);
  if (activeSessionId !== trackedSessionId) {
    setTrackedSessionId(activeSessionId);
    setRightPanelView("agent");
    setPickerOpen(false);
    setPairedMounted(false);
    setSelectedFile(null);
  }

  if (activeSessionId && diffSelectionStale(selectedFile, diffFilesLoading, diffFiles)) {
    setSelectedFile(null);
  }

  // Keep the paired shell mounted once opened so switching views doesn't restart it.
  if (rightPanelView === "paired" && !pairedMounted) {
    setPairedMounted(true);
  }

  if (isPluginPaneId(rightPanelView) && !pluginPanes.some((p) => p.id === rightPanelView)) {
    setRightPanelView("agent");
  }

  // Terminals size themselves on resize; a single-pane view switch changes their box without one.
  useEffect(() => {
    if (!singlePane) return;
    const id = requestAnimationFrame(() => window.dispatchEvent(new Event("resize")));
    return () => cancelAnimationFrame(id);
  }, [singlePane, rightPanelView]);

  const {
    setProxyElement,
    focus: focusKeyboardProxy,
    close: closeKeyboardProxy,
    transition: transitionKeyboardProxy,
  } = useKeyboardProxy(activeSessionId, singlePane ? rightPanelView : "agent");

  const isCoarse = useIsCoarsePointer();
  const openSession = useCallback(
    (picked: SessionResponse) => {
      transitionKeyboardProxy(picked.id, picked.id === activeSessionId && singlePane ? rightPanelView : "agent");
      navigate(`/session/${encodeURIComponent(picked.id)}`);
      if (!isCoarse) {
        focusKeyboardProxy();
        requestSessionInputFocus(picked, isCoarse);
      } else if (picked.tool === "claude" && picked.view !== "structured") {
        closeKeyboardProxy();
      } else if (webSettings.autoOpenKeyboard) {
        focusKeyboardProxy();
        if (picked.view === "structured") setPendingTerminalFocus("composer");
      }
    },
    [
      navigate,
      isCoarse,
      focusKeyboardProxy,
      closeKeyboardProxy,
      transitionKeyboardProxy,
      webSettings.autoOpenKeyboard,
      activeSessionId,
      singlePane,
      rightPanelView,
    ],
  );

  const handleSelectSession = useCallback(
    (sessionId: string) => {
      const picked = workspaces.flatMap((w) => w.sessions).find((s) => s.id === sessionId);
      if (!picked) return;
      openSession(picked);
      if (isPhoneWidth()) setSidebarOpen(false);
    },
    [workspaces, openSession],
  );

  const handleSelectWorkspace = (workspaceId: string, sessionId: string | null) => {
    const ws = workspaces.find((w) => w.id === workspaceId);
    const picked = ws?.sessions.find((s) => s.id === sessionId);
    if (picked) {
      openSession(picked);
    } else if (ws) {
      transitionKeyboardProxy(null, "agent");
      navigate("/");
    }
    if (isPhoneWidth()) setSidebarOpen(false);
  };

  useEffect(() => {
    const onOpen = (e: Event) => {
      const detail = (e as CustomEvent).detail as { sessionId?: string } | undefined;
      if (detail?.sessionId) handleSelectSession(detail.sessionId);
    };
    return listen(onOpen, [window, OPEN_SESSION_EVENT]);
  }, [handleSelectSession]);

  const lifecycle = useSessionLifecycle({
    workspaces,
    trashedWorkspaces,
    activeSessionId,
    setSessionStatus,
    applySession,
    navigate,
  });
  const projectActions = useProjectActions(projects, refreshProjects);

  const openWizard = useCallback((prefill?: WizardPrefill) => {
    setWizardPrefill(prefill);
    setShowSessionWizard(true);
  }, []);

  const closeWizard = () => {
    setShowSessionWizard(false);
    setWizardPrefill(undefined);
  };

  const handleCreateSession = useCallback(
    (repoPath: string) => {
      const latest = sessions
        .filter((s) => (s.main_repo_path || s.project_path) === repoPath)
        .sort((a, b) => (b.last_accessed_at ?? "").localeCompare(a.last_accessed_at ?? ""))[0];
      openWizard({
        path: repoPath,
        tool: latest?.tool ?? "claude",
        yoloMode: latest?.yolo_mode ?? false,
        sandboxEnabled: latest?.is_sandboxed ?? false,
        profile: latest?.profile || undefined,
        group: latest?.group_path || undefined,
      });
    },
    [sessions, openWizard],
  );

  const handleNewSession = useCallback(() => {
    if (!readOnly) openWizard(undefined);
  }, [readOnly, openWizard]);
  const handleNewScratch = useCallback(() => {
    if (!readOnly) openWizard({ scratch: true });
  }, [readOnly, openWizard]);
  const handleCloneFromUrl = useCallback(() => openWizard({ initialTab: "clone" }), [openWizard]);

  const toggleDiff = useCallback(() => {
    if (isMdUp) toggleKind("diff", "right");
    else setPickerOpen((o) => !o);
  }, [isMdUp, toggleKind]);

  const toggleRightDock = useCallback(() => {
    if (!isMdUp) {
      setPickerOpen((o) => !o);
    } else if (!rightDockCollapsed) {
      setDockCollapsed("right", true);
    } else {
      setDockCollapsed("right", false);
      if (availableRightGroups.length === 0) {
        openTab("diff", "right");
        openTab(terminalTabId(0), "right");
      }
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

  // Repo files open in the diff viewer; other absolute paths (and scratch sessions) in the plain viewer.
  const handleOpenFileRef = useCallback(
    (ref: FileRef) => {
      if (!activeSession) return;
      const resolved = resolveToRepoRelative(ref.path, activeSession);
      if (resolved && !activeSession.scratch) {
        setSelectedFile({ path: resolved.relativePath, repoName: resolved.repoName, line: ref.line, cited: true });
      } else if (resolved || isAbsolutePath(ref.path)) {
        setSelectedFile({ path: resolved?.relativePath ?? ref.path, line: ref.line, cited: true, external: true });
      } else {
        toastBus.handler?.error(`Could not open ${ref.path}: not inside this session's repo`);
      }
    },
    [activeSession],
  );

  const handleCloseFile = useCallback(() => setSelectedFile(null), []);

  const handleGoDashboard = useCallback(() => {
    navigate("/");
    setSelectedFile(null);
  }, [navigate]);

  const handleOpenSettings = useCallback(() => {
    navigate("/settings");
    if (isPhoneWidth()) setSidebarOpen(false);
  }, [navigate]);

  useEffect(() => {
    if (profilesMatch) navigate(`/settings/profiles${window.location.search}`, { replace: true });
  }, [profilesMatch, navigate]);

  const handleCloseSettings = useCallback(() => {
    navigate(activeSessionId ? `/session/${encodeURIComponent(activeSessionId)}` : "/");
  }, [navigate, activeSessionId]);

  const handleOpenHelp = useCallback(() => setShowHelp(true), []);
  const handleOpenAbout = useCallback(() => setShowAbout(true), []);
  const handleToggleSidebar = useCallback(() => setSidebarOpen((o) => !o), []);
  const openSidebar = useCallback(() => setSidebarOpen(true), []);
  const openDiff = useCallback(() => {
    if (isMdUp) openTab("diff", "right");
    else setPickerOpen(true);
  }, [isMdUp, openTab]);
  useEdgeSwipe({
    edge: "left",
    enabled: !sidebarOpen && webSettings.sidebarSide !== "right",
    onSwipe: openSidebar,
    blurOnSwipe: true,
    anywhere: true,
  });
  useEdgeSwipe({ edge: "right", enabled: rightDockCollapsed && !!activeSessionId, onSwipe: openDiff });

  // Toggle focus between the agent and the paired shell, opening the shell's tab if needed.
  const handleToggleTerminalFocus = useCallback(() => {
    if (!activeSessionId) return;
    const active = document.activeElement;
    const inPaired =
      !!active && [...document.querySelectorAll<HTMLElement>('[data-term="paired"]')].some((p) => p.contains(active));
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
    if (selectedFilePath) {
      setSelectedFile(null);
      requestAnimationFrame(() => dispatchFocusTerminal("agent"));
      return;
    }
    dispatchFocusTerminal(target);
  }, [activeSessionId, singlePane, paneLayout, openTab, activateTab, selectedFilePath]);

  const { attentionJump } = sidebar;
  const handleJumpToAttention = useCallback(() => {
    const next = nextAttentionSessionId(attentionJump.orderedIds, attentionJump.attention, activeSessionId);
    if (next) handleSelectSession(next);
  }, [attentionJump, activeSessionId, handleSelectSession]);

  const closePalette = () => {
    setShowPalette(false);
    setPaletteQuery("");
  };

  const { deletingWorkspaceId, setDeletingWorkspaceId, stoppingWorkspaceId, setStoppingWorkspaceId } = lifecycle;
  useKeyboardShortcuts(
    useCallback(
      () => ({
        onNew: handleNewSession,
        onJumpToAttention: handleJumpToAttention,
        onNewScratch: handleNewScratch,
        onDiff: () => toggleDiff(),
        // Escape closes the innermost layer only.
        onEscape: () => {
          if (deletingWorkspaceId) return setDeletingWorkspaceId(null);
          if (stoppingWorkspaceId) return setStoppingWorkspaceId(null);
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
        setDeletingWorkspaceId,
        stoppingWorkspaceId,
        setStoppingWorkspaceId,
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
    readOnly,
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
  const settingsCommands = useSettingsCommands({ open: showPalette, readOnly, onOpenSettingsTab: openSettingsTab });
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

  const renderPaneBody = (id: string): ReactNode => {
    const plugin = panes.pluginPaneById.get(id);
    if (plugin) return <PluginPaneBody entry={plugin.entry} />;
    if (id === "agents") return <BackgroundAgentsPanel sessionId={activeSessionId} />;
    if (id === "files") return <FilesPane key={activeSessionId ?? "none"} sessionId={activeSessionId} />;
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

  const renderSessionSplit = (session: SessionResponse, sessionId: string) => (
    <div className="flex-1 flex flex-col min-h-0">
      <PaneDndController
        groupsByDock={panes.groupsByDock}
        descriptorFor={panes.paneDescriptor}
        onPlaceTab={panes.placeVisibleTab}
      >
        <ContentSplit
          collapsed={rightDockCollapsed}
          onToggleCollapse={toggleRightDock}
          left={
            <div className="flex-1 flex flex-col min-h-0 overflow-hidden relative">
              <div className={selectedFilePath ? "hidden" : "flex-1 flex flex-col min-h-0 overflow-hidden"}>
                {session.view === "structured" ? (
                  <Suspense fallback={<AcpLoadingFallback />}>
                    <StructuredView
                      key={sessionId}
                      sessionId={sessionId}
                      acpWorkerState={session.acp_worker_state ?? "absent"}
                      rateLimitAutoResume={session.rate_limit_auto_resume}
                      tool={session.tool}
                      acpAgent={session.acp_agent ?? null}
                      clearAliases={session.clear_aliases}
                      archivedAt={session.archived_at ?? null}
                      snoozedUntil={session.snoozed_until ?? null}
                      trashedAt={session.trashed_at ?? null}
                      onRestore={
                        session.trashed_at
                          ? () => lifecycle.restore(trashedWorkspaceRestoreIds(workspaces, sessionId))
                          : undefined
                      }
                      onOpenFileRef={handleOpenFileRef}
                      fileRefSession={session}
                      onOpenAgentsPane={panes.openAgentsPane}
                      isSandboxed={session.is_sandboxed}
                    />
                  </Suspense>
                ) : (
                  <TerminalSessionStack
                    activeSessionId={sessionId}
                    sessions={sessions.filter((s) => s.view !== "structured")}
                    persistent={webSettings.persistentTerminals}
                    maxPersistentTerminals={webSettings.maxPersistentTerminals}
                  />
                )}
              </div>

              {selectedFilePath &&
                (selectedFile?.external ? (
                  <FileContentViewer sessionId={sessionId} filePath={selectedFilePath} onBack={handleCloseFile} />
                ) : (
                  <DiffFileViewer
                    sessionId={sessionId}
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
                groups={panes.rightGroups}
                descriptorFor={panes.paneDescriptor}
                renderBody={renderPaneBody}
                onActivate={(id) => activateTab("right", id)}
                onMove={panes.movePaneAny}
                onClose={panes.closePaneAny}
                onNewTerminal={readOnly ? undefined : () => panes.addTerminal("right")}
              />
            </div>
          }
        />
        {panes.bottomGroups.length > 0 && (
          <BottomDock
            groups={panes.bottomGroups}
            descriptorFor={panes.paneDescriptor}
            renderBody={renderPaneBody}
            onActivate={(id) => activateTab("bottom", id)}
            onMove={panes.movePaneAny}
            onClose={panes.closePaneAny}
            onNewTerminal={readOnly ? undefined : () => panes.addTerminal("bottom")}
          />
        )}
      </PaneDndController>
      {sendDialogOpen && commentsEnabled && (
        <SendCommentsDialog
          sessionId={sessionId}
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
          onServerAboutRefresh={about.refreshServerAbout}
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

    if (activeSessionId && !sessionsLoaded) return <MainPaneSkeleton />;

    if (!activeWorkspace || !activeSession || !activeSessionId) {
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

    if (!singlePane) return renderSessionSplit(activeSession, activeSessionId);

    return (
      <MobileMainPane
        view={rightPanelView}
        pluginPanes={pluginPanes}
        onBackToAgent={() => handlePickView("agent")}
        pairedMounted={pairedMounted}
        activeSession={activeSession}
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
  const { welcome, tour } = useOnboarding({
    scope: tourScope,
    readOnly,
    cityhall: caps.cityhall,
    isDesktop: !isCoarse,
    autoLaunchReady:
      about.serverAboutLoaded &&
      sessionsLoaded &&
      !activeSessionId &&
      !showSettings &&
      !showSessionWizard &&
      !showHelp &&
      !showAbout &&
      !showPalette &&
      !projectActions.projectForm,
    tips,
    telemetryPending: !about.telemetryConsentKnown || about.telemetryConsentNeeded,
    onNavigate: (tab) => (tab ? navigate(`/settings/${tab}`) : handleCloseSettings()),
  });

  if (!about.serverAboutLoaded) {
    return <div className="h-dvh bg-surface-900 safe-area-inset" />;
  }

  const headerCollapsible =
    singlePane &&
    !showSettings &&
    !!activeWorkspace &&
    activeSession?.view === "structured" &&
    rightPanelView === "agent";
  const { deleting, stoppingSession, switchViewTarget, switchViewSession } = lifecycle;

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
            paneDescriptor={panes.paneDescriptor}
            isPaneOpen={panes.isPaneOpen}
            onTogglePane={panes.togglePaneAny}
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
              groups={sidebar.groups}
              nestedGroups={sidebar.nestedGroups}
              orgGroups={sidebar.orgGroups}
              trashedWorkspaces={trashedWorkspaces}
              onToggleSubgroup={sidebar.toggleSubgroupCollapsed}
              onToggleOrg={sidebar.toggleOrgCollapsed}
              onToggleOrgRepo={sidebar.toggleOrgRepoCollapsed}
              onReorderWorkspaces={sidebar.reorderWorkspaces}
              onReorderGroups={sidebar.reorderRepoGroups}
              activeId={activeWorkspace?.id ?? null}
              open={sidebarOpen}
              onToggle={() => setSidebarOpen(false)}
              onSelect={handleSelectWorkspace}
              onToggleGroup={sidebar.toggleGroup}
              onUpdateRepoAppearance={sidebar.updateRepoAppearance}
              onNew={() => openWizard(undefined)}
              onCreateSession={handleCreateSession}
              onPinProject={projectActions.pinProject}
              onUnpinProject={projectActions.unpinProject}
              savedProjects={sidebar.savedProjects}
              onAddProject={projectActions.addProject}
              onEditProject={projectActions.editProject}
              onRemoveProject={projectActions.removeProject}
              onSettings={handleOpenSettings}
              onDeleteSession={lifecycle.setDeletingWorkspaceId}
              onRestoreSession={lifecycle.restore}
              onEmptyTrash={lifecycle.emptyTrash}
              onStopSession={lifecycle.setStoppingWorkspaceId}
              onStartSession={lifecycle.start}
              onSwitchView={lifecycle.requestSwitchView}
              readOnly={serverAbout?.read_only}
              canManageProjects={caps.canManageProjects}
              sortMode={sidebar.sortMode}
              onSortModeChange={sidebar.selectSortMode}
              pluginSortRef={sidebar.pluginSortRef}
              onPluginSortChange={sidebar.setPluginSortRef}
              axis={sidebar.axis}
              onAxisChange={sidebar.setAxis}
            />
          )}

          <div className="flex-1 flex flex-col min-h-0 min-w-0">{renderContent()}</div>
        </div>

        {showSessionWizard && (
          <SessionWizard
            onClose={closeWizard}
            onCreated={(session?: SessionResponse) => {
              if (session) {
                injectSession(session);
                navigate(`/session/${encodeURIComponent(session.id)}`);
                if (isPhoneWidth()) setSidebarOpen(false);
              }
              closeWizard();
            }}
            prefill={wizardPrefill}
            nameOnly={caps.nameOnlyWizard}
          />
        )}

        {projectActions.projectForm && (
          <ProjectFormModal
            initial={projectActions.projectForm.editProject}
            onClose={projectActions.closeProjectForm}
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
        {about.telemetryConsentNeeded && <TelemetryConsentModal onChoose={about.chooseTelemetryConsent} />}

        {deleting && (
          <DeleteSessionDialog
            sessionTitle={deleting.session.title}
            branchName={deleting.branchName}
            hasManagedWorktree={deleting.sessions.some((s) => s.has_cleanable_worktree ?? false)}
            isSandboxed={deleting.sessions.some((s) => s.is_sandboxed)}
            isScratch={deleting.sessions.some((s) => s.scratch)}
            cleanupDefaults={deleting.cleanupDefaults}
            defaultToTrash={deleting.defaultToTrash}
            affectedSessions={deleting.sessions.map((s) => ({ id: s.id, title: s.title, isSandboxed: s.is_sandboxed }))}
            onConfirm={lifecycle.confirmDelete}
            onTrash={lifecycle.confirmTrash}
            onCancel={() => setDeletingWorkspaceId(null)}
          />
        )}

        {stoppingSession && (
          <StopSessionDialog
            sessionTitle={stoppingSession.title}
            onConfirm={lifecycle.confirmStop}
            onCancel={() => setStoppingWorkspaceId(null)}
          />
        )}

        {switchViewTarget && switchViewSession && (
          <SwitchViewDialog
            sessionTitle={switchViewSession.title}
            toStructured={switchViewTarget.toStructured}
            keepsContext={switchViewSession.keeps_context ?? false}
            onConfirm={lifecycle.confirmSwitchView}
            onCancel={() => lifecycle.setSwitchViewTarget(null)}
          />
        )}

        <CommandPalette
          open={showPalette}
          onClose={closePalette}
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
          ref={setProxyElement}
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
