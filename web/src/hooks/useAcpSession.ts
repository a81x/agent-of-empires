import { useCallback, useEffect, useReducer, useRef, useState, useSyncExternalStore } from "react";
import {
  appendElicitationAnswerRow,
  applyEvent,
  applyReducedState,
  emptyAcpState,
  deriveTurnActive,
  mergePrependedActivity,
  mergeServerRows,
  normaliseTurnState,
  patchServerRow,
  reduceFrames,
  summarizeAnswers,
  transcriptRowToActivity,
  webRendersServerRow,
  type ActivityRow,
  type ApprovalDecision,
  type AcpAttachment,
  type AcpFrame,
  type AcpState,
  type BackgroundAgent,
  type ElicitationResolution,
  type PromptAttachmentInput,
  type QueuedPrompt,
  type ReducedState,
  type TranscriptDelta,
  type TranscriptRow,
} from "../lib/acpTypes";
import { getOrCreateDeviceBindingSecret } from "../lib/deviceBinding";
import { safeSetItem } from "../lib/safeStorage";
import {
  STORAGE_KEY_PREFIX,
  STATE_TTL_MS,
  clearQueueCount,
  setQueueCount,
  type PersistedEntry,
} from "../lib/acpStateStorage";
import { getToken } from "../lib/token";
import {
  clearServerQueue,
  editServerQueuedPrompt,
  enqueueServerPrompt,
  listServerQueue,
  removeServerQueuedPrompt,
  reportAcpInteraction,
  setSessionArchive,
  setSessionSnooze,
  type ServerQueuedPrompt,
} from "../lib/api";

type PromptSendResult =
  | { kind: "dispatched" }
  | { kind: "queued"; queuedId: string }
  | { kind: "retryable_failure" }
  | { kind: "non_retryable_failure" };

interface PromptDispatchBody {
  disposition?: "sent" | "steered" | "queued";
  queued_id?: string;
}

export type Action =
  | { kind: "frame"; frame: AcpFrame }
  | { kind: "reduced_state"; state: ReducedState; unchanged: string[] }
  | { kind: "frames"; frames: AcpFrame[]; rows?: ActivityRow[]; oldestSeq?: number }
  | { kind: "prepend"; rows: ActivityRow[]; oldestSeq: number }
  | { kind: "handshake"; frames: AcpFrame[] }
  | { kind: "transcript_snapshot"; rows: ActivityRow[] }
  | { kind: "transcript_append"; row: ActivityRow }
  | { kind: "transcript_patch"; row: ActivityRow }
  | { kind: "transcript_remove"; id: string }
  | { kind: "lagged"; skipped: number }
  | { kind: "user_prompt"; text: string; attachments?: AcpAttachment[]; id?: string }
  | { kind: "prompt_send_rejected"; id: string }
  | { kind: "settle_inflight_prompt"; id: string }
  | { kind: "rollback_optimistic_prompt"; id: string }
  | { kind: "error"; message: string }
  | { kind: "clear_error" }
  | { kind: "approval_resolved_locally"; nonce: string }
  | { kind: "elicitation_resolved_locally"; nonce: string; resolution: ElicitationResolution }
  | { kind: "lagged_resolved" }
  | { kind: "reset" }
  | { kind: "hydrate"; state: AcpState }
  | {
      kind: "enqueue_prompt";
      id: string;
      text: string;
      attachments?: PromptAttachmentInput[];
    }
  | { kind: "dequeue_prompt"; id: string }
  | { kind: "edit_queued_prompt"; id: string; text: string }
  | { kind: "clear_queue" }
  | { kind: "hydrate_server_queue"; rows: ServerQueuedPrompt[] }
  | { kind: "confirm_queued_prompt"; id: string }
  | { kind: "dismiss_primer" }
  | { kind: "dismiss_compaction_reminder" }
  | { kind: "dismiss_rejected_prompt"; id: string }
  | { kind: "dismiss_mode_switch_failed" }
  | { kind: "set_pending_config_option"; configId: string; value: string }
  | { kind: "clear_pending_config_option" }
  | {
      kind: "clear_pending_config_option_if_match";
      configId: string;
      value: string;
    }
  | { kind: "dismiss_config_option_switch_failed" };

const STATE_CACHE_CAP = 32;
const stateCache = new Map<string, AcpState>();

function storageKey(sessionId: string): string {
  return STORAGE_KEY_PREFIX + sessionId;
}

function evictOldestPersistedAcpState(currentKey: string): boolean {
  if (typeof window === "undefined") return false;
  try {
    let oldestKey: string | null = null;
    let oldestTime = Infinity;
    let firstCorruptKey: string | null = null;
    for (let i = 0; i < window.localStorage.length; i++) {
      const k = window.localStorage.key(i);
      if (!k || !k.startsWith(STORAGE_KEY_PREFIX)) continue;
      if (k === currentKey) continue;
      const raw = window.localStorage.getItem(k);
      if (raw === null) continue;
      try {
        const parsed = JSON.parse(raw) as PersistedEntry | null;
        if (!parsed || typeof parsed.savedAt !== "number" || Number.isNaN(parsed.savedAt)) {
          if (firstCorruptKey === null) firstCorruptKey = k;
          continue;
        }
        if (parsed.savedAt < oldestTime) {
          oldestTime = parsed.savedAt;
          oldestKey = k;
        }
      } catch {
        if (firstCorruptKey === null) firstCorruptKey = k;
      }
    }
    const victim = firstCorruptKey ?? oldestKey;
    if (!victim) return false;
    window.localStorage.removeItem(victim);
    return true;
  } catch {
    return false;
  }
}

function toPersistedState(state: AcpState): AcpState {
  const base: AcpState =
    state.optimisticRows.length > 0 || state.inflightPromptIds.length > 0
      ? { ...state, optimisticRows: [], inflightPromptIds: [] }
      : state;
  if (!base.queuedPrompts.some((q) => q.attachments?.length)) return base;
  return {
    ...base,
    queuedPrompts: base.queuedPrompts.filter((q) => !q.attachments?.length),
  };
}

function persistState(sessionId: string, state: AcpState): void {
  const key = storageKey(sessionId);
  const body = JSON.stringify({
    savedAt: Date.now(),
    state: toPersistedState(state),
  } satisfies PersistedEntry);
  if (safeSetItem(key, body)) {
    setQueueCount(sessionId, state.queuedPrompts.length);
    return;
  }
  if (!evictOldestPersistedAcpState(key)) return;
  if (safeSetItem(key, body)) {
    setQueueCount(sessionId, state.queuedPrompts.length);
  }
}

export const __test = {
  persistState,
  loadPersistedState,
  evictOldestPersistedAcpState,
  STORAGE_KEY_PREFIX,
};

function loadPersistedState(sessionId: string): AcpState | undefined {
  if (typeof window === "undefined") return undefined;
  try {
    const raw = window.localStorage.getItem(storageKey(sessionId));
    if (!raw) return undefined;
    const parsed = JSON.parse(raw) as PersistedEntry | null;
    if (!parsed || typeof parsed.savedAt !== "number" || typeof parsed.state !== "object" || parsed.state === null) {
      return undefined;
    }
    if (Date.now() - parsed.savedAt > STATE_TTL_MS) {
      window.localStorage.removeItem(storageKey(sessionId));
      return undefined;
    }
    const state = parsed.state as Partial<AcpState>;
    if (typeof state.lastSeq !== "number" || !Array.isArray(state.activity) || !Array.isArray(state.queuedPrompts)) {
      window.localStorage.removeItem(storageKey(sessionId));
      return undefined;
    }
    const merged: AcpState = { ...emptyAcpState(), ...(state as AcpState) };
    return normaliseTurnState(merged);
  } catch {
    return undefined;
  }
}

function dropPersistedState(sessionId: string): void {
  if (typeof window === "undefined") return;
  try {
    window.localStorage.removeItem(storageKey(sessionId));
  } catch {
    // Storage unavailable.
  }
}

function dropAllPersistedState(): void {
  if (typeof window === "undefined") return;
  try {
    const toRemove: string[] = [];
    for (let i = 0; i < window.localStorage.length; i++) {
      const k = window.localStorage.key(i);
      if (k && k.startsWith(STORAGE_KEY_PREFIX)) toRemove.push(k);
    }
    for (const k of toRemove) window.localStorage.removeItem(k);
  } catch {
    // Storage unavailable.
  }
}

