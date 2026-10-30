#!/bin/sh
# Installs Forge for the current user, no root needed: the program into ~/.local/bin, the icon and the launcher entry
# into ~/.local/share. Run it from the unpacked folder:  ./install.sh   (remove with:  ./install.sh --uninstall)
set -eu

here=$(cd "$(dirname "$0")" && pwd)
bin="${HOME}/.local/bin"
apps="${HOME}/.local/share/applications"
icons="${HOME}/.local/share/icons/hicolor/scalable/apps"

if [ "${1:-}" = "--uninstall" ]; then
  rm -f "$bin/forge" "$apps/forge.desktop" "$icons/forge.svg"
  echo "Forge removed. Your settings in ~/.config/forge (or \$XDG_CONFIG_HOME/forge) were left alone."
  exit 0
fi

mkdir -p "$bin" "$apps" "$icons"
install -m 755 "$here/forge" "$bin/forge"
install -m 644 "$here/forge-logo.svg" "$icons/forge.svg"
install -m 644 "$here/forge.desktop" "$apps/forge.desktop"
# Point the launcher at the installed copy, so it works even if ~/.local/bin is not on the PATH.
sed -i "s|^Exec=forge|Exec=$bin/forge|" "$apps/forge.desktop"
command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$apps" >/dev/null 2>&1 || true

echo "Forge installed to $bin/forge."
case ":$PATH:" in
  *":$bin:"*) ;;
  *) echo "Note: $bin is not on your PATH; add it to start Forge as 'forge' from a terminal." ;;
esac
echo "Forge needs a Vulkan-capable graphics driver (most desktops have one: check with 'vulkaninfo --summary')."
