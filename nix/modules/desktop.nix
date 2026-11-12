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
    setupAssistant = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = "Open the Setup Assistant (restore a backup, turn backups on) at each user's first login.";
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
    # (which installs Flatpaks and system packages). Iliada's Setup Assistant
    # replaces COSMIC's first-login wizard.
    environment.cosmic.excludePackages = [ pkgs.cosmic-store ]
      ++ lib.optional (pkgs ? cosmic-initial-setup) pkgs.cosmic-initial-setup;
    environment.etc."xdg/autostart/org.iliada.Setup.desktop" = lib.mkIf cfg.setupAssistant {
      source = ../../apps/iliada-settings/res/org.iliada.Setup-autostart.desktop;
    };
    environment.systemPackages = [
      # The Store, Iliada Settings and the Setup Assistant.
      (pkgs.callPackage ../desktop-apps.nix { })
      # Iliada's desktop defaults (users can still change them): Applications
      # in the Files sidebar; the Iliada Store in the dock. hiPrio: they replace
      # COSMIC's own defaults for the same settings.
      (lib.hiPrio (pkgs.runCommand "iliada-cosmic-defaults" { } ''
        files=$out/share/cosmic/com.system76.CosmicFiles/v1
        dock=$out/share/cosmic/com.system76.CosmicAppList/v1
        mkdir -p $files $dock
        cat > $files/favorites <<'EOF'
        [
            Home,
            Path("/Applications"),
            Documents,
            Downloads,
            Music,
            Pictures,
            Videos,
        ]
        EOF
        cat > $dock/favorites <<'EOF'
        [
            "com.system76.CosmicFiles",
            "org.iliada.Store",
            "org.iliada.Settings",
            "com.system76.CosmicEdit",
            "com.system76.CosmicTerm",
            "com.system76.CosmicSettings",
        ]
        EOF
        # Alt+Space opens the Assistant from anywhere (Super+Space stays the
        # keyboard-layout switch).
        keys=$out/share/cosmic/com.system76.CosmicSettings.Shortcuts/v1
        mkdir -p $keys
        cat > $keys/custom <<'EOF'
        {
            (modifiers: [Alt], key: "space"): Spawn("iliada-assistant-app"),
        }
        EOF
        # Frosted glass: windows, the panel and dock, their menus and the
        # launcher are see-through with the desktop blurred behind them
        # (COSMIC Settings → Appearance turns it off). Set for the theme and
        # for the builder COSMIC regenerates the theme from, light and dark.
        for theme in Dark Light Dark.Builder Light.Builder; do
          d=$out/share/cosmic/com.system76.CosmicTheme.$theme/v1
          mkdir -p $d
          for key in frosted_windows frosted_panel frosted_applets frosted_system_interface; do
            echo true > $d/$key
          done
        done
      ''))
    ];
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
