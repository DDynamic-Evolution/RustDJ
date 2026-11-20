#!/usr/bin/env bash
set -euo pipefail

project_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
output_dir=${1:-"$project_dir/outputs"}
mkdir -p -- "$output_dir"
output_dir=$(cd -- "$output_dir" && pwd)
cd -- "$project_dir"

# Use Cargo's resolved package version and build directory, including CARGO_TARGET_DIR.
metadata=$(cargo metadata --locked --no-deps --format-version 1)
version=$(python3 -c 'import json, sys; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"] == "rustdj"))' <<< "$metadata")
target_dir=$(python3 -c 'import json, sys; print(json.load(sys.stdin)["target_directory"])' <<< "$metadata")
architecture=$(dpkg --print-architecture)
cargo build --locked --release --bin rustdj

staging=$(mktemp -d)
trap 'rm -rf -- "$staging"' EXIT
package="$staging/debian/rustdj"
install -Dm755 "$target_dir/release/rustdj" "$package/usr/bin/rustdj"
install -Dm644 packaging/debian/rustdj.desktop "$package/usr/share/applications/rustdj.desktop"
install -Dm644 assets/branding/rustdj-window-icon.png \
    "$package/usr/share/icons/hicolor/256x256/apps/rustdj.png"
install -Dm644 THIRD_PARTY_NOTICES.md "$package/usr/share/doc/rustdj/THIRD_PARTY_NOTICES.md"
install -m644 third_party/licenses/*.txt "$package/usr/share/doc/rustdj/"

# dpkg-shlibdeps requires a source control file in its working directory.
mkdir -p "$staging/debian" "$package/DEBIAN"
printf 'Source: rustdj\n\nPackage: rustdj\nArchitecture: any\n' > "$staging/debian/control"
dependencies=$(cd -- "$staging" && dpkg-shlibdeps -O -e debian/rustdj/usr/bin/rustdj)
dependencies=${dependencies#shlibs:Depends=}
installed_size=$(du -sk "$package" | cut -f1)
cat > "$package/DEBIAN/control" <<CONTROL
Package: rustdj
Version: $version
Section: sound
Priority: optional
Architecture: $architecture
Maintainer: MikoAstral <277569098+DDynamic-Evolution@users.noreply.github.com>
Installed-Size: $installed_size
Depends: $dependencies, libegl1, libgl1, libwayland-client0, libwayland-cursor0, libx11-6, libxcursor1, libxi6, libxkbcommon0, libxkbcommon-x11-0, libxrandr2, xdg-desktop-portal
Recommends: gnome-keyring, libasound2-plugins, xdg-desktop-portal-gtk | xdg-desktop-portal-backend
Homepage: https://github.com/DDynamic-Evolution/RustDJ
Description: DJ mixing, broadcasting and recording application
 RustDJ provides four decks, a music library, headphone monitoring, MIDI
 controller support, SHOUTcast streaming and MP3 recording.
CONTROL

dpkg-deb --root-owner-group --build "$package" "$output_dir/rustdj_${version}_${architecture}.deb"
