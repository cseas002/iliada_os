# The Iliada desktop: COSMIC (Rust, Wayland, Smithay-based compositor).
# Iliada-specific shell pieces (trusted prompts, Applications browser,
# Privacy settings) are layered on top in Phase 2.
{ config, lib, pkgs, ... }:
let
  cfg = config.iliada.desktop;
in
{
  options.iliada.desktop = {
    enable = lib.mkEnableOption "the Iliada desktop (COSMIC)";
    autoLoginUser = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      example = "iliada";
      description = "Log this user in automatically (kiosk / test VMs).";
    };
  };

  config = lib.mkIf cfg.enable {
    # COSMIC Files shows Iliada .app bundles as apps (their own icon, name
    # without ".app"), launches them sandboxed on open, and never treats them
    # as drop targets. See nix/patches/cosmic-files-app-bundles.patch.
    nixpkgs.overlays = [
      (final: prev: {
        cosmic-files = prev.cosmic-files.overrideAttrs (old: {
          patches = (old.patches or [ ]) ++ [ ../patches/cosmic-files-app-bundles.patch ];
        });
      })
    ];
    services.desktopManager.cosmic.enable = true;
    # Apps come only as .app bundles: the Iliada Store replaces COSMIC Store
    # (which installs Flatpaks and system packages).
    environment.cosmic.excludePackages = [ pkgs.cosmic-store ];
    environment.systemPackages = [ (pkgs.callPackage ../store-gui.nix { }) ];
    services.displayManager.cosmic-greeter.enable = true;
    services.displayManager.autoLogin = lib.mkIf (cfg.autoLoginUser != null) {
      enable = true;
      user = cfg.autoLoginUser;
    };
    hardware.graphics.enable = true;
    # Desktop apps reach portals; the Iliada broker will back these in Phase 2.
    xdg.portal.enable = true;
  };
}
