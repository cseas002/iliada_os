# The initramfs of every app micro-VM: iliada-guest-init as /init, modprobe
# and the few kernel modules it loads. Everything else (bubblewrap, waypipe,
# the runtime base, Mesa) comes from the host's /nix/store, shared read-only.
{ lib, makeInitrdNG, makeModulesClosure, kmod, modulesTree, iliadaPackage }:
let
  modules = makeModulesClosure {
    kernel = modulesTree;
    firmware = modulesTree;
    allowMissing = true;
    rootModules = [ "virtiofs" "vmw_vsock_virtio_transport" "virtio_rng" "virtio_console" "virtio_pci" ];
  };
in
makeInitrdNG {
  name = "iliada-guest-initrd";
  compressor = "zstd";
  contents = [
    { source = "${iliadaPackage}/bin/iliada-guest-init"; target = "/init"; }
    { source = "${kmod}/bin/kmod"; target = "/bin/modprobe"; }
    { source = "${modules}/lib/modules"; target = "/lib/modules"; }
  ];
}
