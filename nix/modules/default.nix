# The Iliada declarative system manifest.
#
#   iliada.image         immutable root, dm-verity /nix/store, A/B updates
#   iliada.applications  the /Applications volume and the ingestion daemon
#   iliada.isolation     sandbox tiers, VMM, guest kernel, network policy
#   iliada.broker        the runtime permission broker
#   iliada.desktop       COSMIC desktop
#   iliada.workstation   networking, audio, bluetooth, firmware, boot loader
#   iliada.rustUserland  Rust coreutils, sudo-rs, ntpd-rs, fish
#   iliada.baseTools     everyday command-line tools (lspci, dig, lsof, jq, …)
#   iliada.settingsFile  settings.toml: command-line tools, services
#   iliada.store         the Store catalog and automatic app updates
#   iliada.system        the system service: tools, services, updates, backups
#   iliada.ux            FHS hiding, privilege model
{ lib, pkgs, ... }:
{
  imports = [
    ./image.nix
    ./applications.nix
    ./isolation.nix
    ./broker.nix
    ./desktop.nix
    ./workstation.nix
    ./rust-userland.nix
    ./base-tools.nix
    ./settings.nix
    ./store.nix
    ./system.nix
    ./ux.nix
    ./branding.nix
  ];

  options.iliada.version = lib.mkOption {
    type = lib.types.str;
    # One place for the version: VERSION at the top of the source (checked
    # by `iliada-catalog release`).
    default = lib.trim (builtins.readFile ../../VERSION);
    readOnly = true;
    description = "Iliada release version.";
  };

  options.iliada.package = lib.mkOption {
    type = lib.types.package;
    default = pkgs.callPackage ../package.nix { };
    defaultText = lib.literalExpression "pkgs.callPackage ./nix/package.nix { }";
    description = "Package providing iliada-broker and iliada-ingest.";
  };
}
