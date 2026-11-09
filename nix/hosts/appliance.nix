# The immutable appliance image.
#   nix build .#nixosConfigurations.appliance-x86_64.config.system.build.image
{ ... }:
{
  networking.hostName = "iliada";
  iliada = {
    image.enable = true;
    applications.device = "/dev/disk/by-partlabel/iliada-applications";
    ux.noSudo = true;
  };
  users.users.iliada = {
    isNormalUser = true;
    extraGroups = [ "iliada-apps" "iliada-sandbox" "kvm" ];
    initialPassword = "iliada";
  };
  system.stateVersion = "25.11";
}
