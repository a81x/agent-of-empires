/* eslint-disable react-refresh/only-export-components */
import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { useServerDown, OFFLINE_TITLE } from "../lib/connectionState";
import { ConnectedDevices } from "./ConnectedDevices";
import { McpServers } from "./McpServers";
import { SkillsManager } from "./SkillsManager";
import { NotificationSettings } from "./NotificationSettings";
import { SecuritySettings } from "./SecuritySettings";
import { TerminalSettings } from "./TerminalSettings";
import {
  fetchProfiles,
  fetchSettings,
  getSettingsSchema,
  setDefaultProfile,
  updateProfileSettings,
  updateTheme,
} from "../lib/api";
import type { ProfileInfo, SettingsFieldDescriptor } from "../lib/types";
import { SchemaSection } from "./settings/SchemaSection";
import { SelectField } from "./settings/FormFields";
import { DiffSettings } from "./settings/DiffSettings";
import { PanelsSettings } from "./settings/PanelsSettings";
import { TelemetrySettings } from "./settings/TelemetrySettings";
import { CityHallSettings } from "./settings/CityHallSettings";
import { PluginsSettings } from "./settings/PluginsSettings";
import { TOUR_ANCHORS, tourAnchor } from "../lib/tourSteps";
import { PluginSettingsSections } from "./settings/PluginSettingsSections";
import { SettingsHeader } from "./settings/SettingsHeader";
import { StructuredViewDisplaySettings } from "./settings/StructuredViewDisplaySettings";
import { ProfilesSection } from "./profiles/ProfilesSection";
import { SECTION_TO_TAB, type SettingsSearchHit } from "./settings/settingsSearchIndex";

const TAB_IDS = [
  "profiles",
  "session",
  "sandbox",
  "worktree",
  "theme",
  "diff",
  "sound",
  "tmux",
  "updates",
  "telemetry",
  "notifications",
  "panels",
  "terminal",
  "security",
  "devices",
  "structured-view",
  "mcp",
  "skills",
  "logging",
  "plugins",
  "cityhall",
] as const;
export type TabId = (typeof TAB_IDS)[number];

type SidebarItem = { kind: "tab"; id: TabId; label: string; icon?: ReactNode } | { kind: "divider"; label: string };

const PROFILES_ICON = (
  <svg
    width="14"
    height="14"
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    strokeWidth="1.5"
    strokeLinecap="round"
    strokeLinejoin="round"
    aria-hidden="true"
    className="shrink-0"
  >
    <rect x="3" y="4" width="18" height="16" rx="2" />
    <circle cx="9" cy="10" r="2" />
    <path d="M6 16a3 3 0 0 1 6 0" />
    <path d="M15 9h3" />
    <path d="M15 13h3" />
  </svg>
);

// Mirrors the TUI grouping in `categories_for_scope()` (src/tui/settings/mod.rs); TUI-only categories are omitted.
export function buildSidebar(): SidebarItem[] {
  return [
    { kind: "tab", id: "profiles", label: "Profiles", icon: PROFILES_ICON },
    { kind: "divider", label: "Appearance" },
    { kind: "tab", id: "theme", label: "Theme" },
    { kind: "tab", id: "diff", label: "Diff" },
    { kind: "divider", label: "Sessions" },
    { kind: "tab", id: "session", label: "Session" },
    { kind: "tab", id: "structured-view", label: "Structured view" },
    { kind: "tab", id: "mcp", label: "MCP servers" },
    { kind: "tab", id: "skills", label: "Skills" },
    { kind: "divider", label: "Environment" },
    { kind: "tab", id: "sandbox", label: "Sandbox" },
    { kind: "tab", id: "worktree", label: "Worktree" },
    { kind: "tab", id: "tmux", label: "Tmux" },
    { kind: "divider", label: "Notifications" },
    { kind: "tab", id: "sound", label: "Sound" },
    { kind: "tab", id: "notifications", label: "Notifications" },
    { kind: "divider", label: "Web Dashboard" },
    { kind: "tab", id: "panels", label: "Panels" },
    { kind: "tab", id: "terminal", label: "Terminal" },
    { kind: "tab", id: "security", label: "Security" },
    { kind: "tab", id: "devices", label: "Devices" },
    { kind: "divider", label: "System" },
    { kind: "tab", id: "updates", label: "Updates" },
    { kind: "tab", id: "telemetry", label: "Telemetry" },
    { kind: "tab", id: "logging", label: "Logging" },
    { kind: "tab", id: "plugins", label: "Plugins" },
    { kind: "tab", id: "cityhall", label: "CityHall" },
  ];
}