let sweptStorage = false;
function sweepExpiredStorage(): void {
  if (sweptStorage) return;
  sweptStorage = true;
  if (typeof window === "undefined") return;
  try {
    const toRemove: string[] = [];
    const now = Date.now();
    for (let i = 0; i < window.localStorage.length; i++) {
      const k = window.localStorage.key(i);
      if (!k || !k.startsWith(STORAGE_KEY_PREFIX)) continue;
      const raw = window.localStorage.getItem(k);
      if (!raw) {
        toRemove.push(k);
        continue;
      }
      try {
        const parsed = JSON.parse(raw) as PersistedEntry | null;
        if (!parsed || typeof parsed.savedAt !== "number" || now - parsed.savedAt > STATE_TTL_MS) {
          toRemove.push(k);
        }
      } catch {
        toRemove.push(k);
      }
    }
    for (const k of toRemove) window.localStorage.removeItem(k);
  } catch {
    // Storage unavailable.
  }
}

function cacheGet(sessionId: string): AcpState | undefined {
  const value = stateCache.get(sessionId);
  if (value !== undefined) {
    stateCache.delete(sessionId);
    stateCache.set(sessionId, value);
    return value;
  }
  const persisted = loadPersistedState(sessionId);
  if (persisted !== undefined) {
    stateCache.set(sessionId, persisted);
    while (stateCache.size > STATE_CACHE_CAP) {
      const oldest = stateCache.keys().next().value;
      if (oldest === undefined) break;
      stateCache.delete(oldest);
    }
    queueMicrotask(() => notifyStateListeners(sessionId));
    return persisted;
  }
  return undefined;
}

function cacheSet(sessionId: string, value: AcpState): void {
  stateCache.delete(sessionId);
  stateCache.set(sessionId, value);
  while (stateCache.size > STATE_CACHE_CAP) {
    const oldest = stateCache.keys().next().value;
    if (oldest === undefined) break;
    stateCache.delete(oldest);
  }
  persistState(sessionId, value);
  notifyStateListeners(sessionId);
}

const stateListeners = new Map<string, Set<() => void>>();

function notifyStateListeners(sessionId: string): void {
  const set = stateListeners.get(sessionId);
  if (!set) return;
  for (const cb of set) cb();
}

function subscribeAcpState(sessionId: string, cb: () => void): () => void {
  let set = stateListeners.get(sessionId);
  if (!set) {
    set = new Set();
    stateListeners.set(sessionId, set);
  }
  set.add(cb);
  return () => {
    const s = stateListeners.get(sessionId);
    if (!s) return;
    s.delete(cb);
    if (s.size === 0) stateListeners.delete(sessionId);
  };
}

function peekAcpState(sessionId: string): AcpState | undefined {
  return stateCache.get(sessionId);
}

const EMPTY_BACKGROUND_AGENTS: BackgroundAgent[] = [];

export function useBackgroundAgents(sessionId: string | null): BackgroundAgent[] {
  const subscribe = useCallback(
    (cb: () => void) => (sessionId ? subscribeAcpState(sessionId, cb) : () => {}),
    [sessionId],
  );
  const getSnapshot = useCallback(
    () =>
      sessionId ? (peekAcpState(sessionId)?.backgroundAgents ?? EMPTY_BACKGROUND_AGENTS) : EMPTY_BACKGROUND_AGENTS,
    [sessionId],
  );
  return useSyncExternalStore(subscribe, getSnapshot);
}

const REPLAY_OVERLAP = 50;

const REPLAY_PAGE_SIZE = 1000;

const TAIL_BEFORE = Number.MAX_SAFE_INTEGER;

const HANDSHAKE_PREFIX_SIZE = 50;

type ReplayPageResponse = {
  frames: AcpFrame[];
  rows?: TranscriptRow[] | null;
  lost: boolean;
  highest_seq: number;
  next_cursor?: number | null;
  has_more?: boolean;
};

export function clearAcpCache(sessionId?: string): void {
  if (sessionId === undefined) {
    stateCache.clear();
    dropAllPersistedState();
    clearQueueCount();
  } else {
    stateCache.delete(sessionId);
    dropPersistedState(sessionId);
    clearQueueCount(sessionId);
  }
}

function initialState(sessionId: string | null): AcpState {
  if (!sessionId) return emptyAcpState();
  return cacheGet(sessionId) ?? emptyAcpState();
}

export function acpHookReducer(state: AcpState, action: Action): AcpState {
  return reducer(state, action);
}

export type ApprovalResolveOutcome = { kind: "resolved" } | { kind: "error"; message: string };

export function classifyApprovalResolveResponse(
  ok: boolean,
  status: number,
  detail: string,
  nonce: string,
): ApprovalResolveOutcome {
  if (ok) return { kind: "resolved" };
  if (status === 404 && /no pending approval/i.test(detail) && detail.includes(nonce)) {
    return { kind: "resolved" };
  }
  return {
    kind: "error",
    message: `Could not resolve approval (${status}). ${detail}`.trim(),
  };
}

export function classifyElicitationResolveResponse(
  ok: boolean,
  status: number,
  detail: string,
  nonce: string,
): ApprovalResolveOutcome {
  if (ok) return { kind: "resolved" };
  if (status === 404 && /no pending elicitation/i.test(detail) && detail.includes(nonce)) {
    return { kind: "resolved" };
  }
  return {
    kind: "error",
    message: `Could not resolve question (${status}). ${detail}`.trim(),
  };
}

function settleInflightPrompt(state: AcpState, id: string): AcpState {
  const inflightPromptIds = state.inflightPromptIds.filter((p) => p !== id);
  if (inflightPromptIds.length === state.inflightPromptIds.length) return state;
  return {
    ...state,
    inflightPromptIds,
    turnActive: deriveTurnActive({ serverTurnActive: state.serverTurnActive, inflightPromptIds }),
  };
}

function pruneOptimisticRows(state: AcpState): AcpState {
  if (state.optimisticRows.length === 0) return state;
  const serverIds = new Set(state.activity.map((r) => r.id));
  const kept = state.optimisticRows.filter((o) => !serverIds.has(o.id));
  if (kept.length === state.optimisticRows.length) return state;
  return { ...state, optimisticRows: kept };
}

