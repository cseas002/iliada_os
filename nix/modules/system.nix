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
    githubClientId = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      example = "Iv1.0123456789abcdef";
      description = ''
        Client ID of the GitHub OAuth app that backups sign in with (device
        flow enabled). Public, not a secret. Without it, backups are off.
      '';
    };
    updateUrl = lib.mkOption {
      type = lib.types.str;
      default = "https://github.com/cseas002/iliada_os/releases/latest/download/release.json";
      description = "The signed manifest of the newest Iliada release.";
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

    environment.systemPackages = [ pkg ];

    systemd.services.iliada-system = {
      description = "Iliada system service";
      wantedBy = [ "multi-user.target" ];
      after = [ "network.target" ] ++ lib.optional cfg.firstBoot.enable "iliada-first-boot.service";
      path = [ config.system.build.nixos-rebuild config.nix.package pkgs.git ];
      environment = {
        ILIADA_VERSION = config.iliada.version;
        ILIADA_UPDATE_URL = cfg.updateUrl;
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
