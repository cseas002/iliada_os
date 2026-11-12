# Iliada architecture

This document is the reviewed version of the kickoff draft. Where it departs
from the draft, the reason is stated; the ADRs in `docs/adr/` hold the details.

## 1. Goals and non-goals

**Goals.** A consumer OS that never breaks (immutable, atomic, roll-backable),
presents apps as self-contained bundles, and assumes every third-party app is
hostile: no ambient access to devices, files or other apps.

**Non-goals (for now).** Binary compatibility with arbitrary Linux desktop
integration (system tray hacks, global keyboard grabs, X11 snooping); running
untrusted kernel modules; supporting mutable system administration.

## 2. Layers

```
┌──────────────────────────── user session ─────────────────────────────┐
│  shell / compositor  ── trusted prompt surface (Phase 2) ──┐          │
│                                                           │          │
│  ┌── Tier 2: micro-VM ──┐  ┌── Tier 1: namespaces ──┐     │          │
│  │ app  + guest agent   │  │ app                    │     │          │
│  │ virtio-fs (ro app,   │  │ ro binds, 1 rw dir,    │     │          │
│  │  rw data), vsock     │  │ broker socket          │     │          │
│  └─────────┬────────────┘  └──────────┬─────────────┘     │          │
│            │ vsock :7000              │ unix socket        │          │
│            ▼                          ▼                    │          │
│       iliada-broker  (decides, mediates, audits) ◀────────┘          │
└───────────────────────────────────────────────────────────────────────┘
┌──────────────────────────── host services ────────────────────────────┐
│ iliada-sandbox (launch)   iliada-ingest (build bundles)   sysupdate    │
├────────────────────────────────────────────────────────────────────────┤
│ /Applications (rw volume)    /home, /var/lib (rw)    / = tmpfs         │
├────────────────────────────────────────────────────────────────────────┤
│ /usr + /nix/store: erofs, dm-verity, root hash in signed UKI (A/B)     │
└────────────────────────────────────────────────────────────────────────┘
```

## 3. Pillar 1: foundation

**Decision: NixOS is the build engine; delivery is image-based** (ADR 0001).

Plain NixOS generations give rollback, but the store is writable by
`nix-daemon`, `/etc` is assembled by activation scripts, and nothing proves the
running system is the one that was built. Iliada therefore uses Nix only at
build time:

* `nix/modules/image.nix` builds the closure into an erofs `/usr` partition
  plus a dm-verity hash tree (`image.repart.verityStore`). The root hash is
  embedded in the kernel command line of a UKI; Secure Boot signs the UKI.
  `/nix/store` is a bind of the verified `/usr/nix/store`.
* `/` is a tmpfs. Persistent state exists only on declared volumes:
  `/Applications`, `/home`, `/var/lib`.
* **A/B** is the pair of store partitions. `systemd-sysupdate` writes the
  inactive one plus a new UKI; systemd-boot's boot counting (`TriesLeft`)
  falls back to the previous UKI (which carries the previous root hash) if the
  new one fails to come up. This *is* the "graphical rollback": choosing an
  older UKI.

The draft listed Nix rollbacks and A/B partitions as separate mechanisms; in
this design they are the same mechanism.

## 4. Pillar 2: UI and file model

* **`/Applications`** is a separate writable volume (`iliada.applications`).
  A bundle is a directory (`Name.app/Contents/...`, see `iliada-bundle`), so
  drag-to-install is a copy and drag-to-trash is a delete. Phase 2 adds
  `iliada-appd`, which validates a dropped bundle, registers its `.desktop`
  entry and MIME types, and refuses unsigned bundles in strict mode.
* **Desktop: COSMIC** (`iliada.desktop`). System76's COSMIC is a
  Wayland desktop written in Rust. Its compositor (`cosmic-comp`) is built on
  the Smithay library. We build on it rather than writing a compositor,
  because Iliada needs compositor-level features that GNOME and KDE make
  hard to add:
  - permission prompts drawn by the compositor, which no app can cover or imitate;
  - windows from micro-VMs labelled with their app identity (cf. Qubes borders);
  - per-app clipboard and screenshot mediation through the broker.

  Iliada's own pieces sit on top: an Applications browser, Privacy settings
  (view and revoke grants), and the update UI. COSMIC's default top panel plus
  dock is already close to the Mac layout.
