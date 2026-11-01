#!/bin/sh
# Forge installer for Linux. Run it straight from the web:
#
#   curl -fsSL https://<your-site>/install.sh | sh
#
# It downloads the Forge release for this machine from GitHub, checks it, and installs it for the current user
# (no root needed): the program into ~/.local/bin, the icon and launcher into ~/.local/share, and ~/.local/bin
# onto your PATH. It also looks for the system libraries Forge needs and, if any are missing, tells you the
# command for your distribution's package manager (or runs it for you with FORGE_INSTALL_DEPS=1).
#
# Options (pass after `sh -s --` when piping):
#   --uninstall            remove Forge (settings in ~/.config/forge are kept)
#   --version <x.y.z>      install that version instead of the latest      (or FORGE_VERSION=x.y.z)
#   --install-deps         install missing system libraries with sudo      (or FORGE_INSTALL_DEPS=1)
#   --help
# Environment:
#   FORGE_REPO      GitHub repository to download from      (default sukantamajhi/forge-ide)
#   FORGE_TARBALL   install from this local .tar.gz instead of downloading (offline installs, testing)
#
#   curl -fsSL https://<your-site>/install.sh | sh -s -- --version 0.2.0
set -eu

REPO="${FORGE_REPO:-sukantamajhi/forge-ide}"
VERSION="${FORGE_VERSION:-}"
INSTALL_DEPS="${FORGE_INSTALL_DEPS:-}"
ACTION=install

say() { printf '%s\n' "$*"; }
warn() { printf 'warning: %s\n' "$*" >&2; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }
have() { command -v "$1" >/dev/null 2>&1; }

usage() {
  cat <<'EOF_USAGE'
Forge installer for Linux.
  curl -fsSL https://<your-site>/install.sh | sh
  curl -fsSL https://<your-site>/install.sh | sh -s -- [options]

Options:
  --uninstall          remove Forge (settings in ~/.config/forge are kept)
  --version <x.y.z>    install that version instead of the latest   (FORGE_VERSION)
  --install-deps       install missing system libraries with sudo   (FORGE_INSTALL_DEPS=1)
  -h, --help           this text
Environment: FORGE_REPO (owner/name on GitHub), FORGE_TARBALL (install from a local .tar.gz)
EOF_USAGE
}

while [ $# -gt 0 ]; do
  case "$1" in
    --uninstall) ACTION=uninstall ;;
    --install-deps) INSTALL_DEPS=1 ;;
    --version) [ $# -ge 2 ] || die "--version needs a value"; VERSION="$2"; shift ;;
    --version=*) VERSION="${1#--version=}" ;;
    -h|--help) usage; exit 0 ;;
    *) die "unknown option: $1 (try --help)" ;;
  esac
  shift
done
VERSION="${VERSION#v}"

# ---- what machine is this -------------------------------------------------------------------------------------

[ "$(uname -s)" = "Linux" ] || die "this installer is for Linux. On Windows, download Forge-Setup from https://github.com/$REPO/releases"

case "$(uname -m)" in
  x86_64|amd64) ARCH=x86_64 ;;
  aarch64|arm64) ARCH=aarch64 ;;
  *) die "unsupported CPU architecture: $(uname -m)" ;;
esac

# Forge is built against glibc; musl systems (Alpine, Void-musl) cannot run it.
if have ldd && ldd --version 2>&1 | grep -qi musl; then
  die "this system uses musl libc; Forge needs glibc. Use a glibc distribution, or build Forge from source."
fi

# Distribution family, from /etc/os-release (ID and ID_LIKE), used to name packages.
DISTRO=unknown
FAMILY=unknown
if [ -r /etc/os-release ]; then
  # shellcheck disable=SC1091
  DISTRO=$(. /etc/os-release && printf '%s' "${PRETTY_NAME:-${NAME:-unknown}}")
  ids=$(. /etc/os-release && printf '%s %s' "${ID:-}" "${ID_LIKE:-}")
  for id in $ids; do
    case "$id" in
      debian|ubuntu|linuxmint|pop|raspbian|elementary|kali|neon|zorin) FAMILY=debian; break ;;
      fedora|rhel|centos|rocky|almalinux|ol|amzn|nobara) FAMILY=fedora; break ;;
      arch|manjaro|endeavouros|garuda|cachyos|artix) FAMILY=arch; break ;;
      suse|opensuse*|sles|sled) FAMILY=suse; break ;;
      void) FAMILY=void; break ;;
      alpine) FAMILY=alpine; break ;;
    esac
  done
