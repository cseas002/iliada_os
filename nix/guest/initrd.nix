# The initramfs of every app micro-VM: iliada-guest-init as /init, modprobe
# `ip` and the few kernel modules it loads. Everything else (bubblewrap, waypipe,
# the runtime base, Mesa) comes from the host's /nix/store, shared read-only.
{ lib, makeInitrdNG, makeModulesClosure, kmod, iproute2, modulesTree, iliadaPackage }:
let
  modules = makeModulesClosure {
    kernel = modulesTree;
    firmware = modulesTree;
    allowMissing = true;
    rootModules = [ "virtiofs" "vmw_vsock_virtio_transport" "virtio_rng" "virtio_console" "virtio_pci" "virtio_net" ];
  };
in
makeInitrdNG {
  name = "iliada-guest-initrd";
  compressor = "zstd";
  contents = [
    { source = "${iliadaPackage}/bin/iliada-guest-init"; target = "/init"; }
    { source = "${kmod}/bin/kmod"; target = "/bin/modprobe"; }
    # Brings up the loopback and, for apps with a network, the virtio-net card.
    { source = "${iproute2}/bin/ip"; target = "/bin/ip"; }
    { source = "${modules}/lib/modules"; target = "/lib/modules"; }
  ];
}