* **FHS hiding** (`iliada.ux.hideFhs`) uses the freedesktop `/.hidden`
  convention so file managers show only the user's world. Developers can still
  see everything.
* **"Zero sudo"** is reframed (ADR 0001 §consequences): there is no admin
  password because the everyday actions that used to need one no longer touch
  privileged state. Installing an app writes to `/Applications` (group
  `iliada-apps`). Updates are image swaps authorised by polkit for the active
  local user. Power actions are allowed the same way. What remains privileged
  (repartitioning, changing the image key) is *meant* to require
  authentication. `iliada.ux.noSudo` removes sudo entirely on the appliance.

## 5. Pillar 3: isolation and permissions

**Two tiers, one interface** (`iliada-sandbox`, ADR 0002):

| | Tier 1: namespace | Tier 2: micro-VM |
|---|---|---|
| Mechanism | bubblewrap: all namespaces, `--cap-drop ALL`, ro binds | cloud-hypervisor (KVM), virtio-only devices; *inside*, the same bubblewrap sandbox |
| FS view | bundle `Root` at `/`, runtime base, private data dir | same: virtio-fs shares (host `/nix/store` and the app read-only) mounted at their host paths |
| Display | the host Wayland socket | waypipe over vsock (guest port 7001), software rendering |
| Channel out | broker Unix socket bind-mounted | vsock `CID 2:7000` → broker socket |
| Network | when declared: the host's network namespace | when declared: virtio-net served by passt (user-mode, runs as the user; nothing forwarded in; the host's own local services unreachable) |
| Sound | when allowed: the sound server's sockets | when allowed: the guest relays the PulseAudio socket over vsock (port 7002); `iliada-run` hands it to the user's sound server |
| Used for | **every app, by default** (native speed) | opt-in: "Extra isolation" per app (Settings → Privacy), or "High security mode" for all |

A micro-VM boots the host's kernel with a small initrd whose `/init` is
`iliada-guest-init` (Rust). It mounts the shares listed in `spec.json`,
brings up the loopback (and the network card, with fixed private
addresses, for apps that have a network), starts the sound relay if the
app may play sound, then starts waypipe, and waypipe starts the app's
bubblewrap sandbox; when the app exits, the guest powers off. The host side
(`iliada-run`) starts one unprivileged virtiofsd per share, a waypipe
client, passt for the network, and cloud-hypervisor.

*Why not Firecracker:* no virtio-gpu, no virtio-fs (only block devices), and no
display path. Desktop apps need all three. cloud-hypervisor is Rust,
KVM-based, supports virtio-fs, vsock and vhost-user, and is what Spectrum OS
(the closest prior art: NixOS with per-app VMs) uses. GPU acceleration
(virtio-gpu cross-domain) is later work; waypipe already carries windows.

**Permissions** (ADR 0006):

1. Each app declares an *upper bound* in `Permissions.toml`, with a purpose
   string per capability: in its recipe (`folders = ["pictures"]`), or
   inferred from the libraries it links (audio libraries: sound, microphone).
   Undeclared means never asked, never granted.
2. The first time the app starts, `iliada-run` (on the host, before the
   sandbox exists) asks about each declared folder, the camera and the
   microphone in `iliada-prompt`, a window Iliada draws; the app cannot draw
   or answer it. Sound is granted without asking, as on macOS.
3. Answers are stored per user in `~/.local/share/iliada/permissions.toml`,
   outside every sandbox, and can be changed in Settings → Privacy.
4. Only what was allowed is put into the sandbox: the folder at
   `~/<Folder>` in the app's home, the camera devices, the sound server's
   sockets. Files opened with an app (double-click in Files) are bound one by
   one at their own paths: the app sees exactly those files.
5. Known limit: the sound server's socket carries playback and recording,
   so an app that may play sound could also record. Separating them needs
   PipeWire's portal-based access control (planned).

The broker (`iliada-broker`, `iliada-ipc`, ADR 0003) serves apps that ask at
run time over their socket or vsock (Iliada-aware apps, micro-VMs): identity
is the socket the request arrived on, grants are Once / While running /
Always, and every decision is appended to `audit.jsonl`.

**Network** is not a runtime permission. It is decided when the sandbox is
created (`NetworkPolicy::None | Nat`), because "ask for network" prompts
train users to click Allow.

