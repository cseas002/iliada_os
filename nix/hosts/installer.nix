# The Iliada live/installer ISO: a COSMIC live session plus `iliada-install`.
#   nix build .#installer-iso
{ config, lib, pkgs, modulesPath, iliadaSource, ... }:
let
  iliada-install = pkgs.writeShellApplication {
    name = "iliada-install";
    runtimeInputs = with pkgs; [
      coreutils
      util-linux
      gptfdisk
      dosfstools
      e2fsprogs
      gnused
      nixos-install-tools
      config.nix.package
    ];
    text = builtins.readFile ../installer/iliada-install.sh;
  };
in
{
  imports = [ "${modulesPath}/installer/cd-dvd/installation-cd-graphical-base.nix" ];

  networking.hostName = "iliada-installer";
  iliada = {
    desktop = {
      enable = true;
      autoLoginUser = "nixos";
    };
    applications.ingest.enable = false;
  };
  # The ISO has its own boot loader.
  boot.loader.systemd-boot.enable = lib.mkForce false;
  boot.loader.efi.canTouchEfiVariables = lib.mkForce false;
  # Also log in on the serial console (used by the automated install test).
  boot.kernelParams = [ "console=tty0" "console=ttyS0,115200" ];

  # The Iliada source the installed system is built from.
  environment.etc."iliada/source".source = iliadaSource;
  environment.systemPackages = [ iliada-install ];

  services.getty.helpLine = lib.mkForce ''

    Welcome to the Iliada installer.
    Install with:  sudo iliada-install --disk /dev/<disk> --user <name>
    List disks:    lsblk
  '';
}
