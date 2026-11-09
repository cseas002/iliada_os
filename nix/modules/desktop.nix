# The Iliada desktop: COSMIC (Rust, Wayland, Smithay-based compositor).
# Iliada-specific shell pieces (trusted prompts, Applications browser,
# Privacy settings) are layered on top in Phase 2.
{ config, lib, ... }:
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
    services.desktopManager.cosmic.enable = true;
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
