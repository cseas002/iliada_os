# iliada-install: install Iliada onto a disk (UEFI): the whole disk, or the
# free space next to another system (--free-space).
#
#   sudo iliada-install --disk /dev/nvme0n1 --user alex --hostname alex-laptop
#
# Layout: GPT with a 1 GiB EFI system partition and an ext4 root. With
# --free-space, both go into the largest unpartitioned stretch of a GPT disk
# and nothing else on it is changed; Iliada gets its own EFI system partition,
# and the firmware's start-up menu reaches the other system.
# The installed system's configuration lives in /etc/iliada (a flake) and is
# applied with `sudo nixos-rebuild switch --flake /etc/iliada#iliada`.

SOURCE="${ILIADA_SOURCE:-/etc/iliada/source}"
TEMPLATE="$SOURCE/templates/default"
FREE=0 DISK="" USERNAME="iliada" FULLNAME="" HOSTNAME_="iliada" TIMEZONE="UTC" PASSWORD="" YES=0

die() { echo "iliada-install: $*" >&2; exit 1; }
step() { printf '\n\033[1m==> %s\033[0m\n' "$*"; }
usage() {
  cat <<USAGE
usage: iliada-install --disk DEVICE [--user NAME] [--fullname "Full Name"]
                      [--hostname NAME] [--timezone Area/City] [--password PW] [--yes]
                      [--free-space]

Erases DEVICE entirely and installs Iliada on it; with --free-space, installs
Iliada into the largest free space on DEVICE and keeps everything else.
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
    --free-space) FREE=1; shift ;;
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
  if [ "$FREE" -eq 1 ]; then
    printf '\nIliada goes into the free space on %s; nothing else on it changes. Type the device name to continue: ' "$DISK"
  else
    printf '\nALL DATA ON %s WILL BE ERASED. Type the device name to continue: ' "$DISK"
  fi
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

umount -R /mnt 2>/dev/null || true
if [ "$FREE" -eq 1 ]; then
  step "Making room on $DISK (everything on it stays)"
  sfdisk -d "$DISK" | grep -q '^label: gpt' || die "$DISK has no GPT partition table; Iliada can only go next to systems on GPT disks"
  # The largest free stretch, aligned (sgdisk -F: its first sector, -E: its last).
  first=$(sgdisk -F "$DISK" | tail -1) last=$(sgdisk -E "$DISK" | tail -1)
  sector=$(blockdev --getss "$DISK")
  free=$(( (last - first + 1) * sector ))
  [ "$free" -ge $(( 25 << 30 )) ] || die "only $(( free >> 30 )) GiB free on $DISK; Iliada needs 25 GiB"
  used=$(sfdisk -d "$DISK" | sed -n "s|^${DISK}[p]*\([0-9]*\) :.*|\1|p" | sort -n | tail -1)
  esp=$(( ${used:-0} + 1 )) root=$(( ${used:-0} + 2 ))
  sgdisk -n"$esp":"$first":+1G -t"$esp":EF00 -c"$esp":ILIADA-ESP -n"$root":0:0 -t"$root":8304 -c"$root":iliada "$DISK"
  partx -u "$DISK" || true
else
  step "Partitioning $DISK"
  wipefs -af "$DISK"
  sgdisk --zap-all "$DISK"
  esp=1 root=2
  sgdisk -n1:1M:+1G -t1:EF00 -c1:ILIADA-ESP -n2:0:0 -t2:8304 -c2:iliada "$DISK"
fi
udevadm settle
mkfs.fat -F 32 -n ILIADA-ESP "${P}${esp}"
mkfs.ext4 -F -q -L iliada "${P}${root}"
udevadm settle

step "Mounting"
mount "${P}${root}" /mnt
mkdir -p /mnt/boot
mount -o umask=0077 "${P}${esp}" /mnt/boot

step "Writing the configuration to /etc/iliada"
nixos-generate-config --root /mnt
# /etc/iliada: the configuration (a git repository: what gets backed up and
# synced), and the copy of Iliada it builds from.
iliada-systemd init --root /mnt --source "$SOURCE" --profile hardware \
  --user "$USERNAME" --full-name "$FULLNAME" --hostname "$HOSTNAME_" --timezone "$TIMEZONE"

step "Installing Iliada (this downloads and builds the system)"
nixos-install --root /mnt --flake git+file:///mnt/etc/iliada#iliada --no-root-passwd --no-channel-copy
iliada-systemd finish --root /mnt

step "Setting the password for $USERNAME"
printf '%s:%s\n' "$USERNAME" "$PASSWORD" | nixos-enter --root /mnt -c chpasswd

step "Done"
echo "Iliada is installed on $DISK. Remove the installation medium and reboot."
echo "Change it later in Settings, or with the iliada command."
