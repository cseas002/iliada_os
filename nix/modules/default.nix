# The Iliada declarative system manifest.
#
#   iliada.image         immutable root, dm-verity /nix/store, A/B updates
#   iliada.applications  the /Applications volume and the ingestion daemon
#   iliada.isolation     sandbox tiers, VMM, guest kernel, network policy
#   iliada.broker        the runtime permission broker
#   iliada.desktop       COSMIC desktop
#   iliada.workstation   networking, audio, bluetooth, firmware, boot loader
#   iliada.rustUserland  Rust coreutils, sudo-rs, ntpd-rs, fish
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
    ./ux.nix
  ];

  options.iliada.version = lib.mkOption {
    type = lib.types.str;
    default = "0.1.0";
    readOnly = true;
    description = "Iliada release version.";
  };

  # Branding: "Iliada" in /etc/os-release, the boot menu and the greeter.
  # The distribution ID stays "nixos" so NixOS tooling keeps working.
  config.system.nixos.distroName = "Iliada";

  options.iliada.package = lib.mkOption {
    type = lib.types.package;
    default = pkgs.callPackage ../package.nix { };
    defaultText = lib.literalExpression "pkgs.callPackage ./nix/package.nix { }";
    description = "Package providing iliada-broker and iliada-ingest.";
  };
}
