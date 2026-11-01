#!/bin/sh
# Builds a .deb from an already built release binary:  build-deb.sh <version> <path-to-forge-binary> <output-dir>
# The package installs for all users: /usr/bin/forge, the launcher and the icon under /usr/share.
set -eu

[ $# -eq 3 ] || { echo "usage: $0 <version> <forge-binary> <output-dir>" >&2; exit 2; }
version=$1
binary=$2
out=$3
root=$(cd "$(dirname "$0")/../.." && pwd)

case "$(uname -m)" in
  x86_64) arch=amd64 ;;
  aarch64) arch=arm64 ;;
  *) echo "unsupported architecture: $(uname -m)" >&2; exit 1 ;;
esac
command -v dpkg-deb >/dev/null 2>&1 || { echo "dpkg-deb is needed (sudo apt-get install dpkg)" >&2; exit 1; }
[ -x "$binary" ] || { echo "no built program at $binary (run: make release-linux)" >&2; exit 1; }

stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
install -Dm755 "$binary" "$stage/usr/bin/forge"
install -Dm644 "$root/assets/forge-logo.svg" "$stage/usr/share/icons/hicolor/scalable/apps/forge.svg"
install -Dm644 "$root/installer/linux/forge.desktop" "$stage/usr/share/applications/forge.desktop"
install -Dm644 "$root/README.md" "$stage/usr/share/doc/forge/README.md"

size=$(du -sk "$stage" | cut -f1)
mkdir -p "$stage/DEBIAN"
cat > "$stage/DEBIAN/control" <<CONTROL
Package: forge
Version: $version
Section: editors
Priority: optional
Architecture: $arch
Installed-Size: $size
Depends: libc6 (>= 2.31), libvulkan1, libxkbcommon0, libxkbcommon-x11-0, libxcb1, libfreetype6, libfontconfig1, libwayland-client0
Recommends: git
Maintainer: Forge <noreply@forge.invalid>
Homepage: https://github.com/sukantamajhi/forge-ide
Description: A fast, GPU-rendered code editor
 Forge draws every pixel on the GPU, edits the way VS Code taught you to,
 and installs extensions from Open VSX. Includes Git and debugging.
CONTROL
# Refresh the launcher and icon caches after install and removal.
for script in postinst postrm; do
  cat > "$stage/DEBIAN/$script" <<'HOOK'
#!/bin/sh
set -e
command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database -q /usr/share/applications || true
command -v gtk-update-icon-cache >/dev/null 2>&1 && gtk-update-icon-cache -q -t -f /usr/share/icons/hicolor || true
exit 0
HOOK
  chmod 755 "$stage/DEBIAN/$script"
done

mkdir -p "$out"
file="$out/forge_${version}_${arch}.deb"
dpkg-deb --root-owner-group --build "$stage" "$file" >/dev/null
echo "Built $file"
