//! Per-adapter compatibility policy for ACP agents.

use agent_client_protocol::schema::v1::InitializeResponse;
use agent_client_protocol::schema::ProtocolVersion;

use super::state::StartupErrorDetail;

/// Single source of truth for the `claude-agent-acp` minimum-version floor.
pub const CLAUDE_AGENT_ACP_MIN_VERSION: &str = "0.55.0";

/// Parsed form of [`CLAUDE_AGENT_ACP_MIN_VERSION`].
fn claude_agent_acp_min_version() -> semver::Version {
    semver::Version::parse(CLAUDE_AGENT_ACP_MIN_VERSION)
        .expect("CLAUDE_AGENT_ACP_MIN_VERSION must be valid semver")
}

pub const CLAUDE_AGENT_ACP_STEERING_MIN_VERSION: &str = "0.64.0";

/// Parsed form of [`CLAUDE_AGENT_ACP_STEERING_MIN_VERSION`].
fn claude_agent_acp_steering_min_version() -> semver::Version {
    semver::Version::parse(CLAUDE_AGENT_ACP_STEERING_MIN_VERSION)
        .expect("CLAUDE_AGENT_ACP_STEERING_MIN_VERSION must be valid semver")
}

/// Single source of truth for the `opencode` minimum-version floor.
pub const OPENCODE_MIN_VERSION: &str = "1.16.0";

/// Parsed form of [`OPENCODE_MIN_VERSION`].
fn opencode_min_version() -> semver::Version {
    semver::Version::parse(OPENCODE_MIN_VERSION).expect("OPENCODE_MIN_VERSION must be valid semver")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VersionGate {
    pub expected: ExpectedAgent,
    pub binary: &'static str,
    pub package_name: &'static str,
    pub min_version: &'static str,
    pub install_command: &'static str,
    pub auto_install: bool,
}

/// The adapter aoe is trying to launch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExpectedAgent {
    ClaudeAgentAcp,
    CodexAcp,
    OpenCode,
    AoeAgent,
    Gemini,
    PiAcp,
    /// Unknown / user-configured agent.
    Other,
}

impl ExpectedAgent {
    /// Resolve from the binary name as configured in `AgentRegistry`.
    pub fn from_command(command: &str) -> Self {
        // Scan every whitespace-separated token.
        command
            .split_whitespace()
            .find_map(|token| {
                let basename = token.rsplit(['/', '\\']).next().unwrap_or(token);
                let stem = basename
                    .strip_suffix(".exe")
                    .or_else(|| basename.strip_suffix(".cmd"))
                    .or_else(|| basename.strip_suffix(".bat"))
                    .unwrap_or(basename);
                match stem {
                    "claude-agent-acp" => Some(Self::ClaudeAgentAcp),
                    "codex-acp" => Some(Self::CodexAcp),
                    "opencode" => Some(Self::OpenCode),
                    "aoe-agent" => Some(Self::AoeAgent),
                    "gemini" => Some(Self::Gemini),
                    "pi-acp" => Some(Self::PiAcp),
                    _ => None,
                }
            })
            .unwrap_or(Self::Other)
    }
}

/// What the policy requires from the adapter's `InitializeResponse`.
struct CompatibilityPolicy {
    /// If set, the adapter must report this exact `agent_info.name`.
    expected_name: Option<&'static str>,
    /// If set, the adapter's `agent_info.version` must parse as semver
    /// and be at least this value.
    min_version: Option<semver::Version>,
    /// The protocol version the client requested.
    required_protocol: ProtocolVersion,
    /// If `true`, missing `agent_info` or empty/unparseable version
    /// rejects.
    fail_on_missing_agent_info: bool,
}

impl ExpectedAgent {
    fn policy(self) -> CompatibilityPolicy {
        match self {
            Self::ClaudeAgentAcp => CompatibilityPolicy {
                expected_name: Some("@agentclientprotocol/claude-agent-acp"),
                min_version: Some(claude_agent_acp_min_version()),
                required_protocol: ProtocolVersion::V1,
                fail_on_missing_agent_info: true,
            },
            Self::OpenCode => CompatibilityPolicy {
                expected_name: Some("OpenCode"),
                min_version: Some(opencode_min_version()),
                required_protocol: ProtocolVersion::V1,
                fail_on_missing_agent_info: true,
            },
            Self::CodexAcp => CompatibilityPolicy {
                // The deprecated @zed-industries package still exposes the
                // same binary name, but lacks current Codex model metadata.
                expected_name: Some("@agentclientprotocol/codex-acp"),
                min_version: None,
                required_protocol: ProtocolVersion::V1,
                fail_on_missing_agent_info: false,
            },
            // Other adapters: protocol check only.
            Self::AoeAgent | Self::Gemini | Self::PiAcp | Self::Other => CompatibilityPolicy {
                expected_name: None,
                min_version: None,
                required_protocol: ProtocolVersion::V1,
                fail_on_missing_agent_info: false,
            },
        }
    }
}

