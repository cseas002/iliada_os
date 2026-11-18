# The Iliada system service (iliada-systemd): command-line tools, services,
# Iliada updates and GitHub backups, for administrators without sudo. The
# `iliada` command and the Settings app are its clients.
{ config, lib, pkgs, iliada ? null, ... }:
let
  cfg = config.iliada.system;
  pkg = config.iliada.package;
in
{
  options.iliada.system = {
    enable = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = "The Iliada system service.";
    };
    systemctlBridge = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = ''
        `systemctl enable`/`disable` typed at a terminal turn Iliada's services
        (sshd, cups, nginx, postgresql, mysql, redis, docker) on or off in
        settings.toml, like `iliada install ssh`; `start`/`stop` say they last
        until the next restart. Scripts get the real systemctl unchanged.
      '';
    };
    githubClientId = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      # Iliada's own GitHub OAuth app (device flow on), registered 2026-11-14.
      default = "Ov23liAjiMw2pgeP93Al";
      example = "Iv1.0123456789abcdef";
      description = ''
        Client ID of the GitHub OAuth app that backups sign in with (device
        flow enabled). Public, not a secret. null turns backups off.
      '';
    };
    updateUrl = lib.mkOption {
      type = lib.types.str;
      default = "https://github.com/iliada-os/iliada/releases/latest/download/release.json";
      description = "The signed manifest of the newest Iliada release.";
    };
    testingUpdateUrl = lib.mkOption {
      type = lib.types.str;
      default = "https://github.com/iliada-os/iliada/releases/download/testing/release.json";
      description = ''
        The signed manifest of the newest test update (the newest work,
        between releases). Used when Settings → Updates → Test updates is on
        (`[updates] testing = true` in settings.toml).
      '';
    };
    firstBoot = {
      enable = lib.mkEnableOption "creating /etc/iliada on first boot (ready-to-run images)";
      user = lib.mkOption {
        type = lib.types.str;
        default = "iliada";
        description = "The user the configuration declares.";
      };
      fullName = lib.mkOption {
        type = lib.types.str;
        default = "Iliada";
      };
      autoLogin = lib.mkOption {
        type = lib.types.bool;
        default = false;
      };
      devAccess = lib.mkOption {
        type = lib.types.bool;
        default = false;
        description = "Keep developer SSH access (nix/hosts/utm-dev.nix) in the written configuration.";
      };
    };
  };

  config = lib.mkIf cfg.enable {
    assertions = [{
      assertion = !cfg.firstBoot.enable || iliada != null;
      message = "iliada.system.firstBoot needs the Iliada flake (build the system with iliada.lib.iliadaSystem)";
    }];

    environment.systemPackages = [ pkg ] ++ lib.optional cfg.systemctlBridge (lib.hiPrio (pkgs.writeShellScriptBin "systemctl" ''
      # Typed by a person: through Iliada (see iliada-systemctl). Scripts:
      # the real systemctl, untouched.
      if [ -t 0 ] && [ -t 2 ]; then
        exec ${pkg}/bin/iliada-systemctl ${config.systemd.package}/bin/systemctl "$@"
      fi
      exec ${config.systemd.package}/bin/systemctl "$@"
    ''));

    # "Iliada 0.2.3 is available" in the person's session, once per version.
    systemd.user.services.iliada-update-notice = {
      description = "Tell when an Iliada update is ready";
      path = [ config.iliada.package pkgs.libnotify ];
      serviceConfig = {
        Type = "oneshot";
        ExecStart = "${config.iliada.package}/bin/iliada system notify";
        # Iliada Settings, opened from the notice, outlives this job.
        KillMode = "process";
      };
    };
    systemd.user.timers.iliada-update-notice = {
      wantedBy = [ "timers.target" ];
      timerConfig = {
        # After the system's own check, 2 minutes after start-up.
        OnStartupSec = "4min";
        OnUnitActiveSec = "1h";
      };
    };

    systemd.services.iliada-system = {
      description = "Iliada system service";
      wantedBy = [ "multi-user.target" ];
      after = [ "network.target" ] ++ lib.optional cfg.firstBoot.enable "iliada-first-boot.service";
      path = [ config.system.build.nixos-rebuild config.nix.package pkgs.git ];
      environment = {
        ILIADA_VERSION = config.iliada.version;
        ILIADA_UPDATE_URL = cfg.updateUrl;
        ILIADA_TESTING_UPDATE_URL = cfg.testingUpdateUrl;
      } // lib.optionalAttrs (cfg.githubClientId != null) { ILIADA_GITHUB_CLIENT_ID = cfg.githubClientId; };
      # A switch must not restart the service that is running the switch; it
      # restarts itself once idle when its binary has changed.
      restartIfChanged = false;
      serviceConfig = {
        ExecStart = "${pkg}/bin/iliada-systemd serve --state /var/lib/iliada-system";
        Restart = "always";
        RestartSec = 2;
        RuntimeDirectory = "iliada-system";
        RuntimeDirectoryMode = "0755";
        StateDirectory = "iliada-system";
        StateDirectoryMode = "0700";
      };
    };

    # Ready-to-run images have no installer: write /etc/iliada on first boot
    # so the system can be changed and updated like an installed one.
    systemd.services.iliada-first-boot = lib.mkIf cfg.firstBoot.enable {
      description = "Iliada: create the system configuration";
      wantedBy = [ "multi-user.target" ];
      wants = [ "network-online.target" ];
      after = [ "network-online.target" "local-fs.target" ];
      unitConfig.ConditionPathExists = "!/etc/iliada/flake.nix";
      path = [ config.nix.package pkgs.git ];
      serviceConfig = {
        Type = "oneshot";
        RemainAfterExit = true;
        ExecStart = lib.escapeShellArgs ([
          "${pkg}/bin/iliada-systemd" "init" "--if-missing"
          "--profile" "vm"
          "--source" "${iliada}"
          "--user" cfg.firstBoot.user
          "--full-name" cfg.firstBoot.fullName
          "--hostname" config.networking.hostName
          "--timezone" (if config.time.timeZone == null then "UTC" else config.time.timeZone)
        ] ++ lib.optional cfg.firstBoot.autoLogin "--auto-login"
          ++ lib.optional cfg.firstBoot.devAccess "--dev-access");
      };
    };
  };
}
