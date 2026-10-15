#!/usr/bin/env bash
# ── Fjord · scripts/bump-packages.sh ─────────────────────────────────────────
# Release step 3 (CLAUDE.md → Releasing): point the two pinned packages at a
# published release. Run on `main` after CI has published "Fjord vX.Y.Z".
#
#   scripts/bump-packages.sh [X.Y.Z]     (default: workspace.package.version)
#
# Downloads the release's fjord-x86_64.tar.gz, checks that it was built from
# the vX.Y.Z tag (the commit in its VERSION file), then sets pkgver/pkgrel in
# PKGBUILD (fjord, builds the tag) and fjord-bin/PKGBUILD, plus fjord-bin's
# sha256sums. Shows the diff; committing is left to you.
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

version="${1:-$(awk -F'"' '/^\[workspace.package\]/ { f = 1 } f && /^version = / { print $2; exit }' Cargo.toml)}"
if [[ ! "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "bump-packages: '$version' is not a version like 0.5.1" >&2
    exit 1
fi
tag="v$version"

if ! commit=$(git rev-parse --verify --quiet "$tag^{commit}"); then
    echo "bump-packages: no tag $tag here (git fetch --tags?)" >&2
    exit 1
fi

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
url="https://github.com/KalasKonrad/Fjord/releases/download/$tag/fjord-x86_64.tar.gz"
echo "Downloading $url"
if ! curl -fsSL -o "$tmp/fjord-x86_64.tar.gz" "$url"; then
    echo "bump-packages: no tarball for $tag yet — has the release build finished?" >&2
    exit 1
fi

# VERSION is r<count>.<short hash> of the commit CI built.
built=$(tar -xzOf "$tmp/fjord-x86_64.tar.gz" fjord-bin/VERSION)
built_hash="${built##*.}"
if [[ -z "$built_hash" || "$commit" != "$built_hash"* ]]; then
    echo "bump-packages: the tarball was built from $built, not from $tag (${commit:0:7})" >&2
    exit 1
fi
sum=$(sha256sum "$tmp/fjord-x86_64.tar.gz" | cut -d' ' -f1)

sed -i -E "s/^pkgver=.*/pkgver=$version/; s/^pkgrel=.*/pkgrel=1/" PKGBUILD fjord-bin/PKGBUILD
sed -i -E "s/^sha256sums=.*/sha256sums=('$sum')/" fjord-bin/PKGBUILD

echo "$tag: tarball $built, sha256 $sum"
git --no-pager diff --stat -- PKGBUILD fjord-bin/PKGBUILD
git --no-pager diff -- PKGBUILD fjord-bin/PKGBUILD
