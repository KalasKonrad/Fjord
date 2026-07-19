# Maintainer: KalasKonrad <niat86@hotmail.com>
# shellcheck shell=bash
# shellcheck disable=SC2034,SC2154,SC2164
# SC2034: pkgname/pkgver/… look unused — they are read by makepkg
# SC2154: $srcdir/$pkgdir/$pkgname injected by makepkg, not defined here
# SC2164: cd without || exit — makepkg already aborts on non-zero exit
pkgname=fjord-git
pkgver=r788.d4140c8
pkgrel=1
pkgdesc="Jellyfin media frontend with smooth mpv playback on NVIDIA legacy hardware"
arch=('x86_64')
url="https://github.com/KalasKonrad/Fjord"
license=('GPL-3.0-only')
depends=('mpv' 'fontconfig' 'freetype2' 'libxkbcommon')
optdepends=('yt-dlp: play trailers on the Discover screen')
makedepends=('git' 'rust' 'cargo')
provides=('fjord')
conflicts=('fjord')
install=fjord.install
options=('!debug')
source=("fjord::git+https://github.com/KalasKonrad/Fjord.git")
sha256sums=('SKIP')

pkgver() {
    cd "$srcdir/fjord"
    printf "r%s.%s" "$(git rev-list --count HEAD)" "$(git rev-parse --short HEAD)"
}

build() {
    cd "$srcdir/fjord"
    export CARGO_HOME="$srcdir/cargo-home"
    cargo build --release --locked
}

package() {
    cd "$srcdir/fjord"
    strip --strip-debug "target/release/fjord-app"
    install -Dm755 "target/release/fjord-app" "$pkgdir/usr/bin/fjord"
    install -Dm644 "LICENSE" "$pkgdir/usr/share/licenses/$pkgname/LICENSE"
    local icons=(01 02 04 05 09 10)
    local n=${icons[$((RANDOM % ${#icons[@]}))]}
    printf 'fjord: installing icon fjord_%s.svg\n' "$n"
    install -Dm644 "assets/fjord_${n}.svg" "$pkgdir/usr/share/icons/hicolor/scalable/apps/fjord.svg"

    install -dm755 "$pkgdir/usr/share/applications"
    cat > "$pkgdir/usr/share/applications/fjord.desktop" << 'EOF'
[Desktop Entry]
Name=Fjord
GenericName=Media Player
Comment=Jellyfin media frontend
Exec=fjord
Icon=fjord
Type=Application
Categories=AudioVideo;Video;Player;
Keywords=Jellyfin;mpv;media;
EOF
}