/// Reasons aoe refuses to enter a session after a successful `initialize`
/// handshake.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartupError {
    /// Adapter reported a version below the minimum aoe requires.
    IncompatibleAgentVersion {
        package_name: String,
        installed: String,
        required: String,
        install_command: String,
        auto_install: bool,
    },
    /// Adapter passed name/version checks but reported a protocol version
    /// aoe does not speak.
    UnsupportedProtocolVersion { expected: String, received: String },
    /// Adapter omitted `agent_info` (or `agent_info.version`) entirely,
    /// and the policy for this adapter kind requires it.
    MissingAgentInfo {
        expected_package: String,
        install_command: String,
        auto_install: bool,
    },
    /// Adapter advertised a different package name than aoe expected for
    /// this `ExpectedAgent`.
    MismatchedAgentName {
        expected: String,
        received: String,
        install_command: String,
        auto_install: bool,
    },
    /// `agent_info.version` was present but did not parse as semver.
    UnparseableAgentVersion {
        package_name: String,
        raw_version: String,
        required: String,
        install_command: String,
        auto_install: bool,
    },
}

impl StartupError {
    /// Short, machine-stable identifier used by tests, logs, and the
    /// frontend reducer's discriminator.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::IncompatibleAgentVersion { .. } => "incompatible_agent_version",
            Self::UnsupportedProtocolVersion { .. } => "unsupported_protocol_version",
            Self::MissingAgentInfo { .. } => "missing_agent_info",
            Self::MismatchedAgentName { .. } => "mismatched_agent_name",
            Self::UnparseableAgentVersion { .. } => "unparseable_agent_version",
        }
    }

    /// User-facing one-liner suitable for the legacy
    /// `AgentStartupError { message }` event channel.
    pub fn user_message(&self) -> String {
        match self {
            Self::IncompatibleAgentVersion {
                package_name,
                installed,
                required,
                install_command,
                ..
            } => format!(
                "{package_name} {installed} installed; aoe requires >={required}. Run: {install_command}",
            ),
            Self::MissingAgentInfo {
                expected_package,
                install_command,
                ..
            } => format!(
                "Adapter did not report its package version. aoe requires {expected_package} >={CLAUDE_AGENT_ACP_MIN_VERSION}. Run: {install_command}",
            ),
            Self::MismatchedAgentName {
                expected,
                received,
                install_command,
                ..
            } => format!(
                "Adapter reported package name `{received}` but aoe expected `{expected}`. Run: {install_command}",
            ),
            Self::UnparseableAgentVersion {
                package_name,
                raw_version,
                required,
                install_command,
                ..
            } => format!(
                "{package_name} reported version `{raw_version}` which is not valid semver. aoe requires >={required}. Run: {install_command}",
            ),
            Self::UnsupportedProtocolVersion { expected, received } => format!(
                "Adapter speaks ACP protocol {received}; aoe requires {expected}.",
            ),
        }
    }
}

impl From<&StartupError> for StartupErrorDetail {
    fn from(err: &StartupError) -> Self {
        match err {
            StartupError::IncompatibleAgentVersion {
                package_name,
                installed,
                required,
                install_command,
                auto_install,
            } => StartupErrorDetail::IncompatibleAgentVersion {
                package_name: package_name.clone(),
                installed: installed.clone(),
                required: required.clone(),
                install_command: install_command.clone(),
                auto_install: *auto_install,
            },
            StartupError::MissingAgentInfo {
                expected_package,
                install_command,
                auto_install,
            } => StartupErrorDetail::MissingAgentInfo {
                expected_package: expected_package.clone(),
                install_command: install_command.clone(),
                auto_install: *auto_install,
            },
            StartupError::MismatchedAgentName {
                expected,
                received,
                install_command,
                auto_install,
            } => StartupErrorDetail::MismatchedAgentName {
                expected: expected.clone(),
                received: received.clone(),
                install_command: install_command.clone(),
                auto_install: *auto_install,
            },
            StartupError::UnparseableAgentVersion {
                package_name,
                raw_version,
                required,
                install_command,
                auto_install,
            } => StartupErrorDetail::UnparseableAgentVersion {
                package_name: package_name.clone(),
                raw_version: raw_version.clone(),
                required: required.clone(),
                install_command: install_command.clone(),
                auto_install: *auto_install,
            },
            StartupError::UnsupportedProtocolVersion { expected, received } => {
                StartupErrorDetail::UnsupportedProtocolVersion {
                    expected: expected.clone(),
                    received: received.clone(),
                }
            }
        }
    }
}