export function reducer(state: AcpState, action: Action): AcpState {
  if (action.kind === "frame") {
    return applyEvent(state, action.frame);
  }
  if (action.kind === "reduced_state") {
    return applyReducedState(state, action.state, action.unchanged);
  }
  if (action.kind === "frames") {
    let next = action.frames.reduce(applyEvent, state);
    if (action.rows && action.rows.length > 0) {
      next = { ...next, activity: mergeServerRows(next.activity, action.rows) };
      next = pruneOptimisticRows(next);
    }
    if (action.oldestSeq != null && state.oldestSeq === 0) {
      return { ...next, oldestSeq: action.oldestSeq };
    }
    return next;
  }
  if (action.kind === "prepend") {
    const next = { ...state, oldestSeq: action.oldestSeq };
    if (action.rows.length === 0) return next;
    next.activity = mergePrependedActivity(action.rows, state.activity);
    return next;
  }
  if (action.kind === "transcript_snapshot") {
    if (action.rows.length === 0) return state;
    return pruneOptimisticRows({ ...state, activity: mergeServerRows(state.activity, action.rows) });
  }
  if (action.kind === "transcript_append") {
    return pruneOptimisticRows({ ...state, activity: mergeServerRows(state.activity, [action.row]) });
  }
  if (action.kind === "transcript_patch") {
    return pruneOptimisticRows({ ...state, activity: patchServerRow(state.activity, action.row) });
  }
  if (action.kind === "transcript_remove") {
    const activity = state.activity.filter((r) => r.id !== action.id);
    if (activity.length === state.activity.length) return state;
    return { ...state, activity };
  }
  if (action.kind === "handshake") {
    const hs = reduceFrames(action.frames);
    return {
      ...state,
      agent: state.agent ?? hs.agent,
      model: state.model ?? hs.model,
      mode: state.mode !== "Default" ? state.mode : hs.mode,
      promptCapabilities: state.promptCapabilities ?? hs.promptCapabilities,
      availableModes: state.availableModes.length > 0 ? state.availableModes : hs.availableModes,
      currentModeId: state.currentModeId ?? hs.currentModeId,
      availableCommands: state.availableCommands.length > 0 ? state.availableCommands : hs.availableCommands,
      configOptions: state.configOptions.length > 0 ? state.configOptions : hs.configOptions,
    };
  }
  if (action.kind === "lagged") {
    return { ...state, lagged: true };
  }
  if (action.kind === "lagged_resolved") {
    return { ...state, lagged: false };
  }
  if (action.kind === "error") {
    return { ...state, lastError: action.message };
  }
  if (action.kind === "clear_error") {
    return { ...state, lastError: null };
  }
  if (action.kind === "approval_resolved_locally") {
    const pendingApprovals = state.pendingApprovals.filter((a) => a.nonce !== action.nonce);
    const removed = pendingApprovals.length !== state.pendingApprovals.length;
    return {
      ...state,
      lastError: removed ? null : state.lastError,
      pendingApprovals,
      locallyResolved: [...state.locallyResolved, action.nonce],
    };
  }
  if (action.kind === "elicitation_resolved_locally") {
    const card = state.pendingElicitations.find((e) => e.nonce === action.nonce);
    const pendingElicitations = state.pendingElicitations.filter((e) => e.nonce !== action.nonce);
    const removed = pendingElicitations.length !== state.pendingElicitations.length;
    const answers =
      card && action.resolution.action === "accept" ? summarizeAnswers(card, action.resolution.answers) : [];
    return {
      ...state,
      lastError: removed ? null : state.lastError,
      pendingElicitations,
      locallyResolved: [...state.locallyResolved, action.nonce],
      optimisticRows: appendElicitationAnswerRow(state.optimisticRows, action.nonce, answers),
    };
  }
  if (action.kind === "hydrate") {
    return action.state;
  }
  if (action.kind === "user_prompt") {
    const id = action.id ?? `user-opt-${Date.now()}-${state.optimisticRows.length}`;
    const row: ActivityRow = {
      id,
      kind: "user_prompt",
      text: action.text,
      attachments: action.attachments && action.attachments.length > 0 ? action.attachments : undefined,
      at: new Date().toISOString(),
    };
    return {
      ...state,
      optimisticRows: state.optimisticRows.concat(row),
      startupError: null,
      lastError: null,
      inflightPromptIds: state.inflightPromptIds.includes(id)
        ? state.inflightPromptIds
        : state.inflightPromptIds.concat(id),
      promptSeq: state.promptSeq + 1,
      turnActive: true,
    };
  }
  if (action.kind === "prompt_send_rejected") {
    return settleInflightPrompt({ ...state, inFlightTool: null }, action.id);
  }
  if (action.kind === "settle_inflight_prompt") {
    return settleInflightPrompt(state, action.id);
  }
  if (action.kind === "rollback_optimistic_prompt") {
    const idx = state.optimisticRows.findIndex((r) => r.id === action.id);
    if (idx === -1) return settleInflightPrompt(state, action.id);
    return settleInflightPrompt(
      {
        ...state,
        optimisticRows: state.optimisticRows.slice(0, idx).concat(state.optimisticRows.slice(idx + 1)),
      },
      action.id,
    );
  }
  if (action.kind === "enqueue_prompt") {
    const entry: QueuedPrompt = {
      id: action.id,
      text: action.text,
      queuedAt: new Date().toISOString(),
      pending: true,
      ...(action.attachments && action.attachments.length > 0 ? { attachments: action.attachments } : {}),
    };
    return { ...state, queuedPrompts: state.queuedPrompts.concat(entry) };
  }
  if (action.kind === "dequeue_prompt") {
    return {
      ...state,
      queuedPrompts: state.queuedPrompts.filter((q) => q.id !== action.id),
    };
  }
  if (action.kind === "edit_queued_prompt") {
    return {
      ...state,
      queuedPrompts: state.queuedPrompts.map((q) => (q.id === action.id ? { ...q, text: action.text } : q)),
    };
  }
  if (action.kind === "clear_queue") {
    return { ...state, queuedPrompts: [] };
  }
  if (action.kind === "hydrate_server_queue") {
    const rows = Array.isArray(action.rows) ? action.rows : [];
    const serverIds = new Set(rows.map((r) => r.id));
    const localById = new Map(state.queuedPrompts.map((q) => [q.id, q]));
    const merged: QueuedPrompt[] = rows.map((r) => {
      const local = localById.get(r.id);
      const attachments: PromptAttachmentInput[] | undefined = local?.attachments?.length
        ? local.attachments
        : r.attachments && r.attachments.length > 0
          ? r.attachments.map((a) => ({
              kind: a.kind,
              mimeType: a.mime_type,
              name: a.name ?? undefined,
              dataB64: "",
            }))
          : undefined;
      return {
        id: r.id,
        text: r.text,
        queuedAt: r.created_at || local?.queuedAt || new Date().toISOString(),
        ...(attachments ? { attachments } : {}),
      };
    });
    const stillPending = state.queuedPrompts.filter((q) => q.pending && !serverIds.has(q.id));
    return { ...state, queuedPrompts: merged.concat(stillPending) };
  }
  if (action.kind === "confirm_queued_prompt") {
    return {
      ...state,
      queuedPrompts: state.queuedPrompts.map((q) => (q.id === action.id ? { ...q, pending: false } : q)),
    };
  }
  if (action.kind === "dismiss_primer") {
    return { ...state, contextPrimerAvailable: null };
  }
  if (action.kind === "dismiss_compaction_reminder") {
    return { ...state, compactionReminderDismissed: state.sessionUsage };
  }
  if (action.kind === "dismiss_rejected_prompt") {
    return {
      ...state,
      rejectedPrompts: state.rejectedPrompts.filter((r) => r.id !== action.id),
    };
  }
  if (action.kind === "dismiss_mode_switch_failed") {
    return { ...state, modeSwitchFailed: null };
  }
  if (action.kind === "set_pending_config_option") {
    return {
      ...state,
      pendingConfigOption: { configId: action.configId, value: action.value },
    };
  }
  if (action.kind === "clear_pending_config_option") {
    return { ...state, pendingConfigOption: null };
  }
  if (action.kind === "clear_pending_config_option_if_match") {
    if (state.pendingConfigOption?.configId === action.configId && state.pendingConfigOption?.value === action.value) {
      return { ...state, pendingConfigOption: null };
    }
    return state;
  }
  if (action.kind === "dismiss_config_option_switch_failed") {
    return { ...state, configOptionSwitchFailed: null };
  }
  return emptyAcpState();
}

export function transcriptDeltaAction(delta: TranscriptDelta, sessionId: string): Action | null {
  if ("Append" in delta) {
    if (!webRendersServerRow(delta.Append)) return null;
    return { kind: "transcript_append", row: transcriptRowToActivity(delta.Append, sessionId) };
  }
  if ("Patch" in delta) {
    if (!webRendersServerRow(delta.Patch.row)) return null;
    return { kind: "transcript_patch", row: transcriptRowToActivity(delta.Patch.row, sessionId) };
  }
  if ("Remove" in delta) {
    return { kind: "transcript_remove", id: delta.Remove };
  }
  return null;
}

export type ConnectionStatus = "connecting" | "open" | "closed" | "error";

const ACP_MAX_RETRIES = 7;
const ACP_RETRY_BASE_MS = 1000;
const ACP_RETRY_CAP_MS = 30000;
export function acpRetryDelayMs(attempt: number): number {
  return Math.min(ACP_RETRY_CAP_MS, ACP_RETRY_BASE_MS * 2 ** Math.max(0, attempt - 1));
}
export const ACP_MAX_RETRIES_EXPORT = ACP_MAX_RETRIES;

const ACP_WS_WATCHDOG_INTERVAL_MS = 15000;
export const ACP_WS_STALE_MS = 75000;

function optimisticPromptId(): string {
  const c = globalThis.crypto;
  if (c && typeof c.randomUUID === "function") return c.randomUUID();
  if (c && typeof c.getRandomValues === "function") {
    return "10000000-1000-4000-8000-100000000000".replace(/[018]/g, (digit) =>
      (Number(digit) ^ (c.getRandomValues(new Uint8Array(1))[0]! & (15 >> (Number(digit) / 4)))).toString(16),
    );
  }
  return `${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`;
}

