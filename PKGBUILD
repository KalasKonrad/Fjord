# Maintainer: KalasKonrad <niat86@hotmail.com>
pkgname=fjord-git
pkgver=r0.placeholder
pkgrel=1
pkgdesc="Jellyfin media frontend with smooth mpv playback on NVIDIA legacy hardware"
arch=('x86_64')
url="https://github.com/KalasKonrad/Fjord"
license=('MIT')
depends=('mpv' 'fontconfig' 'freetype2' 'libxkbcommon')
makedepends=('git' 'rust' 'cargo')
provides=('fjord')
conflicts=('fjord')
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

    install -Dm755 "target/release/fjord-app" "$pkgdir/usr/bin/fjord"

    install -Dm644 /dev/stdin "$pkgdir/usr/share/applications/fjord.desktop" << 'EOF'
[Desktop Entry]
Name=Fjord
GenericName=Media Player
Comment=Jellyfin media frontend
Exec=fjord
Icon=multimedia-player
Type=Application
Categories=AudioVideo;Video;Player;
Keywords=Jellyfin;mpv;media;
EOF
}
