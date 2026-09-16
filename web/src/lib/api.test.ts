// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import * as api from "./api";
import type { ServerAbout } from "./api";
import type { CreateSessionRequest, SettingsFieldDescriptor } from "./types";

const fetchSpy = vi.fn<typeof fetch>();

beforeEach(() => {
  fetchSpy.mockReset();
  vi.stubGlobal("fetch", fetchSpy);
});

afterEach(() => {
  vi.unstubAllGlobals();
});

const json = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
const empty = (status = 200) => new Response("", { status });
const offline = () => fetchSpy.mockRejectedValueOnce(new Error("offline"));

function lastCall() {
  const [url, init] = fetchSpy.mock.calls.at(-1)!;
  return { url: String(url), init };
}

const bodyOf = (init: RequestInit | undefined) => JSON.parse(init!.body as string);

interface RequestCase {
  name: string;
  call: () => Promise<unknown>;
  url: string;
  method?: string;
  body?: unknown;
  respond?: Response;
  result?: unknown;
}

const session = { id: "s1" };
const plugins = { plugins: [], load_errors: [] };
const hit = { session_id: "s1", seq: 3, kind: "agent", snippet: "hit", match_count: 2 };
const switched = { session_id: "s-1", agent: "codex", before_seq: 41, switch_seq: 42, status: "ok" };
const skill = {
  directory: "review",
  name: "review",
  description: "",
  provenance: { kind: "aoe-managed" },
  content: "",
};
const preview = { kind: "consent_required", dismissed: false, consent: { id: "p" } };