export function useAcpSession(
  sessionId: string | null,
  workerState: "absent" | "resuming" | "running" | "stopping" = "running",
  archivedAt: string | null = null,
  snoozedUntil: string | null = null,
) {
  sweepExpiredStorage();
  const [state, dispatch] = useReducer(reducer, sessionId, initialState);
  const [status, setStatus] = useState<ConnectionStatus>("connecting");
  const archivedAtRef = useRef(archivedAt);
  const snoozedUntilRef = useRef(snoozedUntil);
  useEffect(() => {
    archivedAtRef.current = archivedAt;
  }, [archivedAt]);
  useEffect(() => {
    snoozedUntilRef.current = snoozedUntil;
  }, [snoozedUntil]);
  const statusRef = useRef<ConnectionStatus>("connecting");
  useEffect(() => {
    statusRef.current = status;
  }, [status]);

  const sessionIdRef = useRef(sessionId);
  sessionIdRef.current = sessionId;
  useEffect(() => {
    if (sessionIdRef.current) cacheSet(sessionIdRef.current, state);
  }, [state]);
  const wsRef = useRef<WebSocket | null>(null);
  const retryCountRef = useRef(0);
  const retryTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const countdownTimerRef = useRef<ReturnType<typeof setInterval> | null>(null);
  const connectRef = useRef<(() => void) | null>(null);
  const dialGenRef = useRef(0);
  const [reconnecting, setReconnecting] = useState(false);
  const [retryCount, setRetryCount] = useState(0);
  const [retryCountdown, setRetryCountdown] = useState(0);
  const lastSeqRef = useRef(0);
  useEffect(() => {
    lastSeqRef.current = state.lastSeq;
  }, [state.lastSeq]);
  const queuedPromptsRef = useRef(state.queuedPrompts);
  useEffect(() => {
    queuedPromptsRef.current = state.queuedPrompts;
  }, [state.queuedPrompts]);
  const oldestSeqRef = useRef(0);
  useEffect(() => {
    oldestSeqRef.current = state.oldestSeq;
  }, [state.oldestSeq]);
  const [hasMoreOlder, setHasMoreOlder] = useState(false);
  const hasMoreOlderRef = useRef(false);
  useEffect(() => {
    hasMoreOlderRef.current = hasMoreOlder;
  }, [hasMoreOlder]);
  const [loadingOlder, setLoadingOlder] = useState(false);
  const loadingOlderRef = useRef(false);
  const [hasEverOpened, setHasEverOpened] = useState(false);
  const prevSessionId1Ref = useRef(sessionId);
  if (sessionId !== prevSessionId1Ref.current) {
    prevSessionId1Ref.current = sessionId;
    setHasEverOpened(false);
  }

  const setStatusRef = useRef(setStatus);
  setStatusRef.current = setStatus;
  const setReconnectingRef = useRef(setReconnecting);
  setReconnectingRef.current = setReconnecting;
  const setRetryCountRef = useRef(setRetryCount);
  setRetryCountRef.current = setRetryCount;
  const setRetryCountdownRef = useRef(setRetryCountdown);
  setRetryCountdownRef.current = setRetryCountdown;
  const setHasEverOpenedRef = useRef(setHasEverOpened);
  setHasEverOpenedRef.current = setHasEverOpened;

  const clearRetryTimers = useCallback(() => {
    if (retryTimerRef.current) {
      clearTimeout(retryTimerRef.current);
      retryTimerRef.current = null;
    }
    if (countdownTimerRef.current) {
      clearInterval(countdownTimerRef.current);
      countdownTimerRef.current = null;
    }
  }, []);

  const lastServerMsgRef = useRef<number>(0);

  const tryAutoReconnectRef = useRef<() => void>(() => {});
  tryAutoReconnectRef.current = () => {
    const ws = wsRef.current;
    const ready = ws?.readyState;
    if (ready === WebSocket.CONNECTING) return;
    if (ready === WebSocket.OPEN && Date.now() - lastServerMsgRef.current < ACP_WS_STALE_MS) {
      return;
    }
    retryCountRef.current = 0;
    setRetryCount(0);
    setRetryCountdown(0);
    clearRetryTimers();
    connectRef.current?.();
  };

  useEffect(() => {
    const id = setInterval(() => {
      if (wsRef.current?.readyState === WebSocket.OPEN) {
        tryAutoReconnectRef.current();
      }
    }, ACP_WS_WATCHDOG_INTERVAL_MS);
    return () => clearInterval(id);
  }, []);

  const visCounterRef = useRef(0);
  const subscribeVisibility = useCallback((cb: () => void) => {
    const handler = () => {
      visCounterRef.current += 1;
      cb();
    };
    document.addEventListener("visibilitychange", handler);
    window.addEventListener("pageshow", handler);
    return () => {
      document.removeEventListener("visibilitychange", handler);
      window.removeEventListener("pageshow", handler);
    };
  }, []);
  const getVisibilitySnapshot = useCallback(() => visCounterRef.current, []);
  const visCounter = useSyncExternalStore(subscribeVisibility, getVisibilitySnapshot, () => 0);

  const isOnline = useSyncExternalStore(
    (cb: () => void) => {
      window.addEventListener("online", cb);
      window.addEventListener("offline", cb);
      return () => {
        window.removeEventListener("online", cb);
        window.removeEventListener("offline", cb);
      };
    },
    () => navigator.onLine,
    () => true, // SSR guard
  );

  const isFirstVis = useRef(true);
  useEffect(() => {
    if (isFirstVis.current) {
      isFirstVis.current = false;
      return;
    }
    tryAutoReconnectRef.current();
  }, [visCounter]);

  const prevOnlineRef = useRef(isOnline);
  useEffect(() => {
    if (!prevOnlineRef.current && isOnline) {
      tryAutoReconnectRef.current();
    }
    prevOnlineRef.current = isOnline;
  }, [isOnline]);

  const lastActivityRef = useRef<number>(0);

  const fetchReplay = useCallback(async (sid: string) => {
    try {
      if (lastSeqRef.current === 0) {
        const tailParams = `before=${TAIL_BEFORE}&limit=${REPLAY_PAGE_SIZE}`;
        const [tailRes, tailRowsRes] = await Promise.all([
          fetch(`/api/sessions/${encodeURIComponent(sid)}/acp/replay?${tailParams}`, { credentials: "same-origin" }),
          fetch(`/api/sessions/${encodeURIComponent(sid)}/acp/replay?${tailParams}&view=rows`, {
            credentials: "same-origin",
          }),
        ]);
        if (!tailRes.ok) return;
        const tail = (await tailRes.json()) as ReplayPageResponse;
        if (tail.lost) {
          dispatch({ kind: "lagged", skipped: tail.highest_seq });
          return;
        }
        if (!tailRowsRes.ok) return;
        const tailRows = ((await tailRowsRes.json()) as ReplayPageResponse).rows ?? [];
        dispatch({
          kind: "frames",
          frames: tail.frames ?? [],
          rows: tailRows.filter(webRendersServerRow).map((r) => transcriptRowToActivity(r, sid)),
          oldestSeq: tail.next_cursor ?? 0,
        });
        setHasMoreOlder(tail.has_more ?? false);
        if (tail.highest_seq > lastSeqRef.current) {
          lastSeqRef.current = tail.highest_seq;
        }
        if ((tail.has_more ?? false) && (tail.next_cursor ?? 0) > 1) {
          const hsRes = await fetch(
            `/api/sessions/${encodeURIComponent(sid)}/acp/replay?since=0&limit=${HANDSHAKE_PREFIX_SIZE}`,
            { credentials: "same-origin" },
          );
          if (hsRes.ok) {
            const hs = (await hsRes.json()) as ReplayPageResponse;
            if ((hs.frames ?? []).length > 0) dispatch({ kind: "handshake", frames: hs.frames });
          }
        }
        dispatch({ kind: "lagged_resolved" });
        return;
      }
      const firstSince = Math.max(0, lastSeqRef.current - REPLAY_OVERLAP);
      let cursor = firstSince;
      let target: number | null = null;
      for (;;) {
        const pageParams = `since=${cursor}&limit=${REPLAY_PAGE_SIZE}`;
        const [res, rowsRes] = await Promise.all([
          fetch(`/api/sessions/${encodeURIComponent(sid)}/acp/replay?${pageParams}`, { credentials: "same-origin" }),
          fetch(`/api/sessions/${encodeURIComponent(sid)}/acp/replay?${pageParams}&view=rows`, {
            credentials: "same-origin",
          }),
        ]);
        if (!res.ok || !rowsRes.ok) return;
        const data = (await res.json()) as ReplayPageResponse;
        const pageRows = ((await rowsRes.json()) as ReplayPageResponse).rows ?? [];
        if (target === null) {
          target = data.highest_seq;
          if (data.highest_seq < firstSince) {
            dispatch({ kind: "reset" });
          }
        }
        if (data.lost) {
          dispatch({ kind: "lagged", skipped: data.highest_seq });
          return;
        }
        if (data.frames.length > 0 || pageRows.length > 0) {
          dispatch({
            kind: "frames",
            frames: data.frames,
            rows: pageRows.filter(webRendersServerRow).map((r) => transcriptRowToActivity(r, sid)),
          });
        }
        const next = data.next_cursor;
        if (data.has_more && next != null && next > cursor && next < target) {
          cursor = next;
          continue;
        }
        break;
      }
      dispatch({ kind: "lagged_resolved" });
    } catch {
      // Best effort; the next lagged notice retries.
    }
  }, []);

  const loadOlder = useCallback(async () => {
    const sid = sessionIdRef.current;
    const before = oldestSeqRef.current;
    if (!sid || before <= 0 || loadingOlderRef.current || !hasMoreOlderRef.current) return;
    loadingOlderRef.current = true;
    setLoadingOlder(true);
    try {
      const res = await fetch(
        `/api/sessions/${encodeURIComponent(sid)}/acp/replay?before=${before}&limit=${REPLAY_PAGE_SIZE}&view=rows`,
        { credentials: "same-origin" },
      );
      if (!res.ok) return;
      const data = (await res.json()) as ReplayPageResponse;
      const rows = (data.rows ?? []).filter(webRendersServerRow).map((r) => transcriptRowToActivity(r, sid));
      if (rows.length > 0) {
        dispatch({ kind: "prepend", rows, oldestSeq: data.next_cursor ?? before });
      }
      setHasMoreOlder(data.has_more ?? false);
    } catch {
      // Keep hasMoreOlder; the next scroll-up retries.
    } finally {
      loadingOlderRef.current = false;
      setLoadingOlder(false);
    }
  }, []);

  const prevSessionId2Ref = useRef(sessionId);
  if (sessionId !== prevSessionId2Ref.current) {
    prevSessionId2Ref.current = sessionId;
    if (!sessionId) {
      setStatus("closed");
    } else {
      setStatus("connecting");
    }
    setReconnecting(false);
    setRetryCount(0);
    setRetryCountdown(0);
    setHasMoreOlder(false);
    setLoadingOlder(false);
    loadingOlderRef.current = false;
    const switched = sessionId ? cacheGet(sessionId) : undefined;
    lastSeqRef.current = switched?.lastSeq ?? 0;
    oldestSeqRef.current = switched?.oldestSeq ?? 0;
  }

  useEffect(() => {
    if (!sessionId) {
      statusRef.current = "closed";
      return;
    }
    dispatch({
      kind: "hydrate",
      state: cacheGet(sessionId) ?? emptyAcpState(),
    });
    statusRef.current = "connecting";
    retryCountRef.current = 0;

    let cancelled = false;

    const scheduleReconnect = () => {
      if (cancelled) return;
      if (retryCountRef.current >= ACP_MAX_RETRIES) {
        setReconnectingRef.current(false);
        setRetryCountRef.current(retryCountRef.current);
        setRetryCountdownRef.current(0);
        return;
      }
      retryCountRef.current += 1;
      const attempt = retryCountRef.current;
      const delayMs = acpRetryDelayMs(attempt);
      let countdown = Math.ceil(delayMs / 1000);
      setReconnectingRef.current(true);
      setRetryCountRef.current(attempt);
      setRetryCountdownRef.current(countdown);
      clearRetryTimers();
      countdownTimerRef.current = setInterval(() => {
        countdown -= 1;
        if (countdown > 0) setRetryCountdownRef.current(countdown);
      }, 1000);
      retryTimerRef.current = setTimeout(() => {
        if (countdownTimerRef.current) {
          clearInterval(countdownTimerRef.current);
          countdownTimerRef.current = null;
        }
        connectRef.current?.();
      }, delayMs);
    };

    const connect = () => {
      if (cancelled) return;
      clearRetryTimers();
      dialGenRef.current += 1;
      if (wsRef.current) {
        try {
          wsRef.current.close();
        } catch {
          // Already closed.
        }
        wsRef.current = null;
      }
      const myGen = dialGenRef.current;
      const isCurrentDial = () => !cancelled && dialGenRef.current === myGen;
      statusRef.current = "connecting";
      void (async () => {
        await fetchReplay(sessionId);
        if (!isCurrentDial()) return;

        const token = getToken();
        const protocol = window.location.protocol === "https:" ? "wss" : "ws";
        const since = lastSeqRef.current;
        const url = `${protocol}://${window.location.host}/sessions/${encodeURIComponent(sessionId)}/acp/ws?since=${since}`;

        // Subprotocols carry both factors on a WS upgrade:
        //   - `aoe-auth` is the legacy signalling protocol the server
        //     expects to see.
        //   - the bare `<token>` is the first-factor auth token
        //     (kept for backward compatibility with PWA tabs that
        //     loaded before the prefixed format landed).
        //   - `aoe-device.<binding-secret>` is the device-binding
        //     second factor introduced in #1131. The middleware
        //     enforces this when passphrase login is configured.
        let bindingSecret: string | null = null;
        try {
          bindingSecret = getOrCreateDeviceBindingSecret();
        } catch {
          // Storage / crypto unavailable; the server will reject this
          // upgrade with 401 and the login page will surface the cause.
        }
        const protocols: string[] = ["aoe-auth"];
        if (token) protocols.push(token);
        if (bindingSecret) protocols.push(`aoe-device.${bindingSecret}`);
        const ws = new WebSocket(url, protocols);
        wsRef.current = ws;

        // Set the ref synchronously alongside setState so sendPrompt's
        // gate (which reads the ref) doesn't race the next render.
        // Without this, a click landing in the same event-loop tick as
        // `onclose` could see statusRef.current === "open" and dispatch
        // an optimistic prompt against a closed socket.
        //
        // Every handler additionally checks `isCurrentDial()`: an
        // orphaned WS from a superseded connect() must not flip status,
        // null wsRef.current, or schedule a retry on top of the new
        // healthy socket.
        ws.onopen = () => {
          if (!isCurrentDial()) {
            try {
              ws.close();
            } catch {
              // ignore
            }
            return;
          }
          statusRef.current = "open";
          setStatusRef.current("open");
          setHasEverOpenedRef.current(true);
          // Seed the liveness clock so a slow first heartbeat doesn't
          // trip the staleness watchdog right after connect. See #2287.
          lastServerMsgRef.current = Date.now();
          // A live socket is the right moment to reset the retry
          // envelope: a future close from here is a genuinely new
          // failure, not a continuation of the prior backoff chain.
          retryCountRef.current = 0;
          setReconnectingRef.current(false);
          setRetryCountRef.current(0);
          setRetryCountdownRef.current(0);
        };
        ws.onerror = () => {
          if (!isCurrentDial()) return;
          statusRef.current = "error";
          setStatusRef.current("error");
        };
        ws.onclose = () => {
          if (!isCurrentDial()) return;
          statusRef.current = "closed";
          setStatusRef.current("closed");
          wsRef.current = null;
          scheduleReconnect();
        };
        ws.onmessage = (ev) => {
          if (!isCurrentDial()) return;
          // Any message on the current socket proves the browser-visible
          // path is alive, so refresh the liveness clock before parsing.
          // The server heartbeat keeps this fresh on quiet sessions; a
          // busy stream keeps it fresh too. See #2287.
          lastServerMsgRef.current = Date.now();
          try {
            const data = JSON.parse(ev.data) as
              | AcpFrame
              | { kind: "lagged"; skipped?: number }
              | { kind: "heartbeat" }
              | { kind: "reduced_state" }
              | { kind: "transcript_snapshot"; rows?: TranscriptRow[] }
              | { kind: "transcript_delta"; delta?: TranscriptDelta };
            const kind = typeof data === "object" && data !== null ? (data as { kind?: unknown }).kind : undefined;
            if (kind === "heartbeat") {
              // Keepalive tick; liveness clock already bumped above.
              return;
            }
            if (kind === "lagged") {
              const skipped = (data as { skipped?: number }).skipped ?? 0;
              dispatch({ kind: "lagged", skipped });
              // Try to recover via the snapshot endpoint.
              fetchReplay(sessionId);
              return;
            }
            if (kind === "reduced_state") {
              // Server-folded control state (Tier 1.2), sent on connect and
              // after every event. Authoritative: the client no longer
              // derives any of these fields.
              const reduced = (data as { state?: ReducedState }).state;
              if (reduced) {
                lastActivityRef.current = Date.now();
                const unchanged = (data as { unchanged?: string[] }).unchanged ?? [];
                dispatch({ kind: "reduced_state", state: reduced, unchanged });
              }
              return;
            }
            if (kind === "transcript_snapshot") {
              // Server-owned transcript connect snapshot (Tier 4). Usually
              // empty (the WS dials at the current lastSeq); carries gap rows
              // on a reconnect that raced live events. Merged by row id.
              const rows = ((data as { rows?: TranscriptRow[] }).rows ?? [])
                .filter(webRendersServerRow)
                .map((r) => transcriptRowToActivity(r, sessionId));
              lastActivityRef.current = Date.now();
              dispatch({ kind: "transcript_snapshot", rows });
              return;
            }
            if (kind === "transcript_delta") {
              const delta = (data as { delta?: TranscriptDelta }).delta;
              const act = delta ? transcriptDeltaAction(delta, sessionId) : null;
              if (act) {
                lastActivityRef.current = Date.now();
                dispatch(act);
              }
              return;
            }
            if (typeof data === "object" && data !== null && "session_id" in data && "event" in data) {
              // Raw event frame: feeds the client-side CONTROL reducer only
              // (the transcript is server-owned now). Every incoming live
              // frame is an "activity" tick for the force-end-turn watchdog:
              // as long as the agent is streaming, the spinner stays "honest"
              // and the escape hatch doesn't appear. See WorkingSpinner.
              lastActivityRef.current = Date.now();
              dispatch({ kind: "frame", frame: data as AcpFrame });
            }
          } catch {
            // Ignore malformed frames; the server should never send them.
          }
        };
      })();
    };
    connectRef.current = connect;
    connect();

    return () => {
      cancelled = true;
      // Bump the generation so any in-flight IIFE / pending WS handlers
      // from this effect's lifetime see themselves as stale.
      dialGenRef.current += 1;
      clearRetryTimers();
      const ws = wsRef.current;
      if (ws) {
        try {
          ws.close();
        } catch {
          // ignore
        }
      }
      wsRef.current = null;
      connectRef.current = null;
    };
  }, [sessionId, fetchReplay, clearRetryTimers]);

  const resolveApproval = useCallback(
    // `optionId` answers with the agent's own option instead of letting
    // the daemon pick by option kind.
    async (nonce: string, decision: ApprovalDecision, optionId?: string) => {
      if (!sessionId) return;
      try {
        const res = await fetch(
          `/api/sessions/${encodeURIComponent(sessionId)}/acp/approvals/${encodeURIComponent(nonce)}`,
          {
            method: "POST",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify(optionId === undefined ? { decision } : { decision, option_id: optionId }),
          },
        );
        const detail = res.ok ? "" : await safeText(res);
        const outcome = classifyApprovalResolveResponse(res.ok, res.status, detail, nonce);
        if (outcome.kind === "resolved") {
          // 204, or a 404 that names the missing nonce: the decision was
          // accepted or already resolved server-side (a concurrent
          // decision, a watchdog cancel, or the agent picking no matching
          // option). Clear the card now rather than waiting on the
          // ApprovalResolved broadcast, which the seq dedupe can drop and
          // strand the card. A session-gone 404 (different body) is a real
          // failure and surfaces an error. See #1821.
          dispatch({ kind: "approval_resolved_locally", nonce });
        } else {
          dispatch({ kind: "error", message: outcome.message });
        }
      } catch (e) {
        dispatch({
          kind: "error",
          message: `Network error resolving approval: ${describeError(e)}`,
        });
      }
    },
    [sessionId],
  );

  const resolveElicitation = useCallback(
    async (nonce: string, resolution: ElicitationResolution) => {
      if (!sessionId) return;
      let res: Response;
      try {
        res = await fetch(
          `/api/sessions/${encodeURIComponent(sessionId)}/acp/elicitations/${encodeURIComponent(nonce)}`,
          {
            method: "POST",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify(resolution),
          },
        );
      } catch (e) {
        dispatch({
          kind: "error",
          message: `Network error resolving question: ${describeError(e)}`,
        });
        // Rethrow so the card re-enables (it can be resubmitted).
        throw e;
      }
      const detail = res.ok ? "" : await safeText(res);
      const outcome = classifyElicitationResolveResponse(res.ok, res.status, detail, nonce);
      if (outcome.kind === "resolved") {
        dispatch({ kind: "elicitation_resolved_locally", nonce, resolution });
        return;
      }
      // A validation rejection (422) leaves the elicitation pending
      // server-side, so surface the reason and rethrow: the card resets to
      // its editable state and the user can correct and resubmit the same
      // nonce instead of the question being stranded. See #2100.
      dispatch({ kind: "error", message: outcome.message });
      throw new Error(outcome.message);
    },
    [sessionId],
  );

  // POST a prompt and report what the daemon did with it. Internal helper
  // used by both sendPrompt and the drain effect below (when popping the head
  // of queuedPrompts on Stopped). The result tells the drain effect what to do
  // with the items it just sent:
  //   - "dispatched" / "queued": the daemon accepted it, retire them.
  //   - "non_retryable_failure": the server rejected them with a 4xx, so
  //     retrying would just re-POST the same failing batch every turn-end;
  //     retire them too (the error banner already surfaced the reason).
  //   - "retryable_failure": a transient disconnect / 5xx / network error,
  //     so keep the queue intact for the next turn-end retry.
  const dispatchPromptNow = useCallback(
    async (text: string, attachments?: PromptAttachmentInput[]): Promise<PromptSendResult> => {
      if (!sessionId) return { kind: "retryable_failure" };
      // Optimistic preview rows: render the attachment inline from a
      // local data URL so the bubble shows immediately, before the
      // server confirms and replay would otherwise back it with the
      // GET endpoint. See #1000 / #965.
      const previews: AcpAttachment[] = (attachments ?? []).map((a, i) => ({
        id: `local-${Date.now()}-${i}`,
        kind: a.kind,
        mimeType: a.mimeType,
        name: a.name,
        size: Math.floor((a.dataB64.length * 3) / 4),
        url: `data:${a.mimeType};base64,${a.dataB64}`,
      }));
      // Mint a stable prompt id and render an optimistic overlay row keyed by
      // it. The POST echoes this id as the `Event::UserPromptSent.prompt_id`,
      // and the server-owned transcript keys the authoritative `user_prompt`
      // row on it, so the overlay reconciles by id (dropped once the server
      // row lands) instead of the old fragile text match. If the POST fails
      // with a 4xx the overlay stays so the user sees what they tried to send;
      // a transient worker_not_ready 503 rolls this exact overlay back (by id)
      // because the prompt is re-queued and the drain would otherwise echo a
      // duplicate. See #3173 / #3094 / #3087.
      const promptId = optimisticPromptId();
      dispatch({
        kind: "user_prompt",
        id: promptId,
        text,
        attachments: previews.length > 0 ? previews : undefined,
      });
      // Submit counts as activity so the force-end-turn watchdog
      // doesn't surface the escape hatch immediately on a fresh prompt
      // (the agent's first chunk can be a few seconds out).
      lastActivityRef.current = Date.now();
      try {
        const body = {
          text,
          prompt_id: promptId,
          attachments: (attachments ?? []).map((a) => ({
            kind: a.kind,
            mime_type: a.mimeType,
            data: a.dataB64,
            name: a.name,
          })),
        };
        const res = await fetch(`/api/sessions/${encodeURIComponent(sessionId)}/acp/prompt`, {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify(body),
        });
        if (!res.ok) {
          const detail = await safeText(res);
          // 4xx means the server rejected the prompt (validation,
          // capability gate, unknown session), so there is no in-flight
          // turn to cancel and no Stopped frame to retire our optimistic
          // turn marker.
          const rejected = res.status >= 400 && res.status < 500;
          // Typed transient: the session was idle-auto-stopped (#1689) and
          // its worker did not finish respawning within send_prompt's wait
          // window. The worker is still coming online, so this is
          // retryable; suppress the error banner (the queued indicator and
          // respawn are the right signal) and let the drain re-fire it on
          // the next AcpSessionAssigned. A capacity 503 ("worker_capacity_full")
          // is NOT this case: it needs operator action, so it keeps its
          // banner. See #1748.
          //
          // Attachments are now re-queued on this transient (the queue
          // carries them in memory; see sendPrompt + the drain effect), so
          // suppress the banner for attachment sends too. See #1833.
          const workerNotReady = res.status === 503 && detail.startsWith("worker_not_ready");
          if (rejected) {
            dispatch({ kind: "prompt_send_rejected", id: promptId });
          } else if (workerNotReady) {
            // Undo the optimistic overlay row: the caller re-queues this
            // prompt, so it must live only in the queue until the drain
            // resends it once the worker is back online. See #3094 / #3087.
            dispatch({ kind: "rollback_optimistic_prompt", id: promptId });
          } else {
            // Any other 5xx. No `UserPromptSent` is coming, so settle the
            // optimistic marker; the overlay row stays so the user can see
            // what they tried to send. See #3417.
            dispatch({ kind: "settle_inflight_prompt", id: promptId });
          }
          if (!workerNotReady) {
            dispatch({
              kind: "error",
              message: `Could not send prompt (${res.status}). ${detail}`.trim(),
            });
          }
          return { kind: rejected ? "non_retryable_failure" : "retryable_failure" };
        }
        // The daemon reports what it did (Tier 3). A `queued` disposition means
        // it parked the prompt rather than starting a turn, so the optimistic
        // transcript row has to become a queue row: the turn-end drain will
        // deliver it, and leaving the transcript row would show the message as
        // sent while it waits.
        const dispatched = (await safeJson<PromptDispatchBody>(res)) ?? {};
        if (dispatched.disposition === "queued") {
          dispatch({ kind: "rollback_optimistic_prompt", id: promptId });
          return { kind: "queued", queuedId: dispatched.queued_id ?? promptId };
        }
        return { kind: "dispatched" };
      } catch (e) {
        // The POST never completed, so nothing will acknowledge this id.
        // Settling it is the safe direction: a brief false idle converges on
        // the next control frame, a false active never converges. See #3417.
        dispatch({ kind: "settle_inflight_prompt", id: promptId });
        dispatch({
          kind: "error",
          message: `Network error sending prompt: ${describeError(e)}`,
        });
        return { kind: "retryable_failure" };
      }
    },
    [sessionId],
  );

  // Queue a prompt on the server with an optimistic local row. The row shows
  // immediately (marked `pending`); the enqueue POST persists it (and buffers
  // any attachment bytes) server-side, then the daemon drains it at turn-end
  // with no tab open. On confirm the `pending` flag clears; on failure the row
  // stays visible with an error so the message is not silently lost. Attachment
  // shapes match (`PromptAttachmentInput` == `QueueAttachmentUpload`), so they
  // pass straight through.
  const enqueueServer = useCallback(
    (text: string, attachments?: PromptAttachmentInput[]) => {
      if (!sessionId) return;
      const id = `q-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
      dispatch({ kind: "enqueue_prompt", id, text, attachments });
      // Queue depth is now server-owned, but keep the opt-in telemetry signal.
      reportAcpInteraction("prompt_queued");
      void (async () => {
        const row = await enqueueServerPrompt(sessionId, { id, text, attachments });
        if (row) {
          dispatch({ kind: "confirm_queued_prompt", id });
        } else {
          dispatch({
            kind: "error",
            message: "Couldn't queue your message on the server; it may not send. Remove it and try again.",
          });
        }
      })();
    },
    [sessionId],
  );

  // The daemon owns the send, steer, or queue decision.
  const sendPrompt = useCallback(
    async (text: string, attachments?: PromptAttachmentInput[]) => {
      if (!sessionId) return;
      // Wake archived or snoozed sessions before posting; the reconciler skips
      // them and therefore cannot drain a prompt queued while they remain sunk.
      if (archivedAtRef.current || snoozedUntilRef.current) {
        const wakeResult = archivedAtRef.current
          ? await setSessionArchive(sessionId, false)
          : await setSessionSnooze(sessionId, null);
        if (!wakeResult) {
          // Do not queue against a session the reconciler still skips.
          dispatch({
            kind: "error",
            message: "Could not wake this session. Please retry, or unarchive / unsnooze from the sidebar.",
          });
          return;
        }
      }
      const result = await dispatchPromptNow(text, attachments);
      if (result.kind === "queued") {
        // The daemon parked it. The row already exists server-side, so show it
        // in the strip as confirmed rather than POSTing a second copy.
        dispatch({ kind: "enqueue_prompt", id: result.queuedId, text, attachments });
        dispatch({ kind: "confirm_queued_prompt", id: result.queuedId });
        reportAcpInteraction("prompt_queued");
        return;
      }
      // A transient 503 means the daemon accepted the request but its worker
      // did not come online within `send_prompt`'s wait window (#1748 / #1833).
      // The prompt is not on the queue (the daemon decided to send it), so
      // enqueue it here or it is lost. Both daemon-side states that answer
      // "sent" for a session with no worker need this: the idle-dormant wake
      // and the rate-limit redelivery-cap park, whose banner tells the user a
      // fresh prompt is the recovery (#3688).
      if (result.kind === "retryable_failure" && (state.workerIdleStopped || state.rateLimitRetriesExhausted)) {
        enqueueServer(text, attachments);
      }
    },
    [sessionId, state.workerIdleStopped, state.rateLimitRetriesExhausted, dispatchPromptNow, enqueueServer],
  );

  // Server-queue hydration. The daemon owns the queue and drains it (even
  // with no tab open), so the client's job is to reflect the server snapshot,
  // not to drain. Re-list on connect and whenever a turn ends (a server drain
  // fires at turn-end, so the drained rows disappear on the following list)
  // and dispatch a reconcile that keeps this session's optimistic thumbnails
  // and any still-in-flight enqueue.
  //
  // The FIRST run also migrates: it pushes any rows queued before the server
  // owned the queue (rows restored from localStorage, or in-flight) to the
  // server, keyed by their existing id so the POST is idempotent, then lists.
  // Attachments survive migration only for rows still in memory (localStorage
  // drops the bytes), matching the prior reload behavior. See the server-side
  // prompt queue design.
  // Keyed by session id, not a bare boolean: the hook instance outlives a
  // session switch (the SPA swaps `sessionId` without remounting), so a single
  // flag meant only the first session a tab ever opened got migrated and every
  // later one silently skipped it, stranding its pre-server-queue rows in
  // localStorage.
  const queueMigratedRef = useRef<Set<string>>(new Set());
  useEffect(() => {
    if (!sessionId) return;
    if (status !== "open") return;
    let cancelled = false;
    void (async () => {
      if (!queueMigratedRef.current.has(sessionId)) {
        queueMigratedRef.current.add(sessionId);
        for (const q of queuedPromptsRef.current) {
          if (cancelled) return;
          await enqueueServerPrompt(sessionId, {
            id: q.id,
            text: q.text,
            createdAt: q.queuedAt,
            attachments: q.attachments,
          });
        }
      }
      const rows = await listServerQueue(sessionId);
      if (cancelled) return;
      dispatch({ kind: "hydrate_server_queue", rows });
    })();
    return () => {
      cancelled = true;
    };
  }, [sessionId, status, state.turnActive]);

  // Optimistic local update + server mutation. The server is authoritative;
  // a later hydrate reconciles. A failed mutation is best-effort (the row
  // reappears on the next hydrate), so we don't roll the optimistic edit back.
  const removeQueuedPrompt = useCallback(
    (id: string) => {
      dispatch({ kind: "dequeue_prompt", id });
      if (sessionId) void removeServerQueuedPrompt(sessionId, id);
    },
    [sessionId],
  );

  const editQueuedPrompt = useCallback(
    (id: string, text: string) => {
      dispatch({ kind: "edit_queued_prompt", id, text });
      if (sessionId) void editServerQueuedPrompt(sessionId, id, text);
    },
    [sessionId],
  );

  const clearQueue = useCallback(() => {
    dispatch({ kind: "clear_queue" });
    if (sessionId) void clearServerQueue(sessionId);
  }, [sessionId]);

  // Stable handle to `cancelPrompt` (defined below) so `sendQueuedNow` can
  // interrupt a running turn without a forward reference. Assigned in the
  // render body right after `cancelPrompt` is created.
  const cancelPromptRef = useRef<() => Promise<void> | void>(() => {});

  // Send directly when possible. For a non-steerable active turn, cancel and
  // let the server drain rather than posting during cancellation.
  const sendQueuedNow = useCallback(
    async (prompt: QueuedPrompt) => {
      const sid = sessionIdRef.current;
      if (!sid) return;
      const steerable = !!state.promptCapabilities?.steering && !state.cancelling && !state.compacting;
      if (state.turnActive && !steerable) {
        await cancelPromptRef.current();
        return;
      }
      // A hydrated attachment row has only server-side bytes. Removing it would
      // delete those bytes before the direct POST, so leave it for the drain.
      if (prompt.attachments?.some((a) => !a.dataB64)) return;
      // Remove server-side first so the turn-end drain can't also deliver this
      // row, then send it directly. The optimistic dequeue hides it locally.
      // The remove takes the same per-instance lock the drain holds across its
      // send, so it cannot interleave with a drain that already snapshotted
      // this row.
      dispatch({ kind: "dequeue_prompt", id: prompt.id });
      const removed = await removeServerQueuedPrompt(sid, prompt.id);
      if (!removed) {
        // The row was already gone, which means the drain claimed it while we
        // were asking. It is being delivered; sending again would double it.
        return;
      }
      const result = await dispatchPromptNow(prompt.text, prompt.attachments);
      if (result.kind === "queued") {
        // The daemon parked it again (the turn it would jump ahead of is still
        // running). Put the row back so the strip keeps showing it.
        dispatch({ kind: "enqueue_prompt", id: result.queuedId, text: prompt.text, attachments: prompt.attachments });
        dispatch({ kind: "confirm_queued_prompt", id: result.queuedId });
      } else if (result.kind === "retryable_failure") {
        // The immediate send bounced (worker still resuming); re-queue it
        // server-side so the drain re-fires it, and restore the optimistic row.
        enqueueServer(prompt.text, prompt.attachments);
      }
    },
    [
      dispatchPromptNow,
      enqueueServer,
      state.turnActive,
      state.promptCapabilities?.steering,
      state.cancelling,
      state.compacting,
    ],
  );

  const dismissPrimer = useCallback(() => {
    dispatch({ kind: "dismiss_primer" });
  }, []);

  const dismissCompactionReminder = useCallback(() => {
    dispatch({ kind: "dismiss_compaction_reminder" });
  }, []);

  const dismissRejectedPrompt = useCallback((id: string) => {
    dispatch({ kind: "dismiss_rejected_prompt", id });
  }, []);

  const dismissModeSwitchFailed = useCallback(() => {
    dispatch({ kind: "dismiss_mode_switch_failed" });
  }, []);

  // Send `session/set_config_option` to the daemon (model / reasoning
  // effort / future selector). Pessimistic: the current value stays put
  // until the adapter pushes a confirming `ConfigOptionsUpdated`. The
  // pending dispatch records the in-flight click so the UI can dim the
  // just-clicked option without lying about active state. On HTTP
  // failure the pending state clears and lastError surfaces a banner;
  // adapter-side rejection comes back as a `ConfigOptionSwitchFailed`
  // frame which clears pending in the reducer and renders a
  // non-blocking notice. See #1403.
  const setConfigOption = useCallback(
    async (configId: string, value: string) => {
      if (!sessionId) return;
      dispatch({ kind: "set_pending_config_option", configId, value });
      try {
        const res = await fetch(`/api/sessions/${encodeURIComponent(sessionId)}/acp/config-option`, {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ config_id: configId, value }),
        });
        if (!res.ok) {
          const detail = await safeText(res);
          // Guard against the user clicking a second option before this
          // request's response landed: clear pending only when it
          // still matches our (configId, value) pair. See #1403.
          dispatch({
            kind: "clear_pending_config_option_if_match",
            configId,
            value,
          });
          dispatch({
            kind: "error",
            message: `Could not set ${configId} (${res.status}). ${detail}`.trim(),
          });
        }
      } catch (e) {
        dispatch({
          kind: "clear_pending_config_option_if_match",
          configId,
          value,
        });
        dispatch({
          kind: "error",
          message: `Network error setting ${configId}: ${describeError(e)}`,
        });
      }
    },
    [sessionId],
  );

  const dismissConfigOptionSwitchFailed = useCallback(() => {
    dispatch({ kind: "dismiss_config_option_switch_failed" });
  }, []);

  // Cancels the in-flight agent turn (ACP session/cancel). Must only
  // fire on an explicit user gesture against a dedicated cancel/stop
  // affordance; never bind this to the Escape key. Claude Code CLI
  // hijacks Escape for cancel and accidental presses lose work the
  // user did not mean to abort; the structured view deliberately keeps Escape
  // for closing local UI surfaces (palette, dialogs, popovers) only.
  // If a future Escape binding is added, route it through
  // useKeyboardShortcuts.onEscape's local-UI dismissal, not here.
  const cancelPrompt = useCallback(async () => {
    if (!sessionId) return;
    try {
      const res = await fetch(`/api/sessions/${encodeURIComponent(sessionId)}/acp/cancel`, { method: "POST" });
      if (!res.ok) {
        const detail = await safeText(res);
        dispatch({
          kind: "error",
          message: `Could not cancel (${res.status}). ${detail}`.trim(),
        });
      }
    } catch (e) {
      dispatch({
        kind: "error",
        message: `Network error cancelling: ${describeError(e)}`,
      });
    }
  }, [sessionId]);
  cancelPromptRef.current = cancelPrompt;

  // Escape hatch for the "spinner stuck" failure mode (#1100). POSTs to
  // the daemon and relies on the server-published Stopped event to drive
  // reducer state: either the synthetic free-the-UI Stopped or the
  // user_forced one from the worker restart. We do NOT fabricate a
  // client-side Stopped seq; the server echo flows back as a real frame
  // on the WS. See #1727.
  const forceEndTurn = useCallback(async () => {
    if (!sessionId) return;
    lastActivityRef.current = Date.now();
    try {
      const res = await fetch(`/api/sessions/${encodeURIComponent(sessionId)}/acp/force_end_turn`, { method: "POST" });
      if (!res.ok) {
        const detail = await safeText(res);
        dispatch({
          kind: "error",
          message: `Could not force end turn (${res.status}). ${detail}`.trim(),
        });
      }
    } catch (e) {
      dispatch({
        kind: "error",
        message: `Network error forcing end turn: ${describeError(e)}`,
      });
    }
  }, [sessionId]);

  const dismissError = useCallback(() => {
    dispatch({ kind: "clear_error" });
  }, []);

  // Public manual-reconnect affordance. Surfaces in the SystemNotices
  // banner once the auto-retry envelope is exhausted; resets the
  // backoff counter and dials a fresh WS immediately. Idempotent
  // against a live socket (the reconnect path checks readyState).
  const manualReconnect = useCallback(() => {
    if (retryTimerRef.current) {
      clearTimeout(retryTimerRef.current);
      retryTimerRef.current = null;
    }
    if (countdownTimerRef.current) {
      clearInterval(countdownTimerRef.current);
      countdownTimerRef.current = null;
    }
    retryCountRef.current = 0;
    setRetryCount(0);
    setRetryCountdown(0);
    setReconnecting(false);
    connectRef.current?.();
  }, []);

  // Whether the per-row "Send now" affordance can do something useful: the
  // socket is open, no worker-down banner is up, and the worker is either
  // Active turns remain eligible because Send now may intentionally interrupt.
  const canSendQueuedNow =
    status === "open" &&
    !state.workerStopped &&
    !state.workerRestarting &&
    (workerState === "running" || state.workerIdleStopped || state.rateLimitRetriesExhausted);

  // True when pressing "Send now" would interrupt a running, non-steerable turn
  // rather than send immediately, so the affordance can warn before it cancels
  // the agent's in-flight work.
  const sendNowInterruptsTurn =
    state.turnActive && !(state.promptCapabilities?.steering && !state.cancelling && !state.compacting);

  return {
    state,
    status,
    /** True while retrying a closed socket. */
    reconnecting,
    /** Current attempt number; 0 while the live socket is healthy,
     *  1..MAX while backing off. */
    retryCount,
    /** Seconds until the next retry. */
    retryCountdown,
    /** Retry limit exposed for banner copy. */
    maxRetries: ACP_MAX_RETRIES,
    /** Reset retry state and dial immediately. */
    manualReconnect,
    /** Distinguishes initial connection from recovery. */
    hasEverOpened,
    resolveApproval,
    resolveElicitation,
    sendPrompt,
    cancelPrompt,
    forceEndTurn,
    /** Fetch and prepend the next-older page of history. No-op when a
     *  fetch is already in flight or no older events remain. See #2236. */
    loadOlder,
    /** True when older events exist on the server beyond what's loaded,
     *  so the scroll-up handler and "Load earlier" button should offer
     *  to fetch more. See #2236. */
    hasMoreOlder,
    /** True while a `loadOlder` fetch is in flight; drives a spinner on
     *  the load-earlier affordance. See #2236. */
    loadingOlder,
    /** Timestamp (ms) of the most recent applied frame. The
     *  WorkingSpinner reads this on a 1s timer to decide whether to
     *  surface the "Force end turn" button. Exposed as a ref so the
     *  hook doesn't rerender every frame just to update a watchdog
     *  clock. See #1100 (C). */
    lastActivityRef,
    dismissError,
    dismissPrimer,
    dismissCompactionReminder,
    removeQueuedPrompt,
    editQueuedPrompt,
    clearQueue,
    sendQueuedNow,
    canSendQueuedNow,
    sendNowInterruptsTurn,
    dismissRejectedPrompt,
    dismissModeSwitchFailed,
    setConfigOption,
    dismissConfigOptionSwitchFailed,
  };
}

async function safeText(res: Response): Promise<string> {
  try {
    return (await res.text()).slice(0, 200);
  } catch {
    return "";
  }
}

/** Parse a JSON body, `null` on anything unparseable. Used where a missing or
 *  malformed body has a sane default rather than being an error. */
async function safeJson<T>(res: Response): Promise<T | null> {
  try {
    return (await res.json()) as T;
  } catch {
    return null;
  }
}

function describeError(e: unknown): string {
  if (e instanceof Error) return e.message;
  return String(e);
}
