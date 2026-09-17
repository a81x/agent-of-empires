# Remote Machines

The TUI can list the sessions of `aoe serve` daemons on other machines beside
your local ones, preview and type into their terminal sessions, open their
structured sessions, and create new sessions on them.

## Registering a remote

```sh
aoe remote add mini https://mini.tailnet.ts.net --token <token>
AOE_REMOTE_PASSPHRASE=… aoe remote add mini https://mini.tailnet.ts.net --token <token>
```

The token is the one the remote daemon prints at startup. A daemon started
with `--remote` also has a passphrase wall: `--passphrase` (or
`AOE_REMOTE_PASSPHRASE`, which keeps it out of `ps`) is exchanged once for a
device-bound login session. The passphrase itself is never stored; when the
session expires, add the remote again.

`aoe remote add` refuses a URL it could never use: it must be `http` or
`https` with a host and no credentials, query or fragment, and a token or
login travels only over HTTPS or a loopback `http://` URL unless you pass
`--insecure` (see below). It then reads the
remote's session list with those credentials and saves the entry only if that
works, so a wrong base path (HTTP 404) or a rejected token (HTTP 401 or 403)
fails the add instead of every later refresh.

`aoe remote list`, `aoe remote toggle <name> [--off]` and
`aoe remote remove <name>` manage the set. Entries live in `remotes.toml` in
the app directory, which must stay owner-only (`0600`). If that file cannot
be read, for example because its permissions were loosened, the home list
shows a `remotes.toml` row carrying the error instead of silently dropping
every remote.

## Plain HTTP on a trusted LAN

Without Tailscale Funnel or a TLS proxy, a daemon bound to a LAN address can
still be registered over plain HTTP. On the remote machine (debug builds
listen on port 8081, release builds on 8080):

```sh
./target/debug/aoe serve --daemon --host 0.0.0.0 --passphrase <passphrase>
./target/debug/aoe serve --status   # the URL line carries ?token=<token>
```

On your machine:

```sh
aoe remote add box1 http://192.168.1.20:8081 --token <token> --passphrase <passphrase> --insecure
```

`--insecure` is stored on that entry only, and `aoe remote list` marks it.
The token, passphrase and login session travel unencrypted, so anyone who can
see traffic on that network can read them and run commands as the remote
user. Use it only on a network you trust.

## Remote sessions in the home view

By default the list groups by machine: a `local` section, then one section per
enabled remote, each collapsible. Press `g` to pick another grouping; remote
sections then sit above the Archived and Trash shelf, which also gathers each
remote's archived and trashed rows. A remote that cannot be reached keeps its
header with the reason.

* **Terminal sessions** show their live output and info panel in the preview
  pane without resizing the remote pane. `Enter` or `Tab` starts live-send
  (see [Live Mode](live-mode.md)), which takes the pane's size until the exit
  chord. Keys typed before the daemon grants control are held and sent in
  order once it does; if it refuses, another viewer takes over, or the
  connection drops, live-send ends with a status message.
* **Structured sessions** open full screen against the remote daemon on
  `Enter`; `Ctrl+Q` returns to the home view.
* **Archived or trashed rows** have to be restored on their own machine.

## Creating a session on a remote

The new-session dialog lists each remote's profiles after the local ones as
`profile@remote`. Choosing one switches the dialog to that machine: its
installed agents, a starting path at its home directory, `Ctrl+P` browsing
its filesystem, and the sandbox option only when the remote reports a running
container runtime. The session is created by the remote daemon.

The dialog never approves repository hooks on another machine. If the repo's
hooks are not yet trusted there, the create is refused (HTTP 403); trust them
on that machine first, for example with `aoe add --trust-hooks`.

## A temporary remote from `AOE_DAEMON_URL`

```sh
AOE_DAEMON_URL=https://aoe.example.com AOE_DAEMON_TOKEN=… aoe
aoe --daemon-url https://aoe.example.com
```

The home view lists that daemon as one more remote, named after its host and
port, without saving it. A registered remote with the same URL is listed once,
under its registered name; a registered remote that merely shares the host
name keeps it, and the temporary one gets an ` (env)` suffix. The TUI still
starts as it does locally (it needs tmux and a valid profile), and local
sessions keep using the local daemon. `aoe acp` verbs and
`aoe serve --status` target the `AOE_DAEMON_URL` daemon instead.
