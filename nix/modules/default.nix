# The Iliada declarative system manifest.
#
#   iliada.image         immutable root, dm-verity /nix/store, A/B updates
#   iliada.applications  the /Applications volume and the ingestion daemon
#   iliada.isolation     sandbox tiers, VMM, guest kernel, network policy
#   iliada.broker        the runtime permission broker
#   iliada.ux            FHS hiding, privilege model
{ lib, pkgs, ... }:
{
  imports = [
    ./image.nix
    ./applications.nix
    ./isolation.nix
    ./broker.nix
    ./ux.nix
  ];

  options.iliada.package = lib.mkOption {
    type = lib.types.package;
    default = pkgs.callPackage ../package.nix { };
    defaultText = lib.literalExpression "pkgs.callPackage ./nix/package.nix { }";
    description = "Package providing iliada-broker and iliada-ingest.";
  };
}
