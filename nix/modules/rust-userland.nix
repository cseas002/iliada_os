# Rust userland: wherever a working Rust implementation exists, Iliada uses it.
#
# The uutils tools take precedence on the interactive PATH. Scripts inside
# the Nix store keep the exact (GNU) tools they were built against, since
# their paths are pinned; that is what keeps system services reproducible.
#
# Still C (no production-ready Rust replacement yet): the kernel, glibc,
# systemd, Mesa, PipeWire, NetworkManager, bash (/bin/sh for scripts).
# Not yet included: uutils-findutils and uutils-tar (do not build at the
# pinned nixpkgs revision).
{ config, lib, pkgs, ... }:
let
  cfg = config.iliada.rustUserland;
in
{
  options.iliada.rustUserland = {
    enable = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = "Prefer Rust implementations of core userland tools.";
    };
    tools = lib.mkOption {
      type = lib.types.listOf lib.types.package;
      default = with pkgs; [
        uutils-coreutils-noprefix # ls cp mv cat chmod … (Ubuntu's default since 25.10)
        uutils-diffutils # diff cmp
        uutils-sed # sed
        uutils-procps # ps pgrep pkill free …
        uutils-util-linux # dmesg lscpu hexdump …
        uutils-hostname # hostname
      ];
      defaultText = lib.literalExpression "with pkgs; [ uutils-coreutils-noprefix uutils-diffutils uutils-sed uutils-procps uutils-util-linux uutils-hostname ]";
      description = "Rust tools placed ahead of their GNU counterparts on the interactive PATH.";
    };
    shell = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = "Use fish (rewritten in Rust as of 4.0) as the default login shell.";
    };
  };

  config = lib.mkIf cfg.enable {
    environment.systemPackages = map lib.hiPrio cfg.tools;

    # sudo-rs replaces C sudo (unless sudo is removed entirely).
    security.sudo-rs.enable = lib.mkDefault (!config.iliada.ux.noSudo);

    # ntpd-rs replaces systemd-timesyncd.
    services.ntpd-rs.enable = lib.mkDefault true;
    services.timesyncd.enable = lib.mkDefault false;

    programs.fish.enable = lib.mkIf cfg.shell true;
    users.defaultUserShell = lib.mkIf cfg.shell pkgs.fish;
  };
}
