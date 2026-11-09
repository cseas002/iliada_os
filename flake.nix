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

      # An Iliada system: NixOS from Iliada's pinned nixpkgs, plus the iliada.* modules.
      iliadaSystem = { system, modules ? [ ], specialArgs ? { } }: lib.nixosSystem {
        inherit specialArgs;
        modules = [ { nixpkgs.hostPlatform = system; } self.nixosModules.default ] ++ modules;
      };
      mkSystem = system: modules: iliadaSystem { inherit system modules; };
      perLinux = prefix: modules: lib.listToAttrs (map
        (system: lib.nameValuePair "${prefix}-${arch system}" (mkSystem system modules))
        linuxSystems);
      installers = lib.listToAttrs (map
        (system: lib.nameValuePair "installer-${arch system}" (iliadaSystem {
          inherit system;
          modules = [ ./nix/hosts/installer.nix ];
          specialArgs.iliadaSource = self;
        }))
        linuxSystems);
    in
    {
      # Rust daemons and tools (iliada-broker, iliada-ingest).
      # For your own system flake: iliada.lib.iliadaSystem { system; modules; }
      lib = { inherit iliadaSystem; };

      # `nix flake init -t github:cseas002/iliada_os` -> flake.nix, configuration.nix, apps.toml
      templates.default = {
        path = ./templates/default;
        description = "An Iliada system configuration";
        welcomeText = ''
          # Your Iliada configuration
          Edit `configuration.nix` (the system) and `apps.toml` (your apps), then run
          `sudo nixos-generate-config --show-hardware-config > hardware-configuration.nix`
          and `sudo nixos-rebuild switch --flake .#iliada`.
        '';
      };

      packages = forAll allSystems (pkgs:
        let system = pkgs.stdenv.hostPlatform.system; in
        rec {
          iliada-tools = pkgs.callPackage ./nix/package.nix { };
          default = iliada-tools;
        }
        // lib.optionalAttrs (lib.elem system linuxSystems) {
          # Bootable live/installer ISO.
          installer-iso = self.nixosConfigurations."installer-${arch system}".config.system.build.isoImage;
        });

      overlays.default = final: _prev: { iliada-tools = final.callPackage ./nix/package.nix { }; };

      # The declarative system manifest: `iliada.*` options.
      nixosModules.default = import ./nix/modules;

      # dev-vm-*:    mutable dev system; `nix run .#nixosConfigurations.dev-vm-x86_64.config.system.build.vm`
      # appliance-*: the immutable image (dm-verity store, A/B updates);
      #              `nix build .#nixosConfigurations.appliance-x86_64.config.system.build.image`
      nixosConfigurations = perLinux "dev-vm" [ ./nix/hosts/dev-vm.nix ]
        // perLinux "appliance" [ ./nix/hosts/appliance.nix ]
        // installers;

      devShells = forAll allSystems (pkgs: {
        default = pkgs.mkShell {
          packages = with pkgs; [ cargo rustc rustfmt clippy rust-analyzer ]
            # glib.dev: glib-compile-schemas for the Package Translator / catalog builder.
            ++ lib.optionals pkgs.stdenv.hostPlatform.isLinux [ patchelf cloud-hypervisor virtiofsd bubblewrap glib.dev ];
        };
      });

      checks = forAll linuxSystems (pkgs: {
        # Builds the workspace and runs its test suite inside the Nix sandbox.
        iliada-tools = self.packages.${pkgs.stdenv.hostPlatform.system}.iliada-tools;
      });

      formatter = forAll allSystems (pkgs: pkgs.nixfmt);
    };
}