const requestCases: RequestCase[] = [
  {
    name: "fetchSessions",
    call: () => api.fetchSessions(),
    url: "/api/sessions",
    respond: json(session),
    result: session,
  },
  {
    name: "searchConversations",
    call: () => api.searchConversations("foo bar"),
    url: "/api/sessions/search?q=foo%20bar",
    respond: json({ results: [hit] }),
    result: [hit],
  },
  {
    name: "fetchRecentProjects",
    call: () => api.fetchRecentProjects(),
    url: "/api/recent-projects",
    respond: json({ projects: [] }),
    result: { projects: [] },
  },
  {
    name: "updateWorkspaceOrdering",
    call: () => api.updateWorkspaceOrdering(["a", "b"]),
    url: "/api/workspace-ordering",
    method: "PUT",
    body: { order: ["a", "b"] },
    result: true,
  },
  {
    name: "ensureTerminal default",
    call: () => api.ensureTerminal("s1"),
    url: "/api/sessions/s1/terminal?index=0",
    method: "POST",
    result: true,
  },
  {
    name: "ensureTerminal container",
    call: () => api.ensureTerminal("s1", 2, true),
    url: "/api/sessions/s1/container-terminal?index=2",
    method: "POST",
  },
  {
    name: "killTerminal",
    call: () => api.killTerminal("s1", 2),
    url: "/api/sessions/s1/terminal?index=2",
    method: "DELETE",
    result: true,
  },
  {
    name: "getSessionDiffFiles",
    call: () => api.getSessionDiffFiles("s1"),
    url: "/api/sessions/s1/diff/files",
    respond: json({ files: [] }),
    result: { files: [] },
  },
  {
    name: "getSessionFileContents",
    call: () => api.getSessionFileContents("s1", "src/a b.ts"),
    url: "/api/sessions/s1/diff/file?path=src%2Fa+b.ts",
  },
  {
    name: "getSessionFileContents repo",
    call: () => api.getSessionFileContents("s1", "a.ts", "myrepo"),
    url: "/api/sessions/s1/diff/file?path=a.ts&repo=myrepo",
  },
  { name: "getSessionFile", call: () => api.getSessionFile("s1", "a b.ts"), url: "/api/sessions/s1/file?path=a+b.ts" },
  { name: "fetchSettings", call: () => api.fetchSettings(), url: "/api/settings" },
  {
    name: "fetchSettings profile",
    call: () => api.fetchSettings("my profile"),
    url: "/api/settings?profile=my%20profile",
  },
  {
    name: "updateSettings",
    call: () => api.updateSettings({ a: 1 }),
    url: "/api/settings",
    method: "PATCH",
    body: { a: 1 },
    result: true,
  },
  {
    name: "updateTheme name",
    call: () => api.updateTheme({ name: "dracula" }),
    url: "/api/theme",
    method: "PATCH",
    body: { name: "dracula" },
    result: true,
  },
  {
    name: "updateTheme color_mode",
    call: () => api.updateTheme({ color_mode: "palette" }),
    url: "/api/theme",
    method: "PATCH",
    body: { color_mode: "palette" },
  },
  {
    name: "getWebUiState",
    call: () => api.getWebUiState(),
    url: "/api/app-state/web-ui-state",
    respond: json({ k: "v" }),
    result: { k: "v" },
  },
  {
    name: "patchWebUiState",
    call: () => api.patchWebUiState({ keep: "1", drop: null }),
    url: "/api/app-state/web-ui-state",
    method: "PATCH",
    body: { keep: "1", drop: null },
    result: true,
  },
  {
    name: "fetchTips",
    call: () => api.fetchTips(),
    url: "/api/tips",
    respond: json({ enabled: true, tips: [] }),
    result: { enabled: true, tips: [] },
  },
  {
    name: "markTipSeen",
    call: () => api.markTipSeen("pin"),
    url: "/api/app-state/tip-seen",
    method: "POST",
    body: { id: "pin" },
    result: true,
  },
  {
    name: "setShowTips",
    call: () => api.setShowTips(false),
    url: "/api/tips/show",
    method: "POST",
    body: { enabled: false },
    result: true,
  },
  {
    name: "fetchVolumeIgnoresPreview",
    call: () => api.fetchVolumeIgnoresPreview("/repo/a b"),
    url: "/api/sandbox/volume-ignores-preview?path=%2Frepo%2Fa+b",
    respond: json({ acknowledged: false, globs: [] }),
    result: { acknowledged: false, globs: [] },
  },
  {
    name: "fetchVolumeIgnoresPreview profile",
    call: () => api.fetchVolumeIgnoresPreview("/repo", "work"),
    url: "/api/sandbox/volume-ignores-preview?path=%2Frepo&profile=work",
  },
  {
    name: "markVolumeIgnoresGlobsAcknowledged",
    call: () => api.markVolumeIgnoresGlobsAcknowledged(),
    url: "/api/app-state/volume-ignores-globs-acknowledged",
    method: "POST",
    result: true,
  },
  {
    name: "createProfile",
    call: () => api.createProfile("work"),
    url: "/api/profiles",
    method: "POST",
    body: { name: "work" },
    result: true,
  },
  {
    name: "deleteProfile",
    call: () => api.deleteProfile("my work"),
    url: "/api/profiles/my%20work",
    method: "DELETE",
    result: true,
  },
  {
    name: "renameProfile",
    call: () => api.renameProfile("old", "new"),
    url: "/api/profiles/old/rename",
    method: "PATCH",
    body: { new_name: "new" },
    result: true,
  },
  {
    name: "setDefaultProfile",
    call: () => api.setDefaultProfile("work"),
    url: "/api/default-profile",
    method: "PATCH",
    body: { name: "work" },
    result: true,
  },
  {
    name: "getProfileSettings",
    call: () => api.getProfileSettings("my work"),
    url: "/api/profiles/my%20work/settings",
    respond: json({ description: "x" }),
    result: { description: "x" },
  },
  {
    name: "fetchThemes",
    call: () => api.fetchThemes(),
    url: "/api/themes",
    respond: json(["empire"]),
    result: ["empire"],
  },
  { name: "fetchResolvedTheme", call: () => api.fetchResolvedTheme("My Theme"), url: "/api/themes/My%20Theme" },
  { name: "fetchCurrentTheme", call: () => api.fetchCurrentTheme(), url: "/api/theme/current" },
  {
    name: "fetchSounds",
    call: () => api.fetchSounds(),
    url: "/api/sounds",
    respond: json(["chime.wav"]),
    result: ["chime.wav"],
  },
  {
    name: "fetchAbout",
    call: () => api.fetchAbout(),
    url: "/api/about",
    respond: json({ build_flavor: "debug" }),
    result: { build_flavor: "debug" },
  },
  {
    name: "fetchTelemetryStatus",
    call: () => api.fetchTelemetryStatus(),
    url: "/api/telemetry/status",
    respond: json({ enabled: true }),
    result: { enabled: true },
  },
  {
    name: "setTelemetryConsent",
    call: () => api.setTelemetryConsent(true),
    url: "/api/telemetry/consent",
    method: "POST",
    body: { enabled: true },
    respond: json({ enabled: true }),
    result: { enabled: true },
  },
  { name: "fetchUpdateStatus", call: () => api.fetchUpdateStatus(), url: "/api/system/update-status" },
  {
    name: "dismissUpdate",
    call: () => api.dismissUpdate("1.2.3"),
    url: "/api/app-state/dismiss-update",
    method: "POST",
    body: { version: "1.2.3" },
    result: true,
  },
  {
    name: "markWebTourSeen",
    call: () => api.markWebTourSeen(),
    url: "/api/app-state/web-tour-seen",
    method: "POST",
    result: true,
  },
  { name: "fetchBranches", call: () => api.fetchBranches("/repo"), url: "/api/git/branches?path=%2Frepo" },
  {
    name: "fetchBranches remote",
    call: () => api.fetchBranches("/repo", true),
    url: "/api/git/branches?path=%2Frepo&include_remote=true",
  },
  {
    name: "fetchIsGitRepo",
    call: () => api.fetchIsGitRepo("/r"),
    url: "/api/git/is-repo?path=%2Fr",
    respond: json({ is_git_repo: false }),
    result: false,
  },
  {
    name: "cloneRepo",
    call: () => api.cloneRepo("u"),
    url: "/api/git/clone",
    method: "POST",
    body: { url: "u" },
    respond: json({ path: "/c" }),
    result: { ok: true, path: "/c" },
  },
  {
    name: "cloneRepo options",
    call: () => api.cloneRepo("u", { destination: "/d", shallow: true, bare: true }),
    url: "/api/git/clone",
    method: "POST",
    body: { url: "u", destination: "/d", shallow: true, bare: true },
  },
  {
    name: "fetchContextPrimer",
    call: () => api.fetchContextPrimer("weird/id", 42),
    url: "/api/sessions/weird%2Fid/acp/context-primer?before_seq=42",
  },
  {
    name: "fetchAcpAgents",
    call: () => api.fetchAcpAgents(),
    url: "/api/acp/agents",
    respond: json([{ name: "codex" }]),
    result: [{ name: "codex" }],
  },
  {
    name: "fetchAcpOptionCatalog",
    call: () => api.fetchAcpOptionCatalog(),
    url: "/api/acp/option-catalog",
    respond: json({ version: 2, agents: {} }),
    result: { version: 2, agents: {} },
  },
  {
    name: "switchAcpAgent",
    call: () => api.switchAcpAgent("weird/id", "codex"),
    url: "/api/sessions/weird%2Fid/acp/switch-agent",
    method: "POST",
    body: { target: "codex" },
    respond: json(switched),
    result: switched,
  },
  {
    name: "switchAcpAgent model",
    call: () => api.switchAcpAgent("s-1", "codex", "opus-4.7"),
    url: "/api/sessions/s-1/acp/switch-agent",
    method: "POST",
    body: { target: "codex", model: "opus-4.7" },
  },
  {
    name: "switchAcpAgent reason",
    call: () => api.switchAcpAgent("s-1", "claude", null, "manual"),
    url: "/api/sessions/s-1/acp/switch-agent",
    method: "POST",
    body: { target: "claude", reason: "manual" },
  },
  {
    name: "acpEnable",
    call: () => api.acpEnable("s-1"),
    url: "/api/sessions/s-1/acp/enable",
    method: "POST",
    respond: json({ view: "structured" }),
    result: { view: "structured" },
  },
  { name: "acpDisable", call: () => api.acpDisable("a/b"), url: "/api/sessions/a%2Fb/acp/disable", method: "POST" },
  {
    name: "enqueueServerPrompt",
    call: () =>
      api.enqueueServerPrompt("s1", {
        id: "q1",
        text: "hi",
        createdAt: "t0",
        attachments: [{ kind: "image", mimeType: "image/png", name: "a.png", dataB64: "AA" }],
      }),
    url: "/api/sessions/s1/queue",
    method: "POST",
    body: {
      id: "q1",
      text: "hi",
      created_at: "t0",
      attachments: [{ kind: "image", mime_type: "image/png", data: "AA", name: "a.png" }],
    },
    respond: json({ id: "q1", seq: 3 }),
    result: { id: "q1", seq: 3 },
  },
  {
    name: "listServerQueue",
    call: () => api.listServerQueue("s1"),
    url: "/api/sessions/s1/queue",
    respond: json([{ id: "a" }]),
    result: [{ id: "a" }],
  },
  {
    name: "editServerQueuedPrompt",
    call: () => api.editServerQueuedPrompt("s1", "q1", "edited"),
    url: "/api/sessions/s1/queue/q1",
    method: "PATCH",
    body: { text: "edited" },
    result: true,
  },
  {
    name: "removeServerQueuedPrompt",
    call: () => api.removeServerQueuedPrompt("a/b", "c d"),
    url: "/api/sessions/a%2Fb/queue/c%20d",
    method: "DELETE",
    result: true,
  },
  {
    name: "clearServerQueue",
    call: () => api.clearServerQueue("s1"),
    url: "/api/sessions/s1/queue",
    method: "DELETE",
    result: true,
  },
  { name: "fetchDevices", call: () => api.fetchDevices(), url: "/api/devices" },
  {
    name: "revokeDevice",
    call: () => api.revokeDevice("sess/1"),
    url: "/api/login/sessions/sess%2F1",
    method: "DELETE",
    result: true,
  },
  {
    name: "signOutAllDevices",
    call: () => api.signOutAllDevices(),
    url: "/api/login/logout-all",
    method: "POST",
    result: true,
  },
  {
    name: "loginStatus",
    call: () => api.loginStatus(),
    url: "/api/login/status",
    respond: json({ required: true }),
    result: { required: true },
  },
  { name: "verifyToken", call: () => api.verifyToken(), url: "/api/login/status", result: true },
  {
    name: "fetchAgents",
    call: () => api.fetchAgents(),
    url: "/api/agents",
    respond: json([{ id: "claude" }]),
    result: [{ id: "claude" }],
  },
  { name: "fetchProfiles", call: () => api.fetchProfiles(), url: "/api/profiles" },
  {
    name: "getHomePath",
    call: () => api.getHomePath(),
    url: "/api/filesystem/home",
    respond: json({ path: "/home/u" }),
    result: "/home/u",
  },
  {
    name: "getHomePath without path",
    call: () => api.getHomePath(),
    url: "/api/filesystem/home",
    respond: json({}),
    result: null,
  },
  {
    name: "browseFilesystem",
    call: () => api.browseFilesystem("/repo"),
    url: "/api/filesystem/browse?path=%2Frepo",
    respond: json({ entries: [], has_more: true }),
    result: { entries: [], has_more: true, ok: true },
  },
  {
    name: "browseFilesystem options",
    call: () => api.browseFilesystem("/repo", 50, "src", true),
    url: "/api/filesystem/browse?path=%2Frepo&limit=50&filter=src&show_hidden=true",
  },
  { name: "fetchGroups", call: () => api.fetchGroups(), url: "/api/groups" },
  { name: "fetchProjects", call: () => api.fetchProjects(), url: "/api/projects" },
  { name: "fetchProjects scope", call: () => api.fetchProjects("profile"), url: "/api/projects?scope=profile" },
  { name: "listClaudeSessions", call: () => api.listClaudeSessions(), url: "/api/claude-sessions" },
  {
    name: "fetchDockerStatus",
    call: () => api.fetchDockerStatus(),
    url: "/api/docker/status",
    respond: json({ available: true, runtime: "docker" }),
    result: { available: true, runtime: "docker" },
  },
  {
    name: "createProject",
    call: () => api.createProject({ path: "/p", name: "p", scope: "global" }),
    url: "/api/projects",
    method: "POST",
    body: { path: "/p", name: "p", scope: "global" },
    respond: json({ name: "p" }),
    result: { ok: true, project: { name: "p" } },
  },
  {
    name: "deleteProject",
    call: () => api.deleteProject("my proj", "profile"),
    url: "/api/projects/my%20proj?scope=profile",
    method: "DELETE",
    result: { ok: true },
  },
  {
    name: "updateProject",
    call: () => api.updateProject("p", "global", "develop"),
    url: "/api/projects/p?scope=global",
    method: "PATCH",
    body: { default_base_branch: "develop" },
    respond: json({ name: "p" }),
    result: { ok: true, project: { name: "p" } },
  },
  {
    name: "updateProject clear",
    call: () => api.updateProject("p", "global", null),
    url: "/api/projects/p?scope=global",
    method: "PATCH",
    body: { default_base_branch: null },
    respond: json({}),
  },
  {
    name: "setProjectPinned",
    call: () => api.setProjectPinned("a b", "profile", true),
    url: "/api/projects/a%20b?scope=profile",
    method: "PATCH",
    body: { pinned: true },
    respond: json({ pinned: true }),
    result: { ok: true, project: { pinned: true } },
  },
  {
    name: "createSession",
    call: () => api.createSession({ path: "/repo", tool: "claude", trust_hooks: true } as CreateSessionRequest),
    url: "/api/sessions",
    method: "POST",
    body: { path: "/repo", tool: "claude", trust_hooks: true },
    respond: json(session, 201),
    result: { ok: true, session },
  },
  {
    name: "renameSession",
    call: () => api.renameSession("s1", "T"),
    url: "/api/sessions/s1",
    method: "PATCH",
    body: { title: "T" },
    result: { ok: true },
  },
  {
    name: "smartRenameSession",
    call: () => api.smartRenameSession("s1"),
    url: "/api/sessions/s1/smart-rename",
    method: "POST",
    respond: empty(202),
    result: { ok: true },
  },
  {
    name: "summarizeSession",
    call: () => api.summarizeSession("s1"),
    url: "/api/sessions/s1/summarize",
    method: "POST",
    respond: empty(202),
    result: { ok: true },
  },
  {
    name: "setWorktreeName",
    call: () => api.setWorktreeName("s1", "feature", true),
    url: "/api/sessions/s1/worktree-name",
    method: "PATCH",
    body: { name: "feature", rename_branch: true },
    result: { ok: true },
  },
  {
    name: "updateSessionGroup",
    call: () => api.updateSessionGroup("a/b", ""),
    url: "/api/sessions/a%2Fb/group",
    method: "PATCH",
    body: { group: "" },
    result: true,
  },
  ...(["off", "all", "default"] as const).map((preset) => {
    const value = { off: false, all: true, default: null }[preset];
    return {
      name: `setSessionNotifications ${preset}`,
      call: () => api.setSessionNotifications("s1", preset),
      url: "/api/sessions/s1/notifications",
      method: "PATCH",
      body: { notify_on_waiting: value, notify_on_idle: value, notify_on_error: value },
      result: true,
    };
  }),
  {
    name: "setSessionDiffBase",
    call: () => api.setSessionDiffBase("s1", "develop"),
    url: "/api/sessions/s1/diff-base",
    method: "PATCH",
    body: { base_branch: "develop" },
    respond: json(session),
    result: session,
  },
  {
    name: "setSessionDiffBase clear",
    call: () => api.setSessionDiffBase("s1", null),
    url: "/api/sessions/s1/diff-base",
    method: "PATCH",
    body: { base_branch: null },
  },
  {
    name: "setSessionDiffBase repo",
    call: () => api.setSessionDiffBase("s1", "main", "r"),
    url: "/api/sessions/s1/diff-base",
    method: "PATCH",
    body: { base_branch: "main", repo: "r" },
  },
  {
    name: "setSessionPin",
    call: () => api.setSessionPin("s1", false),
    url: "/api/sessions/s1/pin",
    method: "PATCH",
    body: { pinned: false },
    respond: json(session),
    result: session,
  },
  {
    name: "setSessionColor",
    call: () => api.setSessionColor("s1", null),
    url: "/api/sessions/s1/color",
    method: "PATCH",
    body: { color: null },
  },
  {
    name: "setSessionArchive",
    call: () => api.setSessionArchive("s1", true),
    url: "/api/sessions/s1/archive",
    method: "PATCH",
    body: { archived: true, kill_pane: true },
  },
  {
    name: "setSessionArchive keep pane",
    call: () => api.setSessionArchive("s1", false, false),
    url: "/api/sessions/s1/archive",
    method: "PATCH",
    body: { archived: false, kill_pane: false },
  },
  {
    name: "trashSession",
    call: () => api.trashSession("s1"),
    url: "/api/sessions/s1/trash",
    method: "POST",
    body: { kill_pane: true },
    respond: json(session),
    result: session,
  },
  {
    name: "trashSession keep pane",
    call: () => api.trashSession("s1", false),
    url: "/api/sessions/s1/trash",
    method: "POST",
    body: { kill_pane: false },
  },
  {
    name: "restoreSession",
    call: () => api.restoreSession("s1"),
    url: "/api/sessions/s1/restore",
    method: "POST",
    respond: json(session),
    result: session,
  },
  {
    name: "stopSession",
    call: () => api.stopSession("s1"),
    url: "/api/sessions/s1/stop",
    method: "POST",
    respond: json(session),
    result: session,
  },
  {
    name: "startSession",
    call: () => api.startSession("s1"),
    url: "/api/sessions/s1/start",
    method: "POST",
    respond: json(session),
    result: session,
  },
  {
    name: "setSessionSnooze",
    call: () => api.setSessionSnooze("s1", 60),
    url: "/api/sessions/s1/snooze",
    method: "PATCH",
    body: { minutes: 60 },
  },
  {
    name: "setSessionSnooze clear",
    call: () => api.setSessionSnooze("s1", null),
    url: "/api/sessions/s1/snooze",
    method: "PATCH",
    body: { minutes: null },
  },
  {
    name: "setSessionUnread",
    call: () => api.setSessionUnread("s1", true),
    url: "/api/sessions/s1/unread",
    method: "PATCH",
    body: { unread: true },
  },
  {
    name: "deleteWorkspace",
    call: () => api.deleteWorkspace(["a", "b"], { delete_worktree: true }),
    url: "/api/workspaces",
    method: "DELETE",
    body: { session_ids: ["a", "b"], delete_worktree: true },
    respond: json({ deleted: ["a"], failed: [{ id: "b", error: "boom" }], messages: ["m"] }),
    result: { ok: true, messages: ["m"], deleted: ["a"], failed: [{ id: "b", error: "boom" }] },
  },
  {
    name: "attachSessionProject",
    call: () => api.attachSessionProject("s1", "/r"),
    url: "/api/sessions/s1/projects",
    method: "POST",
    body: { project: "/r", attach_existing_branch: false },
    respond: json({
      worker: "restart_failed",
      worker_message: "boom",
      attached: { name: "r", branch: "b", branch_created: false, moved_to: "/w" },
      warnings: ["w"],
    }),
    result: {
      ok: true,
      worker: "restart_failed",
      message: "boom",
      name: "r",
      branch: "b",
      branchCreated: false,
      movedTo: "/w",
      warnings: ["w"],
    },
  },
  { name: "fetchMcpServers", call: () => api.fetchMcpServers(), url: "/api/mcp/servers" },
  {
    name: "fetchMcpServers agent",
    call: () => api.fetchMcpServers("my agent"),
    url: "/api/mcp/servers?agent=my%20agent",
  },
  {
    name: "resolveMcpConflict",
    call: () => api.resolveMcpConflict("a/b", "claude", "aoe", "fp"),
    url: "/api/mcp/servers/a%2Fb/resolve",
    method: "POST",
    body: { agent: "claude", winner: "aoe", fingerprint: "fp" },
    result: "applied",
  },
  {
    name: "keepMcpServer",
    call: () => api.keepMcpServer("fs", "claude"),
    url: "/api/mcp/servers/fs/keep",
    method: "POST",
    body: { agent: "claude" },
    result: true,
  },
  {
    name: "dropMcpServer",
    call: () => api.dropMcpServer("fs", "claude"),
    url: "/api/mcp/servers/fs/drop",
    method: "POST",
    body: { agent: "claude" },
    result: true,
  },
  {
    name: "fetchSkills",
    call: () => api.fetchSkills(),
    url: "/api/skills",
    respond: json({ skills: [], roots: [] }),
    result: { skills: [], roots: [] },
  },
  {
    name: "fetchSkill",
    call: () => api.fetchSkill("claude user", "review/a"),
    url: "/api/skills/claude%20user/review%2Fa",
    respond: json(skill),
    result: skill,
  },
  {
    name: "createSkill",
    call: () => api.createSkill("mine", "Mine"),
    url: "/api/skills",
    method: "POST",
    body: { directory: "mine", description: "Mine" },
    respond: json({ directory: "mine" }, 201),
    result: { ok: true, directory: "mine", status: 201 },
  },
  {
    name: "updateSkill",
    call: () => api.updateSkill("mine", "c"),
    url: "/api/skills/mine",
    method: "PUT",
    body: { content: "c" },
    respond: json({}),
    result: { ok: true, status: 200 },
  },
  {
    name: "adoptSkill",
    call: () => api.adoptSkill("claude-user", "review", "adopted"),
    url: "/api/skills/claude-user/review/adopt",
    method: "POST",
    body: { destination: "adopted" },
  },
  {
    name: "deleteSkill",
    call: () => api.deleteSkill("mine"),
    url: "/api/skills/mine",
    method: "DELETE",
    result: { ok: true, status: 200 },
  },
  {
    name: "syncSkills",
    call: () => api.syncSkills(),
    url: "/api/skills/sync",
    method: "POST",
    body: {},
    respond: json({ outcomes: [] }),
    result: { ok: true, outcomes: [], status: 200 },
  },
  {
    name: "syncSkills options",
    call: () => api.syncSkills({ roots: ["r"], replace: ["x"], directories: ["d"] }),
    url: "/api/skills/sync",
    method: "POST",
    body: { roots: ["r"], replace: ["x"], directories: ["d"] },
  },
  {
    name: "fetchPlugins",
    call: () => api.fetchPlugins(),
    url: "/api/plugins",
    respond: json(plugins),
    result: plugins,
  },
  { name: "fetchPluginCommands", call: () => api.fetchPluginCommands(), url: "/api/plugins/commands" },
  { name: "fetchPluginUiState", call: () => api.fetchPluginUiState(), url: "/api/plugins/ui-state" },
  {
    name: "invokePluginCommand",
    call: () => api.invokePluginCommand("plugin.a.b", "s1"),
    url: "/api/plugins/commands/plugin.a.b/invoke",
    method: "POST",
    body: { session_id: "s1" },
    respond: empty(202),
    result: true,
  },
  {
    name: "invokePluginAction",
    call: () => api.invokePluginAction("p", "m", "s1"),
    url: "/api/plugins/p/action",
    method: "POST",
    body: { method: "m", params: {}, session_id: "s1" },
    respond: json({ baseline_revision: 3 }),
    result: { baselineRevision: 3 },
  },
  {
    name: "invokePluginAction without baseline",
    call: () => api.invokePluginAction("p", "m"),
    url: "/api/plugins/p/action",
    method: "POST",
    body: { method: "m", params: {}, session_id: null },
    respond: json({}),
    result: { baselineRevision: null },
  },
  {
    name: "resolvePluginOptions",
    call: () => api.resolvePluginOptions("acme.cron", "acp_agents", ["x"]),
    url: "/api/plugins/acme.cron/settings/options/resolve",
    method: "POST",
    body: { source: "acp_agents", depends: ["x"] },
    respond: json({ options: [{ value: "v", label: "l" }] }),
    result: [{ value: "v", label: "l" }],
  },
  {
    name: "resolvePluginOptions no options",
    call: () => api.resolvePluginOptions("p", "s", []),
    url: "/api/plugins/p/settings/options/resolve",
    method: "POST",
    body: { source: "s", depends: [] },
    respond: json({}),
    result: [],
  },
  {
    name: "setPluginEnabled",
    call: () => api.setPluginEnabled("acme/weird id", false),
    url: "/api/plugins/acme%2Fweird%20id/enabled",
    method: "POST",
    body: { enabled: false },
    respond: json(plugins),
    result: { kind: "ok", data: plugins },
  },
  {
    name: "fetchPluginUpdates",
    call: () => api.fetchPluginUpdates(),
    url: "/api/plugins/updates",
    respond: json({ updates: [] }),
    result: { kind: "ok", updates: [] },
  },
  {
    name: "discoverPlugins",
    call: () => api.discoverPlugins(" foo "),
    url: "/api/plugins/discover?q=foo",
    respond: json({ results: [] }),
    result: { kind: "ok", results: [] },
  },
  { name: "discoverPlugins blank", call: () => api.discoverPlugins("  "), url: "/api/plugins/discover" },
  {
    name: "fetchPluginDetails",
    call: () => api.fetchPluginDetails("gh:a/b"),
    url: "/api/plugins/details?source=gh%3Aa%2Fb",
    respond: json({ source: "gh:a/b" }),
    result: { kind: "ok", detail: { source: "gh:a/b" } },
  },
  {
    name: "previewPluginUpdate",
    call: () => api.previewPluginUpdate("acme.plugin"),
    url: "/api/plugins/acme.plugin/update/preview",
    respond: json(preview),
    result: { kind: "ok", preview },
  },
  {
    name: "previewPluginInstall",
    call: () => api.previewPluginInstall("gh:a/b"),
    url: "/api/plugins/install/preview",
    method: "POST",
    body: { source: "gh:a/b" },
    respond: json({ fingerprint: "f" }),
    result: { kind: "ok", consent: { fingerprint: "f" } },
  },
  {
    name: "applyPluginUpdate",
    call: () => api.applyPluginUpdate("acme.plugin", "fp"),
    url: "/api/plugins/acme.plugin/update/apply",
    method: "POST",
    body: { expected_fingerprint: "fp" },
    respond: json({ job_id: "job1" }, 202),
    result: { kind: "ok", jobId: "job1" },
  },
  {
    name: "startPluginInstall",
    call: () => api.startPluginInstall("gh:a/b", "fp"),
    url: "/api/plugins/install",
    method: "POST",
    body: { source: "gh:a/b", expected_fingerprint: "fp" },
    respond: json({ job_id: "j" }),
    result: { kind: "ok", jobId: "j" },
  },
  {
    name: "startPluginUninstall",
    call: () => api.startPluginUninstall("p"),
    url: "/api/plugins/p/uninstall",
    method: "POST",
    body: {},
    respond: json({ job_id: "j" }),
  },
  {
    name: "fetchPluginJob",
    call: () => api.fetchPluginJob("j/1"),
    url: "/api/plugins/jobs/j%2F1?tail=200",
    respond: json({ job: { id: "j" } }),
    result: { kind: "ok", job: { job: { id: "j" } } },
  },
  {
    name: "dismissPluginUpdate",
    call: () => api.dismissPluginUpdate("p", "fp"),
    url: "/api/plugins/p/update/dismiss",
    method: "POST",
    body: { fingerprint: "fp" },
    result: { kind: "ok" },
  },
];

