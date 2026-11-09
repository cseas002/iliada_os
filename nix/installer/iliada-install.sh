# iliada-install: install Iliada onto a disk (UEFI, whole disk).
#
#   sudo iliada-install --disk /dev/nvme0n1 --user alex --hostname alex-laptop
#
# Layout: GPT with a 1 GiB EFI system partition and an ext4 root.
# The installed system's configuration lives in /etc/iliada (a flake) and is
# applied with `sudo nixos-rebuild switch --flake /etc/iliada#iliada`.

SOURCE="${ILIADA_SOURCE:-/etc/iliada/source}"
TEMPLATE="$SOURCE/templates/default"
DISK="" USERNAME="iliada" FULLNAME="" HOSTNAME_="iliada" TIMEZONE="UTC" PASSWORD="" YES=0

die() { echo "iliada-install: $*" >&2; exit 1; }
step() { printf '\n\033[1m==> %s\033[0m\n' "$*"; }
usage() {
  cat <<USAGE
usage: iliada-install --disk DEVICE [--user NAME] [--fullname "Full Name"]
                      [--hostname NAME] [--timezone Area/City] [--password PW] [--yes]

Erases DEVICE entirely and installs Iliada on it.
USAGE
  exit 2
}

while [ $# -gt 0 ]; do
  case "$1" in
    --disk) DISK="$2"; shift 2 ;;
    --user) USERNAME="$2"; shift 2 ;;
    --fullname) FULLNAME="$2"; shift 2 ;;
    --hostname) HOSTNAME_="$2"; shift 2 ;;
    --timezone) TIMEZONE="$2"; shift 2 ;;
    --password) PASSWORD="$2"; shift 2 ;;
    --yes) YES=1; shift ;;
    -h | --help) usage ;;
    *) die "unknown argument: $1" ;;
  esac
done

[ -n "$DISK" ] || usage
[ "$(id -u)" -eq 0 ] || die "run as root (sudo iliada-install ...)"
[ -d /sys/firmware/efi ] || die "this machine did not boot in UEFI mode; Iliada needs UEFI"
[ -b "$DISK" ] || die "$DISK is not a block device"
[ -d "$TEMPLATE" ] || die "Iliada source not found at $SOURCE"
[[ "$USERNAME" =~ ^[a-z_][a-z0-9_-]{0,30}$ ]] || die "invalid user name: $USERNAME"
[[ "$HOSTNAME_" =~ ^[a-zA-Z0-9-]{1,63}$ ]] || die "invalid host name: $HOSTNAME_"
[ -e "/etc/zoneinfo/$TIMEZONE" ] || [ -e "$(dirname "$(readlink -f /etc/localtime)")/$TIMEZONE" ] || echo "warning: unknown time zone $TIMEZONE"
FULLNAME="${FULLNAME:-$USERNAME}"

if [ "$YES" -ne 1 ]; then
  lsblk -o NAME,SIZE,MODEL "$DISK"
  printf '\nALL DATA ON %s WILL BE ERASED. Type the device name to continue: ' "$DISK"
  read -r answer
  [ "$answer" = "$DISK" ] || die "aborted"
fi
if [ -z "$PASSWORD" ]; then
  read -r -s -p "Password for $USERNAME: " PASSWORD; echo
  read -r -s -p "Repeat password: " again; echo
  [ "$PASSWORD" = "$again" ] || die "passwords do not match"
fi
[ -n "$PASSWORD" ] || die "empty password"

case "$DISK" in
  *[0-9]) P="${DISK}p" ;; # nvme0n1 -> nvme0n1p1, mmcblk0 -> mmcblk0p1
  *) P="$DISK" ;;
esac

step "Partitioning $DISK"
umount -R /mnt 2>/dev/null || true
wipefs -af "$DISK"
sgdisk --zap-all "$DISK"
sgdisk -n1:1M:+1G -t1:EF00 -c1:ILIADA-ESP -n2:0:0 -t2:8304 -c2:iliada "$DISK"
udevadm settle
mkfs.fat -F 32 -n ILIADA-ESP "${P}1"
mkfs.ext4 -F -q -L iliada "${P}2"
udevadm settle

step "Mounting"
mount "${P}2" /mnt
mkdir -p /mnt/boot
mount -o umask=0077 "${P}1" /mnt/boot

step "Writing the configuration to /etc/iliada"
nixos-generate-config --root /mnt
mkdir -p /mnt/etc/iliada
cp -rL "$SOURCE" /mnt/etc/iliada/source
chmod -R u+w /mnt/etc/iliada/source
cp "$TEMPLATE/flake.nix" "$TEMPLATE/configuration.nix" "$TEMPLATE/apps.toml" /mnt/etc/iliada/
mv /mnt/etc/nixos/hardware-configuration.nix /mnt/etc/iliada/
rm -rf /mnt/etc/nixos
system="$(uname -m)-linux"
# The installed system builds from the copy of Iliada it was installed with.
# During installation that copy is at /mnt/etc/iliada/source (a real
# directory; the live system's /etc/iliada/source is a symlink Nix refuses);
# afterwards the path is rewritten to what the installed system sees.
sed -i -e 's|inputs.iliada.url = .*|inputs.iliada.url = "path:/mnt/etc/iliada/source";|' \
  -e "s|system = \"x86_64-linux\";|system = \"$system\";|" /mnt/etc/iliada/flake.nix
sed -i -e "s|networking.hostName = \"iliada\";|networking.hostName = \"$HOSTNAME_\";|" \
  -e "s|time.timeZone = \"UTC\";.*|time.timeZone = \"$TIMEZONE\";|" \
  -e "s|users.users.iliada = {|users.users.$USERNAME = {|" \
  -e "s|description = \"Iliada User\";|description = \"${FULLNAME//\"/}\";|" \
  -e '/initialPassword = /d' /mnt/etc/iliada/configuration.nix

step "Installing Iliada (this downloads and builds the system)"
# Lock first: creating flake.lock during the install would change the flake
# directory after Nix has hashed it.
nix --extra-experimental-features "nix-command flakes" flake lock /mnt/etc/iliada
nixos-install --root /mnt --flake /mnt/etc/iliada#iliada --no-root-passwd --no-channel-copy

sed -i 's|path:/mnt/etc/iliada/source|path:/etc/iliada/source|' /mnt/etc/iliada/flake.nix
[ -f /mnt/etc/iliada/flake.lock ] && sed -i 's|"/mnt/etc/iliada/source"|"/etc/iliada/source"|g' /mnt/etc/iliada/flake.lock

step "Setting the password for $USERNAME"
printf '%s:%s\n' "$USERNAME" "$PASSWORD" | nixos-enter --root /mnt -c chpasswd

step "Done"
echo "Iliada is installed on $DISK. Remove the installation medium and reboot."
echo "Your configuration: /etc/iliada (apply changes with: sudo nixos-rebuild switch --flake /etc/iliada#iliada)"
