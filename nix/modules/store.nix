# The Iliada Store: where the catalog is, and the background job that keeps
# Store apps installed and up to date.
{ config, lib, ... }:
let
  cfg = config.iliada.store;
  apps = config.iliada.applications;
in
{
  options.iliada.store = {
    enable = lib.mkOption {
      type = lib.types.bool;
      default = apps.enable;
      defaultText = lib.literalExpression "config.iliada.applications.enable";
      description = "The Iliada Store and automatic app updates.";
    };
    catalog = lib.mkOption {
      type = lib.types.str;
      default = "https://github.com/cseas002/iliada_apps/releases/download/catalog/catalog.json";
      description = "The signed app catalog (its signature must match the key built into Iliada).";
    };
    updateInterval = lib.mkOption {
      type = lib.types.str;
      default = "6h";
      description = "How often to update apps and install missing ones from the app list.";
    };
  };

  config = lib.mkIf cfg.enable {
    environment.etc."iliada/store.json".text = builtins.toJSON {
      inherit (cfg) catalog;
      appList = apps.list;
      applications = apps.root;
    };

    users.users.iliada-store = {
      isSystemUser = true;
      group = "iliada-apps";
    };

    # Installs apps on the app list that are missing (restores) and updates the rest.
    systemd.services.iliada-store-sync = {
      description = "Iliada Store: install and update apps";
      wants = [ "network-online.target" ];
      after = [ "network-online.target" ];
      serviceConfig = {
        Type = "oneshot";
        ExecStart = "${config.iliada.package}/bin/iliada-store sync";
        User = "iliada-store";
        Group = "iliada-apps";
        UMask = "0002";
        ReadWritePaths = [ apps.root apps.list ];
        ProtectSystem = "strict";
        ProtectHome = true;
        PrivateTmp = true;
        PrivateDevices = true;
        NoNewPrivileges = true;
        CapabilityBoundingSet = "";
        RestrictAddressFamilies = [ "AF_INET" "AF_INET6" "AF_UNIX" ];
        RestrictNamespaces = true;
        LockPersonality = true;
        MemoryDenyWriteExecute = true;
        SystemCallArchitectures = "native";
        SystemCallFilter = [ "@system-service" "~@privileged" ];
      };
    };
    systemd.timers.iliada-store-sync = {
      wantedBy = [ "timers.target" ];
      timerConfig = {
        OnBootSec = "2min";
        OnUnitActiveSec = cfg.updateInterval;
        Persistent = true;
      };
    };
  };
}
