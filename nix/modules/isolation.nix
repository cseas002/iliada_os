# Sandbox tiers. Writes /etc/iliada/isolation.json, read by iliada-sandbox
# (HostIsolationConfig); keep the field names in sync with
# crates/iliada-sandbox/src/host.rs.
{ config, lib, pkgs, ... }:
let
  cfg = config.iliada.isolation;
  # "bzImage" on x86_64, "Image" on aarch64.
  kernelFile = config.system.boot.loader.kernelFile;
in
{
  options.iliada.isolation = {
    defaultTier = lib.mkOption {
      type = lib.types.enum [ "namespace" "micro-vm" ];
      default = "namespace";
      description = ''
        Tier for apps that do not ask for one. The namespace sandbox (like macOS's
        App Sandbox) runs at native speed; micro-VMs cost start-up time, memory and
        graphics speed, so they are opt-in.
      '';
    };
    highSecurity = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = ''
        High security mode: run every app in its own micro-VM (Qubes-style),
        regardless of its own setting. Strongest isolation, slower apps.
      '';
    };
    vmm = lib.mkOption {
      type = lib.types.enum [ "cloud-hypervisor" ];
      default = "cloud-hypervisor";
      description = "Virtual machine monitor for Tier-2 sandboxes.";
    };
    guestKernel = lib.mkOption {
      type = lib.types.package;
      default = config.boot.kernelPackages.kernel;
      defaultText = lib.literalExpression "config.boot.kernelPackages.kernel";
      description = "Kernel booted inside app micro-VMs.";
    };
    guestInitrd = lib.mkOption {
      type = lib.types.str;
      default = "/run/current-system/iliada/guest/initrd";
      description = "Guest initramfs with the Iliada guest agent (built in Phase 2).";
    };
    runtime = lib.mkOption {
      type = lib.types.package;
      # Must provide every soname in crates/iliada-ingest/runtime/iliada-base-1.toml.
      default = pkgs.symlinkJoin {
        name = "iliada-base-1";
        # bin/: a POSIX shell and the (Rust) core utilities, so the launcher
        # scripts many apps ship (/usr/bin/chromium, soffice, …) run unmodified.
        postBuild = "ln -s dash $out/bin/sh";
        paths = map lib.getBin (with pkgs; [ dash uutils-coreutils-noprefix gnugrep gnused ]) ++ map lib.getLib (with pkgs; [
          glibc
          stdenv.cc.cc # libgcc_s, libstdc++
          libxcrypt
          libglvnd # libGL, libEGL, libGLX, libGLESv2 (dispatch to the host's Mesa)
          libgbm
          libdrm
          vulkan-loader
          wayland
          libxkbcommon
          alsa-lib
          pipewire
          libpulseaudio
          fontconfig
          freetype
        ]);
      };
      defaultText = lib.literalExpression "glibc, GCC runtime, libglvnd, Wayland, audio and font libraries";
      description = ''
        The runtime base (ABI "iliada-base-1") mapped read-only into every sandbox.
        Must provide the sonames listed in crates/iliada-ingest/runtime/iliada-base-1.toml.
      '';
    };
    defaultNetwork = lib.mkOption {
      type = lib.types.enum [ "none" "nat" ];
      default = "none";
      description = "Network policy for sandboxes without an explicit grant.";
    };
    defaultVcpus = lib.mkOption {
      type = lib.types.ints.between 1 64;
      default = 2;
    };
    defaultMemoryMib = lib.mkOption {
      type = lib.types.ints.between 128 65536;
      default = 1024;
    };
    socketDir = lib.mkOption {
      type = lib.types.str;
      default = "/run/iliada/sandbox";
      description = "Per-app VMM, vsock and broker sockets (one directory, by cloud-hypervisor convention).";
    };
    dataRoot = lib.mkOption {
      type = lib.types.str;
      default = "/var/lib/iliada/apps";
      description = "Parent of per-app private data directories.";
    };
  };

  config = {
    users.groups.iliada-sandbox = { };
    environment.systemPackages = with pkgs; [ cloud-hypervisor virtiofsd bubblewrap ];
    systemd.tmpfiles.rules = [
      "d ${cfg.socketDir} 0770 root iliada-sandbox -"
      "d ${cfg.dataRoot} 0750 root iliada-sandbox -"
    ];
    environment.etc."iliada/isolation.json".text = builtins.toJSON {
      guestKernel = "${cfg.guestKernel}/${kernelFile}";
      guestInitrd = cfg.guestInitrd;
      cloudHypervisor = lib.getExe pkgs.cloud-hypervisor;
      virtiofsd = lib.getExe pkgs.virtiofsd;
      bwrap = lib.getExe pkgs.bubblewrap;
      runtimeDir = "${cfg.runtime}";
      socketDir = cfg.socketDir;
      dataRoot = cfg.dataRoot;
      defaultVcpus = cfg.defaultVcpus;
      defaultMemoryMib = cfg.defaultMemoryMib;
      highSecurity = cfg.highSecurity;
    };
  };
}
