//! Named agent registry: maps an agent name (e.g. `claude-code`,
//! `aoe-agent`, `gemini`) to a spawn command + args.

use super::install_hints::{env_allowlist_for, install_hint_for, AOE_AGENT_BINARY};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Convert the static per-binary slice from `install_hints::env_allowlist_for`
/// into the owned `Option<Vec<String>>` field on `AgentSpec`.
fn default_env_allowlist(binary: &str) -> Option<Vec<String>> {
    let keys = env_allowlist_for(binary);
    (!keys.is_empty()).then(|| keys.iter().map(|s| s.to_string()).collect())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSpec {
    /// Executable to run, e.g. `npx` or `/usr/local/bin/aoe-agent`.
    pub command: String,
    pub args: Vec<String>,
    /// Human-readable description shown in the settings TUI and
    /// `aoe acp agents`.
    pub description: String,
    /// Provider env vars forwarded to this agent on top of the infrastructure
    /// inheritance set defined by `ALWAYS_FORWARD_ENV` in `acp_client/spawn.rs`.
    pub env_allowlist: Option<Vec<String>>,
}

impl AgentSpec {
    /// Build an ACP `AgentSpec` from a custom agent's `agent_acp_cmd`
    /// string.
    pub fn from_acp_cmd(name: &str, cmd: &str) -> Result<AgentSpec, String> {
        let argv = shell_words::split(cmd).map_err(|e| {
            format!("custom agent `{name}` has a malformed structured view command ({e})")
        })?;
        let mut argv = argv.into_iter();
        let command = argv
            .next()
            .filter(|c| !c.trim().is_empty())
            .ok_or_else(|| format!("custom agent `{name}` has an empty structured view command"))?;
        Ok(AgentSpec {
            command,
            args: argv.collect(),
            description: format!("Custom ACP agent `{name}`"),
            env_allowlist: None,
        })
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentRegistry {
    pub agents: HashMap<String, AgentSpec>,
}

impl AgentRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns a registry seeded with one entry per aoe tool that has
    /// a published ACP server, plus aoe's own multi-provider `aoe-agent`.
    pub fn with_defaults() -> Self {
        let mut reg = Self::new();

        let claude_install = install_hint_for("claude-agent-acp").unwrap_or("(see project docs)");
        reg.agents.insert(
            "claude".into(),
            AgentSpec {
                command: "claude-agent-acp".into(),
                args: vec![],
                description: format!(
                    "Anthropic Claude via the official ACP adapter ({claude_install})"
                ),
                env_allowlist: default_env_allowlist("claude-agent-acp"),
            },
        );
        // Legacy alias used by older session records before the
        // tool-keyed naming.
        reg.agents.insert(
            "claude-code".into(),
            AgentSpec {
                command: "claude-agent-acp".into(),
                args: vec![],
                description: "Alias for `claude` (legacy name)".into(),
                env_allowlist: default_env_allowlist("claude-agent-acp"),
            },
        );
        reg.agents.insert(
            "opencode".into(),
            AgentSpec {
                command: "opencode".into(),
                args: vec!["acp".into()],
                description: "OpenCode (SST), native ACP via `opencode acp`".into(),
                env_allowlist: default_env_allowlist("opencode"),
            },
        );
        reg.agents.insert(
            "gemini".into(),
            AgentSpec {
                command: "gemini".into(),
                args: vec!["--acp".into()],
                description: "Google Gemini CLI, native ACP via `gemini --acp`".into(),
                env_allowlist: default_env_allowlist("gemini"),
            },
        );
        reg.agents.insert(
            "codex".into(),
            AgentSpec {
                command: "codex-acp".into(),
                args: vec![],
                description:
                    "OpenAI Codex CLI via ACP adapter (npm i -g @agentclientprotocol/codex-acp@latest)".into(),
                env_allowlist: default_env_allowlist("codex-acp"),
            },
        );
        reg.agents.insert(
            "vibe".into(),
            AgentSpec {
                command: "vibe-acp".into(),
                args: vec![],
                description: "Mistral Vibe, native ACP via the bundled `vibe-acp` binary".into(),
                env_allowlist: default_env_allowlist("vibe-acp"),
            },
        );
        reg.agents.insert(
            "pi".into(),
            AgentSpec {
                command: "pi-acp".into(),
                args: vec![],
                description: "Pi coding agent (`pi`) via the pi-acp adapter (npm i -g pi-acp)"
                    .into(),
                env_allowlist: default_env_allowlist("pi-acp"),
            },
        );
        reg.agents.insert(
            "omp".into(),
            AgentSpec {
                command: "omp".into(),
                args: vec!["acp".into()],
                description: "Oh My Pi coding agent, native ACP via `omp acp`".into(),
                env_allowlist: default_env_allowlist("omp"),
            },
        );
        reg.agents.insert(
            "kimi".into(),
            AgentSpec {
                command: "kimi".into(),
                args: vec!["acp".into()],
                description: "Kimi Code (Moonshot AI), native ACP via `kimi acp`".into(),
                env_allowlist: default_env_allowlist("kimi"),
            },
        );
        reg.agents.insert(
            "prime-agent".into(),
            AgentSpec {
                command: "prime-agent".into(),
                args: vec!["--mode".into(), "acp".into()],
                description: "PrimeIntellect Prime Agent, native ACP via `prime-agent --mode acp`"
                    .into(),
                env_allowlist: default_env_allowlist("prime-agent"),
            },
        );
        reg.agents.insert(
            "aoe-agent".into(),
            AgentSpec {
                // Installed into the app dir like the npm adapters and found
                // through `bundled_adapter_bin`.
                command: AOE_AGENT_BINARY.into(),
                args: vec![],
                description: "aoe's bundled multi-provider agent (Vercel AI SDK)".into(),
                env_allowlist: default_env_allowlist(AOE_AGENT_BINARY),
            },
        );
        reg
    }

    pub fn get(&self, name: &str) -> Option<&AgentSpec> {
        self.agents.get(name)
    }

    pub fn upsert(&mut self, name: String, spec: AgentSpec) {
        self.agents.insert(name, spec);
    }

    pub fn remove(&mut self, name: &str) -> Option<AgentSpec> {
        self.agents.remove(name)
    }

    pub fn list(&self) -> Vec<(&String, &AgentSpec)> {
        let mut entries: Vec<_> = self.agents.iter().collect();
        entries.sort_by_key(|(n, _)| n.as_str());
        entries
    }
}

pub fn inherited_acp_base(tool: &str, agent_detect_as: &HashMap<String, String>) -> Option<String> {
    let base = agent_detect_as.get(tool)?;
    AgentRegistry::with_defaults()
        .get(base)
        .map(|_| base.clone())
}

/// The agent a structured-view session of `tool` spawns as.
pub fn pick_acp_agent_name(
    registry: &AgentRegistry,
    session: &crate::session::config::SessionConfig,
    acp: &crate::session::config::AcpConfig,
    tool: &str,
    explicit_override: Option<&str>,
) -> String {
    if let Some(name) = explicit_override {
        if !name.is_empty() {
            return name.to_string();
        }
    }
    if registry.get(tool).is_some() {
        return tool.to_string();
    }
    if session
        .agent_acp_cmd
        .get(tool)
        .is_some_and(|cmd| AgentSpec::from_acp_cmd(tool, cmd).is_ok())
    {
        return tool.to_string();
    }
    if let Some(base) = inherited_acp_base(tool, &session.agent_detect_as) {
        return base;
    }
    if tool == "claude" {
        "claude".into()
    } else {
        acp.resolved_default_agent().to_string()
    }
}

/// The model pinned for the agent `tool` spawns as, if any.
pub fn pinned_model_for_tool(
    config: &crate::session::config::Config,
    tool: &str,
    explicit_override: Option<&str>,
) -> Option<String> {
    let agent = pick_acp_agent_name(
        &AgentRegistry::with_defaults(),
        &config.session,
        &config.acp,
        tool,
        explicit_override,
    );
    config.acp.pinned_model_for(&agent)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_include_claude_code_and_aoe_agent() {
        let reg = AgentRegistry::with_defaults();
        assert!(reg.get("claude-code").is_some());
        assert!(reg.get("aoe-agent").is_some());
        assert!(reg.get("omp").is_some());
    }

    #[test]
    fn pick_acp_agent_name_resolves_the_agent_a_spawn_runs() {
        let registry = AgentRegistry::with_defaults();
        let session = crate::session::config::SessionConfig {
            agent_detect_as: [
                ("my-claude".to_string(), "claude".to_string()),
                ("my-cursor".to_string(), "cursor".to_string()),
                ("bad-sp".to_string(), "claude".to_string()),
            ]
            .into(),
            agent_acp_cmd: [
                ("oc-sp".to_string(), "ocp run sp acp".to_string()),
                ("bad-sp".to_string(), String::new()),
            ]
            .into(),
            ..Default::default()
        };
        let acp = crate::session::config::AcpConfig {
            default_agent: "opencode".into(),
            ..Default::default()
        };

        for (tool, explicit, want) in [
            ("claude", Some("gemini"), "gemini"),
            ("claude", Some(""), "claude"),
            ("opencode", None, "opencode"),
            ("oc-sp", None, "oc-sp"),
            ("my-claude", None, "claude"),
            ("my-cursor", None, "opencode"),
            ("claude", None, "claude"),
            ("unknown", None, "opencode"),
            ("bad-sp", None, "claude"),
        ] {
            assert_eq!(
                pick_acp_agent_name(&registry, &session, &acp, tool, explicit),
                want,
                "{tool} / {explicit:?}"
            );
        }
    }

    #[test]
    fn pinned_model_for_tool_reads_the_resolved_agents_pin() {
        let mut config = crate::session::config::Config::default();
        config
            .session
            .agent_detect_as
            .insert("my-claude".into(), "claude".into());
        config.acp.acp_defaults.insert(
            "claude".into(),
            crate::session::config::AcpAgentDefaults {
                model: Some("claude-pinned".into()),
                pin_model: true,
                ..Default::default()
            },
        );

        assert_eq!(
            pinned_model_for_tool(&config, "my-claude", None).as_deref(),
            Some("claude-pinned")
        );
        assert_eq!(
            pinned_model_for_tool(&config, "claude", None).as_deref(),
            Some("claude-pinned")
        );
        assert_eq!(
            pinned_model_for_tool(&config, "claude", Some("gemini")),
            None
        );
        assert_eq!(pinned_model_for_tool(&config, "opencode", None), None);
    }

    #[test]
    fn from_acp_cmd_splits_argv() {
        let spec = AgentSpec::from_acp_cmd("oc-sp", "ocp run sp acp").unwrap();
        assert_eq!(spec.command, "ocp");
        assert_eq!(spec.args, vec!["run", "sp", "acp"]);
        assert_eq!(spec.description, "Custom ACP agent `oc-sp`");
        assert!(spec.env_allowlist.is_none());
    }

    #[test]
    fn from_acp_cmd_honors_quoting() {
        let spec = AgentSpec::from_acp_cmd("wrap", "sh -lc 'ocp run sp acp'").unwrap();
        assert_eq!(spec.command, "sh");
        assert_eq!(spec.args, vec!["-lc", "ocp run sp acp"]);
    }

    #[test]
    fn from_acp_cmd_rejects_empty() {
        assert!(AgentSpec::from_acp_cmd("x", "").is_err());
        assert!(AgentSpec::from_acp_cmd("x", "   ").is_err());
    }

    #[test]
    fn from_acp_cmd_rejects_unbalanced_quotes() {
        assert!(AgentSpec::from_acp_cmd("x", "ocp run \"unterminated").is_err());
    }

    #[test]
    fn default_env_allowlists_match_verified_providers() {
        let reg = AgentRegistry::with_defaults();
        let al = |name: &str| reg.get(name).and_then(|s| s.env_allowlist.clone());

        let claude_keys = [
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_AUTH_TOKEN",
            "CLAUDE_CODE_OAUTH_TOKEN",
            "CLAUDE_CONFIG_DIR",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect::<Vec<_>>();
        assert_eq!(al("claude").as_deref(), Some(claude_keys.as_slice()));
        assert_eq!(al("claude-code").as_deref(), Some(claude_keys.as_slice()));
        assert_eq!(
            al("codex").as_deref(),
            Some(
                &[
                    "CODEX_API_KEY",
                    "OPENAI_API_KEY",
                    "OPENAI_BASE_URL",
                    "CODEX_HOME"
                ][..]
                    .iter()
                    .map(|s| s.to_string())
                    .collect::<Vec<_>>()[..]
            )
        );
        assert_eq!(
            al("aoe-agent").as_deref(),
            Some(
                &[
                    "ANTHROPIC_API_KEY",
                    "OPENAI_API_KEY",
                    "OPENAI_BASE_URL",
                    "GOOGLE_GENERATIVE_AI_API_KEY"
                ][..]
                    .iter()
                    .map(|s| s.to_string())
                    .collect::<Vec<_>>()[..]
            )
        );
        let gemini = al("gemini").unwrap_or_default();
        assert!(gemini.iter().any(|k| k == "GEMINI_API_KEY"));
        assert!(!gemini.iter().any(|k| k == "GOOGLE_GENERATIVE_AI_API_KEY"));
        for key in [
            "GOOGLE_GENAI_USE_VERTEXAI",
            "GOOGLE_APPLICATION_CREDENTIALS",
            "GOOGLE_CLOUD_PROJECT",
            "GOOGLE_CLOUD_LOCATION",
        ] {
            assert!(gemini.iter().any(|k| k == key), "gemini missing {key}");
        }
        let aoe = al("aoe-agent").unwrap_or_default();
        assert!(!aoe.iter().any(|k| k == "GEMINI_API_KEY"));

        let opencode = al("opencode").unwrap_or_default();
        for key in [
            "ANTHROPIC_API_KEY",
            "OPENROUTER_API_KEY",
            "OPENCODE_API_KEY",
            "GOOGLE_GENERATIVE_AI_API_KEY",
            "GOOGLE_API_KEY",
            "GEMINI_API_KEY",
        ] {
            assert!(opencode.iter().any(|k| k == key), "opencode missing {key}");
        }

        // Pinned whole so a dropped or guessed name fails here.
        assert_eq!(
            al("prime-agent"),
            Some(
                [
                    "PRIME_API_KEY",
                    "PRIME_TEAM_ID",
                    "ANTHROPIC_OAUTH_TOKEN",
                    "ANTHROPIC_API_KEY",
                    "OPENAI_API_KEY",
                    "AZURE_OPENAI_API_KEY",
                    "DEEPSEEK_API_KEY",
                    "GEMINI_API_KEY",
                    "GROQ_API_KEY",
                    "CEREBRAS_API_KEY",
                    "XAI_API_KEY",
                    "OPENROUTER_API_KEY",
                    "AI_GATEWAY_API_KEY",
                    "ZAI_API_KEY",
                    "MISTRAL_API_KEY",
                    "MINIMAX_API_KEY",
                    "MINIMAX_CN_API_KEY",
                    "MOONSHOT_API_KEY",
                    "HF_TOKEN",
                    "FIREWORKS_API_KEY",
                    "OPENCODE_API_KEY",
                    "KIMI_API_KEY",
                    "CLOUDFLARE_API_KEY",
                    "XIAOMI_API_KEY",
                    "XIAOMI_TOKEN_PLAN_CN_API_KEY",
                    "XIAOMI_TOKEN_PLAN_AMS_API_KEY",
                    "XIAOMI_TOKEN_PLAN_SGP_API_KEY",
                    "COPILOT_GITHUB_TOKEN",
                    "GH_TOKEN",
                    "GITHUB_TOKEN",
                    "GOOGLE_CLOUD_API_KEY",
                    "GOOGLE_APPLICATION_CREDENTIALS",
                    "GOOGLE_CLOUD_PROJECT",
                    "GCLOUD_PROJECT",
                    "GOOGLE_CLOUD_LOCATION",
                    "AWS_BEARER_TOKEN_BEDROCK",
                    "AWS_PROFILE",
                    "AWS_ACCESS_KEY_ID",
                    "AWS_SECRET_ACCESS_KEY",
                    "AWS_SESSION_TOKEN",
                    "AWS_REGION",
                    "AWS_DEFAULT_REGION",
                    "AWS_CONFIG_FILE",
                    "AWS_SHARED_CREDENTIALS_FILE",
                    "AWS_WEB_IDENTITY_TOKEN_FILE",
                    "AWS_ROLE_ARN",
                    "AWS_ROLE_SESSION_NAME",
                    "AWS_CONTAINER_CREDENTIALS_RELATIVE_URI",
                    "AWS_CONTAINER_CREDENTIALS_FULL_URI",
                    "AWS_CONTAINER_AUTHORIZATION_TOKEN",
                    "AWS_CONTAINER_AUTHORIZATION_TOKEN_FILE",
                ]
                .map(String::from)
                .to_vec()
            )
        );

        for name in ["pi", "omp", "kimi", "vibe"] {
            assert!(
                al(name).is_none(),
                "{name} must have None env_allowlist until source-verified"
            );
        }

        let with_allowlist: std::collections::BTreeSet<&str> = reg
            .list()
            .into_iter()
            .filter(|(_, spec)| spec.env_allowlist.is_some())
            .map(|(name, _)| name.as_str())
            .collect();
        assert_eq!(
            with_allowlist,
            [
                "aoe-agent",
                "claude",
                "claude-code",
                "codex",
                "gemini",
                "opencode",
                "prime-agent",
            ]
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>(),
            "the set of adapters with an env_allowlist changed; update env_allowlist_for and this assertion together"
        );
    }

    #[test]
    fn defaults_spawn_commands_match_expected() {
        let reg = AgentRegistry::with_defaults();
        let expected: &[(&str, &str, &[&str])] = &[
            ("claude", "claude-agent-acp", &[]),
            ("claude-code", "claude-agent-acp", &[]),
            ("opencode", "opencode", &["acp"]),
            ("gemini", "gemini", &["--acp"]),
            ("codex", "codex-acp", &[]),
            ("vibe", "vibe-acp", &[]),
            ("pi", "pi-acp", &[]),
            ("omp", "omp", &["acp"]),
            ("kimi", "kimi", &["acp"]),
            ("prime-agent", "prime-agent", &["--mode", "acp"]),
            ("aoe-agent", "aoe-agent", &[]),
        ];
        for (name, command, args) in expected {
            let spec = reg
                .get(name)
                .unwrap_or_else(|| panic!("missing adapter {name}"));
            assert_eq!(spec.command, *command, "{name} command drifted");
            assert_eq!(spec.args, *args, "{name} args drifted");
        }
    }

    #[test]
    fn inherited_acp_base_resolves_only_registry_backed_bases() {
        let mut detect_as = HashMap::new();
        // Wrapper inheriting a base that has an ACP adapter → resolves to base.
        detect_as.insert("lenovo-claude".to_string(), "claude".to_string());
        detect_as.insert("work-codex".to_string(), "codex".to_string());
        // Wrapper inheriting a terminal-only base (no ACP adapter) → None.
        detect_as.insert("my-cursor".to_string(), "cursor".to_string());
        // Base is another custom name, not a registry key → None.
        detect_as.insert("chain".to_string(), "lenovo-claude".to_string());

        let cases = [
            ("lenovo-claude", Some("claude")),
            ("work-codex", Some("codex")),
            ("my-cursor", None),
            ("chain", None),
            // No mapping at all.
            ("unmapped", None),
        ];
        for (tool, expected) in cases {
            assert_eq!(
                inherited_acp_base(tool, &detect_as).as_deref(),
                expected,
                "{tool}"
            );
        }
    }

    #[test]
    fn list_is_sorted() {
        let mut reg = AgentRegistry::new();
        reg.upsert(
            "zeta".into(),
            AgentSpec {
                command: "z".into(),
                args: vec![],
                description: "z".into(),
                env_allowlist: None,
            },
        );
        reg.upsert(
            "alpha".into(),
            AgentSpec {
                command: "a".into(),
                args: vec![],
                description: "a".into(),
                env_allowlist: None,
            },
        );
        let names: Vec<&str> = reg.list().iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, vec!["alpha", "zeta"]);
    }
}
