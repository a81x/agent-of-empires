//! Move global-only profile overrides into the global config before ignoring them.

use anyhow::{Context, Result};
use std::{collections::HashSet, fs, io::ErrorKind, path::Path};
use tracing::info;

use crate::session::{
    acquire_storage_flock, atomic_write,
    config::{
        settings_schema::{merge_json, schema_ref},
        CONFIG_LOCK_FILENAME,
    },
    list_profile_names_in, Config,
};

pub fn run() -> Result<()> {
    run_in(&crate::session::get_app_dir()?, atomic_write)
}

fn read_config(path: &Path) -> Result<Option<toml::Table>> {
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("Read {}", path.display())),
    };
    content
        .parse()
        .map(Some)
        .with_context(|| format!("Parse {} during v030 migration", path.display()))
}

fn run_in(app_dir: &Path, mut write: impl FnMut(&Path, &[u8]) -> Result<()>) -> Result<()> {
    let _lock = acquire_storage_flock(app_dir, CONFIG_LOCK_FILENAME)?;
    let global_path = app_dir.join("config.toml");
    let mut global = read_config(&global_path)?.unwrap_or_default();
    let profiles_dir = app_dir.join("profiles");
    let mut paths = if profiles_dir.exists() {
        list_profile_names_in(&profiles_dir)?
            .into_iter()
            .map(|name| profiles_dir.join(name).join("config.toml"))
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    if let Some(default) = global
        .get("default_profile")
        .and_then(toml::Value::as_str)
        .filter(|name| !name.is_empty())
    {
        paths.insert(0, profiles_dir.join(default).join("config.toml"));
    }
    // Keep each file's highest-priority alias. A global alias still participates
    // in precedence, but must never be cleaned as a profile.
    let global_identity = if global_path.exists() {
        Some(fs::canonicalize(&global_path)?)
    } else {
        None
    };
    let mut seen = HashSet::new();
    let mut unique_paths = Vec::new();
    for path in paths {
        match fs::canonicalize(&path) {
            Ok(target) => {
                let is_global = global_identity.as_ref() == Some(&target);
                if seen.insert(target) {
                    unique_paths.push((path, is_global));
                }
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(error).with_context(|| format!("Resolve {}", path.display())),
        }
    }

    // Default profile wins, then alphabetical order. Clean the winners last
    // so a retry after a partial cleanup cannot promote a losing value.
    let mut profiles = Vec::new();
    for (path, is_global) in unique_paths.into_iter().rev() {
        let Some(mut profile) = read_config(&path)? else {
            continue;
        };
        let mut changed = false;
        for field in schema_ref()
            .iter()
            .filter(|field| !field.profile_overridable)
        {
            let Some(section) = profile
                .get_mut(&field.section)
                .and_then(toml::Value::as_table_mut)
            else {
                continue;
            };
            let Some(value) = section.remove(&field.field) else {
                continue;
            };
            if section.is_empty() {
                profile.remove(&field.section);
            }
            let global_section = global
                .entry(&field.section)
                .or_insert_with(|| toml::Value::Table(toml::Table::new()))
                .as_table_mut()
                .with_context(|| {
                    format!("Global config section '{}' must be a table", field.section)
                })?;
            if let Some(current) = global_section.get_mut(&field.field) {
                let mut merged = serde_json::to_value(&*current)?;
                merge_json(&mut merged, &serde_json::to_value(value)?);
                *current = toml::Value::try_from(merged)?;
            } else {
                global_section.insert(field.field.clone(), value);
            }
            changed = true;
        }
        if changed && !is_global {
            profiles.push((path, toml::to_string_pretty(&profile)?));
        }
    }
    if profiles.is_empty() {
        return Ok(());
    }
    let _: Config = toml::Value::Table(global.clone())
        .try_into()
        .context("Invalid global config after migrating profile settings")?;
    write(&global_path, toml::to_string_pretty(&global)?.as_bytes())?;
    for (path, content) in profiles {
        write(&path, content.as_bytes())?;
        info!(path = %path.display(), "v030: moved global-only profile settings to global config");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seed(app: &Path, path: &str, contents: &str) {
        let path = app.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    fn read(app: &Path, path: &str) -> toml::Table {
        read_config(&app.join(path)).unwrap().unwrap()
    }

    #[test]
    fn promotes_global_fields_with_default_profile_precedence_and_preserves_other_data() {
        for default in [
            "default_profile = 'work'\n",
            "",
            "default_profile = ''\n",
            "default_profile = 'missing'\n",
        ] {
            let dir = tempfile::tempdir().unwrap();
            let app = dir.path();
            seed(
                app,
                "config.toml",
                &format!("{default}[theme]\nname = 'empire'\n[unknown]\nkeep = 7\n"),
            );
            seed(
                app,
                "profiles/alpha/config.toml",
                "[theme]\nname = 'dracula'\n[web]\nnotify_on_idle = true\n",
            );
            seed(app, "profiles/work/config.toml", "description = 'keep me'\n[theme]\nname = 'rose-pine'\ncolor_mode = 'palette'\nidle_decay_minutes = 5\n[session]\nconfirm_before_quit = false\nsession_id_poller_max_threads = 12\nsidebar_position = 'right'\ndefault_tool = 'codex'\n[web]\nnotify_on_error = false\n[unknown]\nkeep = 'profile'\n");

            run_in(app, atomic_write).unwrap();

            let global = read(app, "config.toml");
            assert_eq!(
                global["theme"]["name"].as_str(),
                Some(if default.contains("'work'") {
                    "rose-pine"
                } else {
                    "dracula"
                })
            );
            assert_eq!(global["theme"]["color_mode"].as_str(), Some("palette"));
            assert_eq!(
                global["session"]["confirm_before_quit"].as_bool(),
                Some(false)
            );
            assert_eq!(
                global["session"]["session_id_poller_max_threads"].as_integer(),
                Some(12)
            );
            assert_eq!(
                global["session"]["sidebar_position"].as_str(),
                Some("right")
            );
            assert_eq!(global["web"]["notify_on_idle"].as_bool(), Some(true));
            assert_eq!(global["web"]["notify_on_error"].as_bool(), Some(false));
            assert_eq!(global["unknown"]["keep"].as_integer(), Some(7));
            assert_eq!(read(app, "profiles/work/config.toml"), "description = 'keep me'\n[theme]\nidle_decay_minutes = 5\n[session]\ndefault_tool = 'codex'\n[unknown]\nkeep = 'profile'\n".parse::<toml::Table>().unwrap());
            assert!(read(app, "profiles/alpha/config.toml").is_empty());
            run_in(app, |_, _| {
                anyhow::bail!("idempotent migration must not write")
            })
            .unwrap();
        }
    }

    #[test]
    fn resumes_after_each_write_failure_without_changing_the_winner() {
        for fail_at in 0..4 {
            let dir = tempfile::tempdir().unwrap();
            let app = dir.path();
            seed(app, "config.toml", "default_profile = 'work'\n");
            for (name, theme) in [
                ("alpha", "dracula"),
                ("beta", "empire"),
                ("work", "rose-pine"),
            ] {
                seed(
                    app,
                    &format!("profiles/{name}/config.toml"),
                    &format!("[theme]\nname = '{theme}'\n"),
                );
            }
            let mut writes = 0;
            let result = run_in(app, |path, contents| {
                let current = writes;
                writes += 1;
                anyhow::ensure!(current != fail_at, "injected write failure");
                atomic_write(path, contents)
            });
            assert!(result.is_err());
            assert_eq!(
                read(app, "profiles/work/config.toml")["theme"]["name"].as_str(),
                Some("rose-pine")
            );
            if fail_at > 0 {
                assert_eq!(
                    read(app, "config.toml")["theme"]["name"].as_str(),
                    Some("rose-pine")
                );
            }
            run_in(app, atomic_write).unwrap();
            assert_eq!(
                read(app, "config.toml")["theme"]["name"].as_str(),
                Some("rose-pine")
            );
            for name in ["alpha", "beta", "work"] {
                assert!(read(app, &format!("profiles/{name}/config.toml")).is_empty());
            }
        }
    }

    #[test]
    fn rejects_malformed_input_before_any_writes() {
        for (path, contents) in [
            ("config.toml", "[invalid"),
            ("profiles/work/config.toml", "[invalid"),
            (
                "profiles/work/config.toml",
                "[session]\nconfirm_before_quit = 'wrong type'\n",
            ),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let app = dir.path();
            seed(
                app,
                "profiles/other/config.toml",
                "[theme]\nname = 'dracula'\n",
            );
            seed(app, path, contents);
            assert!(run_in(app, |_, _| panic!(
                "must validate all inputs before writing"
            ))
            .is_err());
            assert_eq!(fs::read_to_string(app.join(path)).unwrap(), contents);
        }
    }

    #[test]
    fn handles_missing_files_and_does_not_rewrite_unrelated_settings() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path();
        run_in(app, |_, _| panic!("fresh install must not write")).unwrap();
        fs::create_dir_all(app.join("profiles/empty")).unwrap();
        seed(
            app,
            "profiles/work/config.toml",
            "# keep this comment\n[session]\ndefault_tool = 'codex'\n",
        );
        run_in(app, |_, _| {
            panic!("unrelated settings must not be rewritten")
        })
        .unwrap();
        seed(
            app,
            "profiles/other/config.toml",
            "[theme]\nname = 'dracula'\n",
        );
        run_in(app, atomic_write).unwrap();
        assert_eq!(
            read(app, "config.toml")["theme"]["name"].as_str(),
            Some("dracula")
        );
        assert!(fs::read_to_string(app.join("profiles/work/config.toml"))
            .unwrap()
            .starts_with("# keep this comment"));
    }

    #[test]
    fn merges_maps_and_replaces_lists_with_profile_precedence() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path();
        seed(app, "config.toml", "default_profile = 'work'\n[logging.targets]\ntmux = 'info'\nserver = 'error'\n[acp]\nallowed_agents = ['claude']\n");
        seed(app, "profiles/alpha/config.toml", "[logging.targets]\nsession = 'debug'\nserver = 'warn'\n[acp]\nallowed_agents = ['gemini']\n");
        seed(
            app,
            "profiles/work/config.toml",
            "[logging.targets]\nserver = 'debug'\n[acp]\nallowed_agents = ['codex']\n",
        );
        run_in(app, atomic_write).unwrap();
        let global = read(app, "config.toml");
        assert_eq!(
            global["logging"]["targets"],
            toml::Value::Table(
                "tmux = 'info'\nserver = 'debug'\nsession = 'debug'\n"
                    .parse::<toml::Table>()
                    .unwrap()
            )
        );
        assert_eq!(
            global["acp"]["allowed_agents"].as_array().unwrap(),
            &[toml::Value::String("codex".into())]
        );
        assert!(read(app, "profiles/work/config.toml").is_empty());
        assert!(read(app, "profiles/alpha/config.toml").is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn profile_aliases_preserve_precedence_and_retry_safety() {
        for (default, winner) in [("", "dracula"), ("default_profile = 'aaa'\n", "rose-pine")] {
            let dir = tempfile::tempdir().unwrap();
            let app = dir.path();
            seed(app, "config.toml", default);
            seed(
                app,
                "profiles/alpha/config.toml",
                "[theme]\nname = 'dracula'\n",
            );
            seed(
                app,
                "profiles/zulu/config.toml",
                "[theme]\nname = 'rose-pine'\n",
            );
            std::os::unix::fs::symlink("zulu", app.join("profiles/aaa")).unwrap();
            run_in(app, atomic_write).unwrap();
            assert_eq!(
                read(app, "config.toml")["theme"]["name"].as_str(),
                Some(winner)
            );
            assert!(app.join("profiles/aaa").is_symlink());
        }
        for fail_at in 0..3 {
            let dir = tempfile::tempdir().unwrap();
            let app = dir.path();
            seed(app, "config.toml", "default_profile = 'zulu'\n");
            seed(
                app,
                "profiles/alpha/config.toml",
                "[theme]\nname = 'dracula'\n",
            );
            seed(
                app,
                "profiles/zulu/config.toml",
                "[theme]\nname = 'rose-pine'\n",
            );
            fs::create_dir_all(app.join("profiles/beta")).unwrap();
            std::os::unix::fs::symlink(
                "../zulu/config.toml",
                app.join("profiles/beta/config.toml"),
            )
            .unwrap();
            let mut writes = 0;
            assert!(run_in(app, |path, content| {
                let current = writes;
                writes += 1;
                anyhow::ensure!(current != fail_at, "injected write failure");
                atomic_write(path, content)
            })
            .is_err());
            run_in(app, atomic_write).unwrap();
            assert_eq!(
                read(app, "config.toml")["theme"]["name"].as_str(),
                Some("rose-pine")
            );
            assert!(read(app, "profiles/beta/config.toml").is_empty());
            assert!(app.join("profiles/beta/config.toml").is_symlink());
        }
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path();
        seed(
            app,
            "config.toml",
            "default_profile = 'work'\n[theme]\nname = 'dracula'\n",
        );
        fs::create_dir_all(app.join("profiles/work")).unwrap();
        std::os::unix::fs::symlink("../../config.toml", app.join("profiles/work/config.toml"))
            .unwrap();
        seed(
            app,
            "profiles/alpha/config.toml",
            "[theme]\nname = 'rose-pine'\n[web]\nnotify_on_idle = true\n",
        );
        run_in(app, atomic_write).unwrap();
        assert!(read(app, "profiles/alpha/config.toml").is_empty());
        assert_eq!(
            read(app, "config.toml")["web"]["notify_on_idle"].as_bool(),
            Some(true)
        );
        assert!(app.join("profiles/work/config.toml").is_symlink());
        run_in(app, |_, _| {
            panic!("migrated aliases must not trigger writes")
        })
        .unwrap();
        assert_eq!(
            read(app, "config.toml")["theme"]["name"].as_str(),
            Some("dracula")
        );
    }

    #[test]
    fn holds_the_global_config_lock_through_profile_cleanup() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path();
        seed(
            app,
            "profiles/work/config.toml",
            "[theme]\nname = 'dracula'\n",
        );
        let mut written = Vec::new();
        run_in(app, |path, contents| {
            assert!(
                crate::session::try_acquire_storage_flock(app, CONFIG_LOCK_FILENAME)?.is_none()
            );
            atomic_write(path, contents)?;
            written.push(path.strip_prefix(app).unwrap().to_path_buf());
            Ok(())
        })
        .unwrap();
        assert_eq!(
            written,
            [
                Path::new("config.toml"),
                Path::new("profiles/work/config.toml")
            ]
        );
        assert!(
            crate::session::try_acquire_storage_flock(app, CONFIG_LOCK_FILENAME)
                .unwrap()
                .is_some()
        );
    }
}
