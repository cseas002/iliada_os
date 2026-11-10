#!/usr/bin/env bash
# Update a running test VM (the utm-dev image) to this checkout, in place.
#
#   scripts/dev-push.sh [host]        default: iliada.local
#
# Copies the tracked files of this checkout (with uncommitted edits) to the
# VM's /etc/iliada/source, relocks and rebuilds there (natively, so ARM
# builds run at full speed on an Apple Silicon Mac), and records the update
# in /etc/iliada's history. The tester's apps, settings and files stay; if
# the build fails, nothing changes.
set -euo pipefail
HOST="${1:-iliada.local}"
cd "$(dirname "$0")/.."
REV="$(git rev-parse --short HEAD)$([ -n "$(git status --porcelain)" ] && echo +edits || true)"
SSH=(ssh -o StrictHostKeyChecking=accept-new -o ConnectTimeout=10 "root@$HOST")
echo "==> Copying Iliada $REV to $HOST"
git ls-files -z | tar --null -T - -czf - | "${SSH[@]}" "cat > /tmp/iliada-source.tgz"
echo "==> Rebuilding on $HOST (the first time takes a while)"
"${SSH[@]}" bash -s -- "$REV" <<'REMOTE'
set -euo pipefail
REV="$1"
G() { git -C /etc/iliada -c user.name=Iliada -c user.email=iliada@localhost "$@"; }
cd /etc/iliada
grep -q '^\.source-' .gitignore || printf '.source-*\n' >> .gitignore
rm -rf .source-dev .source-old && mkdir .source-dev
tar -xzf /tmp/iliada-source.tgz -C .source-dev && rm /tmp/iliada-source.tgz
mv source .source-old && mv .source-dev source
restore() {
  echo "build failed: putting the previous source back"
  rm -rf source && mv .source-old source
  G checkout -- flake.lock
  exit 1
}
nix --extra-experimental-features 'nix-command flakes' flake update iliada \
  --flake git+file:///etc/iliada 2>&1 | grep -v 'is dirty' || true
nixos-rebuild switch --flake git+file:///etc/iliada#iliada > /tmp/iliada-rebuild.log 2>&1 || {
  grep -vE '^(copying|building|these|  /nix)' /tmp/iliada-rebuild.log | tail -25; restore; }
rm -rf .source-old
G add -A && { G commit -qm "Developer update to $REV" || true; }
echo "now running $(readlink /run/current-system)"
REMOTE
echo "==> $HOST is on Iliada $REV. Reopen any app that changed."
