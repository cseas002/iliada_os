# Your Iliada system. Edit configuration.nix, then apply it with:
#   sudo nixos-rebuild switch --flake /etc/iliada#iliada
{
  description = "My Iliada system";

  inputs.iliada.url = "github:cseas002/iliada_os";

  outputs = { iliada, ... }: {
    nixosConfigurations.iliada = iliada.lib.iliadaSystem {
      system = "x86_64-linux";
      modules = [
        ./hardware-configuration.nix
        ./configuration.nix
      ];
    };
  };
}