**Verified boot** comes from Pillar 1: dm-verity on the store plus a signed UKI.

## 6. Pillar 4: the Package Translator

`iliada-ingest`, the **Package Translator** (ADR 0004):

1. **Fetch** over HTTPS with a size cap and SHA-256. A `debian:<pkg>` source is
   resolved via the suite's `Packages` index and checked against the hash
   listed there.
2. **Extract** the `.deb` in memory with pure-Rust decoders (ar, tar, xz,
   zstd, gzip). Maintainer scripts never run. Every tar entry is path-checked:
   absolute and `..` paths are rejected, writes through symlinks are refused,
   symlink targets are checked lexically, hardlinks become copies, setuid bits
   are stripped, and sizes are capped. On Linux the worker thread is also
   confined with **Landlock** (write access only to its staging dir).
3. **Analyse** ELF objects (goblin): `DT_NEEDED`, `DT_SONAME`, interpreter.
4. **Resolve.** The needed sonames are split into: provided by the package;
   provided by the **runtime base** (`iliada-base-1`: glibc, GCC runtime,
   graphics, audio, fonts); bundled from dependency packages found by walking
   `Depends` (only `lib*` packages, then a second chance for oddly named ones
   such as `zlib1g`); or unresolved.
5. **Suggest.** Unresolved sonames go to library resolvers: naming rules
   (Debian policy plus known exceptions), then optionally the **AI library
   resolver** (`--features ai`, `--resolver ai`; it calls a Claude model via
   the Anthropic API and needs `ANTHROPIC_API_KEY`). A suggestion is a
   hypothesis: the package is
   downloaded, hash-checked and accepted only if it really ships that soname.
   A wrong or malicious suggestion costs at most an extra download.
6. **Bundle.** The upstream tree goes into `Contents/Root`, the needed
   libraries into `Contents/Libraries`, plus a launcher (with RUNPATH
   rewriting when `patchelf` is present), the icon and `.desktop` entry, and
   the three manifests. Permission *declarations* are inferred conservatively
   from linked libraries (PulseAudio → may ask for the mic), and every use is
   still prompted. `Provenance.toml` records every package, URL, hash and
   soname resolution, including which resolver (and which model) influenced
   what.

The API key belongs to whoever runs translation: an Iliada build service
that publishes bundles, or a developer translating locally. End-user
machines receive finished bundles and never need a key.

*Why no static linking:* a prebuilt dynamically linked ELF cannot be relinked
statically after the fact. Bundling plus RUNPATH is what AppImage, Flatpak
extensions and Nix's `autoPatchelfHook` do.

Watch mode (`iliada-ingest serve`) re-ingests when an upstream URL's
ETag/Last-Modified changes or a Debian package's version changes.

**Crowdsourced recipes (AUR-style)** are Phase 2: a recipe is a reviewed TOML
overlay on this pipeline (source, exec override, extra permissions with
justification, runtime base), validated by the same Rust types.

## 7. Trust boundaries (summary)

| Boundary | Enforced by |
|---|---|
| Upstream package → host during ingestion | pure-Rust parsers, path checks, Landlock, size caps |
| AI library resolver → bundle contents | mechanical verification (index hash + DT_SONAME) |
| On-device assistant → system | broker prompts it cannot answer; its own sandbox and capability list (§10) |
| App → host resources | sandbox (namespaces / KVM) + broker as the only channel |
| App → user | compositor-owned prompts; app text sanitised and labelled |
| Disk → running system | dm-verity root hash in a signed UKI |

## 8. Status

