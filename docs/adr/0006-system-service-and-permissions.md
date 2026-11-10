# ADR 0006: One system service; permissions asked on first launch

**Status:** accepted, implemented

## Context
ADR 0005 requires that users never need a terminal or `sudo`, yet changing
tools, services, updates and backups needs root. Apps also need a way to
get a folder, the camera or the microphone without seeing the rest of the
user's files.

## Decision

### 1. `iliada-systemd`: the only thing that changes the system
One root service (`crates/iliada-system`) on `/run/iliada-system/socket`:

* **Who:** the socket's peer credentials. Reading status is open to every
  local user; changes need an administrator (root or `wheel`). Nothing in a
  request can claim an identity.
* **Protocol:** one JSON request per connection, answered by progress events
  and exactly one `done` or `failed` (`protocol.rs`).
* **What:** command-line tools and services (`settings.toml`, applied with
  `nixos-rebuild`; the previous file is restored if the build fails), apply,
  app updates now, Iliada updates, GitHub backup and restore, and creating
  `/etc/iliada` (installer, first boot of the VM image).
* **One change at a time;** others are told it is busy.
* **Self-update:** a switch never restarts the service that is running it
  (`restartIfChanged = false`); it restarts itself once idle when its own
  binary changed.

Clients: Iliada Settings, the Setup Assistant, the `iliada` command.

### 2. Backups are `/etc/iliada`'s history
`/etc/iliada` is a git repository; every change is a commit. Backup pushes
it to a private `iliada_<name>` repository on the user's GitHub account,
after every change and hourly. Sign-in is GitHub's device flow (the user
approves a code in a browser); the token is root-only and reaches git
through the environment, never a command line. Restore takes only what is
about the person (`settings.toml`, `apps.toml`), keeps the machine's own
hardware configuration, and continues the backup's history so later pushes
fast-forward. Two computers on one backup: the pusher's files win, history
is merged.

### 3. Iliada updates are signed like the catalog
`release.json` names the release's source archive with its size and
SHA-256, signed with the catalog key. An update swaps `/etc/iliada/source`,
relocks and rebuilds; a failed build puts everything back. Every update is
a boot-menu entry.

### 4. Permissions: declared, asked once, bound in
Apps declare what they may ask for (recipes, or inferred from libraries).
`iliada-run` asks before the sandbox exists, in a window Iliada draws
(`iliada-prompt`), remembers the answer outside every sandbox, and binds
only what was allowed: the folder, the camera devices, the sound server.
Files opened with the app are bound individually. Settings → Privacy
changes the answers.

## Consequences
* No `sudo` anywhere in normal use; `wheel` is what makes an administrator.
* Asking at launch is coarser than macOS's ask-on-first-use: an app asks for
  its declared folders the first time it starts, whether or not it needs
  them yet. Recipes therefore declare only what the app obviously needs.
* Sound and microphone share one socket (see architecture §5).
