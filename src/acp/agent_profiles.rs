//! Server-side per-agent capability and naming profiles.

/// Per-agent server-side profile.
#[derive(Debug, Clone)]
pub struct AgentProfile {
    /// Registry key.
    pub key: &'static str,
    /// `_meta.<namespace>.parentToolUseId` lookup order for subagent
    /// linkage.
    pub parent_meta_namespaces: &'static [&'static str],
    /// Slash commands that reset the conversation.
    pub clear_aliases: &'static [&'static str],
    pub clear_requires_driven_reset: bool,
    /// When true, the server synthesises a `PlanUpdated` event from a
    /// `kind: switch_mode` tool call (Claude's ExitPlanMode shape).
    pub supports_exit_plan_mode: bool,
    /// When true, the server synthesises a `WakeupScheduled` event from
    /// a tool call titled `"ScheduleWakeup"`.
    pub supports_wakeup_tools: bool,
    /// When true, the agent emits keepalive progress pings for
    /// long-running tools under a derived id `<baseToolId>-heartbeat-<N>`
    /// (see `acp_client::is_heartbeat_tool_call_id`).
    pub emits_heartbeat_keepalives: bool,
    /// ACP session-mode id that means "bypass all permission prompts"
    /// (the wizard's "Auto-approve" / profile `yolo_mode_default`).
    pub yolo_mode_id: Option<&'static str>,
}

impl AgentProfile {
    /// True when `text` matches any of this profile's clear-conversation
    /// slash aliases, tolerating surrounding whitespace and a trailing
    /// argument cluster.
    pub fn is_clear_command(&self, text: &str) -> bool {
        let trimmed = text.trim();
        for alias in self.clear_aliases {
            if trimmed == *alias {
                return true;
            }
            if let Some(rest) = trimmed.strip_prefix(*alias) {
                if rest.starts_with(char::is_whitespace) {
                    return true;
                }
            }
        }
        false
    }

    /// Read a parent tool-call id from an ACP `_meta` blob, trying each
    /// namespace this profile knows about.
    pub fn parent_tool_use_id_from_meta(
        &self,
        meta: &Option<serde_json::Map<String, serde_json::Value>>,
    ) -> Option<String> {
        let map = meta.as_ref()?;
        for namespace in self.parent_meta_namespaces {
            if let Some(v) = map
                .get(*namespace)
                .and_then(|ns| ns.get("parentToolUseId"))
                .and_then(|v| v.as_str())
            {
                return Some(v.to_string());
            }
        }
        None
    }

    /// True iff the agent surfaces session-start memory recall through
    /// the tool channel with the `_meta.claudeCode.toolName` namespace
    /// claude-agent-acp adopted in v0.37.0 (upstream).
    pub fn supports_memory_recall_tool(&self) -> bool {
        self.parent_meta_namespaces.contains(&"claudeCode")
    }
}

/// Claude via `claude-agent-acp`.
pub const CLAUDE: AgentProfile = AgentProfile {
    key: "claude",
    parent_meta_namespaces: &["claudeCode"],
    clear_aliases: &["/clear"],
    clear_requires_driven_reset: true,
    supports_exit_plan_mode: true,
    supports_wakeup_tools: true,
    emits_heartbeat_keepalives: true,
    yolo_mode_id: Some("bypassPermissions"),
};

/// Legacy alias key carried by older session records (`agent_name="claude-code"`).
pub const CLAUDE_CODE: AgentProfile = AgentProfile {
    key: "claude-code",
    ..CLAUDE
};

/// OpenAI Codex CLI via `@agentclientprotocol/codex-acp`.
pub const CODEX: AgentProfile = AgentProfile {
    key: "codex",
    parent_meta_namespaces: &[],
    clear_aliases: &["/new"],
    clear_requires_driven_reset: true,
    supports_exit_plan_mode: false,
    supports_wakeup_tools: false,
    emits_heartbeat_keepalives: false,
    yolo_mode_id: Some("agent-full-access"),
};