fi
# The package manager actually present wins when the family could not be told from os-release.
if [ "$FAMILY" = unknown ]; then
  if have apt-get; then FAMILY=debian
  elif have dnf || have yum; then FAMILY=fedora
  elif have pacman; then FAMILY=arch
  elif have zypper; then FAMILY=suse
  elif have xbps-install; then FAMILY=void
  fi
fi

# ---- helpers --------------------------------------------------------------------------------------------------

# fetch URL OUTPUT-FILE: curl, or wget when there is no curl.
fetch() {
  if have curl; then curl -fsSL --retry 3 -o "$2" "$1"
  elif have wget; then wget -q -O "$2" "$1"
  else die "need curl or wget to download Forge"
  fi
}

# The final address a URL redirects to (how GitHub says which release is the latest).
final_url() {
  if have curl; then curl -fsSL -o /dev/null -w '%{url_effective}' "$1"
  elif have wget; then wget -q --spider -S --max-redirect=5 "$1" 2>&1 | sed -n 's/^ *[Ll]ocation: *//p' | tail -n 1 | tr -d '\r'
  else die "need curl or wget to download Forge"
  fi
}

TMP=$(mktemp -d "${TMPDIR:-/tmp}/forge-install.XXXXXX")
trap 'rm -rf "$TMP"' EXIT INT TERM

# Get the release package unpacked into $TMP/pkg and print nothing. Sets PKG to its folder.
get_package() {
  tarball="${FORGE_TARBALL:-}"
  if [ -z "$tarball" ]; then
    if [ -z "$VERSION" ]; then
      say "Looking up the latest Forge release..."
      tag=$(final_url "https://github.com/$REPO/releases/latest" | sed -n 's|.*/tag/||p')
      [ -n "$tag" ] || die "could not find the latest release of $REPO (is there a published release?)"
      VERSION="${tag#v}"
    fi
    name="forge-$VERSION-linux-$ARCH"
    url="https://github.com/$REPO/releases/download/v$VERSION/$name.tar.gz"
    say "Downloading Forge $VERSION for Linux ($ARCH)..."
    fetch "$url" "$TMP/forge.tar.gz" || die "could not download $url
There may be no Linux build for $ARCH of version $VERSION. See https://github.com/$REPO/releases"
    tarball="$TMP/forge.tar.gz"
    verify_checksum "$url" "$tarball"
  fi
  mkdir -p "$TMP/pkg"
  tar -xzf "$tarball" -C "$TMP/pkg" || die "the download is not a valid archive"
  # The archive holds one folder; take the one that has install.sh in it.
  PKG=$(dirname "$(find "$TMP/pkg" -name install.sh -type f | head -n 1)")
  [ -f "$PKG/forge" ] && [ -f "$PKG/install.sh" ] || die "the archive does not look like a Forge package"
}

# Compare against the .sha256 file published beside the archive. Skipped, with a note, when there is none.
verify_checksum() {
  if fetch "$1.sha256" "$TMP/forge.sha256" 2>/dev/null; then
    want=$(awk '{print $1; exit}' "$TMP/forge.sha256")
    if have sha256sum; then got=$(sha256sum "$2" | awk '{print $1}')
    elif have shasum; then got=$(shasum -a 256 "$2" | awk '{print $1}')
    else warn "no sha256sum here, so the download was not verified"; return 0
    fi
    [ "$want" = "$got" ] || die "checksum mismatch: the download is damaged or was tampered with"
    say "Checksum verified."
  else
    warn "no checksum is published for this release, so the download was not verified"
  fi
}

# ---- system libraries -----------------------------------------------------------------------------------------

