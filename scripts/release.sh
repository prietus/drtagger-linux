#!/usr/bin/env bash
# Builds the Flatpak for this machine's architecture and writes an
# installable bundle:
#
#   scripts/release.sh [out-dir]      → <out-dir>/drtagger-<version>-<arch>.flatpak
#
# Install with `flatpak install --user drtagger-….flatpak` (the GNOME runtime
# comes from Flathub). The CI workflow (.github/workflows/flatpak.yml) does
# the same for x86_64 and aarch64 and attaches both to a release on a tag.
# Work folders: $DRTAGGER_FLATPAK_WORK (default ~/.cache/drtagger-flatpak).
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
OUT=${1:-$ROOT}
WORK=${DRTAGGER_FLATPAK_WORK:-$HOME/.cache/drtagger-flatpak}
ARCH=$(flatpak --default-arch)
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -1)
APP=us.priet.drtagger

# The vendored crate list must match Cargo.lock.
python3 "$ROOT/scripts/cargo-sources.py" > /dev/null
git -C "$ROOT" diff --quiet -- build-aux/cargo-sources.json 2>/dev/null \
    || echo "note: build-aux/cargo-sources.json was regenerated; commit it"

mkdir -p "$WORK" "$OUT"
flatpak-builder --user --force-clean --install-deps-from=flathub \
    --state-dir "$WORK/state" --repo "$WORK/repo" "$WORK/build" \
    "$ROOT/build-aux/$APP.json"
BUNDLE="$OUT/drtagger-$VERSION-$ARCH.flatpak"
flatpak build-bundle --runtime-repo=https://dl.flathub.org/repo/flathub.flatpakrepo \
    "$WORK/repo" "$BUNDLE" "$APP"
echo "==> $BUNDLE"
