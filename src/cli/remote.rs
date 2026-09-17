//! `aoe remote`: manage the daemon endpoints the TUI can connect to.

use anyhow::{bail, Context, Result};
use clap::{Args, Subcommand};

use reqwest::StatusCode;

use crate::daemon::login::{self, LoginError};
use crate::daemon::remotes::{self, Remote};
use crate::daemon::{DaemonClient, DaemonClientError, SessionCredential};

#[derive(Subcommand)]
pub enum RemoteCommands {
    /// Add or update a remote daemon endpoint
    Add(RemoteAddArgs),

    /// List configured remotes
    #[command(alias = "ls")]
    List,

    /// Remove a configured remote
    #[command(alias = "rm")]
    Remove(RemoteRemoveArgs),

    /// Enable or disable a remote without removing it
    Toggle(RemoteToggleArgs),
}

#[derive(Args)]
pub struct RemoteAddArgs {
    /// Short name used to select this remote
    pub name: String,

    /// Base URL, e.g. `https://box.tailnet.ts.net`. A `?token=` query, as
    /// `aoe serve --status` prints it, supplies the token.
    pub url: String,

    /// Bearer token the daemon prints at startup
    #[arg(long)]
    pub token: Option<String>,

    /// Passphrase for a daemon started with `--remote`. Exchanged once for a
    /// device-bound session; never stored.
    #[arg(long, env = "AOE_REMOTE_PASSPHRASE")]
    pub passphrase: Option<String>,

    /// Send credentials over plain HTTP to a non-loopback URL, for a daemon
    /// on a network you trust. Anyone on that network can read the token and
    /// session.
    #[arg(long)]
    pub insecure: bool,
}

#[derive(Args)]
pub struct RemoteRemoveArgs {
    pub name: String,
}

#[derive(Args)]
pub struct RemoteToggleArgs {
    pub name: String,

    /// Disable instead of enable
    #[arg(long)]
    pub off: bool,
}

pub async fn run(command: RemoteCommands) -> Result<()> {
    match command {
        RemoteCommands::Add(args) => add(args).await,
        RemoteCommands::List => list(),
        RemoteCommands::Remove(args) => remove(args),
        RemoteCommands::Toggle(args) => toggle(args),
    }
}

async fn add(args: RemoteAddArgs) -> Result<()> {
    if args.name.trim().is_empty() {
        bail!("remote name must not be empty");
    }
    let (url, token) = token_from_url(&args.url, args.token.clone())?;
    let url = url.trim_end_matches('/').to_string();
    let args = RemoteAddArgs { token, ..args };
    // The same URL and transport rules every later poll applies, so an entry
    // that could never be used is refused now rather than stored.
    let plaintext_refused = |url: &str| {
        format!(
            "cannot use {url:?} as a remote; credentials need HTTPS, or pass --insecure \
             for a daemon on a trusted LAN"
        )
    };
    match DaemonClient::with_login(&url, args.token.as_deref(), None, args.insecure) {
        Err(DaemonClientError::InsecureBearerTransport) => bail!(plaintext_refused(&url)),
        other => other.with_context(|| format!("cannot use {url:?} as a remote"))?,
    };

    let mut entry = Remote {
        name: args.name.clone(),
        url: url.clone(),
        enabled: true,
        token: args.token.clone(),
        session: None,
        binding: None,
        insecure: args.insecure,
    };

    let mut no_login_wall = false;
    if let Some(passphrase) = args.passphrase.as_deref() {
        let binding = login::new_binding_secret().context("generate device binding secret")?;
        match login::login(
            &url,
            args.token.as_deref(),
            passphrase,
            &binding,
            args.insecure,
        )
        .await
        {
            Ok(credentials) => {
                entry.session = Some(credentials.session);
                entry.binding = Some(credentials.binding);
            }
            // Either a token-only daemon or a wrong URL; the session read
            // below tells them apart.
            Err(LoginError::NotEnabled) => no_login_wall = true,
            Err(LoginError::InsecureTransport) => bail!(plaintext_refused(&url)),
            Err(e) => return Err(e).context("passphrase login failed"),
        }
    }

    verify(&entry).await?;
    if no_login_wall {
        println!("note: this daemon has no passphrase login; saved with the token only");
    }

    let mut registry = remotes::load()?;
    let replaced = registry.upsert(entry);
    remotes::save(&registry)?;

    println!(
        "{} remote {:?} -> {}",
        if replaced { "Updated" } else { "Added" },
        args.name,
        url
    );
    if args.passphrase.is_none() {
        println!(
            "Its sessions now list in `aoe`. Re-add with --passphrase if it has a login wall."
        );
    }
    Ok(())
}

