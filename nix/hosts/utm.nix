# A ready-to-run Iliada disk image for virtual machines: UTM on Apple Silicon
# (aarch64), or QEMU/UTM/Proxmox on x86_64. No installer: download, open, boot.
#
#   nix build .#packages.aarch64-linux.utm-image   ->  result/iliada.qcow2
#
# Built with systemd-repart (no VM needed at build time, so it can be built
# for ARM on an x86 machine). UEFI: systemd-boot + a unified kernel image.
{ config, lib, pkgs, modulesPath, ... }:
let
  efiArch = config.nixpkgs.hostPlatform.efiArch;
  toplevel = config.system.build.toplevel;
in
{
  imports = [
    "${modulesPath}/image/repart.nix"
    ../profiles/vm-guest.nix
  ];

  networking.hostName = "iliada";
  users.users.iliada = {
    isNormalUser = true;
    description = "Iliada";
    extraGroups = [ "wheel" "iliada-apps" "networkmanager" ];
    initialPassword = "iliada";
  };

  iliada = {
    desktop = {
      enable = true;
      autoLoginUser = "iliada";
    };
    settingsFile = ../../templates/default/settings.toml;
    # First boot writes /etc/iliada (like the installer does), so Settings
    # and `iliada` can change and update this system.
    system.firstBoot = {
      enable = true;
      user = "iliada";
      fullName = "Iliada";
      autoLogin = true;
    };
  };

  # First boot: register the store paths with Nix and create the system
  # profile, so the system can be updated like an installed one.
  boot.postBootCommands = ''
    if [ -f /nix-path-registration ]; then
      ${config.nix.package.out}/bin/nix-store --load-db < /nix-path-registration
      touch /etc/NIXOS
      ${config.nix.package.out}/bin/nix-env -p /nix/var/nix/profiles/system --set /run/current-system
      rm -f /nix-path-registration
    fi
  '';

  image.repart = {
    enable = true;
    name = "iliada";
    sectorSize = 512; # UEFI firmware in QEMU/UTM expects 512-byte sectors
    partitions = {
      # Sorted by name: the ESP first, the root last (so it can grow).
      "10-esp" = {
        contents = {
          "/EFI/BOOT/BOOT${lib.toUpper efiArch}.EFI".source =
            "${config.systemd.package}/lib/systemd/boot/efi/systemd-boot${efiArch}.efi";
          "/EFI/Linux/${config.system.boot.loader.ukiFile}".source =
            "${config.system.build.uki}/${config.system.boot.loader.ukiFile}";
        };
        repartConfig = {
          Type = "esp";
          Format = "vfat";
          Label = "ESP";
          SizeMinBytes = "512M";
        };
      };
      "20-root" = {
        storePaths = [ toplevel ];
        contents."/nix-path-registration".source = "${pkgs.closureInfo { rootPaths = [ toplevel ]; }}/registration";
        repartConfig = {
          Type = "root";
          Format = "ext4";
          Label = "iliada-root";
          Minimize = "guess";
        };
      };
    };
  };

  system.stateVersion = "26.11";
}