describe("request shapes", () => {
  it.each(requestCases)("$name", async ({ call, url, method, body, respond, result }) => {
    fetchSpy.mockResolvedValueOnce(respond ?? empty());
    const out = await call();
    const last = lastCall();
    expect(last.url).toBe(url);
    expect(last.init?.method ?? "GET").toBe(method ?? "GET");
    if (body === undefined) expect(last.init?.body).toBeUndefined();
    else expect(bodyOf(last.init)).toEqual(body);
    if (result !== undefined) expect(out).toEqual(result);
  });
});

const failureCases: [string, () => Promise<unknown>, unknown][] = [
  ["fetchSessions", () => api.fetchSessions(), null],
  ["fetchRecentProjects", () => api.fetchRecentProjects(), null],
  ["searchConversations", () => api.searchConversations("q"), []],
  ["updateWorkspaceOrdering", () => api.updateWorkspaceOrdering([]), false],
  ["ensureTerminal", () => api.ensureTerminal("s1"), false],
  ["getSessionFileContents", () => api.getSessionFileContents("s1", "a"), null],
  ["fetchVolumeIgnoresPreview", () => api.fetchVolumeIgnoresPreview("/r"), null],
  ["markTipSeen", () => api.markTipSeen("x"), false],
  ["setTelemetryConsent", () => api.setTelemetryConsent(true), null],
  ["fetchThemes", () => api.fetchThemes(), []],
  ["fetchAcpAgents", () => api.fetchAcpAgents(), []],
  ["fetchAcpOptionCatalog", () => api.fetchAcpOptionCatalog(), { version: 1, agents: {} }],
  ["fetchIsGitRepo", () => api.fetchIsGitRepo("/r"), null],
  ["getHomePath", () => api.getHomePath(), null],
  ["browseFilesystem", () => api.browseFilesystem("/r"), { entries: [], has_more: false, ok: false }],
  ["fetchDockerStatus", () => api.fetchDockerStatus(), { available: false, runtime: null }],
  [
    "loginStatus",
    () => api.loginStatus(),
    { required: false, authenticated: true, elevated: true, elevated_until_secs: null },
  ],
  ["verifyToken", () => api.verifyToken(), false],
  ["enqueueServerPrompt", () => api.enqueueServerPrompt("s1", { id: "q", text: "t" }), null],
  ["listServerQueue", () => api.listServerQueue("s1"), []],
  ["clearServerQueue", () => api.clearServerQueue("s1"), false],
  ["setSessionPin", () => api.setSessionPin("s1", true), null],
  ["trashSession", () => api.trashSession("s1"), null],
  ["updateSessionGroup", () => api.updateSessionGroup("s1", "g"), false],
  ["renameSession", () => api.renameSession("s1", "x"), { ok: false }],
  ["smartRenameSession", () => api.smartRenameSession("s1"), { ok: false }],
  ["summarizeSession", () => api.summarizeSession("s1"), { ok: false }],
  ["setWorktreeName", () => api.setWorktreeName("s1", "x", false), { ok: false }],
  ["attachSessionProject", () => api.attachSessionProject("s1", "p"), { ok: false }],
  ["resolveMcpConflict", () => api.resolveMcpConflict("n", "a", "aoe", "fp"), "error"],
  ["keepMcpServer", () => api.keepMcpServer("n", "a"), false],
  ["fetchSkill", () => api.fetchSkill("s", "d"), null],
  ["fetchPlugins", () => api.fetchPlugins(), null],
  ["invokePluginAction", () => api.invokePluginAction("p", "m"), null],
  ["resolvePluginOptions", () => api.resolvePluginOptions("p", "s", []), []],
  ["fetchSoundBlob", () => api.fetchSoundBlob("x.wav"), null],
];

