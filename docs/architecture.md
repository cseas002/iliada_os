# Iliada architecture (Phase 1)

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
| Mechanism | bubblewrap: all namespaces, `--cap-drop ALL`, ro binds | cloud-hypervisor (KVM), virtio-only devices |
| FS view | bundle `Root` at `/`, runtime base, private data dir | same, via virtio-fs (app share read-only) |
| Channel out | broker Unix socket bind-mounted | vsock `CID 2:7000` → broker socket |
| Default for | first-party / reviewed apps | everything ingested from upstream |

*Why not Firecracker:* no virtio-gpu, no virtio-fs (only block devices), and no
display path. Desktop apps need all three. cloud-hypervisor is Rust,
KVM-based, supports virtio-fs, vsock and vhost-user, and is what Spectrum OS
(the closest prior art: NixOS with per-app VMs) uses. Display forwarding
(virtio-gpu cross-domain / Wayland) is Phase 2.

**Runtime permissions** (`iliada-broker`, `iliada-ipc`, ADR 0003):

1. Each app declares an *upper bound* in `Permissions.toml`, with a purpose
   string per capability. Undeclared means denied without a prompt.
2. A request goes to the broker over the sandbox's only channel. The app's
   identity is the socket the request arrived on, assigned by the host, never
   a claim made by the guest.
3. The broker checks policy, then stored ("Always") grants, then session
   ("While running") grants. Only then does it prompt. Grants: Once / While
   running / Always. Every decision is appended to `audit.jsonl`.
4. A grant yields a **broker-mediated handle**, never a device node or host
   path. In Phase 1, user-picked files are streamed through the broker (the
   app learns only the file name). Mic, camera and screen grants return a
   `Media` resource whose PipeWire stream bridging is Phase 2.
5. Prompts must be drawn by the compositor, outside any app's surface, so an
   app cannot spoof or overlay them. Phase 1 ships a terminal frontend and a
   deny-all frontend; app-supplied text is sanitised and labelled as coming
   from the app.

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

## 8. Phase 1 status

| Area | State |
|---|---|
| Bundle format + validation | done, tested |
| Package Translator (.deb → .app), index, resolvers, provenance | done; tested offline and against Debian trixie (jq, gnome-calculator) |
| Landlock confinement of extraction | done; enforced on Linux |
| IPC protocol, broker, guest client | done; end-to-end tests over real sockets |
| Tier-1 sandbox (`iliada-run`) | done; GUI apps (GNOME Calculator) run sandboxed from the launcher |
| Tier-2 micro-VMs | config generation tested; booting needs the guest image (Phase 2) |
| Desktop (COSMIC), app registry (`iliada-appd`) | done; apps appear in search and launch sandboxed |
| Apps without rebuild (drag, drop a .deb, Trash, auto-recorded list) | implemented and unit-tested; VM validation pending |
| Installer ISO + `iliada-install`, config template | done; end-to-end install test passes (UEFI VM) |
| Rust userland (uutils, sudo-rs, ntpd-rs, fish) | done |
| Immutable appliance image | evaluates; build and boot not yet tested |
| No-terminal settings, Setup Assistant, GitHub sync | designed (ADR 0005); not implemented |

## 8b. The user layer: no terminal, configuration that follows you

Users never need a terminal (ADR 0005). The system layer (Nix) is changed by
the Settings app through `iliada-configd`, which applies it with a
background rebuild. The user layer (apps, preferences) changes instantly
and never needs a rebuild. Apps are installed by dragging `.app` bundles or
downloaded packages into Applications, and uninstalled by dragging them to
the Trash. `iliada-appd` records every change in `/etc/iliada/apps.toml`.

`/etc/iliada` is a git repository. With backup on, `iliada-syncd` pushes it
to a private `iliada_<name>` repository. On a new computer the Setup
Assistant restores it, so apps, preferences and settings come back. Only
commits signed by the profile's enrolled devices are applied.

## 9. Roadmap

**Phase 2: Mac-like, no terminal** (ADR 0005, in this order)

1. Apps without terminal or rebuild: mutable `apps.toml`, drag-to-install,
   drop-a-package-to-install, drag-to-Trash uninstall, automatic recording.
2. `iliada-configd` + `settings.toml`: GUI settings applied in the background.
3. Graphical installer and first-boot Setup Assistant.
4. `iliada-syncd`: GitHub backup/restore, device enrolment, signed changes.
5. "Get Apps" catalog; signed `.iliadapp` downloads.

**Phase 2b: isolation and the desktop shell**

1. COSMIC desktop VM, testable remotely (QEMU on the dev server, VNC over SSH).
2. Guest image (kernel + initrd + guest agent) and the first booted Tier-2 app.
3. Wayland/virtio-gpu display path from micro-VMs; PipeWire bridging for media grants.
4. Compositor-owned prompt frontend in cosmic-comp; broker as a per-user service.
5. Bundle signing, with grants tied to signing keys; `iliada-appd`
   (drag-to-install, MIME and `.desktop` registration).
6. OpenPGP verification of Debian `InRelease`; Flatpak and AppImage sources.
7. Recipe repository; seccomp profiles; GTK/Qt in the runtime base.

**Phase 3: polish and reach**

1. aarch64 builds (ARM laptops, UTM on Apple Silicon).
2. Installer, first-run setup, the update server, Secure Boot key management.
3. The on-device assistant (§10).

## 10. Future: the on-device assistant

An optional assistant built into the system, helping users with the OS
itself. For example: "why can't this app see my Downloads folder?", "set up
my printer", "make the text bigger", "what did this update change?". It
replaces the forum search and the terminal commands that Linux usually
demands.

It is designed under the same rules as everything else:

* **It cannot grant permissions.** Broker prompts are answered only by the
  person at the keyboard, through the compositor's trusted surface. The
  assistant may *explain* a prompt, never click it.
* **It is an app, not root.** It runs sandboxed like any other app, with an
  explicit, small set of system actions (open a Settings page, change a
  declared preference, start an update check). Each action is exposed as a
  typed tool and shown to the user before it runs.
* **Its sources are labelled.** Content from apps, web pages or documents is
  treated as data, not instructions, so prompt injection cannot become a
  system action.
* **Local first.** Prefer an on-device model. A cloud model is opt-in and
  shows the user what leaves the machine.
* **Everything is logged.** Assistant actions appear in the same audit log as
  broker decisions.
