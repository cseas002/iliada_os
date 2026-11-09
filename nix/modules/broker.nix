# The runtime permission broker.
#
# Phase 1 runs it as a hardened system service with the "deny" frontend:
# until the compositor-owned prompt exists, every request that would need a
# prompt is denied (safe default), while stored grants and declarations are
# enforced. Phase 2 moves it to a per-user service bound to the graphical
# session, which is where prompts and user files belong.
{ config, lib, pkgs, ... }:
let
  cfg = config.iliada.broker;
in
{
  options.iliada.broker = {
    enable = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = "Run the Iliada permission broker.";
    };
    frontend = lib.mkOption {
      type = lib.types.enum [ "deny" "terminal" ];
      default = "deny";
      description = "How prompts are shown. `terminal` is only useful for debugging on a console.";
    };
  };

  config = lib.mkIf cfg.enable {
    users.users.iliada-broker = {
      isSystemUser = true;
      group = "iliada-sandbox";
    };
    systemd.services.iliada-broker = {
      description = "Iliada permission broker";
      wantedBy = [ "multi-user.target" ];
      after = [ "systemd-tmpfiles-setup.service" ];
      serviceConfig = {
        ExecStart = lib.escapeShellArgs [
          "${config.iliada.package}/bin/iliada-broker"
          "--applications" config.iliada.applications.root
          "--socket-dir" config.iliada.isolation.socketDir
          "--state-dir" "/var/lib/iliada-broker"
          "--frontend" cfg.frontend
        ];
        User = "iliada-broker";
        Group = "iliada-sandbox";
        StateDirectory = "iliada-broker";
        StateDirectoryMode = "0700";
        ReadWritePaths = [ config.iliada.isolation.socketDir ];
        ProtectSystem = "strict";
        ProtectHome = "read-only";
        PrivateTmp = true;
        PrivateDevices = true;
        PrivateNetwork = true;
        NoNewPrivileges = true;
        CapabilityBoundingSet = "";
        RestrictAddressFamilies = [ "AF_UNIX" ];
        RestrictNamespaces = true;
        LockPersonality = true;
        MemoryDenyWriteExecute = true;
        ProtectKernelTunables = true;
        ProtectKernelModules = true;
        ProtectControlGroups = true;
        SystemCallArchitectures = "native";
        SystemCallFilter = [ "@system-service" "~@privileged" ];
        Restart = "on-failure";
      };
    };
  };
}
