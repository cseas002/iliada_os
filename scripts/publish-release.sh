#!/usr/bin/env bash
# Publish an Iliada release that installed systems update to.
#
#   scripts/publish-release.sh 0.2.0 ~/.config/iliada/catalog.key
#
# Tags v<version>, packs the committed source (iliada-<v>-source.tar.zst),
# writes release.json signed with the catalog key, and uploads all three to
# the GitHub release v<version>. Installed systems fetch
# releases/latest/download/release.json, so the repository must be public
# for anonymous downloads.
set -euo pipefail
VERSION="${1:?version, e.g. 0.2.0}"
KEY="${2:?signing key (catalog.key)}"
cd "$(dirname "$0")/.."
[ -z "$(git status --porcelain)" ] || { echo "commit everything first" >&2; exit 1; }
grep -q "default = \"$VERSION\";" nix/modules/default.nix || {
  echo "set iliada.version to $VERSION in nix/modules/default.nix first" >&2; exit 1; }
OUT=$(mktemp -d)
cargo run -q --release -p iliada-catalog --features builder -- \
  release --source . --version "$VERSION" --key "$KEY" --out "$OUT"
git tag -a "v$VERSION" -m "Iliada $VERSION"
git push origin "v$VERSION"
gh release create "v$VERSION" --title "Iliada $VERSION" --notes "Iliada $VERSION" \
  "$OUT/release.json" "$OUT/release.json.sig" "$OUT/iliada-$VERSION-source.tar.zst"
echo "published v$VERSION"