/// Move a `?token=` query (as `aoe serve --status` and the TUI print it) into
/// the token, so the stored URL stays a base URL. A different `--token` is
/// refused rather than silently preferred.
fn token_from_url(raw: &str, token: Option<String>) -> Result<(String, Option<String>)> {
    let Ok(mut url) = reqwest::Url::parse(raw) else {
        return Ok((raw.to_string(), token));
    };
    let (found, rest): (Vec<_>, Vec<_>) = url
        .query_pairs()
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .partition(|(key, _)| key == "token");
    let Some((_, found)) = found.into_iter().next_back() else {
        return Ok((raw.to_string(), token));
    };
    if token.as_ref().is_some_and(|token| *token != found) {
        bail!("the URL carries a different token than --token; pass only one");
    }
    if rest.is_empty() {
        url.set_query(None);
    } else {
        url.query_pairs_mut().clear().extend_pairs(rest);
    }
    Ok((url.to_string(), Some(found)))
}

/// Read the session list with the entry's credentials, so a wrong base path,
/// a missing login or an unreachable host fails the add instead of every poll.
async fn verify(entry: &Remote) -> Result<()> {
    let login = entry
        .session
        .clone()
        .zip(entry.binding.clone())
        .map(|(session, binding)| SessionCredential { session, binding });
    let client = DaemonClient::with_login(
        &entry.url,
        entry.token.as_deref(),
        login.as_ref(),
        entry.insecure,
    )?;
    match client.list_sessions(None).await {
        Ok(_) => Ok(()),
        Err(DaemonClientError::Status { status, .. }) if status == StatusCode::NOT_FOUND => bail!(
            "no aoe daemon API at {} (HTTP 404); check the URL and any base path",
            entry.url
        ),
        Err(DaemonClientError::Status { status, .. })
            if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) =>
        {
            bail!(
                "the daemon at {} refused these credentials (HTTP {}); check --token, and pass \
                 --passphrase if it has a login wall",
                entry.url,
                status.as_u16()
            )
        }
        Err(DaemonClientError::Status { status, .. }) => {
            bail!("the daemon at {} returned HTTP {status}", entry.url)
        }
        Err(e) => Err(e).with_context(|| format!("could not reach a daemon at {}", entry.url)),
    }
}

fn list() -> Result<()> {
    let registry = remotes::load()?;
    if registry.remotes().is_empty() {
        println!("No remotes configured. Add one with `aoe remote add <name> <url>`.");
        return Ok(());
    }
    // Credentials are never printed, only whether they are present.
    println!("{:<16} {:<44} {:<8} AUTH", "NAME", "URL", "STATE");
    for remote in registry.remotes() {
        let auth = match (remote.token.is_some(), remote.has_login()) {
            (true, true) => "token+login",
            (true, false) => "token",
            (false, true) => "login",
            (false, false) => "none",
        };
        println!(
            "{:<16} {:<44} {:<8} {}{}",
            remote.name,
            remote.url,
            if remote.enabled { "enabled" } else { "off" },
            auth,
            if remote.insecure { " (insecure)" } else { "" }
        );
    }
    Ok(())
}

fn remove(args: RemoteRemoveArgs) -> Result<()> {
    let mut registry = remotes::load()?;
    if !registry.remove(&args.name) {
        bail!("no remote named {:?}", args.name);
    }
    remotes::save(&registry)?;
    println!("Removed remote {:?}", args.name);
    Ok(())
}

fn toggle(args: RemoteToggleArgs) -> Result<()> {
    let mut registry = remotes::load()?;
    let Some(existing) = registry.get(&args.name).cloned() else {
        bail!("no remote named {:?}", args.name);
    };
    let mut updated = existing;
    updated.enabled = !args.off;
    let enabled = updated.enabled;
    registry.upsert(updated);
    remotes::save(&registry)?;
    println!(
        "Remote {:?} is now {}",
        args.name,
        if enabled { "enabled" } else { "disabled" }
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_from_url_moves_the_query_token_into_the_token() {
        let token = |t: &str| Some(t.to_string());
        for (raw, flag, expected) in [
            (
                "http://10.0.0.2:8081/?token=abc",
                None,
                ("http://10.0.0.2:8081/", token("abc")),
            ),
            (
                "http://10.0.0.2:8081/?token=abc",
                token("abc"),
                ("http://10.0.0.2:8081/", token("abc")),
            ),
            (
                "https://box.ts.net/?a=1&token=abc",
                None,
                ("https://box.ts.net/?a=1", token("abc")),
            ),
            (
                "https://box.ts.net",
                token("xyz"),
                ("https://box.ts.net", token("xyz")),
            ),
            ("not a url", None, ("not a url", None)),
        ] {
            let (url, found) = token_from_url(raw, flag).unwrap();
            assert_eq!((url.as_str(), found), (expected.0, expected.1), "{raw}");
        }
        assert!(token_from_url("http://10.0.0.2:8081/?token=abc", token("other")).is_err());
    }
}
