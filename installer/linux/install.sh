#!/bin/sh
# Installs Forge for the current user, no root needed: the program into ~/.local/bin, the icon and the launcher entry
# into ~/.local/share, and ~/.local/bin is added to the PATH of your shell if it is not there yet.
# Run it from the unpacked folder:  ./install.sh   (remove with:  ./install.sh --uninstall)
set -eu

here=$(cd "$(dirname "$0")" && pwd)
bin="${HOME}/.local/bin"
apps="${HOME}/.local/share/applications"
icons="${HOME}/.local/share/icons/hicolor/scalable/apps"
fish_conf="${XDG_CONFIG_HOME:-$HOME/.config}/fish/conf.d/forge.fish"
# Every startup file a shell may read: each one that exists gets the PATH line.
rcfiles="$HOME/.bashrc $HOME/.bash_profile $HOME/.zshrc $HOME/.zprofile $HOME/.profile"
begin="# >>> forge >>>"
end="# <<< forge <<<"

if [ "${1:-}" = "--uninstall" ]; then
  rm -f "$bin/forge" "$apps/forge.desktop" "$icons/forge.svg" "$fish_conf"
  for rc in $rcfiles; do
    [ -f "$rc" ] && grep -qF "$begin" "$rc" && sed -i "/^$begin\$/,/^$end\$/d" "$rc"
  done
  echo "Forge removed. Your settings in ~/.config/forge (or \$XDG_CONFIG_HOME/forge) were left alone."
  exit 0
fi

mkdir -p "$bin" "$apps" "$icons"
install -m 755 "$here/forge" "$bin/forge"
install -m 644 "$here/forge-logo.svg" "$icons/forge.svg"
install -m 644 "$here/forge.desktop" "$apps/forge.desktop"
# Point the launcher at the installed copy, so it works even if ~/.local/bin is not on the PATH.
sed -i "s|^Exec=forge|Exec=$bin/forge|" "$apps/forge.desktop"
command -v gtk-update-icon-cache >/dev/null 2>&1 && gtk-update-icon-cache -q -t -f "${HOME}/.local/share/icons/hicolor" >/dev/null 2>&1 || true
command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$apps" >/dev/null 2>&1 || true

echo "Forge installed to $bin/forge."
case ":$PATH:" in
  *":$bin:"*) ;;
  *)
    # Add it to the PATH of future shells (once: the marked block is skipped if it is already there).
    added=""
    have=""
    for rc in $rcfiles; do
      [ -f "$rc" ] || continue
      have=1
      if ! grep -qF "$begin" "$rc"; then
        printf '\n%s\nexport PATH="$HOME/.local/bin:$PATH"\n%s\n' "$begin" "$end" >> "$rc"
        added="$added $rc"
      fi
    done
    # With none of them present, create ~/.profile so at least login shells pick it up.
    if [ -z "$have" ]; then
      printf '\n%s\nexport PATH="$HOME/.local/bin:$PATH"\n%s\n' "$begin" "$end" >> "$HOME/.profile"
      added="$added $HOME/.profile"
    fi
    if [ -d "$(dirname "$fish_conf")" ] || command -v fish >/dev/null 2>&1; then
      mkdir -p "$(dirname "$fish_conf")"
      printf 'fish_add_path -g "$HOME/.local/bin"\n' > "$fish_conf"
      added="$added $fish_conf"
    fi
    [ -n "$added" ] && echo "Added $bin to your PATH in:$added"
    echo "Open a new terminal (or run: export PATH=\"$bin:\$PATH\") to start Forge as 'forge'."
    ;;
esac
echo "Forge needs a Vulkan-capable graphics driver (most desktops have one: check with 'vulkaninfo --summary')."
