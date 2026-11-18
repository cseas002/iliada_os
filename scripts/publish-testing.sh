#!/usr/bin/env bash
# Publish what is on main as a test update: the GitHub pre-release "testing",
# which computers with Settings → Updates → Test updates on install from.
# Its version is VERSION plus the time (0.2.2.202611181530), so the next
# release is newer than every test update before it.
#   bash scripts/publish-testing.sh            (from a clean checkout of main)
set -euo pipefail
cd "$(dirname "$0")/.."
export NIX_CONFIG="experimental-features = nix-command flakes"

[ -z "$(git status --porcelain)" ] || { echo "Commit first: test updates are made from committed work."; exit 1; }
git fetch -q origin main
[ "$(git rev-parse HEAD)" = "$(git rev-parse origin/main)" ] ||
  { echo "HEAD is not origin/main: push (or pull) first."; exit 1; }

stamp=$(date -u +%Y%m%d%H%M)
out=$(mktemp -d)
trap 'rm -rf "$out"' EXIT
nix develop -c cargo run -q -p iliada-catalog --features builder -- \
  testing --source . --stamp "$stamp" --key ~/.config/iliada/catalog.key --out "$out"

# The tag follows the source of the newest test update.
git tag -f testing >/dev/null
git push -qf origin testing

if ! gh release view testing >/dev/null 2>&1; then
  gh release create testing --prerelease --latest=false --title "Test updates" --notes \
"The newest work on Iliada, between releases. Computers get these only when
Settings → Updates → Test updates is on; everyone else gets releases.
This page's files are replaced by each new test update."
fi

old=$(gh release view testing --json assets -q '.assets[].name' | grep -- '-source\.tar\.zst$' || true)
# The archive first, so release.json never names a file that isn't there.
gh release upload testing "$out"/iliada-*-source.tar.zst --clobber
gh release upload testing "$out/release.json" "$out/release.json.sig" --clobber
for a in $old; do
  [ -e "$out/$a" ] || gh release delete-asset testing "$a" -y
done
echo "Published test update $(sed -n 's/.*"version": "\(.*\)".*/\1/p' "$out/release.json")"