// CityHall client mode: a curated, end-user-safe subset of Settings.
const CITYHALL_SIDEBAR: SidebarItem[] = [
  { kind: "tab", id: "theme", label: "Theme" },
  { kind: "tab", id: "session", label: "Sessions" },
  { kind: "tab", id: "mcp", label: "MCP servers" },
  { kind: "tab", id: "telemetry", label: "Telemetry" },
  { kind: "tab", id: "plugins", label: "Plugins" },
];
const CITYHALL_TAB_IDS = new Set<TabId>(["theme", "session", "mcp", "telemetry", "plugins"]);
// Shared with `curateCityhallSchema` so search and the rendered tabs agree.
const CITYHALL_SESSION_FIELDS = ["delete_to_trash", "confirm_delete", "trash_retention_days"];
const CITYHALL_THEME_HIDDEN = ["color_mode", "idle_decay_minutes"];

// Search may only surface fields the curated tabs render; a hidden tab would clamp the jump back to Theme.
function curateCityhallSchema(schema: SettingsFieldDescriptor[]): SettingsFieldDescriptor[] {
  return schema.filter((d) => {
    const tab = SECTION_TO_TAB[d.section];
    if (!tab || !CITYHALL_TAB_IDS.has(tab)) return false;
    if (d.section === "theme") return !CITYHALL_THEME_HIDDEN.includes(d.field);
    if (d.section === "session") return CITYHALL_SESSION_FIELDS.includes(d.field);
    return true;
  });
}

interface Props {
  onClose: () => void;
  tab: string | null;
  onSelectTab: (tab: TabId | string) => void;
  onServerAboutRefresh: () => Promise<void> | void;
  onSettingsRefresh?: () => Promise<void> | void;
  /** Preselected profile from the `?profile=` query. */
  profile?: string | null;
  /** Keeps `?profile=` in sync with the header picker. */
  onSelectProfile?: (profile: string) => void;
  /** Read-only server: the Profiles tab hides its create/edit controls. */
  readOnly?: boolean;
  /** CityHall client mode: curated tabs, no profile switcher; the advanced PATCH is closed server-side. */
  cityhall?: boolean;
}

const ALL_TAB_IDS = new Set<string>(TAB_IDS);

function isTabId(value: unknown): value is TabId {
  return typeof value === "string" && ALL_TAB_IDS.has(value);
}

// Tabs sharing one schema loading/error guard.
// Tabs whose whole body is one SchemaSection, with their Advanced fold subtitle.
const PURE_SCHEMA_TABS: Partial<Record<TabId, string | undefined>> = {
  sandbox: "Resource limits, custom instructions, environment, volumes, and ports.",
  worktree: "Bare-repo and workspace path templates, branch cleanup, and submodules.",
  theme: undefined,
  sound: undefined,
  tmux: undefined,
  updates: undefined,
  logging: "Sink and rotation; some fields require restarting aoe to take effect.",
};

// Tabs that render without the profile settings payload.
const SETTINGS_FREE_TABS = new Set<TabId>([
  "profiles",
  "notifications",
  "terminal",
  "security",
  "devices",
  "structured-view",
  "mcp",
  "skills",
  "plugins",
  "telemetry",
  "cityhall",
  "panels",
]);