/// Validate an `InitializeResponse` against the policy for the adapter
/// aoe was launching.
pub fn validate(expected: ExpectedAgent, init: &InitializeResponse) -> Result<(), StartupError> {
    let policy = expected.policy();

    if init.protocol_version != policy.required_protocol {
        return Err(StartupError::UnsupportedProtocolVersion {
            expected: format!("{:?}", policy.required_protocol),
            received: format!("{:?}", init.protocol_version),
        });
    }

    // Fast path for adapters with no name/version requirement.
    if policy.min_version.is_none() && policy.expected_name.is_none() {
        return Ok(());
    }

    let install_command =
        install_command_for(expected).unwrap_or_else(|| "(see project docs)".to_string());
    let auto_install = auto_install_for(expected);

    let Some(info) = init.agent_info.as_ref() else {
        if policy.fail_on_missing_agent_info {
            return Err(StartupError::MissingAgentInfo {
                expected_package: policy.expected_name.unwrap_or("(unspecified)").to_string(),
                install_command,
                auto_install,
            });
        }
        return Ok(());
    };

    if let Some(expected_name) = policy.expected_name {
        if info.name != expected_name {
            return Err(StartupError::MismatchedAgentName {
                expected: expected_name.to_string(),
                received: info.name.clone(),
                install_command,
                auto_install,
            });
        }
    }

    if let Some(min) = policy.min_version {
        let raw = info.version.trim();
        if raw.is_empty() {
            if policy.fail_on_missing_agent_info {
                return Err(StartupError::MissingAgentInfo {
                    expected_package: policy
                        .expected_name
                        .unwrap_or(info.name.as_str())
                        .to_string(),
                    install_command,
                    auto_install,
                });
            }
            return Ok(());
        }
        let parsed = match semver::Version::parse(raw) {
            Ok(v) => v,
            Err(_) => {
                return Err(StartupError::UnparseableAgentVersion {
                    package_name: info.name.clone(),
                    raw_version: raw.to_string(),
                    required: min.to_string(),
                    install_command,
                    auto_install,
                });
            }
        };
        if parsed < min {
            return Err(StartupError::IncompatibleAgentVersion {
                package_name: info.name.clone(),
                installed: parsed.to_string(),
                required: min.to_string(),
                install_command,
                auto_install,
            });
        }
    }

    Ok(())
}