/// SST OpenCode via native `opencode acp`.
pub const OPENCODE: AgentProfile = AgentProfile {
    key: "opencode",
    parent_meta_namespaces: &[],
    clear_aliases: &["/new"],
    clear_requires_driven_reset: false,
    supports_exit_plan_mode: false,
    supports_wakeup_tools: false,
    emits_heartbeat_keepalives: false,
    // OpenCode's bypass-mode id over ACP is unverified; leave YOLO a no-op
    // until observed rather than guessing an id the adapter would reject.
    yolo_mode_id: None,
};

/// Google Gemini CLI via native `gemini --acp`.
pub const GEMINI: AgentProfile = AgentProfile {
    key: "gemini",
    parent_meta_namespaces: &[],
    clear_aliases: &[],
    clear_requires_driven_reset: false,
    supports_exit_plan_mode: false,
    supports_wakeup_tools: false,
    emits_heartbeat_keepalives: false,
    // gemini-cli surfaces its YOLO approval mode over `gemini --acp` with
    // the `yolo` id (see the CurrentModeUpdate mapping in acp_client/update_events.rs).
    yolo_mode_id: Some("yolo"),
};

/// Mistral Vibe via bundled `vibe-acp`.
pub const VIBE: AgentProfile = AgentProfile {
    key: "vibe",
    parent_meta_namespaces: &[],
    clear_aliases: &[],
    clear_requires_driven_reset: false,
    supports_exit_plan_mode: false,
    supports_wakeup_tools: false,
    emits_heartbeat_keepalives: false,
    yolo_mode_id: None,
};

/// Pi coding agent via `pi-acp`.
pub const PI: AgentProfile = AgentProfile {
    key: "pi",
    parent_meta_namespaces: &[],
    clear_aliases: &[],
    clear_requires_driven_reset: false,
    supports_exit_plan_mode: false,
    supports_wakeup_tools: false,
    emits_heartbeat_keepalives: false,
    yolo_mode_id: None,
};

/// Oh My Pi via native `omp acp`.
pub const OMP: AgentProfile = AgentProfile {
    key: "omp",
    parent_meta_namespaces: &[],
    clear_aliases: &["/new"],
    clear_requires_driven_reset: false,
    supports_exit_plan_mode: false,
    supports_wakeup_tools: false,
    emits_heartbeat_keepalives: false,
    yolo_mode_id: None,
};

/// Kimi Code (Moonshot AI) via native `kimi acp`.
pub const KIMI: AgentProfile = AgentProfile {
    key: "kimi",
    parent_meta_namespaces: &[],
    clear_aliases: &["/new"],
    clear_requires_driven_reset: false,
    supports_exit_plan_mode: false,
    supports_wakeup_tools: false,
    emits_heartbeat_keepalives: false,
    yolo_mode_id: Some("yolo"),
};

/// PrimeIntellect Prime Agent via native `prime-agent --mode acp`.
pub const PRIME_AGENT: AgentProfile = AgentProfile {
    key: "prime-agent",
    parent_meta_namespaces: &[],
    clear_aliases: &[],
    clear_requires_driven_reset: false,
    supports_exit_plan_mode: false,
    supports_wakeup_tools: false,
    emits_heartbeat_keepalives: false,
    yolo_mode_id: None,
};

/// aoe's own multi-provider agent (Vercel AI SDK 7), at
/// `acp-worker/aoe-agent/src/index.ts`.
pub const AOE_AGENT: AgentProfile = AgentProfile {
    key: "aoe-agent",
    parent_meta_namespaces: &[],
    clear_aliases: &["/clear"],
    // A forwarded /clear is ordinary model text.
    clear_requires_driven_reset: true,
    // No ExitPlanMode tool and no mode channel; no ScheduleWakeup or cron
    // tools.
    supports_exit_plan_mode: false,
    supports_wakeup_tools: false,
    emits_heartbeat_keepalives: false,
    // `session/set_mode` is `() => ({})`: it accepts any id and changes
    // nothing, and the adapter advertises no modes.
    yolo_mode_id: None,
};

/// Permissive default for unknown registry keys: no claude-specific
/// gates fire, no clear aliases match, no parent-meta lookup.
pub const DEFAULT: AgentProfile = AgentProfile {
    key: "default",
    parent_meta_namespaces: &[],
    clear_aliases: &[],
    clear_requires_driven_reset: false,
    supports_exit_plan_mode: false,
    supports_wakeup_tools: false,
    emits_heartbeat_keepalives: false,
    yolo_mode_id: None,
};