const SCHEMA_BACKED_TABS = new Set<TabId>([
  "session",
  "sandbox",
  "worktree",
  "theme",
  "sound",
  "tmux",
  "updates",
  "logging",
  "notifications",
  "structured-view",
]);

/** Keeps a still-valid selection made before the profile fetch resolved, else the default profile. */
export function resolveSelectedProfile(current: string, profiles: ProfileInfo[]): string {
  if (profiles.some((p) => p.name === current)) return current;
  return profiles.find((p) => p.is_default)?.name ?? "default";
}

export function SettingsView({
  onClose,
  tab,
  onSelectTab,
  onServerAboutRefresh,
  onSettingsRefresh = () => {},
  profile,
  onSelectProfile,
  readOnly,
  cityhall = false,
}: Props) {
  const offline = useServerDown();
  const [settings, setSettings] = useState<Record<string, unknown> | null>(null);
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  // Seeded empty, not "default", so no settings fetch fires for a profile that may not exist.
  const [selectedProfile, setSelectedProfile] = useState(profile ?? "");
  // Bumped only by a user profile switch; the content remounts on it, but not on the initial profile resolution.
  const [profileEpoch, setProfileEpoch] = useState(0);
  const handleSelectProfile = useCallback(
    (next: string) => {
      setSelectedProfile(next);
      setProfileEpoch((e) => e + 1);
      onSelectProfile?.(next);
    },
    [onSelectProfile],
  );
  const sidebar: SidebarItem[] = cityhall ? CITYHALL_SIDEBAR : buildSidebar();
  const tabs = sidebar.filter((s): s is { kind: "tab"; id: TabId; label: string } => s.kind === "tab");
  const activeTab: TabId = cityhall
    ? isTabId(tab) && CITYHALL_TAB_IDS.has(tab)
      ? tab
      : "theme"
    : isTabId(tab)
      ? tab
      : "session";
  const [profiles, setProfiles] = useState<ProfileInfo[]>([]);
  const [schema, setSchema] = useState<SettingsFieldDescriptor[]>([]);
  const [schemaLoading, setSchemaLoading] = useState(true);
  const [schemaError, setSchemaError] = useState<string | null>(null);
  const searchSchema = useMemo(() => (cityhall ? curateCityhallSchema(schema) : schema), [cityhall, schema]);
  // A search jump switches tab and asks the section to reveal the field; the nonce re-triggers repeats.
  const [focusRequest, setFocusRequest] = useState<{ section: string; field: string; nonce: number } | null>(null);
  const handleSearchJump = useCallback(
    (hit: SettingsSearchHit) => {
      setFocusRequest((prev) => ({ section: hit.section, field: hit.field, nonce: (prev?.nonce ?? 0) + 1 }));
      onSelectTab(hit.tab);
    },
    [onSelectTab],
  );

  useEffect(() => {
    fetchProfiles().then((p) => {
      setProfiles(p);
      setSelectedProfile((current) => resolveSelectedProfile(current, p));
    });
  }, []);

  const loadSchema = useCallback(async () => {
    setSchemaLoading(true);
    setSchemaError(null);
    try {
      const s = await getSettingsSchema();
      if (!s) {
        setSchemaError("Failed to load settings schema.");
        return;
      }
      setSchema(s);
    } catch {
      setSchemaError("Failed to load settings schema.");
    } finally {
      setSchemaLoading(false);
    }
  }, []);

  useEffect(() => {
    const timer = setTimeout(() => {
      void loadSchema();
    }, 0);
    return () => clearTimeout(timer);
  }, [loadSchema]);

  if (profile && profile !== selectedProfile) {
    setSelectedProfile(profile);
  }

  const defaultProfile = profiles.find((p) => p.is_default)?.name ?? "default";

  const handleSetDefault = async (name: string) => {
    const ok = await setDefaultProfile(name);
    if (ok) fetchProfiles().then(setProfiles);
  };

  // Drops a slow fetch for a previous profile that lands after a switch.
  const loadSeq = useRef(0);
  const loadSettings = useCallback(() => {
    if (!selectedProfile) return;
    const seq = ++loadSeq.current;
    fetchSettings(selectedProfile)
      .then((s) => {
        if (seq !== loadSeq.current) return;
        if (s) setSettings(s);
      })
      .catch(() => {
        if (seq !== loadSeq.current) return;
        setSettings(null);
      });
  }, [selectedProfile]);

  useEffect(() => {
    loadSettings();
  }, [loadSettings]);

  const sendSave = useCallback(
    async (section: string, data: Record<string, unknown>): Promise<boolean> => {
      if (!selectedProfile) return false;
      setSaving(true);
      setSaveError(null);
      const ok = await updateProfileSettings(selectedProfile, {
        [section]: data,
      });
      setSaving(false);
      if (!ok) {
        setSaveError("Failed to save, please try again");
        loadSettings();
      }
      return ok;
    },
    [selectedProfile, loadSettings],
  );

  const updateLocal = useCallback(
    (patch: Record<string, unknown>) => {
      if (settings) setSettings({ ...settings, ...patch });
    },
    [settings],
  );

  const session = (settings?.session ?? {}) as Record<string, unknown>;
  const web = (settings?.web ?? {}) as Record<string, unknown>;

  const saveField = useCallback(
    (section: string, sectionData: Record<string, unknown>, field: string, value: unknown): Promise<boolean> => {
      updateLocal({ [section]: { ...sectionData, [field]: value } });
      return sendSave(section, { [field]: value });
    },
    [updateLocal, sendSave],
  );

  const saveSubField = useCallback(
    (section: string, field: string, value: unknown): Promise<boolean> => {
      const sectionData = (settings?.[section] ?? {}) as Record<string, unknown>;
      return saveField(section, sectionData, field, value);
    },
    [settings, saveField],
  );

  // Global theme fields write through /api/theme; a profile override would shadow the global pick.
  const saveThemeField = useCallback(
    async (section: string, field: string, value: unknown): Promise<boolean> => {
      const overridable = schema.some((d) => d.section === section && d.field === field && d.profile_overridable);
      if (overridable) return saveSubField(section, field, value);
      const sectionData = (settings?.theme ?? {}) as Record<string, unknown>;
      updateLocal({ theme: { ...sectionData, [field]: value } });
      setSaving(true);
      setSaveError(null);
      const ok = await updateTheme({ [field]: value });
      setSaving(false);
      if (!ok) {
        setSaveError("Failed to save, please try again");
        loadSettings();
      }
      return ok;
    },
    [schema, settings, updateLocal, loadSettings, saveSubField],
  );

  const renderTabContent = () => {
    if (!settings && !SETTINGS_FREE_TABS.has(activeTab)) {
      return <div className="text-sm text-text-dim">Loading settings...</div>;
    }

    const schemaGuard = () => {
      if (schemaLoading) {
        return <div className="text-sm text-text-dim">Loading settings schema...</div>;
      }
      if (schemaError) {
        return (
          <div className="space-y-3">
            <div className="text-sm text-status-error">{schemaError}</div>
            <button
              type="button"
              onClick={() => void loadSchema()}
              className="rounded px-3 py-1 text-xs font-medium bg-surface-700 text-text-secondary hover:bg-surface-600 cursor-pointer"
            >
              Retry
            </button>
          </div>
        );
      }
      return null;
    };

    // Mixed tabs guard only their schema slot, so their other rows survive a failed schema fetch.
    if (SCHEMA_BACKED_TABS.has(activeTab) && activeTab !== "session" && activeTab !== "notifications") {
      const guard = schemaGuard();
      if (guard) return guard;
    }

    if (activeTab in PURE_SCHEMA_TABS) {
      return (
        <SchemaSection
          section={activeTab}
          schema={schema}
          focusRequest={focusRequest}
          values={(settings?.[activeTab] ?? {}) as Record<string, unknown>}
          onSaveField={activeTab === "theme" ? saveThemeField : saveSubField}
          advancedSubtitle={PURE_SCHEMA_TABS[activeTab]}
          hideFields={activeTab === "theme" && cityhall ? CITYHALL_THEME_HIDDEN : undefined}
          fieldAnchor={
            activeTab === "worktree" ? { field: "path_template", anchor: TOUR_ANCHORS.settingsWorktree } : undefined
          }
        />
      );
    }

    switch (activeTab) {
      case "profiles":
        return <ProfilesSection readOnly={readOnly} />;

      case "session":
        if (cityhall) {
          return (
            <div className="space-y-4">
              {schemaGuard() ?? (
                <SchemaSection
                  section="session"
                  schema={schema}
                  focusRequest={focusRequest}
                  values={session}
                  onSaveField={saveSubField}
                  onlyFields={CITYHALL_SESSION_FIELDS}
                />
              )}
            </div>
          );
        }
        return (
          <div className="space-y-4">
            <SelectField
              label="Default profile"
              description="Profile used for new sessions"
              value={defaultProfile}
              onChange={(v) => handleSetDefault(v)}
              options={profiles.map((p) => ({ value: p.name, label: p.name }))}
            />
            {schemaGuard() ?? (
              <SchemaSection
                section="session"
                schema={schema}
                focusRequest={focusRequest}
                values={session}
                onSaveField={saveSubField}
                onAfterSave={(descriptor) => {
                  if (descriptor.field === "row_tag" || descriptor.field === "show_session_colors") {
                    return onSettingsRefresh();
                  }
                }}
                advancedSubtitle="Idle auto-stop, attach modes, live-send, and other session tuning."
              />
            )}
          </div>
        );

      case "diff":
        return <DiffSettings />;
      case "panels":
        return <PanelsSettings />;
      case "telemetry":
        return <TelemetrySettings />;
      case "cityhall":
        return <CityHallSettings />;
      case "plugins":
        return (
          <div className="space-y-6" {...tourAnchor(TOUR_ANCHORS.settingsPlugins)}>
            <PluginsSettings readOnly={cityhall} />
            {!cityhall &&
              (schemaGuard() ?? <PluginSettingsSections schema={schema} settings={settings} onSaved={loadSettings} />)}
          </div>
        );

      case "notifications":
        return (
          <div className="space-y-6">
            <NotificationSettings />
            <div className="space-y-4">
              <h4 className="text-xs font-mono uppercase tracking-widest text-text-muted">Server Defaults</h4>
              <p className="text-xs text-text-dim">
                Controls which session events trigger push notifications on the server.
              </p>
              {schemaGuard() ??
                (settings && (
                  <SchemaSection
                    section="web"
                    schema={schema}
                    focusRequest={focusRequest}
                    values={web}
                    onSaveField={saveSubField}
                  />
                ))}
            </div>
          </div>
        );

      case "terminal":
        return <TerminalSettings />;
      case "security":
        return <SecuritySettings />;
      case "devices":
        return <ConnectedDevices />;
      case "mcp":
        return <McpServers readOnly={cityhall} />;
      case "skills":
        return <SkillsManager readOnly={readOnly || cityhall} />;
      case "structured-view": {
        if (!settings) {
          return <div className="text-sm text-text-dim">Loading settings...</div>;
        }
        const acp = (settings.acp ?? {}) as Record<string, unknown>;
        return (
          <div className="space-y-4">
            {/* The tour anchors on this intro, not the far-down async widget, so joyride need not scroll to it. */}
            <p className="text-xs text-text-dim" {...tourAnchor(TOUR_ANCHORS.settingsAgentDefaults)}>
              Defaults for structured-view (ACP) sessions: which agent starts, how many workers run at once, how much
              history is replayed on reconnect, and the per-agent model, mode, and thinking defaults below. These apply
              when a session renders in the structured view instead of a raw terminal.
            </p>
            <StructuredViewDisplaySettings />
            <SchemaSection
              section="acp"
              schema={schema}
              focusRequest={focusRequest}
              values={acp}
              onSaveField={saveSubField}
              // Some acp fields mirror into serverAbout, which live surfaces read.
              onAfterSave={() => onServerAboutRefresh()}
              advancedSubtitle="Replay retention caps and daemon watchdog tuning. Touch only when triaging a specific failure mode."
            />
          </div>
        );
      }
    }
  };

  const currentTabLabel = tabs.find((t) => t.id === activeTab)?.label ?? "";

  return (
    <div className="flex-1 flex flex-col overflow-hidden bg-surface-900">
      <SettingsHeader
        onClose={onClose}
        saving={saving}
        saveError={saveError}
        selectedProfile={selectedProfile}
        onSelectProfile={handleSelectProfile}
        schema={searchSchema}
        schemaLoading={schemaLoading}
        onSearchJump={handleSearchJump}
        hideProfileSelector={cityhall}
      />

      <div className="md:hidden border-b border-surface-700 bg-surface-850 overflow-x-auto">
        <div className="flex items-center">
          {sidebar.map((item) =>
            item.kind === "divider" ? (
              <div key={item.label} className="h-4 w-px bg-surface-700 mx-1 shrink-0" />
            ) : (
              <button
                key={item.id}
                onClick={() => onSelectTab(item.id)}
                className={`flex items-center gap-1.5 px-4 py-2.5 text-xs font-medium whitespace-nowrap cursor-pointer transition-colors ${
                  activeTab === item.id
                    ? "text-brand-500 border-b-2 border-brand-500"
                    : "text-text-secondary hover:text-text-primary"
                }`}
              >
                {item.icon}
                {item.label}
              </button>
            ),
          )}
        </div>
      </div>

      <div className="flex-1 flex min-h-0">
        <nav className="hidden md:flex flex-col w-44 shrink-0 border-r border-surface-700 bg-surface-850 py-2 overflow-y-auto">
          {sidebar.map((item, i) =>
            item.kind === "divider" ? (
              <div
                key={item.label}
                className={`px-4 pt-3 pb-1 text-[10px] font-mono uppercase tracking-widest text-text-dim ${i > 0 ? "mt-2 border-t border-surface-700/40" : ""}`}
              >
                {item.label}
              </div>
            ) : (
              <button
                key={item.id}
                onClick={() => onSelectTab(item.id)}
                className={`flex items-center gap-2 px-4 py-2 text-sm text-left cursor-pointer transition-colors ${
                  activeTab === item.id
                    ? "text-brand-500 bg-surface-800 border-r-2 border-brand-500"
                    : "text-text-secondary hover:text-text-primary hover:bg-surface-800/50"
                }`}
              >
                {item.icon}
                {item.label}
              </button>
            ),
          )}
        </nav>

        <div className="flex-1 overflow-y-auto" style={{ paddingBottom: "env(safe-area-inset-bottom)" }}>
          {/* Skills needs the full width for its two panes. */}
          <div className={activeTab === "skills" ? "p-6 space-y-5" : "p-6 max-w-5xl mx-auto space-y-5"}>
            <h2 className="text-lg font-semibold text-text-bright">{currentTabLabel}</h2>

            {offline && (
              <div className="text-sm text-status-error bg-status-error/10 rounded-lg p-3">
                {OFFLINE_TITLE}: toggles will not save while disconnected.
              </div>
            )}
            {/* Remounting on tab or user profile switch resets Advanced folds and drops half-typed drafts. */}
            <fieldset
              key={`${activeTab}-${profileEpoch}-${focusRequest?.nonce ?? 0}`}
              disabled={offline}
              className="space-y-5 disabled:opacity-50 border-0 m-0 p-0 min-w-0"
            >
              {renderTabContent()}
            </fieldset>
          </div>
        </div>
      </div>
    </div>
  );
}