describe("failure fallbacks", () => {
  it.each(failureCases)("%s on non-2xx and network failure", async (_name, call, fallback) => {
    fetchSpy.mockResolvedValueOnce(new Response("nope", { status: 500 }));
    expect(await call()).toEqual(fallback);
    offline();
    expect(await call()).toEqual(fallback);
  });
});

describe("ensureSession", () => {
  it.each([
    ["success", json({ status: "restarted" }), { ok: true, status: "restarted" }],
    [
      "server error",
      json({ error: "boom", message: "no good" }, 500),
      { ok: false, error: "boom", message: "no good" },
    ],
    ["empty error body", empty(502), { ok: false, message: "Server error (502)" }],
  ])("%s", async (_name, response, expected) => {
    fetchSpy.mockResolvedValueOnce(response);
    expect(await api.ensureSession("s1")).toEqual(expected);
    expect(lastCall()).toMatchObject({ url: "/api/sessions/s1/ensure", init: { method: "POST" } });
  });

  it("distinguishes an abort from a network failure", async () => {
    fetchSpy.mockRejectedValueOnce(Object.assign(new Error("x"), { name: "AbortError" }));
    expect(await api.ensureSession("s1")).toEqual({ ok: false, error: "aborted" });
    offline();
    expect(await api.ensureSession("s1")).toEqual({ ok: false, message: "offline" });
  });
});