# pkg_for LIBRARY: the package that provides a shared library, for this distribution family.
pkg_for() {
  case "$1:$FAMILY" in
    libvulkan.so.1:debian|libvulkan.so.1:suse) echo libvulkan1 ;;
    libvulkan.so.1:fedora) echo vulkan-loader ;;
    libvulkan.so.1:arch) echo vulkan-icd-loader ;;
    libvulkan.so.1:void) echo vulkan-loader ;;
    libxkbcommon.so.0:debian|libxkbcommon.so.0:suse) echo libxkbcommon0 ;;
    libxkbcommon.so.0:fedora|libxkbcommon.so.0:arch|libxkbcommon.so.0:void) echo libxkbcommon ;;
    libxkbcommon-x11.so.0:debian|libxkbcommon-x11.so.0:suse) echo libxkbcommon-x11-0 ;;
    libxkbcommon-x11.so.0:fedora|libxkbcommon-x11.so.0:arch) echo libxkbcommon-x11 ;;
    libxkbcommon-x11.so.0:void) echo libxkbcommon-x11 ;;
    libxcb.so.1:debian|libxcb.so.1:suse) echo libxcb1 ;;
    libxcb.so.1:fedora|libxcb.so.1:arch|libxcb.so.1:void) echo libxcb ;;
    libfreetype.so.6:debian|libfreetype.so.6:suse) echo libfreetype6 ;;
    libfreetype.so.6:fedora|libfreetype.so.6:void) echo freetype ;;
    libfreetype.so.6:arch) echo freetype2 ;;
    libfontconfig.so.1:debian|libfontconfig.so.1:suse) echo libfontconfig1 ;;
    libfontconfig.so.1:fedora|libfontconfig.so.1:arch) echo fontconfig ;;
    libfontconfig.so.1:void) echo fontconfig ;;
    libwayland-client.so.0:debian|libwayland-client.so.0:suse) echo libwayland-client0 ;;
    libwayland-client.so.0:fedora) echo libwayland-client ;;
    libwayland-client.so.0:arch|libwayland-client.so.0:void) echo wayland ;;
    *) return 1 ;;
  esac
}

# lib_present LIBRARY: is it known to the dynamic linker?
lib_present() {
  if have ldconfig && ldconfig -p 2>/dev/null | grep -q "$1"; then return 0; fi
  for d in /usr/lib /usr/lib64 /lib /lib64 /usr/lib/x86_64-linux-gnu /usr/lib/aarch64-linux-gnu /usr/local/lib; do
    [ -e "$d/$1" ] && return 0
  done
  return 1
}

install_command() { # install_command PACKAGES...
  case "$FAMILY" in
    debian) echo "apt-get install -y $*" ;;
    fedora) if have dnf; then echo "dnf install -y $*"; else echo "yum install -y $*"; fi ;;
    arch) echo "pacman -S --needed --noconfirm $*" ;;
    suse) echo "zypper --non-interactive install $*" ;;
    void) echo "xbps-install -y $*" ;;
    *) return 1 ;;
  esac
}

check_libraries() {
  missing=""
  # What the program is linked against, as the dynamic linker sees it.
  if have ldd; then
    missing=$(ldd "$PKG/forge" 2>/dev/null | sed -n 's/^[[:space:]]*\(lib[^ ]*\) => not found.*/\1/p')
  fi
  # Loaded at run time rather than linked, so ldd cannot see them.
  for lib in libvulkan.so.1; do
    lib_present "$lib" || missing="$missing $lib"
  done
  missing=$(printf '%s\n' $missing | sort -u)
  [ -n "$missing" ] || return 0

  pkgs=""
  unknown=""
  for lib in $missing; do
    if p=$(pkg_for "$lib"); then pkgs="$pkgs $p"; else unknown="$unknown $lib"; fi
  done
  pkgs=$(printf '%s\n' $pkgs | sort -u | tr '\n' ' ' | sed 's/ $//')

  warn "Forge needs system libraries that are not installed on $DISTRO:"
  for lib in $missing; do say "  - $lib" >&2; done
  if [ -n "$pkgs" ] && cmd=$(install_command $pkgs); then
    if [ -n "$INSTALL_DEPS" ]; then
      say "Installing: $pkgs"
      if [ "$(id -u)" = 0 ]; then sh -c "$cmd" || warn "installing the libraries failed"
      elif have sudo; then sh -c "sudo $cmd" || warn "installing the libraries failed"
      else warn "sudo is not available; run as root: $cmd"
      fi
    else
      say "Install them with:  sudo $cmd" >&2
      say "(or run this installer again with --install-deps)" >&2
    fi
  else
    say "Install the packages that provide them with your package manager." >&2
  fi
  [ -z "$unknown" ] || say "No package name known for:$unknown" >&2
}

# ---- go -------------------------------------------------------------------------------------------------------

if [ "$ACTION" = uninstall ]; then
  get_package
  sh "$PKG/install.sh" --uninstall
  exit 0
fi

get_package
check_libraries
# The package's own installer puts the files in place and adds ~/.local/bin to the PATH.
sh "$PKG/install.sh"
say ""
say "Done. Start Forge from your applications menu, or run 'forge .' in a project folder."
