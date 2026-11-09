# Everything a physical (or virtual) desktop computer needs beyond Iliada's
# own services: networking, sound, bluetooth, firmware, boot loader.
{ config, lib, ... }:
let
  cfg = config.iliada.workstation;
in
{
  options.iliada.workstation.enable = lib.mkOption {
    type = lib.types.bool;
    default = config.iliada.desktop.enable;
    defaultText = lib.literalExpression "config.iliada.desktop.enable";
    description = "Sensible defaults for a desktop computer.";
  };

  config = lib.mkIf cfg.enable {
    boot.loader.systemd-boot.enable = lib.mkDefault true;
    boot.loader.efi.canTouchEfiVariables = lib.mkDefault true;
    hardware.enableRedistributableFirmware = lib.mkDefault true;
    networking.networkmanager.enable = lib.mkDefault true;
    services.pipewire = {
      enable = lib.mkDefault true;
      alsa.enable = lib.mkDefault true;
      pulse.enable = lib.mkDefault true;
    };
    security.rtkit.enable = lib.mkDefault true;
    hardware.bluetooth.enable = lib.mkDefault true;
    services.fwupd.enable = lib.mkDefault true;
    zramSwap.enable = lib.mkDefault true;
    nix.settings.experimental-features = [ "nix-command" "flakes" ];
  };
}