it("forwards abort signals", async () => {
  const { signal } = new AbortController();
  fetchSpy.mockResolvedValueOnce(json({ results: [] }));
  await api.searchConversations("q", signal);
  expect(lastCall().init?.signal).toBe(signal);
  fetchSpy.mockResolvedValueOnce(json({}));
  await api.fetchContextPrimer("s1", 1, signal);
  expect(lastCall().init?.signal).toBe(signal);
});

it("pasteImage uploads raw base64 and returns the host path", async () => {
  fetchSpy.mockResolvedValueOnce(json({ path: "/wt/img.png" }));
  const file = new File(["hi"], "img.png", { type: "image/png" });
  expect(await api.pasteImage("s1", file)).toBe("/wt/img.png");
  expect(lastCall().url).toBe("/api/sessions/s1/paste-image");
  expect(bodyOf(lastCall().init)).toEqual({ mime_type: "image/png", data: btoa("hi") });
  fetchSpy.mockResolvedValueOnce(json({}));
  expect(await api.pasteImage("s1", file)).toBeNull();
});

it("fetchSoundBlob returns the file as a Blob", async () => {
  fetchSpy.mockResolvedValueOnce(new Response("bytes"));
  expect(await (await api.fetchSoundBlob("my sound.wav"))!.text()).toBe("bytes");
  expect(lastCall().url).toBe("/api/sounds/file/my%20sound.wav");
});

