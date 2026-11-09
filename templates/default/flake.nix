# Your Iliada system. Edit configuration.nix, then apply it with:
#   iliada system apply
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
