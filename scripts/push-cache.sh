#!/usr/bin/env bash
# Fill cache.iliada-os.com (Cloudflare R2) with what Iliada computers would
# otherwise build themselves, for one Iliada release or test update: an
# installed system is set up from its source archive exactly as the installer
# does it, built here, and every path of it that cache.nixos.org doesn't have
# is uploaded, signed. Computers then download instead of compiling.
#
#   bash scripts/push-cache.sh <iliada-…-source.tar.zst | unpacked source dir>
#
# Needs ~/.config/iliada/r2.env (R2_ACCOUNT_ID, R2_ACCESS_KEY_ID,
# R2_SECRET_ACCESS_KEY) and the signing key ~/.config/iliada/cache.key.
set -euo pipefail
export NIX_CONFIG="experimental-features = nix-command flakes"
key=~/.config/iliada/cache.key
[ -r "$key" ] || { echo "no signing key at $key"; exit 1; }
set -a; . ~/.config/iliada/r2.env; set +a
export AWS_ACCESS_KEY_ID=$R2_ACCESS_KEY_ID AWS_SECRET_ACCESS_KEY=$R2_SECRET_ACCESS_KEY

work=$(mktemp -d)
trap 'chmod -R u+w "$work" 2>/dev/null; rm -rf "$work"' EXIT
if [ -d "$1" ]; then
  src=$(realpath "$1")
else
  mkdir "$work/unpacked"
  zstd=$(nix build --no-link --print-out-paths nixpkgs#zstd.bin 2>/dev/null || nix build --no-link --print-out-paths nixpkgs#zstd)
  "$zstd/bin/zstd" -dc "$1" | tar -x -C "$work/unpacked"
  src=$(echo "$work"/unpacked/*)
fi
[ -e "$src/templates/default/flake.nix" ] || { echo "$src is not an Iliada source"; exit 1; }
echo "Iliada $(cat "$src/VERSION")"

# An installed system, set up as the installer does (a typical PC).
root="$work/root"
mkdir -p "$root/etc/nixos"
cat > "$root/etc/nixos/hardware-configuration.nix" <<'EOF'
{ lib, ... }: {
  fileSystems."/" = { device = "/dev/disk/by-label/iliada"; fsType = "ext4"; };
  fileSystems."/boot" = { device = "/dev/disk/by-label/ILIADA-ESP"; fsType = "vfat"; };
  boot.initrd.availableKernelModules = [ "xhci_pci" "ahci" "nvme" "usb_storage" "sd_mod" ];
  nixpkgs.hostPlatform = lib.mkDefault "x86_64-linux";
}
EOF
tools=$(nix build --no-link --print-out-paths "path:$src#packages.x86_64-linux.iliada-tools")
"$tools/bin/iliada-systemd" init --root "$root" --source "$src" --profile hardware \
  --user person --hostname iliada >/dev/null
sed -i "s|path:/etc/iliada/source|path:$root/etc/iliada/source|" "$root/etc/iliada/flake.nix"
system=$(nix build --no-link --print-out-paths \
  "path:$root/etc/iliada#nixosConfigurations.iliada.config.system.build.toplevel")
echo "built $system"

# What cache.nixos.org doesn't have (asked 16 at a time).
missing="$work/missing"
nix path-info -r "$system" | xargs -P 16 -n 1 sh -c '
  h=$(basename "$0" | cut -d- -f1)
  [ "$(curl -s -o /dev/null -w "%{http_code}" "https://cache.nixos.org/$h.narinfo")" = 200 ] || echo "$0"
' > "$missing"
echo "$(wc -l < "$missing") paths to upload"

# Packed and signed locally first, only the missing paths (--no-recursive:
# without it `nix copy` packs the whole system, 1,400 paths, for nothing),
# then they go up: their .narinfo and NAR.
# What they depend on comes from cache.nixos.org, as it does for everyone.
local_cache="$work/cache"
# shellcheck disable=SC2046
nix copy --no-recursive --to "file://$local_cache?compression=zstd&secret-key=$key" $(cat "$missing")
aws=$(nix build --no-link --print-out-paths nixpkgs#awscli2 | head -1)/bin/aws
endpoint="https://$R2_ACCOUNT_ID.r2.cloudflarestorage.com"
export AWS_DEFAULT_REGION=auto
up=0
while read -r p; do
  h=$(basename "$p" | cut -d- -f1)
  nar=$(sed -n 's/^URL: //p' "$local_cache/$h.narinfo")
  "$aws" s3 cp --quiet "$local_cache/$nar" "s3://iliada-cache/$nar" --endpoint-url "$endpoint"
  "$aws" s3 cp --quiet "$local_cache/$h.narinfo" "s3://iliada-cache/$h.narinfo" \
    --content-type text/x-nix-narinfo --endpoint-url "$endpoint"
  up=$((up + $(stat -c %s "$local_cache/$nar")))
done < "$missing"
echo "uploaded $(wc -l < "$missing") paths, $((up / 1048576)) MB"
echo "cache.iliada-os.com has Iliada $(cat "$src/VERSION")"
