#!/bin/sh
# Fill in the public address (canonical links, social image, sitemap) when the container starts.
# SITE_URL wins; on Railway RAILWAY_PUBLIC_DOMAIN is used when SITE_URL is not set.
set -e
base="${SITE_URL:-}"
if [ -z "$base" ] && [ -n "${RAILWAY_PUBLIC_DOMAIN:-}" ]; then base="https://${RAILWAY_PUBLIC_DOMAIN}"; fi
base="${base%/}"
root=/usr/share/nginx/html
for f in index.html robots.txt sitemap.xml; do sed -i "s|__SITE_URL__|${base}|g" "$root/$f"; done
if [ -z "$base" ]; then
  sed -i "/^Sitemap:/d" "$root/robots.txt"
  echo "40-site-url: SITE_URL is not set, so canonical, social image and sitemap URLs are relative. Set SITE_URL to https://your-domain." >&2
fi
