//! Persisted registry of remote daemon endpoints.
//!
//! Lives at `<app_dir>/remotes.toml`, owner-only, alongside the other
//! credential stores rather than in `config.toml`: entries carry a bearer
//! token and, for a daemon behind a passphrase wall, a login session and its
//! device-binding secret. The passphrase itself is never stored; an expired
//! session is re-established by prompting for it again.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

const REMOTES_FILE: &str = "remotes.toml";

/// Bumped on a breaking layout change. An unparseable or newer file is an
/// error rather than a silent reset: unlike a login session, losing an entry
/// costs the user a re-add they did not ask for.
const REMOTES_SCHEMA_VERSION: u32 = 1;

/// One configured endpoint. `token`, `session` and `binding` are credentials;
/// never render them.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Remote {
    pub name: String,
    pub url: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    /// `aoe_session` id from a passphrase login.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    /// base64url of the 32-byte device-binding secret paired with `session`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding: Option<String>,
    /// Added with `--insecure`: credentials may travel over non-loopback HTTP.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub insecure: bool,
}

fn default_enabled() -> bool {
    true
}

impl std::fmt::Debug for Remote {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Remote")
            .field("name", &self.name)
            .field("enabled", &self.enabled)
            .field("authenticated", &(self.token.is_some() || self.has_login()))
            .field("insecure", &self.insecure)
            .finish_non_exhaustive()
    }
}

impl Remote {
    /// Whether this entry carries a passphrase-login credential pair. Both
    /// halves are required: a session without its binding is unusable.
    pub fn has_login(&self) -> bool {
        self.session.is_some() && self.binding.is_some()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Registry {
    #[serde(default)]
    version: u32,
    #[serde(default, rename = "remote")]
    remotes: Vec<Remote>,
}

impl Registry {
    pub fn remotes(&self) -> &[Remote] {
        &self.remotes
    }

    pub fn enabled(&self) -> impl Iterator<Item = &Remote> {
        self.remotes.iter().filter(|r| r.enabled)
    }

    pub fn get(&self, name: &str) -> Option<&Remote> {
        self.remotes.iter().find(|r| r.name == name)
    }

    /// Insert or replace by name. Returns whether an existing entry was
    /// replaced, so the caller can word its output accurately.
    pub fn upsert(&mut self, remote: Remote) -> bool {
        match self.remotes.iter_mut().find(|r| r.name == remote.name) {
            Some(existing) => {
                *existing = remote;
                true
            }
            None => {
                self.remotes.push(remote);
                false
            }
        }
    }

    pub fn remove(&mut self, name: &str) -> bool {
        let before = self.remotes.len();
        self.remotes.retain(|r| r.name != name);
        self.remotes.len() != before
    }
}

pub fn registry_path() -> Result<PathBuf> {
    Ok(crate::session::get_app_dir()?.join(REMOTES_FILE))
}

pub fn load() -> Result<Registry> {
    load_from(&registry_path()?)
}

pub fn save(registry: &Registry) -> Result<()> {
    save_to(&registry_path()?, registry)
}

pub fn load_from(path: &Path) -> Result<Registry> {
    if !path.exists() {
        return Ok(Registry {
            version: REMOTES_SCHEMA_VERSION,
            remotes: Vec::new(),
        });
    }
    crate::util::check_owner_only_file(path, "remotes registry")?;
    let raw = std::fs::read_to_string(path).context("read remotes registry")?;
    let registry: Registry = toml::from_str(&raw).context("parse remotes registry")?;
    if registry.version > REMOTES_SCHEMA_VERSION {
        bail!(
            "remotes.toml was written by a newer aoe (schema {} > {}); upgrade or move the file aside",
            registry.version,
            REMOTES_SCHEMA_VERSION
        );
    }
    Ok(registry)
}

pub fn save_to(path: &Path, registry: &Registry) -> Result<()> {
    crate::util::check_owner_only_file(path, "remotes registry")?;
    let mut out = registry.clone();
    out.version = REMOTES_SCHEMA_VERSION;
    let body = toml::to_string(&out).context("serialize remotes registry")?;
    crate::session::atomic_write(path, body.as_bytes()).context("write remotes registry")?;
    // `atomic_write` lands a 0600 temp file; re-assert so an entry written
    // over a previously loose file cannot stay readable.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn remote(name: &str) -> Remote {
        Remote {
            name: name.to_string(),
            url: format!("https://{name}.example.ts.net"),
            enabled: true,
            token: Some("tok".to_string()),
            session: None,
            binding: None,
            insecure: false,
        }
    }

    #[test]
    fn round_trips_through_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("remotes.toml");
        let mut registry = Registry::default();
        registry.upsert(remote("mini"));
        registry.upsert(Remote {
            insecure: true,
            ..remote("lan")
        });
        save_to(&path, &registry).unwrap();

        let loaded = load_from(&path).unwrap();
        assert_eq!(loaded.remotes(), registry.remotes());
        assert!(loaded.get("lan").unwrap().insecure);
        assert_eq!(loaded.version, REMOTES_SCHEMA_VERSION);
    }

    #[test]
    fn a_missing_file_is_an_empty_registry() {
        let dir = tempfile::tempdir().unwrap();
        let loaded = load_from(&dir.path().join("absent.toml")).unwrap();
        assert!(loaded.remotes().is_empty());
    }

    #[test]
    fn upsert_replaces_by_name_and_reports_it() {
        let mut registry = Registry::default();
        assert!(!registry.upsert(remote("mini")));
        let mut changed = remote("mini");
        changed.url = "https://other.example.ts.net".to_string();
        assert!(registry.upsert(changed));
        assert_eq!(registry.remotes().len(), 1);
        assert_eq!(
            registry.get("mini").unwrap().url,
            "https://other.example.ts.net"
        );
    }

    #[test]
    fn remove_reports_whether_anything_went() {
        let mut registry = Registry::default();
        registry.upsert(remote("mini"));
        assert!(registry.remove("mini"));
        assert!(!registry.remove("mini"));
    }

    #[test]
    fn enabled_filters_disabled_entries() {
        let mut registry = Registry::default();
        registry.upsert(remote("on"));
        let mut off = remote("off");
        off.enabled = false;
        registry.upsert(off);
        let names: Vec<_> = registry.enabled().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["on"]);
    }

    #[test]
    fn a_newer_schema_is_refused_rather_than_silently_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("remotes.toml");
        std::fs::write(&path, "version = 99\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        assert!(load_from(&path).is_err());
    }

    #[test]
    fn login_requires_both_halves() {
        let mut r = remote("mini");
        assert!(!r.has_login());
        r.session = Some("s".to_string());
        assert!(!r.has_login());
        r.binding = Some("b".to_string());
        assert!(r.has_login());
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_registry_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("planted.toml");
        std::fs::write(&target, "version = 1\n").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600)).unwrap();
        let link = dir.path().join("remotes.toml");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(load_from(&link).is_err());
        assert!(save_to(&link, &Registry::default()).is_err());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "version = 1\n");
    }
}
