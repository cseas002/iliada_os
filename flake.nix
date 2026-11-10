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
        # `iliada`: this flake, for modules that need its source or profiles.
        specialArgs = { iliada = self; } // specialArgs;
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
        let
          system = pkgs.stdenv.hostPlatform.system;
          utmImage = host: pkgs.callPackage ./nix/utm-image.nix {
            image =
              let image = self.nixosConfigurations."${host}-${arch system}".config.system.build.image; in
              # Built on an x86 machine, the ARM image runs under emulation,
              # where `unshare --map-root-user` fails; fakeroot alone gives
              # the files their owners.
              if system == "aarch64-linux" then
                image.overrideAttrs (old: {
                  buildPhase = lib.replaceStrings [ "unshare --map-root-user fakeroot" ] [ "fakeroot" ] old.buildPhase;
                })
              else image;
          };
        in
        rec {
          iliada-tools = pkgs.callPackage ./nix/package.nix { };
          default = iliada-tools;
        }
        // lib.optionalAttrs (lib.elem system linuxSystems) {
          # The Store, Settings and the Setup Assistant (libcosmic).
          iliada-desktop-apps = pkgs.callPackage ./nix/desktop-apps.nix { };
        }
        // lib.optionalAttrs (lib.elem system linuxSystems) {
          # Bootable live/installer ISO.
          installer-iso = self.nixosConfigurations."installer-${arch system}".config.system.build.isoImage;
          # Ready-to-run VM disk (UTM on Apple Silicon: the aarch64 one).
          utm-image = utmImage "utm";
          # The same, reachable and updatable from the developer's Mac.
          utm-dev-image = utmImage "utm-dev";
        });

      overlays.default = final: _prev: { iliada-tools = final.callPackage ./nix/package.nix { }; };

      # The declarative system manifest: `iliada.*` options.
      nixosModules.default = import ./nix/modules;
      # A VM started from the ready-to-run image (disks, boot, guest tools).
      nixosModules.vm-guest = import ./nix/profiles/vm-guest.nix;
      # Developer SSH access to a test VM (keys in nix/hosts/dev-keys/).
      nixosModules.dev-access = import ./nix/profiles/dev-access.nix;

      # dev-vm-*:    mutable dev system; `nix run .#nixosConfigurations.dev-vm-x86_64.config.system.build.vm`
      # appliance-*: the immutable image (dm-verity store, A/B updates);
      #              `nix build .#nixosConfigurations.appliance-x86_64.config.system.build.image`
      nixosConfigurations = perLinux "dev-vm" [ ./nix/hosts/dev-vm.nix ]
        // perLinux "appliance" [ ./nix/hosts/appliance.nix ]
        // perLinux "utm" [ ./nix/hosts/utm.nix ]
        // perLinux "utm-dev" [ ./nix/hosts/utm-dev.nix ]
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
