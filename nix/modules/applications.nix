# /Applications: a separate, writable volume of self-contained .app bundles.
# Installing an app never touches the system image, which is why no
# administrator password is needed for it.
{ config, lib, pkgs, ... }:
let
  cfg = config.iliada.applications;
in
{
  options.iliada.applications = {
    enable = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = "Provide the /Applications bundle directory.";
    };
    root = lib.mkOption {
      type = lib.types.str;
      default = "/Applications";
      description = "Directory holding .app bundles.";
    };
    device = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      example = "/dev/disk/by-partlabel/iliada-applications";
      description = "Block device mounted at `root`. Required on the immutable image (its / is a tmpfs).";
    };
    fsType = lib.mkOption {
      type = lib.types.str;
      default = "btrfs";
      description = "Filesystem of `device`.";
    };
    ingest = {
      enable = lib.mkEnableOption "the iliada-ingest daemon that keeps watched apps up to date";
      watchlist = lib.mkOption {
        type = lib.types.nullOr lib.types.path;
        default = null;
        description = "Watchlist TOML (see examples/watchlist.toml).";
      };
      extraArgs = lib.mkOption {
        type = lib.types.listOf lib.types.str;
        default = [ ];
        example = [ "--suite" "trixie" ];
        description = "Extra arguments for `iliada-ingest serve`.";
      };
    };
  };

  config = lib.mkIf cfg.enable {
    assertions = [
      {
        assertion = !cfg.ingest.enable || cfg.ingest.watchlist != null;
        message = "iliada.applications.ingest.enable requires iliada.applications.ingest.watchlist";
      }
      {
        assertion = !config.iliada.image.enable || cfg.device != null;
        message = "the immutable image has a tmpfs root; set iliada.applications.device";
      }
    ];

    users.groups.iliada-apps = { };
    systemd.tmpfiles.rules = [ "d ${cfg.root} 2775 root iliada-apps -" ];
    fileSystems = lib.mkIf (cfg.device != null) {
      ${cfg.root} = {
        inherit (cfg) device fsType;
        options = [ "nodev" "nosuid" ];
      };
    };
    environment.systemPackages = [ config.iliada.package ];

    # iliada-appd exports desktop entries for GUI bundles; desktops search here.
    # Appended *last* so system entries always take precedence: an app bundle
    # must never be able to shadow a system app's desktop entry.
    environment.sessionVariables.XDG_DATA_DIRS = lib.mkAfter [ "/var/lib/iliada-desktop" ];
    users.users.iliada-appd = {
      isSystemUser = true;
      group = "iliada-apps";
    };
    systemd.services.iliada-appd = {
      description = "Iliada application registry";
      wantedBy = [ "multi-user.target" ];
      serviceConfig = {
        ExecStart = lib.escapeShellArgs [
          "${config.iliada.package}/bin/iliada-appd"
          "--applications" cfg.root
          "--export" "/var/lib/iliada-desktop"
          "--iliada-run" "/run/current-system/sw/bin/iliada-run"
        ];
        User = "iliada-appd";
        Group = "iliada-apps";
        StateDirectory = "iliada-desktop";
        StateDirectoryMode = "0755";
        UMask = "0022";
        ProtectSystem = "strict";
        ProtectHome = true;
        PrivateTmp = true;
        PrivateDevices = true;
        PrivateNetwork = true;
        NoNewPrivileges = true;
        CapabilityBoundingSet = "";
        RestrictAddressFamilies = [ "AF_UNIX" ];
        RestrictNamespaces = true;
        LockPersonality = true;
        MemoryDenyWriteExecute = true;
        SystemCallArchitectures = "native";
        SystemCallFilter = [ "@system-service" "~@privileged" ];
        Restart = "on-failure";
      };
    };

    users.users.iliada-ingest = lib.mkIf cfg.ingest.enable {
      isSystemUser = true;
      group = "iliada-apps";
    };

    systemd.services.iliada-ingest = lib.mkIf cfg.ingest.enable {
      description = "Iliada ingestion daemon";
      wantedBy = [ "multi-user.target" ];
      wants = [ "network-online.target" ];
      after = [ "network-online.target" ];
      # Schema compilation (Debian does it in a trigger) and RUNPATH rewriting.
      path = [ pkgs.glib.dev pkgs.patchelf ];
      serviceConfig = {
        ExecStart = lib.escapeShellArgs ([
          "${config.iliada.package}/bin/iliada-ingest" "serve"
          "--watchlist" "${cfg.ingest.watchlist}"
          "--out" cfg.root
          "--cache" "/var/cache/iliada-ingest"
        ] ++ cfg.ingest.extraArgs);
        User = "iliada-ingest";
        Group = "iliada-apps";
        UMask = "0022";
        CacheDirectory = "iliada-ingest";
        ReadWritePaths = [ cfg.root ];
        # Hardening. Extraction is additionally confined with Landlock per worker thread.
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
