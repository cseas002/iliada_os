# Everyday command-line tools, present on every Iliada system so a terminal
# behaves like people expect from any Linux (lscpu, ip a, lspci, dig, …).
# Users never need these; they are for whoever opens a terminal. More can
# be added with `iliada tool add <name>` (settings.toml).
#
# Already present without this list: coreutils, sed, ps/free/top (Rust,
# see rust-userland.nix), util-linux (lscpu, lsblk, mount, fdisk), iproute2
# (ip, ss), ping, curl, wget, ssh, tar, gzip, xz, zstd, less, nano, htop.
{ config, lib, pkgs, ... }:
let
  cfg = config.iliada.baseTools;
in
{
  options.iliada.baseTools = {
    enable = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = "Install everyday command-line tools.";
    };
    packages = lib.mkOption {
      type = lib.types.listOf lib.types.package;
      default = with pkgs; [
        # Hardware
        pciutils # lspci
        usbutils # lsusb
        dmidecode
        lm_sensors # sensors
        smartmontools # smartctl
        nvme-cli
        hdparm
        # Disks and file systems
        parted
        gptfdisk # gdisk, sgdisk
        dosfstools
        exfatprogs
        ntfs3g
        # Network
        dnsutils # dig, nslookup, host
        net-tools # ifconfig, netstat, route (old, still widely used)
        traceroute
        mtr
        ethtool
        iw
        tcpdump
        whois
        wireguard-tools
        # Processes and files
        psmisc # killall, pstree, fuser
        lsof
        strace
        file
        tree
        rsync
        pv
        zip
        unzip
        p7zip
        # Text and data
        jq
        bc
        vim
        git
        tmux
        man-pages
        # Rust tools people install first anyway
        ripgrep # rg
        fd
        bat
        tealdeer # tldr
        dust
        btop
      ];
      defaultText = lib.literalMD "everyday tools: lspci, lsusb, sensors, dig, traceroute, lsof, killall, jq, vim, git, tmux, ripgrep, …";
      description = "The everyday command-line tools.";
    };
  };

  config = lib.mkIf cfg.enable {
    environment.systemPackages = cfg.packages;
  };
}
