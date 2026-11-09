# Iliada

An immutable, micro-isolated Linux OS with a Mac-like app model.

## Vision

Iliada is a desktop OS for people who never want to think about their OS.
It should feel like a Mac: apps are single bundles you drag in and out, the
system never half-breaks after an update, and there is no command line or
administrator password in everyday use. Underneath, it treats every
third-party app as potentially hostile. Each app runs isolated, and it can
touch your camera, microphone, files or location only after you say yes.

Principles:

1. **The system is read-only and verifiable.** Updates replace it atomically
   and can always be rolled back.
2. **Apps are self-contained and isolated.** An app can't see other apps, your
   files, or your devices unless you grant it.
3. **The user decides, and only the user.** Neither apps nor AI can grant
   permissions on the user's behalf.
4. **AI helps, but never acts as an authority.** AI suggestions are verified
   mechanically or confirmed by the user before they take effect.
5. **Everything is declarative.** The whole system is defined in Nix.
6. **Rust wherever it is ready.** Everything Iliada writes is Rust, and so is
   every system component with a working Rust implementation:

   | Rust 🦀 | Still C (no production Rust replacement yet) |
   |---|---|
   | coreutils, diff, sed, ps (uutils) · sudo-rs · ntpd-rs · fish · COSMIC desktop, compositor, terminal, files · cloud-hypervisor · virtiofsd · all Iliada daemons | Linux kernel · glibc · systemd · Mesa · PipeWire · NetworkManager · bash (for scripts) |

* **Immutable base.** The system is composed with Nix and shipped as a signed,
  read-only image (dm-verity `/nix/store`, UKI, tmpfs root). Updates are atomic
  A/B swaps via `systemd-sysupdate` with automatic fallback.
* **`/Applications`.** Every app is a self-contained `.app` bundle on its own
  volume. Installing an app never touches the system image, so it needs no
  administrator password.
* **Isolation by default.** Apps run in a namespace sandbox (Tier 1) or their
  own cloud-hypervisor micro-VM (Tier 2). Their only way to reach the camera,
  microphone, files, location or clipboard is the **permission broker**, which
  asks the user, Android-style.
* **Package Translator.** `iliada-ingest` turns upstream packages (`.deb`
  today) into `.app` bundles. It resolves dependencies, records provenance,
  and has an optional **AI library resolver** whose suggestions are always
  verified mechanically.
* **Desktop.** COSMIC (Rust, Wayland), extended with Iliada's own shell
  pieces: an Applications browser, trusted permission prompts, and Privacy
  settings.

Status: **Phase 1** (architecture + proofs of concept). See
[docs/architecture.md](docs/architecture.md).

## Install

Iliada 0.1 is a developer preview: build the installer ISO, boot it, and run
`sudo iliada-install --disk /dev/<disk> --user <name>`. Afterwards the whole
system is configured in `/etc/iliada` (`configuration.nix` + `apps.toml`).
See **[docs/install.md](docs/install.md)**.

## Repository layout

```
crates/
  iliada-types     AppId, Capability: shared vocabulary
  iliada-bundle    .app format: Info / Permissions / Provenance manifests
  iliada-ingest    ingestion pipeline + daemon (bin: iliada-ingest)
  iliada-ipc       guest↔host wire protocol (postcard over vsock / unix)
  iliada-broker    permission broker daemon (bin: iliada-broker)
  iliada-guest     client library used inside sandboxes (+ examples)
  iliada-sandbox   Tier-1 bubblewrap / Tier-2 cloud-hypervisor controller (bin: iliada-run)
  iliada-appd      exports GUI bundles to the desktop; records installs in apps.toml
  iliada-applist   locked, comment-preserving edits of apps.toml
  iliada-catalog   app catalog format, signatures, and the catalog builder
  iliada-store     the Store engine + CLI (bin: iliada-store)
  iliada-cli       the `iliada` command
apps/
  iliada-store-gui the Store (COSMIC GUI)
nix/
  modules/         the declarative system manifest (iliada.* NixOS options)
  hosts/           dev-vm, appliance (immutable image) and installer ISO
  installer/       iliada-install
templates/default/ the user configuration (nix flake init -t …)
tests/installer/   end-to-end install test (QEMU/KVM)
  package.nix      Nix package for the Rust workspace
docs/              architecture, IPC spec, ADRs
examples/          example bundle, watchlist, isolation config
xtask/             cargo xtask automation
```

## Quick start

```sh
# Rust (macOS or Linux)
cargo test --workspace
cargo xtask ci                       # fmt + clippy -D warnings + tests

# On an installed Iliada
iliada list                     # apps in the Store
iliada install firefox          # install an app (.app, sandboxed)
iliada tool add htop            # a command-line tool (system setting)
iliada service enable ssh       # a service

# Ingest a real Debian package into a bundle
cargo run -p iliada-ingest -- run debian:jq --out out
out/jq.app/Contents/Executables/jq --version     # on Linux

# Permission broker demo (two terminals)
cargo xtask demo-broker
cargo run -p iliada-guest --example request-mic -- <socket printed above>

# Linux-only tests (Landlock) from macOS, via Docker or Lima
cargo xtask linux-test

# Nix (Linux)
nix flake check
nix build .#iliada-tools
nix run .#nixosConfigurations.dev-vm-x86_64.config.system.build.vm
nix build .#nixosConfigurations.appliance-x86_64.config.system.build.image
```

Optional AI library resolver (Anthropic API): `cargo run -p iliada-ingest
--features ai -- run <src> --resolver ai` with `ANTHROPIC_API_KEY` set. The key
belongs to whoever runs package translation (a build service or a developer).
End-user machines receive finished bundles and need no key.

## Roadmap

See [docs/architecture.md §9](docs/architecture.md#9-roadmap). Next up: the
first app booted in its own micro-VM, the COSMIC desktop VM, and the trusted
permission prompt. Further out: an **on-device assistant** that helps users
with their system, under strict rules about what it may do.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option. Unless you explicitly state
otherwise, any contribution intentionally submitted for inclusion in this
project shall be dual licensed as above, without any additional terms.