/// Resolve a static profile by registry key.
pub fn resolve(key: &str) -> &'static AgentProfile {
    match key {
        "claude" => &CLAUDE,
        "claude-code" => &CLAUDE_CODE,
        "codex" => &CODEX,
        "opencode" => &OPENCODE,
        "gemini" => &GEMINI,
        "vibe" => &VIBE,
        "pi" => &PI,
        "omp" => &OMP,
        "kimi" => &KIMI,
        "prime-agent" => &PRIME_AGENT,
        "aoe-agent" => &AOE_AGENT,
        _ => &DEFAULT,
    }
}

pub fn is_reviewed(key: &str) -> bool {
    matches!(
        key,
        "claude" | "claude-code" | "codex" | "gemini" | "kimi" | "aoe-agent"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_known_agents() {
        assert_eq!(resolve("claude").key, "claude");
        assert_eq!(resolve("claude-code").key, "claude-code");
        assert_eq!(resolve("codex").key, "codex");
        assert_eq!(resolve("opencode").key, "opencode");
        assert_eq!(resolve("gemini").key, "gemini");
        assert_eq!(resolve("vibe").key, "vibe");
        assert_eq!(resolve("pi").key, "pi");
        assert_eq!(resolve("omp").key, "omp");
        assert_eq!(resolve("kimi").key, "kimi");
        assert_eq!(resolve("prime-agent").key, "prime-agent");
        assert_eq!(resolve("aoe-agent").key, "aoe-agent");
    }

    #[test]
    fn resolve_falls_back_to_default() {
        assert_eq!(resolve("").key, "default");
        assert_eq!(resolve("unknown-agent").key, "default");
    }

    #[test]
    fn is_reviewed_covers_only_verified_approval_conventions() {
        // Verified adapters get the benign automation classifications.
        for key in [
            "claude",
            "claude-code",
            "codex",
            "gemini",
            "kimi",
            "aoe-agent",
        ] {
            assert!(is_reviewed(key), "{key} should be reviewed");
        }
        for key in [
            "opencode",
            "vibe",
            "pi",
            "omp",
            "prime-agent",
            "unknown-agent",
            "",
        ] {
            assert!(!is_reviewed(key), "{key} should not be reviewed");
        }
    }

    #[test]
    fn yolo_mode_id_is_adapter_specific() {
        assert_eq!(resolve("claude").yolo_mode_id, Some("bypassPermissions"));
        // Inherited from CLAUDE via `..CLAUDE`.
        assert_eq!(
            resolve("claude-code").yolo_mode_id,
            Some("bypassPermissions")
        );
        assert_eq!(resolve("aoe-agent").yolo_mode_id, None);
        assert_eq!(resolve("codex").yolo_mode_id, Some("agent-full-access"));
        assert_eq!(resolve("gemini").yolo_mode_id, Some("yolo"));
        assert_eq!(resolve("kimi").yolo_mode_id, Some("yolo"));
        // Adapters with no verified bypass mode keep YOLO a no-op.
        assert_eq!(resolve("opencode").yolo_mode_id, None);
        assert_eq!(resolve("vibe").yolo_mode_id, None);
        assert_eq!(resolve("pi").yolo_mode_id, None);
        assert_eq!(resolve("omp").yolo_mode_id, None);
        assert_eq!(resolve("prime-agent").yolo_mode_id, None);
        assert_eq!(resolve("unknown-agent").yolo_mode_id, None);
    }

    #[test]
    fn is_clear_command_per_profile() {
        assert!(CLAUDE.is_clear_command("/clear"));
        assert!(CLAUDE.is_clear_command("  /clear  "));
        assert!(CLAUDE.is_clear_command("/clear --hard"));
        assert!(!CLAUDE.is_clear_command("/new"));

        assert!(CODEX.is_clear_command("/new"));
        assert!(!CODEX.is_clear_command("/clear"));

        assert!(OPENCODE.is_clear_command("/new"));
        assert!(!OPENCODE.is_clear_command("/clear"));

        // Gemini has no clear alias; nothing matches.
        assert!(!GEMINI.is_clear_command("/clear"));
        assert!(!GEMINI.is_clear_command("/new"));
        assert!(!GEMINI.is_clear_command("/restore"));
        assert!(OMP.is_clear_command("/new"));
        assert!(!OMP.is_clear_command("/clear"));
    }

    /// Two different defects share the driven-reset remedy.
    #[test]
    fn clear_requires_driven_reset_for_codex_and_claude() {
        for profile in [&CODEX, &CLAUDE, &CLAUDE_CODE, &AOE_AGENT] {
            assert!(profile.clear_requires_driven_reset, "{}", profile.key);
        }
        for profile in [
            &OPENCODE,
            &GEMINI,
            &VIBE,
            &PI,
            &OMP,
            &KIMI,
            &PRIME_AGENT,
            &DEFAULT,
        ] {
            assert!(!profile.clear_requires_driven_reset, "{}", profile.key);
        }
    }

    #[test]
    fn is_clear_command_rejects_partial_matches() {
        assert!(!CLAUDE.is_clear_command("clear"));
        assert!(!CLAUDE.is_clear_command("/cleart"));
        assert!(!CLAUDE.is_clear_command("hello /clear world"));
        assert!(!CLAUDE.is_clear_command(""));
    }

    #[test]
    fn parent_tool_use_id_from_meta_reads_claudecode_for_claude() {
        let mut meta = serde_json::Map::new();
        meta.insert(
            "claudeCode".to_string(),
            serde_json::json!({ "parentToolUseId": "tc-parent-7" }),
        );
        assert_eq!(
            CLAUDE.parent_tool_use_id_from_meta(&Some(meta)),
            Some("tc-parent-7".to_string())
        );
    }

    #[test]
    fn parent_tool_use_id_from_meta_returns_none_for_unverified_agents() {
        let mut meta = serde_json::Map::new();
        meta.insert(
            "opencode".to_string(),
            serde_json::json!({ "parentToolUseId": "tc-9" }),
        );
        assert!(OPENCODE.parent_tool_use_id_from_meta(&Some(meta)).is_none());

        let mut claude_meta = serde_json::Map::new();
        claude_meta.insert(
            "claudeCode".to_string(),
            serde_json::json!({ "parentToolUseId": "tc-parent-7" }),
        );
        assert!(AOE_AGENT
            .parent_tool_use_id_from_meta(&Some(claude_meta))
            .is_none());
    }

    #[test]
    fn parent_tool_use_id_from_meta_returns_none_for_missing_namespace() {
        let mut meta = serde_json::Map::new();
        meta.insert(
            "otherNamespace".to_string(),
            serde_json::json!({ "parentToolUseId": "tc-x" }),
        );
        assert!(CLAUDE.parent_tool_use_id_from_meta(&Some(meta)).is_none());
    }

    #[test]
    fn parent_tool_use_id_from_meta_returns_none_for_non_string_value() {
        let mut meta = serde_json::Map::new();
        meta.insert(
            "claudeCode".to_string(),
            serde_json::json!({ "parentToolUseId": 42 }),
        );
        assert!(CLAUDE.parent_tool_use_id_from_meta(&Some(meta)).is_none());
    }

    #[test]
    fn parent_tool_use_id_from_meta_returns_none_for_none_meta() {
        assert!(CLAUDE.parent_tool_use_id_from_meta(&None).is_none());
    }

    #[test]
    fn deferred_profiles_keep_parent_linkage_disabled() {
        for profile in [&VIBE, &PI, &OMP, &KIMI, &PRIME_AGENT] {
            assert!(
                profile.parent_meta_namespaces.is_empty(),
                "{}: parent linkage must stay off until observed",
                profile.key
            );
        }
    }

    #[test]
    fn capability_flags_only_set_for_claude_family() {
        for profile in [&CLAUDE, &CLAUDE_CODE] {
            assert!(profile.supports_exit_plan_mode);
            assert!(profile.supports_wakeup_tools);
        }
        for profile in [
            &CODEX,
            &OPENCODE,
            &GEMINI,
            &VIBE,
            &PI,
            &OMP,
            &KIMI,
            &PRIME_AGENT,
            &AOE_AGENT,
            &DEFAULT,
        ] {
            assert!(!profile.supports_exit_plan_mode, "{}", profile.key);
            assert!(!profile.supports_wakeup_tools, "{}", profile.key);
        }
    }
}
