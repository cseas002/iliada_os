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

## 6. Pillar 4: ingestion

`iliada-ingest` (ADR 0004):

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
5. **Advise.** Unresolved sonames go to advisors: a rule-based one (Debian
   naming policy plus known exceptions), then optionally **Claude**
   (`--features claude`). An advisor's answer is a hypothesis: the package is
   downloaded, hash-checked and accepted only if it really ships that soname.
   A wrong or malicious suggestion costs at most an extra download.
6. **Bundle.** The upstream tree goes into `Contents/Root`, the needed
   libraries into `Contents/Libraries`, plus a launcher (with RUNPATH
   rewriting when `patchelf` is present), the icon and `.desktop` entry, and
   the three manifests. Permission *declarations* are inferred conservatively
   from linked libraries (PulseAudio → may ask for the mic), and every use is
   still prompted. `Provenance.toml` records every package, URL, hash and
   soname resolution, including which advisor influenced what.

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
| AI advisor → bundle contents | mechanical verification (index hash + DT_SONAME) |
| App → host resources | sandbox (namespaces / KVM) + broker as the only channel |
| App → user | compositor-owned prompts; app text sanitised and labelled |
| Disk → running system | dm-verity root hash in a signed UKI |

## 8. Phase 1 status

| Area | State |
|---|---|
| Bundle format + validation | done, tested |
| Ingestion (.deb → .app), index, advisors, provenance | done; tested offline and against Debian trixie (jq, gnome-calculator) |
| Landlock confinement of extraction | done; enforced on Linux |
| IPC protocol, broker, guest client | done; end-to-end tests over real sockets |
| Sandbox controller | config generation tested; launching needs Linux/KVM (Phase 2 bring-up) |
| NixOS modules, dev VM, appliance image | evaluate cleanly; image boot testing is Phase 2 |

## 9. Phase 2 (next)

1. Guest image (kernel + initrd + guest agent) and the first booted Tier-2 app.
2. Wayland/virtio-gpu display path; PipeWire bridging for media grants.
3. Compositor-owned prompt frontend; broker as a per-user service.
4. Bundle signing, with grant keys tied to signing keys; `iliada-appd`.
5. OpenPGP verification of Debian `InRelease`; Flatpak and AppImage sources.
6. Recipe repository; seccomp profiles; GTK/Qt in the runtime base.
