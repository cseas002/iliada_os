# A mutable development system with all Iliada services and the desktop.
#
# Run it headless on a server and view it from another machine:
#   server$ nix run .#nixosConfigurations.dev-vm-x86_64.config.system.build.vm
#   mac$    ssh -N -L 5900:localhost:5900 personalServerLocal
#   mac$    open vnc://localhost:5900          (password: iliada)
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
    memorySize = 6144;
    cores = 4;
    diskSize = 16384;
    graphics = true;
    qemu.options = [
      # virtio GPU (2D; the guest renders with Mesa llvmpipe), no local window,
      # VNC bound to the host's loopback only: reach it through an SSH tunnel.
      "-vga none"
      "-device virtio-vga,xres=1600,yres=1000"
      "-display none"
      "-object secret,id=vncpw,data=iliada"
      "-vnc 127.0.0.1:0,password-secret=vncpw"
    ];
  };

  iliada = {
    applications.ingest = {
      enable = true;
      watchlist = ../../examples/watchlist.toml;
    };
    isolation.defaultTier = "micro-vm";
    broker.frontend = "deny";
    desktop = {
      enable = true;
      autoLoginUser = "iliada";
    };
    ux.hideFhs = true;
  };

  system.stateVersion = "25.11";
}