describe("fetchCityHallBundle", () => {
  it("returns the TOML body", async () => {
    fetchSpy.mockResolvedValueOnce(new Response("schema_version = 1\n"));
    await expect(api.fetchCityHallBundle()).resolves.toBe("schema_version = 1\n");
    expect(fetchSpy).toHaveBeenCalledWith("/api/cityhall/bundle");
  });

  it.each([
    [json({ message: "disabled in CityHall client mode" }, 403), "disabled in CityHall client mode"],
    [new Response("<html>502</html>", { status: 502 }), "Export failed (HTTP 502)"],
  ])("throws the server message or status", async (response, message) => {
    fetchSpy.mockResolvedValueOnce(response);
    await expect(api.fetchCityHallBundle()).rejects.toThrow(message);
  });
});

describe("installAcpAgent", () => {
  it("POSTs and returns the parsed body", async () => {
    const body = { session_id: "s-1", package: "p", success: true, exit_code: 0, stdout: "", stderr: "" };
    fetchSpy.mockResolvedValueOnce(json(body));
    expect(await api.installAcpAgent("weird/id")).toEqual(body);
    expect(lastCall()).toMatchObject({ url: "/api/sessions/weird%2Fid/acp/install-agent", init: { method: "POST" } });
  });

  it.each([
    ["server message", json({ error: "install_disabled", message: "Installing is off." }, 403), "Installing is off."],
    ["error code", json({ error: "install_disabled" }, 403), "install_disabled"],
    ["status fallback", new Response("boom", { status: 500 }), "Server returned 500"],
    ["invalid 2xx body", new Response("not json"), "invalid or empty response"],
  ])("throws on %s", async (_name, response, message) => {
    fetchSpy.mockResolvedValueOnce(response);
    await expect(api.installAcpAgent("s-1")).rejects.toThrow(message);
  });
});

describe("project mutations", () => {
  const calls: [string, () => Promise<{ ok: boolean; error?: string }>][] = [
    ["createProject", () => api.createProject({ path: "/p" })],
    ["deleteProject", () => api.deleteProject("p", "global")],
    ["updateProject", () => api.updateProject("p", "global", "x")],
    ["setProjectPinned", () => api.setProjectPinned("p", "global", true)],
  ];

  it.each(calls)("%s maps JSON, text, and network errors", async (_name, call) => {
    fetchSpy.mockResolvedValueOnce(new Response(JSON.stringify({ message: "dup" }), { status: 409 }));
    expect(await call()).toEqual({ ok: false, error: "dup" });
    fetchSpy.mockResolvedValueOnce(new Response("boom", { status: 500 }));
    expect(await call()).toEqual({ ok: false, error: "boom" });
    fetchSpy.mockResolvedValueOnce(empty(500));
    expect(await call()).toEqual({ ok: false, error: "Server error (500)" });
    offline();
    expect(await call()).toEqual({ ok: false, error: "offline" });
  });
});

