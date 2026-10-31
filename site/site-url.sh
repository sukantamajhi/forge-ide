#!/bin/sh
# Fill in the public address (canonical links, social image, sitemap) when the container starts.
# SITE_URL wins; on Railway RAILWAY_PUBLIC_DOMAIN is used when SITE_URL is not set.
# Runs as the unprivileged nginx user, which can edit the page files but cannot create files in their
# folder, so edits go through /tmp and are written back in place (no `sed -i`).
set -e
base="${SITE_URL:-}"
if [ -z "$base" ] && [ -n "${RAILWAY_PUBLIC_DOMAIN:-}" ]; then base="https://${RAILWAY_PUBLIC_DOMAIN}"; fi
base="${base%/}"
root="${SITE_ROOT:-/usr/share/nginx/html}"
tmp="$(mktemp)"
trap 'rm -f "$tmp"' EXIT

edit() { # edit FILE SED-ARGS...
  file="$1"; shift
  sed "$@" "$file" > "$tmp"
  cat "$tmp" > "$file"
}

for f in index.html robots.txt sitemap.xml; do
  edit "$root/$f" "s|__SITE_URL__|${base}|g"
done
if [ -z "$base" ]; then
  edit "$root/robots.txt" "/^Sitemap:/d"
  echo "40-site-url: SITE_URL is not set, so canonical, social image and sitemap URLs are relative. Set SITE_URL to https://your-domain." >&2
fi
