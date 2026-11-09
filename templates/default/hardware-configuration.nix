# Placeholder. Generate the real one for this machine with
#   sudo nixos-generate-config --show-hardware-config > hardware-configuration.nix
# (iliada-install does this for you.)
{
  fileSystems."/" = {
    device = "/dev/disk/by-label/iliada";
    fsType = "ext4";
  };
  fileSystems."/boot" = {
    device = "/dev/disk/by-label/ILIADA-ESP";
    fsType = "vfat";
    options = [ "umask=0077" ];
  };
}
