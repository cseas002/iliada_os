# A mutable development system with all Iliada services enabled.
#   nix run .#nixosConfigurations.dev-vm-x86_64.config.system.build.vm
{ lib, ... }:
{
  networking.hostName = "iliada-dev";
  boot.loader.systemd-boot.enable = true;
  fileSystems."/" = {
    device = "/dev/disk/by-label/nixos";
    fsType = "ext4";
  };

  users.users.iliada = {
    isNormalUser = true;
    extraGroups = [ "wheel" "iliada-apps" "iliada-sandbox" "kvm" ];
    initialPassword = "iliada";
  };

  virtualisation.vmVariant.virtualisation = {
    memorySize = 4096;
    cores = 4;
  };

  iliada = {
    applications.ingest = {
      enable = true;
      watchlist = ../../examples/watchlist.toml;
    };
    isolation.defaultTier = "micro-vm";
    broker.frontend = "deny";
    ux.hideFhs = true;
  };

  system.stateVersion = "25.11";
}
