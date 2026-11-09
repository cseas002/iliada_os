# A virtual machine started from the ready-to-run Iliada image (UTM, QEMU,
# Proxmox): its disks (by partition label), boot loader and guest tools.
# The image itself and the installed configuration both use it.
{ lib, modulesPath, ... }:
{
  imports = [ "${modulesPath}/profiles/qemu-guest.nix" ];

  boot.loader.systemd-boot.enable = true;
  boot.loader.grub.enable = false;
  boot.loader.efi.canTouchEfiVariables = lib.mkForce false;
  boot.growPartition = true;
  fileSystems."/" = {
    device = "/dev/disk/by-partlabel/iliada-root";
    fsType = "ext4";
    autoResize = true;
  };
  fileSystems."/boot" = {
    device = "/dev/disk/by-partlabel/ESP";
    fsType = "vfat";
    options = [ "umask=0077" ];
  };

  # Clipboard sharing and window resizing (UTM / QEMU).
  services.spice-vdagentd.enable = true;
  services.qemuGuest.enable = true;
}