| Area | State |
|---|---|
| Bundle format + validation | done, tested |
| Package Translator (.deb → .app), index, resolvers, provenance | done; 15 apps published for x86-64, 14 for ARM (LibreOffice Writer x86-64 only for now) |
| Landlock confinement of extraction | done; enforced on Linux |
| IPC protocol, broker, guest client | done; end-to-end tests over real sockets |
| Tier-1 sandbox (`iliada-run`) | done; every published app runs sandboxed from the launcher |
| Apps for each processor (x86-64, ARM) | done; ARM builds launch-tested in the ARM image under emulation (14 of 15 open); `iliada-run` explains, in a window, when an app is for another processor (validated in the x86 VM) |
| Permissions: first-launch prompts, Settings → Privacy, opened files | built; unit-tested |
| Tier-2 micro-VMs (guest init, initrd, virtio-fs, waypipe, passt, sound relay) | works: Firefox in its own VM loads HTTPS pages (its own content sandbox runs too); Text Editor opens and saves a double-clicked file; Firefox's sound reaches the desktop's sound server; Calculator saves its settings |
| Desktop (COSMIC), app registry (`iliada-appd`) | done |
| Apps without rebuild (drag a .app in, Trash, auto-recorded list) | done; validated in the VM |
| Iliada Store (GUI + CLI), signed catalog, automatic updates | done; validated in the VM. `catalog-2.json` (an archive per processor) validated in the x86 VM against the published catalog; `catalog.json` unchanged for 0.1 |
| System service (`iliada-systemd`): tools, services, apply, no sudo | done; validated in the VM (GUI and CLI) |
| Iliada Settings (updates, backup, tools, services, privacy) | done; validated in the VM |
| Setup Assistant (first login) | done; shown on first login in the VM |
| Frosted glass (COSMIC theme defaults `frosted_windows`, `frosted_panel`, `frosted_applets`, `frosted_system_interface`) | done; a fresh dev VM disk comes up frosted at first login (2026-11-12); the Alt+Space panel, Settings and the Welcome window are blurred. COSMIC apps read their built-in fallback theme first, so a user service writes the four keys into each user's theme at login unless already set |
| GitHub backup and restore | built; needs a GitHub OAuth app to test |
| Assistant (Ask / Auto, Settings → Assistant, `iliada ask`, the Assistant app) | live-tested in the x86 VM with DeepSeek V4 Flash (OpenRouter): app install, dark/light, volume, commands, email (local mail server; approval and decline), a prompt-injected file not obeyed; Settings → Assistant and the app clicked through; answers stream in as they are written (2026-11-12, both providers covered by tests, DeepSeek live in the panel) |
| Store: Software and Services (not apps); `iliada install`/`remove`/`search` for all three | done: on an installed 0.2.0 VM, `iliada install python` gave Python 3.14, `iliada remove python` removed it, `iliada install ssh` and a typo checked (2026-11-12); the Store's search box finds Python from Discover (dev VM) |
| Text field right-click menu (libcosmic patch, `nix/patches/libcosmic-context-menu-position.patch`) | done: opens at the pointer on a scrolled Settings page (it opened at the bottom of the screen); patched in the Nix build, upstream unchanged as of 2026-11-12 |
| Iliada updates (signed release manifest) | validated privately: an installed test VM found a locally served, signed 0.2.0, installed it from Settings → Updates, reported 0.2.0 after the restart and then "up to date"; 0.2.0 is published: a fresh 0.2.0 install checks GitHub from Settings → Updates and reports "up to date". The service restarts itself after an apply that changes its own settings (verified on the installed 0.2.0: restarted 5 s after the apply, with the new update address) |
| Graphical installer, `iliada-install`, config template | built; the script's end-to-end install test passes (0.2.0 ISO, 2026-11-11: installed system boots, Setup Assistant, Settings, SSH from Settings, Store install of Audacity with its permission prompt, Alt+Space all checked by hand) |
| Ready-to-run VM image (UTM on Apple Silicon, QEMU) | x86 validated; ARM image boots (emulated here) and runs the ARM apps once the sandbox group fix is in |
| Rust userland (uutils, sudo-rs, ntpd-rs, fish) | done |
| Everyday command-line tools (`iliada.baseTools`: lspci, lsusb, dig, traceroute, ifconfig, lsof, killall, jq, vim, git, tmux, ripgrep, …) | done: every tool runs in the dev VM |
| Immutable appliance image | evaluates; build and boot not yet tested |

## 8b. The user layer: no terminal, configuration that follows you

Users never need a terminal (ADR 0005). Everything that changes the system
goes through one root service, `iliada-systemd` (ADR 0006): command-line
tools and services (`settings.toml`), applying the configuration, Iliada
updates and backups. Its clients are Iliada Settings, the Setup Assistant
and the `iliada` command; it checks that the caller is an administrator
(`wheel`) from the socket's credentials, so nothing needs `sudo`. The user
layer (apps, preferences) changes instantly and never needs a rebuild:
apps are installed by dragging a `.app` into Applications (or with the
Store) and removed by dragging it to the Trash; `iliada-appd` records every
change in `/etc/iliada/apps.toml`.

