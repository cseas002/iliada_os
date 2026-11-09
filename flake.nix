{
  description = "Iliada: an immutable, micro-isolated Linux OS with a Mac-like app model";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { self, nixpkgs }:
    let
      lib = nixpkgs.lib;
      linuxSystems = [ "x86_64-linux" "aarch64-linux" ];
      allSystems = linuxSystems ++ [ "aarch64-darwin" "x86_64-darwin" ];
      forAll = systems: f: lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
      arch = system: builtins.head (lib.splitString "-" system);

      mkSystem = system: extraModules: lib.nixosSystem {
        modules = [ { nixpkgs.hostPlatform = system; } self.nixosModules.default ] ++ extraModules;
      };
      perLinux = prefix: modules: lib.listToAttrs (map
        (system: lib.nameValuePair "${prefix}-${arch system}" (mkSystem system modules))
        linuxSystems);
    in
    {
      # Rust daemons and tools (iliada-broker, iliada-ingest).
      packages = forAll allSystems (pkgs: rec {
        iliada-tools = pkgs.callPackage ./nix/package.nix { };
        default = iliada-tools;
      });

      overlays.default = final: _prev: { iliada-tools = final.callPackage ./nix/package.nix { }; };

      # The declarative system manifest: `iliada.*` options.
      nixosModules.default = import ./nix/modules;

      # dev-vm-*:    mutable dev system; `nix run .#nixosConfigurations.dev-vm-x86_64.config.system.build.vm`
      # appliance-*: the immutable image (dm-verity store, A/B updates);
      #              `nix build .#nixosConfigurations.appliance-x86_64.config.system.build.image`
      nixosConfigurations = perLinux "dev-vm" [ ./nix/hosts/dev-vm.nix ]
        // perLinux "appliance" [ ./nix/hosts/appliance.nix ];

      devShells = forAll allSystems (pkgs: {
        default = pkgs.mkShell {
          packages = with pkgs; [ cargo rustc rustfmt clippy rust-analyzer ]
            ++ lib.optionals pkgs.stdenv.hostPlatform.isLinux [ patchelf cloud-hypervisor virtiofsd bubblewrap ];
        };
      });

      checks = forAll linuxSystems (pkgs: {
        # Builds the workspace and runs its test suite inside the Nix sandbox.
        iliada-tools = self.packages.${pkgs.stdenv.hostPlatform.system}.iliada-tools;
      });

      formatter = forAll allSystems (pkgs: pkgs.nixfmt);
    };
}