describe("createSession errors", () => {
  const body = { path: "/repo", tool: "claude" } as CreateSessionRequest;

  it.each([
    [
      "full hooks trust",
      {
        error: "hooks_need_trust",
        message: "trust me",
        on_create: ["a"],
        on_launch: ["b"],
        on_destroy: ["c"],
        needs_mcp_trust: true,
      },
      { onCreate: ["a"], onLaunch: ["b"], onDestroy: ["c"], needsMcpTrust: true },
    ],
    [
      "hooks trust defaults",
      { error: "hooks_need_trust", message: "trust me" },
      { onCreate: [], onLaunch: [], onDestroy: [], needsMcpTrust: false },
    ],
  ])("surfaces %s", async (_name, payload, hooksNeedTrust) => {
    fetchSpy.mockResolvedValueOnce(new Response(JSON.stringify(payload), { status: 403 }));
    expect(await api.createSession(body)).toEqual({ ok: false, error: "trust me", hooksNeedTrust });
  });

  it("maps plain JSON, text, and network errors", async () => {
    fetchSpy.mockResolvedValueOnce(
      new Response(JSON.stringify({ error: "create_failed", message: "nope" }), { status: 400 }),
    );
    expect(await api.createSession(body)).toEqual({ ok: false, error: "nope" });
    fetchSpy.mockResolvedValueOnce(new Response("boom", { status: 500 }));
    expect(await api.createSession(body)).toEqual({ ok: false, error: "Server error (500): boom" });
    offline();
    expect(await api.createSession(body)).toEqual({ ok: false, error: "Network error: offline" });
  });
});

it("cloneRepo maps server and network errors", async () => {
  fetchSpy.mockResolvedValueOnce(json({ message: "no repo" }, 404));
  expect(await api.cloneRepo("u")).toEqual({ ok: false, error: "no repo" });
  fetchSpy.mockResolvedValueOnce(empty(500));
  expect(await api.cloneRepo("u")).toEqual({ ok: false, error: "Clone failed (500)" });
  offline();
  expect(await api.cloneRepo("u")).toEqual({ ok: false, error: "Network error: offline" });
});

describe("login", () => {
  it("login sends the passphrase with a device-binding secret", async () => {
    fetchSpy.mockResolvedValueOnce(empty());
    expect(await api.login("hunter2")).toEqual({ ok: true });
    const { url, init } = lastCall();
    expect(url).toBe("/api/login");
    const sent = bodyOf(init);
    expect(sent.passphrase).toBe("hunter2");
    expect(sent.device_binding_secret).toMatch(/.+/);
  });

  it("elevateLogin sends the binding header and returns the window", async () => {
    fetchSpy.mockResolvedValueOnce(json({ elevated_until_secs: 900 }));
    expect(await api.elevateLogin("hunter2")).toEqual({ ok: true, elevated_until_secs: 900 });
    const { url, init } = lastCall();
    expect(url).toBe("/api/login/elevate");
    expect((init?.headers as Record<string, string>)["X-Aoe-Device-Binding"]).toBeTruthy();
    expect(bodyOf(init)).toEqual({ passphrase: "hunter2" });
  });

  it.each([
    ["login", api.login, "Login failed (401)"],
    ["elevateLogin", api.elevateLogin, "Elevation failed (401)"],
  ] as const)("%s maps server and network errors", async (_name, call, statusMessage) => {
    fetchSpy.mockResolvedValueOnce(json({ message: "wrong" }, 401));
    expect(await call("bad")).toEqual({ ok: false, error: "wrong" });
    fetchSpy.mockResolvedValueOnce(empty(401));
    expect(await call("bad")).toEqual({ ok: false, error: statusMessage });
    offline();
    expect(await call("x")).toEqual({ ok: false, error: "Network error" });
  });

  it("logout POSTs and resolves even when the request fails", async () => {
    fetchSpy.mockResolvedValueOnce(empty());
    await api.logout();
    expect(lastCall()).toMatchObject({ url: "/api/logout", init: { method: "POST" } });
    offline();
    await expect(api.logout()).resolves.toBeUndefined();
  });
});

describe("session mutation messages", () => {
  it("renameSession keeps only string warnings", async () => {
    fetchSpy.mockResolvedValueOnce(json({ warnings: ["kept", 3, null] }));
    expect(await api.renameSession("s1", "T")).toEqual({ ok: true, warnings: ["kept"] });
  });

  it.each([
    ["renameSession", () => api.renameSession("s1", "x")],
    ["smartRenameSession", () => api.smartRenameSession("s1")],
    ["summarizeSession", () => api.summarizeSession("s1")],
    ["setWorktreeName", () => api.setWorktreeName("s1", "x", false)],
    ["attachSessionProject", () => api.attachSessionProject("s1", "p")],
  ])("%s surfaces the server message", async (_name, call) => {
    fetchSpy.mockResolvedValueOnce(json({ message: "running" }, 409));
    expect(await call()).toEqual({ ok: false, message: "running" });
  });
});

describe("deleteWorkspace errors", () => {
  it("treats a 2xx without a deleted array as unconfirmed", async () => {
    fetchSpy.mockResolvedValueOnce(json({ status: "ok" }));
    expect(await api.deleteWorkspace(["a"])).toEqual({
      ok: false,
      error: "Server did not confirm which sessions were deleted",
    });
  });

  it("maps server and network errors", async () => {
    const failed = [{ id: "a", error: "dirty" }];
    fetchSpy.mockResolvedValueOnce(json({ message: "dirty", failed }, 500));
    expect(await api.deleteWorkspace(["a"])).toEqual({ ok: false, error: "dirty", failed });
    fetchSpy.mockResolvedValueOnce(empty(500));
    expect(await api.deleteWorkspace(["a"])).toEqual({ ok: false, error: "Server error (500)" });
    offline();
    expect(await api.deleteWorkspace(["a"])).toEqual({ ok: false, error: "Network error: offline" });
  });
});

it("resolveMcpConflict maps 409 to stale", async () => {
  fetchSpy.mockResolvedValueOnce(empty(409));
  expect(await api.resolveMcpConflict("n", "a", "native", "fp")).toBe("stale");
});

it("skill mutations keep the status and message", async () => {
  fetchSpy.mockResolvedValueOnce(json({ message: "already exists" }, 409));
  expect(await api.createSkill("mine")).toEqual({ ok: false, error: "already exists", status: 409 });
  offline();
  expect(await api.deleteSkill("mine")).toEqual({ ok: false, error: "Network error: offline" });
  fetchSpy.mockResolvedValueOnce(json({ message: "read only" }, 403));
  expect(await api.syncSkills()).toEqual({ ok: false, outcomes: [], error: "read only", status: 403 });
  fetchSpy.mockResolvedValueOnce(empty(500));
  expect(await api.syncSkills()).toEqual({ ok: false, outcomes: [], error: "Server error (500)", status: 500 });
  offline();
  expect(await api.syncSkills()).toEqual({ ok: false, outcomes: [], error: "Network error: offline" });
});

