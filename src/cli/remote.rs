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

    /// Base URL, e.g. `https://box.tailnet.ts.net`
    pub url: String,

    /// Bearer token the daemon prints at startup
    #[arg(long)]
    pub token: Option<String>,

    /// Passphrase for a daemon started with `--remote`. Exchanged once for a
    /// device-bound session; never stored.
    #[arg(long, env = "AOE_REMOTE_PASSPHRASE")]
    pub passphrase: Option<String>,
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
    let url = args.url.trim_end_matches('/').to_string();
    // The same URL and transport rules every later poll applies, so an entry
    // that could never be used is refused now rather than stored.
    DaemonClient::new(&url, args.token.as_deref())
        .with_context(|| format!("cannot use {url:?} as a remote"))?;

    let mut entry = Remote {
        name: args.name.clone(),
        url: url.clone(),
        enabled: true,
        token: args.token.clone(),
        session: None,
        binding: None,
    };

    let mut no_login_wall = false;
    if let Some(passphrase) = args.passphrase.as_deref() {
        let binding = login::new_binding_secret().context("generate device binding secret")?;
        match login::login(&url, args.token.as_deref(), passphrase, &binding).await {
            Ok(credentials) => {
                entry.session = Some(credentials.session);
                entry.binding = Some(credentials.binding);
            }
            // Either a token-only daemon or a wrong URL; the session read
            // below tells them apart.
            Err(LoginError::NotEnabled) => no_login_wall = true,
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

/// Read the session list with the entry's credentials, so a wrong base path,
/// a missing login or an unreachable host fails the add instead of every poll.
async fn verify(entry: &Remote) -> Result<()> {
    let login = entry
        .session
        .clone()
        .zip(entry.binding.clone())
        .map(|(session, binding)| SessionCredential { session, binding });
    let client = DaemonClient::with_login(&entry.url, entry.token.as_deref(), login.as_ref())?;
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
            "{:<16} {:<44} {:<8} {}",
            remote.name,
            remote.url,
            if remote.enabled { "enabled" } else { "off" },
            auth
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