/// Whether AoE may steer this agent's running turn via the
/// `_session/steering` extension request.
pub fn supports_steering(expected: ExpectedAgent, init: &InitializeResponse) -> bool {
    let advertised = init
        .meta
        .as_ref()
        .and_then(|meta| meta.get("steering"))
        .and_then(|steering| steering.get("supported"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    if !advertised {
        return false;
    }
    if expected != ExpectedAgent::ClaudeAgentAcp {
        return true;
    }
    init.agent_info
        .as_ref()
        .and_then(|info| semver::Version::parse(info.version.trim()).ok())
        .is_some_and(|version| version >= claude_agent_acp_steering_min_version())
}

/// The ACP binary name aoe expects for this agent, or `None` for agents
/// with no fixed binary (`AoeAgent`, `Other`).
fn binary_for(expected: ExpectedAgent) -> Option<&'static str> {
    Some(match expected {
        ExpectedAgent::ClaudeAgentAcp => "claude-agent-acp",
        ExpectedAgent::CodexAcp => "codex-acp",
        ExpectedAgent::OpenCode => "opencode",
        ExpectedAgent::Gemini => "gemini",
        ExpectedAgent::PiAcp => "pi-acp",
        ExpectedAgent::AoeAgent | ExpectedAgent::Other => return None,
    })
}

/// Lookup table for the install commands surfaced in startup errors.
fn install_command_for(expected: ExpectedAgent) -> Option<String> {
    let bin = binary_for(expected)?;
    crate::acp::install_hints::install_hint_for(bin).map(|s| s.to_string())
}

/// Whether the web "Update & restart" action can install this agent itself
/// via a plain `npm install -g`.
fn auto_install_for(expected: ExpectedAgent) -> bool {
    binary_for(expected)
        .and_then(crate::acp::install_hints::npm_package_for)
        .is_some()
}

pub fn version_gate_for(expected: ExpectedAgent) -> Option<VersionGate> {
    let (binary, package_name, min_version) = match expected {
        ExpectedAgent::ClaudeAgentAcp => (
            "claude-agent-acp",
            "@agentclientprotocol/claude-agent-acp",
            CLAUDE_AGENT_ACP_MIN_VERSION,
        ),
        ExpectedAgent::OpenCode => ("opencode", "OpenCode", OPENCODE_MIN_VERSION),
        _ => return None,
    };
    Some(VersionGate {
        expected,
        binary,
        package_name,
        min_version,
        install_command: crate::acp::install_hints::install_hint_for(binary)
            .unwrap_or("(see project docs)"),
        auto_install: crate::acp::install_hints::npm_package_for(binary).is_some(),
    })
}

pub fn version_gates() -> impl Iterator<Item = VersionGate> {
    [ExpectedAgent::ClaudeAgentAcp, ExpectedAgent::OpenCode]
        .into_iter()
        .filter_map(version_gate_for)
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::schema::v1::Implementation;

    fn make_init(name: &str, version: &str) -> InitializeResponse {
        InitializeResponse::new(ProtocolVersion::V1).agent_info(Implementation::new(name, version))
    }

    fn make_init_no_info() -> InitializeResponse {
        InitializeResponse::new(ProtocolVersion::V1)
    }

    #[test]
    fn claude_below_minimum_rejected() {
        let init = make_init("@agentclientprotocol/claude-agent-acp", "0.0.0");
        let err = validate(ExpectedAgent::ClaudeAgentAcp, &init).unwrap_err();
        assert_eq!(err.kind(), "incompatible_agent_version");
        let StartupError::IncompatibleAgentVersion {
            installed,
            required,
            auto_install,
            ..
        } = err
        else {
            panic!()
        };
        assert_eq!(installed, "0.0.0");
        assert_eq!(required, CLAUDE_AGENT_ACP_MIN_VERSION);
        assert!(auto_install);
    }

    #[test]
    fn auto_install_only_for_npm_agents() {
        assert!(auto_install_for(ExpectedAgent::ClaudeAgentAcp));
        assert!(auto_install_for(ExpectedAgent::CodexAcp));
        assert!(auto_install_for(ExpectedAgent::Gemini));
        // Manual-install agents fall back to the displayed hint.
        assert!(!auto_install_for(ExpectedAgent::OpenCode));
        assert!(!auto_install_for(ExpectedAgent::PiAcp));
        assert!(!auto_install_for(ExpectedAgent::AoeAgent));
        assert!(!auto_install_for(ExpectedAgent::Other));
    }

    #[test]
    fn version_gates_expose_floor_metadata() {
        let claude = version_gate_for(ExpectedAgent::ClaudeAgentAcp).unwrap();
        assert_eq!(claude.binary, "claude-agent-acp");
        assert_eq!(claude.package_name, "@agentclientprotocol/claude-agent-acp");
        assert_eq!(claude.min_version, CLAUDE_AGENT_ACP_MIN_VERSION);
        assert!(claude.auto_install);

        let opencode = version_gate_for(ExpectedAgent::OpenCode).unwrap();
        assert_eq!(opencode.binary, "opencode");
        assert_eq!(opencode.package_name, "OpenCode");
        assert_eq!(opencode.min_version, OPENCODE_MIN_VERSION);
        assert!(!opencode.auto_install);

        assert!(version_gate_for(ExpectedAgent::CodexAcp).is_none());
        assert!(version_gate_for(ExpectedAgent::Other).is_none());
    }

    #[test]
    fn claude_just_below_floor_rejected() {
        let version = format!("{CLAUDE_AGENT_ACP_MIN_VERSION}-alpha.1");
        let init = make_init("@agentclientprotocol/claude-agent-acp", &version);
        let err = validate(ExpectedAgent::ClaudeAgentAcp, &init).unwrap_err();
        assert_eq!(err.kind(), "incompatible_agent_version");
    }

    #[test]
    fn claude_at_minimum_accepted() {
        let init = make_init(
            "@agentclientprotocol/claude-agent-acp",
            CLAUDE_AGENT_ACP_MIN_VERSION,
        );
        validate(ExpectedAgent::ClaudeAgentAcp, &init).unwrap();
    }

    #[test]
    fn claude_above_minimum_accepted() {
        let init = make_init("@agentclientprotocol/claude-agent-acp", "999.0.0");
        validate(ExpectedAgent::ClaudeAgentAcp, &init).unwrap();
    }

    #[test]
    fn dockerfile_pin_matches_floor() {
        let dockerfile = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/docker/Dockerfile"));
        let needle = "@agentclientprotocol/claude-agent-acp@^";
        let pins: Vec<String> = dockerfile
            .match_indices(needle)
            .map(|(idx, _)| {
                dockerfile[idx + needle.len()..]
                    .chars()
                    .take_while(|c| c.is_ascii_digit() || *c == '.')
                    .collect()
            })
            .collect();
        assert_eq!(
            pins,
            vec![CLAUDE_AGENT_ACP_MIN_VERSION.to_string()],
            "docker/Dockerfile claude-agent-acp pin must match CLAUDE_AGENT_ACP_MIN_VERSION",
        );
    }

    #[test]
    fn steering_gate_requires_advert_and_floor() {
        let below = "0.63.9";
        let cases: [(ExpectedAgent, Option<bool>, &str, bool); 8] = [
            // (agent, advertised bit, version, expected)
            (
                ExpectedAgent::ClaudeAgentAcp,
                Some(true),
                CLAUDE_AGENT_ACP_STEERING_MIN_VERSION,
                true,
            ),
            (ExpectedAgent::ClaudeAgentAcp, Some(true), "999.0.0", true),
            // Advertised but pre-opt-in: the case the floor exists for.
            (ExpectedAgent::ClaudeAgentAcp, Some(true), below, false),
            // A prerelease of the floor sorts strictly below it.
            (
                ExpectedAgent::ClaudeAgentAcp,
                Some(true),
                "0.64.0-alpha.1",
                false,
            ),
            // Floor met but the adapter never advertised.
            (ExpectedAgent::ClaudeAgentAcp, Some(false), "999.0.0", false),
            (ExpectedAgent::ClaudeAgentAcp, None, "999.0.0", false),
            // Unparseable version cannot clear the floor.
            (ExpectedAgent::ClaudeAgentAcp, Some(true), "nightly", false),
            // Non-claude adapters have no floor to clear, only the bit.
            (ExpectedAgent::CodexAcp, Some(true), "0.0.1", true),
        ];
        for (agent, advertised, version, expected) in cases {
            let mut init = make_init("@agentclientprotocol/claude-agent-acp", version);
            if let Some(supported) = advertised {
                init = init.meta(
                    serde_json::json!({ "steering": { "supported": supported } })
                        .as_object()
                        .unwrap()
                        .clone(),
                );
            }
            assert_eq!(
                supports_steering(agent, &init),
                expected,
                "{agent:?} advertised={advertised:?} version={version}"
            );
        }
    }

    #[test]
    fn steering_floor_at_or_above_hard_floor() {
        assert!(
            claude_agent_acp_steering_min_version() >= claude_agent_acp_min_version(),
            "steering floor {CLAUDE_AGENT_ACP_STEERING_MIN_VERSION} must not sit below the startup floor {CLAUDE_AGENT_ACP_MIN_VERSION}",
        );
    }

    #[test]
    fn claude_missing_agent_info_rejected() {
        let init = make_init_no_info();
        let err = validate(ExpectedAgent::ClaudeAgentAcp, &init).unwrap_err();
        assert_eq!(err.kind(), "missing_agent_info");
    }

    #[test]
    fn claude_empty_version_rejected() {
        let init = make_init("@agentclientprotocol/claude-agent-acp", "");
        let err = validate(ExpectedAgent::ClaudeAgentAcp, &init).unwrap_err();
        assert_eq!(err.kind(), "missing_agent_info");
    }

    #[test]
    fn claude_unparseable_version_rejected() {
        let init = make_init("@agentclientprotocol/claude-agent-acp", "not-semver");
        let err = validate(ExpectedAgent::ClaudeAgentAcp, &init).unwrap_err();
        assert_eq!(err.kind(), "unparseable_agent_version");
    }

    #[test]
    fn claude_mismatched_name_rejected() {
        let init = make_init("some-other-package", "0.39.0");
        let err = validate(ExpectedAgent::ClaudeAgentAcp, &init).unwrap_err();
        assert_eq!(err.kind(), "mismatched_agent_name");
    }

    #[test]
    fn non_gated_permissive_on_missing_info() {
        let init = make_init_no_info();
        validate(ExpectedAgent::CodexAcp, &init).unwrap();
        validate(ExpectedAgent::AoeAgent, &init).unwrap();
        validate(ExpectedAgent::Other, &init).unwrap();
    }

    #[test]
    fn opencode_below_floor_rejected() {
        let init = make_init("OpenCode", "1.15.13");
        let err = validate(ExpectedAgent::OpenCode, &init).unwrap_err();
        assert_eq!(err.kind(), "incompatible_agent_version");
    }

    #[test]
    fn opencode_at_floor_accepted() {
        let init = make_init("OpenCode", OPENCODE_MIN_VERSION);
        validate(ExpectedAgent::OpenCode, &init).unwrap();
    }

    #[test]
    fn opencode_above_floor_accepted() {
        let init = make_init("OpenCode", "1.17.9");
        validate(ExpectedAgent::OpenCode, &init).unwrap();
    }

    #[test]
    fn opencode_missing_agent_info_rejected() {
        let init = make_init_no_info();
        let err = validate(ExpectedAgent::OpenCode, &init).unwrap_err();
        assert_eq!(err.kind(), "missing_agent_info");
    }

    #[test]
    fn opencode_mismatched_name_rejected() {
        let init = make_init("opencode", OPENCODE_MIN_VERSION);
        let err = validate(ExpectedAgent::OpenCode, &init).unwrap_err();
        assert_eq!(err.kind(), "mismatched_agent_name");
    }

    #[test]
    fn codex_accepts_current_package_without_a_version_floor() {
        let init = make_init("@agentclientprotocol/codex-acp", "0.0.1");
        validate(ExpectedAgent::CodexAcp, &init).unwrap();
    }

    #[test]
    fn codex_rejects_legacy_adapter_reported_name() {
        let init = make_init("codex-acp", "0.16.0");
        let err = validate(ExpectedAgent::CodexAcp, &init).unwrap_err();
        assert_eq!(err.kind(), "mismatched_agent_name");
        assert!(err
            .user_message()
            .contains("@agentclientprotocol/codex-acp"));
        assert!(err
            .user_message()
            .contains("npm install -g @agentclientprotocol/codex-acp@latest"));
    }

    #[test]
    fn from_command_recognises_path_prefixed_binary() {
        assert_eq!(
            ExpectedAgent::from_command("/usr/local/bin/claude-agent-acp"),
            ExpectedAgent::ClaudeAgentAcp
        );
        assert_eq!(
            ExpectedAgent::from_command("claude-agent-acp"),
            ExpectedAgent::ClaudeAgentAcp
        );
        assert_eq!(
            ExpectedAgent::from_command("unknown-bin"),
            ExpectedAgent::Other
        );
    }

    #[test]
    fn from_command_handles_windows_paths_and_extensions() {
        assert_eq!(
            ExpectedAgent::from_command(
                "C:\\Users\\u\\AppData\\Roaming\\npm\\claude-agent-acp.cmd"
            ),
            ExpectedAgent::ClaudeAgentAcp
        );
        assert_eq!(
            ExpectedAgent::from_command("claude-agent-acp.exe"),
            ExpectedAgent::ClaudeAgentAcp
        );
        assert_eq!(
            ExpectedAgent::from_command("D:\\bin\\claude-agent-acp.bat"),
            ExpectedAgent::ClaudeAgentAcp
        );
    }

    #[test]
    fn from_command_handles_wrapper_token_prefix() {
        assert_eq!(
            ExpectedAgent::from_command("claude-agent-acp --some-flag"),
            ExpectedAgent::ClaudeAgentAcp
        );
        assert_eq!(
            ExpectedAgent::from_command("  /usr/local/bin/claude-agent-acp  "),
            ExpectedAgent::ClaudeAgentAcp
        );
        assert_eq!(
            ExpectedAgent::from_command("bash claude-agent-acp"),
            ExpectedAgent::ClaudeAgentAcp
        );
        assert_eq!(
            ExpectedAgent::from_command("env FOO=bar /usr/local/bin/claude-agent-acp"),
            ExpectedAgent::ClaudeAgentAcp
        );
    }
}