describe("plugin results", () => {
  const cases: [string, () => Promise<unknown>, string, unknown][] = [
    ["fetchPluginUpdates", () => api.fetchPluginUpdates(), "Update check failed (HTTP 500).", {}],
    ["discoverPlugins", () => api.discoverPlugins("q"), "Discovery failed (HTTP 500).", {}],
    ["fetchPluginDetails", () => api.fetchPluginDetails("s"), "Details failed (HTTP 500).", {}],
    ["previewPluginUpdate", () => api.previewPluginUpdate("p"), "Update preview failed (HTTP 500).", { kind: "bogus" }],
    ["previewPluginInstall", () => api.previewPluginInstall("s"), "Install preview failed (HTTP 500).", {}],
    ["applyPluginUpdate", () => api.applyPluginUpdate("p", null), "Request failed (HTTP 500).", { nope: true }],
    ["setPluginEnabled", () => api.setPluginEnabled("p", true), "Failed to enable plugin (500).", { nope: true }],
    ["dismissPluginUpdate", () => api.dismissPluginUpdate("p", "f"), "Dismiss failed (HTTP 500).", undefined],
  ];

  it.each(cases)("%s maps failures to an error message", async (_name, call, statusMessage, malformed) => {
    fetchSpy.mockResolvedValueOnce(json({ message: "boom" }, 400));
    expect(await call()).toEqual({ kind: "error", message: "boom" });
    fetchSpy.mockResolvedValueOnce(new Response("not json", { status: 500 }));
    expect(await call()).toEqual({ kind: "error", message: statusMessage });
    offline();
    expect(await call()).toEqual({ kind: "error", message: "Network error." });
    if (malformed !== undefined) {
      fetchSpy.mockResolvedValueOnce(json(malformed));
      expect(await call()).toMatchObject({ kind: "error" });
    }
  });

  it("previewPluginUpdate rejects payloads missing per-kind fields", async () => {
    for (const bad of [
      { kind: "safe_update", to_version: "2" },
      { kind: "consent_required", dismissed: false },
    ]) {
      fetchSpy.mockResolvedValueOnce(json(bad));
      expect((await api.previewPluginUpdate("p")).kind).toBe("error");
    }
    const noUpdate = { kind: "no_update" };
    fetchSpy.mockResolvedValueOnce(json(noUpdate));
    expect(await api.previewPluginUpdate("p")).toEqual({ kind: "ok", preview: noUpdate });
  });

  it("fetchPluginJob keeps the HTTP status on failure", async () => {
    fetchSpy.mockResolvedValueOnce(json({ message: "gone" }, 404));
    expect(await api.fetchPluginJob("j")).toEqual({ kind: "error", status: 404, message: "gone" });
    fetchSpy.mockResolvedValueOnce(json({}, 200));
    expect(await api.fetchPluginJob("j")).toEqual({
      kind: "error",
      status: 200,
      message: "Job status failed (HTTP 200).",
    });
    offline();
    expect(await api.fetchPluginJob("j")).toEqual({ kind: "error", status: 0, message: "Network error." });
  });
});

describe("telemetry pings", () => {
  it.each([
    ["reportTelemetrySeen", () => api.reportTelemetrySeen("web"), "/api/telemetry/seen", { surface: "web" }],
    [
      "reportAcpInteraction",
      () => api.reportAcpInteraction("prompt_queued"),
      "/api/telemetry/structured-interaction",
      { kind: "prompt_queued" },
    ],
  ])("%s POSTs fire-and-forget and swallows failures", (_name, call, url, body) => {
    fetchSpy.mockResolvedValueOnce(empty());
    call();
    expect(lastCall()).toMatchObject({ url, init: { method: "POST" } });
    expect(bodyOf(lastCall().init)).toMatchObject(body);
    offline();
    expect(call).not.toThrow();
  });

  it("reportTelemetrySeen includes the form factor", () => {
    fetchSpy.mockResolvedValueOnce(empty());
    api.reportTelemetrySeen("diff_panel");
    expect(bodyOf(lastCall().init)).toHaveProperty("form_factor");
  });
});

it.each([
  [{ build_flavor: "debug" }, true],
  [{ build_flavor: "release" }, false],
  [null, false],
  [undefined, false],
])("isDebugBuild(%o) is %s", (about, expected) => {
  expect(api.isDebugBuild(about as ServerAbout | null | undefined)).toBe(expected);
});

describe("profile settings write guard", () => {
  const field = (section: string): SettingsFieldDescriptor => ({
    section,
    field: "f",
    category: "Test",
    label: "f",
    description: "",
    widget: { kind: "toggle" },
    web_write: { policy: "allow" },
    profile_overridable: true,
    validation: { rule: "none" },
    advanced: false,
  });
  const schema = [field("theme"), field("session")];

  beforeEach(() => api.resetSettingsSchemaCache());

  it("derives writable sections from the schema plus description", () => {
    const writable = api.profileWritableSections(schema);
    expect([...writable].sort()).toEqual(["description", "session", "theme"]);
  });

  it("caches a successful schema fetch and retries a failed one", async () => {
    fetchSpy.mockResolvedValueOnce(empty(503));
    expect(await api.getSettingsSchema()).toBeNull();
    fetchSpy.mockResolvedValueOnce(json(schema));
    expect(await api.getSettingsSchema()).toEqual(schema);
    expect(await api.getSettingsSchema()).toEqual(schema);
    expect(fetchSpy).toHaveBeenCalledTimes(2);
    expect(lastCall().url).toBe("/api/settings/schema");
  });

  it.each([
    ["hooks", { hooks: { on_create: ["rm -rf /"] } }],
    ["a blocked key beside an allowed one", { theme: { name: "empire" }, custom_agents: { evil: "x" } }],
  ])("refuses %s without sending", async (_name, updates) => {
    fetchSpy.mockResolvedValueOnce(json(schema));
    const errSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    expect(await api.updateProfileSettings("work", updates)).toBe(false);
    expect(fetchSpy).toHaveBeenCalledTimes(1);
    expect(errSpy).toHaveBeenCalled();
    errSpy.mockRestore();
  });

  it("PATCHes an allowed section", async () => {
    fetchSpy.mockResolvedValueOnce(json(schema)).mockResolvedValueOnce(empty());
    expect(await api.updateProfileSettings("work", { description: "mine" })).toBe(true);
    expect(lastCall()).toMatchObject({ url: "/api/profiles/work/settings", init: { method: "PATCH" } });
    expect(bodyOf(lastCall().init)).toEqual({ description: "mine" });
  });

  it("defers to the server when the schema is unavailable", async () => {
    fetchSpy.mockResolvedValueOnce(empty(503)).mockResolvedValueOnce(empty());
    expect(await api.updateProfileSettings("work", { hooks: {} })).toBe(true);
    expect(fetchSpy).toHaveBeenCalledTimes(2);
    expect(lastCall().url).toBe("/api/profiles/work/settings");
  });
});
