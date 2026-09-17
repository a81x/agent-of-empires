#[derive(Clone, Debug)]
pub struct VolumeMount {
    pub host_path: String,
    pub container_path: String,
    pub read_only: bool,
}

pub struct NamedVolumeMount {
    pub volume_name: String,
    pub container_path: String,
}

/// `Inherit` passes the value through the process environment (`-e KEY`) so secrets stay
/// out of `ps`; `Literal` emits `-e KEY=VALUE`.
#[derive(Debug, Clone, PartialEq)]
pub enum EnvEntry {
    Inherit { key: String, value: String },
    Literal { key: String, value: String },
}

impl EnvEntry {
    pub fn key(&self) -> &str {
        match self {
            EnvEntry::Inherit { key, .. } | EnvEntry::Literal { key, .. } => key,
        }
    }

    pub fn value(&self) -> &str {
        match self {
            EnvEntry::Inherit { value, .. } | EnvEntry::Literal { value, .. } => value,
        }
    }
}

/// The caller must set the returned inherit pairs on the spawning `Command`. Dedupes by
/// key, first wins.
pub fn docker_env_args(entries: &[EnvEntry]) -> (Vec<String>, Vec<(String, String)>) {
    let mut argv = Vec::with_capacity(entries.len() * 2);
    let mut inherit = Vec::new();
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for entry in entries {
        let key = entry.key();
        if !seen.insert(key) {
            continue;
        }
        argv.push("-e".to_string());
        match entry {
            EnvEntry::Inherit { key, value } => {
                argv.push(key.clone());
                inherit.push((key.clone(), value.clone()));
            }
            EnvEntry::Literal { key, value } => {
                argv.push(format!("{}={}", key, value));
            }
        }
    }
    (argv, inherit)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunFlag {
    Privileged,
    CapAdd,
    CapDrop,
    SecurityOpt,
}

#[derive(Debug, Default, Clone)]
pub struct RunPolicy {
    pub privileged: bool,
    pub cap_add: Vec<String>,
    pub cap_drop: Vec<String>,
    pub security_opt: Vec<String>,
    pub extra_run_args: Vec<String>,
}

#[derive(Default)]
pub struct ContainerConfig {
    pub working_dir: String,
    pub volumes: Vec<VolumeMount>,
    pub anonymous_volumes: Vec<String>,
    pub named_ignore_volumes: Vec<NamedVolumeMount>,
    /// False unless the paths come from the real project layout; gates volume reclaim.
    pub named_ignore_volumes_authoritative: bool,
    pub environment: Vec<EnvEntry>,
    pub cpu_limit: Option<String>,
    pub memory_limit: Option<String>,
    pub port_mappings: Vec<String>,
    pub network: Option<String>,
    pub selinux_relabel: bool,
    pub identity_publisher_installed: bool,
    /// Labelled at create so a container built before a file was shared can be told apart.
    pub shared_credential_mounts: Vec<String>,
    /// Labelled at create so a container reused after a tool swap can be told apart.
    pub agent_tool: String,
    pub run_policy: RunPolicy,
}

pub(crate) const SHARED_CREDENTIAL_MOUNTS_LABEL: &str =
    "com.agent-of-empires.shared-credential-mounts";

pub(crate) const AGENT_TOOL_LABEL: &str = "com.agent-of-empires.agent-tool";

impl ContainerConfig {
    pub(crate) fn mount_fingerprint(&self) -> String {
        use sha2::{Digest, Sha256};

        let mut digest = Sha256::new();
        for volume in &self.volumes {
            for value in [&volume.host_path, &volume.container_path] {
                digest.update((value.len() as u64).to_le_bytes());
                digest.update(value.as_bytes());
            }
            digest.update([u8::from(volume.read_only)]);
        }
        if let Some(home) = self.environment.iter().find(|entry| entry.key() == "HOME") {
            digest.update(b"HOME");
            digest.update((home.value().len() as u64).to_le_bytes());
            digest.update(home.value().as_bytes());
        } else {
            digest.update(b"NO_HOME");
        }
        digest
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    pub(crate) fn shared_credential_label(&self) -> String {
        self.shared_credential_mounts.join(",")
    }

    pub(crate) fn host_path_for_container_path(
        &self,
        container_path: &std::path::Path,
        writable: bool,
    ) -> Option<std::path::PathBuf> {
        let (volume, relative) = self
            .volumes
            .iter()
            .filter_map(|volume| {
                container_path
                    .strip_prefix(std::path::Path::new(&volume.container_path))
                    .ok()
                    .map(|relative| (volume, relative))
            })
            .max_by_key(|(volume, _)| {
                std::path::Path::new(&volume.container_path)
                    .components()
                    .count()
            })?;
        let bind_depth = std::path::Path::new(&volume.container_path)
            .components()
            .count();
        let shadow_depth = self
            .anonymous_volumes
            .iter()
            .map(String::as_str)
            .chain(
                self.named_ignore_volumes
                    .iter()
                    .map(|volume| volume.container_path.as_str()),
            )
            .filter_map(|mounted| {
                container_path
                    .strip_prefix(std::path::Path::new(mounted))
                    .ok()
                    .map(|_| std::path::Path::new(mounted).components().count())
            })
            .max();
        // Ignore volumes own their subtree and are emitted after equal bind destinations.
        if shadow_depth.is_some_and(|depth| depth >= bind_depth) || (writable && volume.read_only) {
            return None;
        }
        Some(std::path::Path::new(&volume.host_path).join(relative))
    }

    pub(crate) fn path_is_mounted(
        &self,
        host_path: &std::path::Path,
        container_path: &std::path::Path,
        writable: bool,
    ) -> bool {
        self.host_path_for_container_path(container_path, writable)
            .is_some_and(|mapped| mapped == host_path)
    }

    pub(crate) fn uses_default_container_home(&self) -> bool {
        self.environment
            .iter()
            .find(|entry| entry.key() == "HOME")
            .is_some_and(|entry| entry.value() == "/root")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publisher_home_must_be_explicit_and_is_fingerprinted() {
        let missing = ContainerConfig::default();
        assert!(!missing.uses_default_container_home());

        let mut root = ContainerConfig::default();
        root.environment.push(EnvEntry::Literal {
            key: "HOME".to_string(),
            value: "/root".to_string(),
        });
        assert!(root.uses_default_container_home());

        let mut alternate = ContainerConfig::default();
        alternate.environment.push(EnvEntry::Literal {
            key: "HOME".to_string(),
            value: "/alternate".to_string(),
        });
        assert!(!alternate.uses_default_container_home());
        assert_ne!(root.mount_fingerprint(), alternate.mount_fingerprint());
        assert_ne!(missing.mount_fingerprint(), root.mount_fingerprint());
    }

    #[test]
    fn docker_env_args_inherit_keeps_value_out_of_argv() {
        let entries = vec![EnvEntry::Inherit {
            key: "GH_TOKEN".to_string(),
            value: "ghp_secret".to_string(),
        }];
        let (argv, inherit) = docker_env_args(&entries);
        assert_eq!(argv, vec!["-e".to_string(), "GH_TOKEN".to_string()]);
        assert_eq!(
            inherit,
            vec![("GH_TOKEN".to_string(), "ghp_secret".to_string())]
        );
        assert!(
            !argv.iter().any(|a| a.contains("ghp_secret")),
            "secret leaked into argv"
        );
    }

    #[test]
    fn docker_env_args_literal_emits_key_eq_value() {
        let entries = vec![EnvEntry::Literal {
            key: "TERM".to_string(),
            value: "xterm-256color".to_string(),
        }];
        let (argv, inherit) = docker_env_args(&entries);
        assert_eq!(
            argv,
            vec!["-e".to_string(), "TERM=xterm-256color".to_string()]
        );
        assert!(inherit.is_empty());
    }

    #[test]
    fn docker_env_args_mixed_preserves_order() {
        let entries = vec![
            EnvEntry::Inherit {
                key: "SECRET".to_string(),
                value: "s3cr3t".to_string(),
            },
            EnvEntry::Literal {
                key: "TERM".to_string(),
                value: "xterm".to_string(),
            },
            EnvEntry::Inherit {
                key: "TOKEN".to_string(),
                value: "tok".to_string(),
            },
        ];
        let (argv, inherit) = docker_env_args(&entries);
        assert_eq!(
            argv,
            vec![
                "-e".to_string(),
                "SECRET".to_string(),
                "-e".to_string(),
                "TERM=xterm".to_string(),
                "-e".to_string(),
                "TOKEN".to_string(),
            ]
        );
        assert_eq!(
            inherit,
            vec![
                ("SECRET".to_string(), "s3cr3t".to_string()),
                ("TOKEN".to_string(), "tok".to_string()),
            ]
        );
    }

    #[test]
    fn docker_env_args_empty() {
        let (argv, inherit) = docker_env_args(&[]);
        assert!(argv.is_empty());
        assert!(inherit.is_empty());
    }

    #[test]
    fn docker_env_args_dedupes_duplicate_keys_first_wins() {
        let entries = vec![
            EnvEntry::Inherit {
                key: "GH_TOKEN".to_string(),
                value: "ghp_first".to_string(),
            },
            EnvEntry::Literal {
                key: "GH_TOKEN".to_string(),
                value: "literal_should_be_skipped".to_string(),
            },
            EnvEntry::Inherit {
                key: "OTHER".to_string(),
                value: "kept".to_string(),
            },
        ];
        let (argv, inherit) = docker_env_args(&entries);
        assert_eq!(
            argv,
            vec![
                "-e".to_string(),
                "GH_TOKEN".to_string(),
                "-e".to_string(),
                "OTHER".to_string(),
            ]
        );
        assert_eq!(
            inherit,
            vec![
                ("GH_TOKEN".to_string(), "ghp_first".to_string()),
                ("OTHER".to_string(), "kept".to_string()),
            ]
        );
        assert!(
            !argv.iter().any(|a| a.contains("literal_should_be_skipped")),
            "duplicate key's value leaked into argv"
        );
    }

    #[test]
    fn container_path_mapping_respects_shadow_volumes() {
        let mut config = ContainerConfig::default();
        config.volumes.push(VolumeMount {
            host_path: "/host/project".to_string(),
            container_path: "/workspace/project".to_string(),
            read_only: false,
        });
        let source = std::path::Path::new("/workspace/project/src/lib.rs");
        assert_eq!(
            config.host_path_for_container_path(source, false),
            Some(std::path::PathBuf::from("/host/project/src/lib.rs"))
        );

        let settings = std::path::Path::new("/workspace/project/.prime/agent/settings.json");
        config
            .anonymous_volumes
            .push("/workspace/project/.prime".to_string());
        assert_eq!(config.host_path_for_container_path(settings, false), None);

        config.anonymous_volumes.clear();
        config.named_ignore_volumes.push(NamedVolumeMount {
            volume_name: "aoe-prime-shadow".to_string(),
            container_path: "/workspace/project/.prime".to_string(),
        });
        assert_eq!(config.host_path_for_container_path(settings, false), None);
        assert_eq!(
            config.host_path_for_container_path(source, false),
            Some(std::path::PathBuf::from("/host/project/src/lib.rs"))
        );
    }
}
