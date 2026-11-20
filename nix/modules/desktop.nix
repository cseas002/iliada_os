# The Iliada desktop: COSMIC (Rust, Wayland, Smithay-based compositor).
# Iliada-specific shell pieces (trusted prompts, Applications browser,
# Privacy settings) are layered on top in Phase 2.
{ config, lib, pkgs, ... }:
let
  cfg = config.iliada.desktop;
  desktopApps = pkgs.callPackage ../desktop-apps.nix { };
  # What COSMIC Settings needs of Iliada's source for its pages.
  iliadaSource = lib.fileset.toSource {
    root = ../..;
    fileset = lib.fileset.unions [ ../../Cargo.toml ../../Cargo.lock ../../crates ../../apps ];
  };
  # The default wallpaper: Homer's wine-dark sea at dusk, Troy on its hill,
  # the Achaean ships on the horizon (nix/branding/wallpaper).
  wallpaper = pkgs.runCommand "iliada-wallpaper" { nativeBuildInputs = [ pkgs.librsvg ]; } ''
    mkdir -p $out/share/backgrounds/iliada
    rsvg-convert -w 3840 ${../branding/wallpaper/wine-dark-sea.svg} -o $out/share/backgrounds/iliada/wine-dark-sea.png
  '';
  themeIds = "com.system76.CosmicTheme.Dark com.system76.CosmicTheme.Light com.system76.CosmicTheme.Dark.Builder com.system76.CosmicTheme.Light.Builder";
  # COSMIC's apps under plain names (left out when a system excludes the app).
  renamedApps = lib.filter (a: !(lib.elem a.pkg config.environment.cosmic.excludePackages)) [
    { pkg = pkgs.cosmic-settings; id = "com.system76.CosmicSettings"; name = "Settings"; }
    { pkg = pkgs.cosmic-files; id = "com.system76.CosmicFiles"; name = "Files"; }
    { pkg = pkgs.cosmic-edit; id = "com.system76.CosmicEdit"; name = "Text Editor"; }
    { pkg = pkgs.cosmic-term; id = "com.system76.CosmicTerm"; name = "Terminal"; }
    { pkg = pkgs.cosmic-player; id = "com.system76.CosmicPlayer"; name = "Media Player"; }
    { pkg = pkgs.cosmic-viewer; id = "com.system76.CosmicViewer"; name = "Photos"; }
    { pkg = pkgs.cosmic-monitor; id = "com.system76.CosmicMonitor"; name = "System Monitor"; }
    { pkg = pkgs.cosmic-reader; id = "com.system76.CosmicReader"; name = "Document Reader"; }
    { pkg = pkgs.cosmic-screenshot; id = "com.system76.CosmicScreenshot"; name = "Screenshot"; }
  ];
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
        # One Settings app: COSMIC Settings shows Iliada's pages (account,
        # privacy, Homer, updates, backup…) among its own. Iliada's source
        # goes next to it (the pages are iliada_settings::panel), and the
        # lock file covers both.
        cosmic-settings = prev.cosmic-settings.overrideAttrs (old: {
          patches = (old.patches or [ ]) ++ [ ../patches/cosmic-settings-iliada-pages.patch ];
          postPatch = (old.postPatch or "") + ''
            cp -r ${iliadaSource} iliada
            chmod -R u+w iliada
            cp ${../patches/cosmic-settings-iliada.Cargo.lock} Cargo.lock
          '';
          cargoDeps = final.rustPlatform.importCargoLock {
            lockFile = ../patches/cosmic-settings-iliada.Cargo.lock;
            allowBuiltinFetchGit = true;
          };
          # The Iliada account's backups are compressed with zstd.
          buildInputs = (old.buildInputs or [ ]) ++ [ final.zstd ];
        });
        # Every COSMIC app is built with this hook, which points at COSMIC
        # Settings' theme files. Give it the stock COSMIC Settings (same
        # theme files) so the other apps stay byte-identical to nixpkgs and
        # download from its cache, instead of all compiling here.
        libcosmicAppHook = prev.libcosmicAppHook.override {
          targetPackages = prev.targetPackages // { inherit (prev) cosmic-settings; };
        };
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
      # Notifications from scripts and the Setup Assistant's background installs.
      pkgs.libnotify
      # The Store, Iliada Settings and the Setup Assistant.
      desktopApps
      wallpaper
      # Iliada's desktop defaults (users can still change them): Applications
      # in the Files sidebar; the Iliada Store in the dock; the Iliad look.
      # hiPrio: they replace COSMIC's own defaults for the same settings.
      (lib.hiPrio (pkgs.runCommand "iliada-cosmic-defaults" { } ''
        # The Iliad look: bronze on navy (dark), deep bronze on parchment
        # (light), computed by COSMIC's own theme code (iliada-theme).
        ${desktopApps}/bin/iliada-theme $out/share
        bg=$out/share/cosmic/com.system76.CosmicBackground/v1
        mkdir -p $bg
        cat > $bg/all <<EOF
        (
            output: "all",
            source: Path("${wallpaper}/share/backgrounds/iliada/wine-dark-sea.png"),
            filter_by_theme: false,
            rotation_frequency: 3600,
            filter_method: Lanczos,
            scaling_mode: Zoom,
            sampling_method: Alphanumeric,
        )
        EOF
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
        apps=$out/share/applications
        mkdir -p $apps
        # COSMIC's apps by plain names ("Files", not "COSMIC Files"), and
        # Iliada's icons for them (nix/branding/cosmic-icons), at every size
        # their own come in. One Settings app: COSMIC Settings, which shows
        # Iliada's pages among its own, is "Settings".
        rename() {
          [ -f "$1" ] || return 0
          sed -e '/^Name\[/d' -e "s/^Name=.*/Name=$2/" "$1" > $apps/$(basename "$1")
        }
        ${lib.concatMapStrings ({ pkg, id, name }: ''
          rename ${pkg}/share/applications/${id}.desktop "${name}"
        '') renamedApps}
        for icon in ${../branding/cosmic-icons}/*.svg; do
          for size in 16x16 24x24 32x32 48x48 64x64 128x128 256x256 scalable; do
            install -Dm644 "$icon" $out/share/icons/hicolor/$size/apps/$(basename "$icon")
          done
        done
        # Tools for the terminal and for Linux experts stay out of the app
        # library (they still run from a terminal).
        for id in htop btop nixos-manual qt5ct qt6ct gvim vim nm-connection-editor; do
          printf '[Desktop Entry]\nType=Application\nName=%s\nExec=true\nNoDisplay=true\n' $id > $apps/$id.desktop
        done
        cat > $dock/favorites <<'EOF'
        [
            "com.system76.CosmicFiles",
            "org.iliada.Store",
            "org.iliada.Assistant",
            "com.system76.CosmicSettings",
            "com.system76.CosmicEdit",
            "com.system76.CosmicTerm",
        ]
        EOF
        # Alt+Space opens the assistant's quick panel from anywhere (Super+Space stays the
        # keyboard-layout switch).
        keys=$out/share/cosmic/com.system76.CosmicSettings.Shortcuts/v1
        mkdir -p $keys
        cat > $keys/custom <<'EOF'
        {
            (modifiers: [Alt], key: "space"): Spawn("iliada-ask"),
        }
        EOF
        # Frosted glass: windows, the panel and dock, their menus and the
        # launcher are see-through with the desktop blurred behind them
        # (COSMIC Settings → Appearance turns it off). Set for the theme and
        # for the builder COSMIC regenerates the theme from, light and dark.
        for theme in Dark Light Dark.Builder Light.Builder; do
          for v in v1 v2; do
            d=$out/share/cosmic/com.system76.CosmicTheme.$theme/$v
            mkdir -p $d
            for key in frosted_windows frosted_panel frosted_applets frosted_system_interface; do
              echo true > $d/$key
            done
          done
        done
      ''))
    ];
    # COSMIC's apps read the built-in fallback theme before the system
    # defaults above, so each person gets Iliada's look (colours and frosted
    # glass) in their own settings at login. A theme the person changed in
    # Appearance is theirs and stays; one still as Iliada left it follows
    # Iliada's (a newer Iliada can refresh its look).
    systemd.user.services.iliada-look = {
      description = "Iliada's look: the Iliad theme and frosted glass";
      wantedBy = [ "default.target" ];
      path = [ pkgs.diffutils ];
      serviceConfig.Type = "oneshot";
      script = ''
        base=''${XDG_CONFIG_HOME:-$HOME/.config}/cosmic
        sys=/run/current-system/sw/share/cosmic
        saved=''${XDG_STATE_HOME:-$HOME/.local/state}/iliada/look
        for id in ${themeIds}; do
          for v in v1 v2; do
            src=$sys/$id/$v
            [ -d "$src" ] || continue
            dst=$base/$id/$v
            was=$saved/$id/$v
            mkdir -p "$dst"
            if [ -z "$(ls -A "$dst")" ] || { [ -d "$was" ] && diff -rq "$dst" "$was" >/dev/null; }; then
              rm -rf "$dst" "$was"
              mkdir -p "$dst" "$was"
              cp -L "$src"/* "$dst"/
              cp -L "$src"/* "$was"/
              chmod -R u+w "$dst" "$was"
            fi
          done
        done
      '';
    };
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
