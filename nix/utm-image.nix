# Turn the raw repart image into a compact qcow2 with a 40 GB virtual disk
# (the root partition grows into the free space on first boot).
{ runCommand, qemu-utils, image }:
runCommand "iliada-utm-image" { nativeBuildInputs = [ qemu-utils ]; } ''
  mkdir -p $out
  qemu-img convert -O qcow2 -c ${image}/iliada*.raw $out/iliada.qcow2
  qemu-img resize $out/iliada.qcow2 40G
''
