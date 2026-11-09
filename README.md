# Iliada

An immutable, micro-isolated Linux OS with a Mac-like app model.

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
* **Ingestion.** `iliada-ingest` turns upstream packages (`.deb` today) into
  `.app` bundles: dependency resolution, provenance, and an optional AI advisor
  whose suggestions are always verified mechanically.

Status: **Phase 1** (architecture + proofs of concept). See
[docs/architecture.md](docs/architecture.md).

## Repository layout

```
crates/
  iliada-types     AppId, Capability: shared vocabulary
  iliada-bundle    .app format: Info / Permissions / Provenance manifests
  iliada-ingest    ingestion pipeline + daemon (bin: iliada-ingest)
  iliada-ipc       guest↔host wire protocol (postcard over vsock / unix)
  iliada-broker    permission broker daemon (bin: iliada-broker)
  iliada-guest     client library used inside sandboxes (+ examples)
  iliada-sandbox   Tier-1 bubblewrap / Tier-2 cloud-hypervisor controller
nix/
  modules/         the declarative system manifest (iliada.* NixOS options)
  hosts/           dev-vm (mutable) and appliance (immutable image) systems
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

Optional AI advisor for ingestion: `cargo run -p iliada-ingest --features claude
-- run <src> --advisor claude` with `ANTHROPIC_API_KEY` set.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option. Unless you explicitly state
otherwise, any contribution intentionally submitted for inclusion in this
project shall be dual licensed as above, without any additional terms.