`/etc/iliada` is a git repository: every change is a commit. With backup on,
the service pushes it to a private `iliada_<name>` repository on the user's
GitHub account (device-flow sign-in: the user approves a code in the
browser; the token stays root-only). It pushes after every change and every
hour. Restoring (Setup Assistant or Settings) takes the backup's
`settings.toml` and `apps.toml`, keeps the new computer's hardware
configuration, applies it and reinstalls the apps; the computer's history
then continues the backup's.

## 9. Roadmap

**Next**

1. Test GitHub backup and restore end to end (needs the GitHub OAuth app).
2. Each release: set `VERSION` and the two Cargo workspace versions, commit, tag
   `v<version>`, `iliada-catalog release` (it checks all three),
   upload `release.json`, its `.sig` and the source archive to the GitHub release
   (0.2.0 published 2026-11-11). Systems fetch them anonymously from the public
   repository.
3. LibreOffice Writer on ARM (its ARM build stops at start-up with a UNO
   exception; the x86-64 build works).
4. Check frosted glass on real hardware (GPU drivers).

**Later**

1. Microphone separate from sound (PipeWire access control via portals).
2. GPU acceleration for micro-VMs (virtio-gpu cross-domain).
3. Bundle signing, with grants tied to signing keys.
4. OpenPGP verification of Debian `InRelease`; Flatpak and AppImage sources.
5. Seccomp profiles; a filtered D-Bus proxy for portals.
6. The immutable appliance image booted and updated (A/B, dm-verity).
7. Secure Boot key management.
8. The on-device assistant (§10).

## 10. The assistant

An optional assistant built into the system (`crates/iliada-assistant`, the
Assistant app, `iliada ask`). People ask in plain words ("install Firefox
and turn on the SSH server", "why can't GIMP see my Pictures?") and it does
it, instead of a forum search and terminal commands.

* **Configuration.** `[assistant]` in `settings.toml`: `enabled`, `mode`
  (`ask` or `auto`), `model` and `full_access`; changed in Settings →
  Assistant or with `iliada assistant`, without a rebuild, and backed up
  like the rest. Models: Claude (Anthropic key) or any OpenRouter model
  (`vendor/model`, e.g. `deepseek/deepseek-v4-flash`, OpenRouter key). The
  keys are the user's own, in `~/.config/iliada/anthropic-key` /
  `openrouter-key` (0600), never backed up. What it has cost is kept in
  `~/.local/share/iliada/assistant-spend.json` and shown in Settings.
* **Ask mode** (default): every action is shown ("Install Firefox") and runs
  only if the user allows it. **Auto mode**: it acts on its own, with any
  model (Settings notes that models other than Opus, Fable and Sonnet make
  more mistakes on their own).
* **A typed toolset.** Iliada's own operations: Store apps, software and
  services, updates, backup, app permissions; dark/light appearance,
  volume, opening links; **email** from the user's own account. System
  changes go through the system service, so the assistant can never do more
  than the user it works for (a non-administrator's assistant cannot
  install software).
* **Full access** (off by default): it may also run commands as the user
  (`run_command`: bash in their home, output cut, time limit), so it can do
  anything the user can without an administrator's password. Text that
  comes back from tools is information, never instructions.
* **Email.** `send_email` sends plain-text mail through the user's own SMTP
  account (Settings → Assistant → Email or `iliada assistant email`;
  `~/.config/iliada/email.toml` and `email-password`, 0600, never backed
  up; TLS or STARTTLS, plain only to a server on this computer). Every
  email is shown to the person before it is sent, **even in Auto mode**:
  it leaves the computer in their name, and a prompt-injected document must
  not be able to mail anything out on its own.
* **It cannot answer permission prompts.** Those are drawn by Iliada for the
  person at the keyboard. It can change a decision only through Settings'
  own operation, as the user could, and says so.
* **Logged.** Every action it takes or is refused is appended to
  `~/.local/share/iliada/assistant-log.jsonl` (time, model, mode, tool,
  input, outcome).
* **No invented results.** Its instructions require looking things up before
  acting and reporting only what a tool confirmed; tool failures are passed
  back as failures.

Later: an on-device model for the simplest requests, and tools for Settings
pages (display, network) once those have an API.
