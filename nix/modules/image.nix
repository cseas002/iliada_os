# Immutable delivery: the system closure becomes a read-only erofs image whose
# integrity is enforced by dm-verity, booted through a UKI, with A/B slots
# updated by systemd-sysupdate. Root (/) is a tmpfs; persistent state lives
# only on explicitly declared volumes (/Applications, /home, /var/lib).
{ config, lib, pkgs, modulesPath, ... }:
let
  cfg = config.iliada.image;
  inherit (config.image.repart.verityStore) partitionIds;
  efiArch = config.nixpkgs.hostPlatform.efiArch;
in
{
  imports = [
    "${modulesPath}/image/repart.nix"
  ];

  options.iliada.image = {
    enable = lib.mkEnableOption "the immutable Iliada appliance image";
    version = lib.mkOption {
      type = lib.types.str;
      default = "0.1.0";
      description = "Image version; sysupdate compares it to pick the newest slot.";
    };
    updateUrl = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      example = "https://updates.iliada.example/stable/x86_64";
      description = "Where systemd-sysupdate fetches new UKIs and store images. null disables online updates.";
    };
  };

  config = lib.mkIf cfg.enable {
    system.image = {
      id = "iliada";
      version = cfg.version;
    };

    boot.loader.grub.enable = false;
    # Nothing in the image may be mutated in place.
    system.activationScripts.usrbinenv = lib.mkForce "";

    image.repart = {
      enable = true;
      name = "iliada";
      # The verityStore module builds the erofs store + hash tree partitions,
      # embeds the root hash in the UKI command line, and mounts /usr and
      # /nix/store from the verified device. We only size the ESP.
      verityStore = {
        enable = true;
        ukiPath = "/EFI/BOOT/BOOT${lib.toUpper efiArch}.EFI";
      };
      partitions.${partitionIds.esp}.repartConfig = {
        Type = "esp";
        Format = "vfat";
        SizeMinBytes = "256M";
      };
    };

    # Root is a tmpfs: nothing outside declared volumes survives a reboot.
    fileSystems."/" = {
      fsType = "tmpfs";
      options = [ "mode=0755" "size=50%" ];
    };

    # A/B: sysupdate writes the inactive slot and a new UKI; systemd-boot's
    # boot counting falls back to the old UKI if the new one fails to boot.
    systemd.sysupdate = lib.mkIf (cfg.updateUrl != null) {
      enable = true;
      reboot.enable = false;
      transfers = {
        "10-uki" = {
          Source = {
            Type = "url-file";
            Path = cfg.updateUrl;
            MatchPattern = "iliada_@v.efi";
          };
          Target = {
            Type = "regular-file";
            Path = "/EFI/Linux";
            PathRelativeTo = "boot";
            MatchPattern = "iliada_@v+@l-@d.efi iliada_@v+@l.efi iliada_@v.efi";
            Mode = "0444";
            TriesLeft = 3;
            TriesDone = 0;
            InstancesMax = 2;
          };
        };
        "20-store-verity" = {
          Source = {
            Type = "url-file";
            Path = cfg.updateUrl;
            MatchPattern = "iliada_@v_@u.verity";
          };
          Target = {
            Type = "partition";
            Path = "auto";
            MatchPattern = "verity-@v";
            MatchPartitionType = "usr-verity";
            ReadOnly = 1;
          };
        };
        "30-store" = {
          Source = {
            Type = "url-file";
            Path = cfg.updateUrl;
            MatchPattern = "iliada_@v_@u.store";
          };
          Target = {
            Type = "partition";
            Path = "auto";
            MatchPattern = "store-@v";
            MatchPartitionType = "usr";
            ReadOnly = 1;
          };
        };
      };
    };
  };
}
