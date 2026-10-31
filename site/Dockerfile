# Forge landing page: static files served by nginx, as a non-root user.
#
# Railway: set the service's Root Directory to `site`; it builds this file and passes the port in $PORT.
# Set SITE_URL (https://your-domain) so canonical links and the sitemap use it; Railway's own domain is used otherwise.
# Locally:
#   docker build -t forge-site site
#   docker run --rm -p 8080:8080 forge-site        # http://localhost:8080
#
# The build context is the site/ folder.

FROM nginxinc/nginx-unprivileged:1.27-alpine

# nginx fills ${PORT} into the config at start-up (only PORT, so nginx's own $variables are left alone).
ENV PORT=8080 \
    NGINX_ENVSUBST_FILTER=^PORT$
COPY nginx.conf.template /etc/nginx/templates/default.conf.template
COPY security-headers.conf /etc/nginx/snippets/security-headers.conf
COPY --chown=nginx:nginx --chmod=755 site-url.sh /docker-entrypoint.d/40-site-url.sh
COPY --chown=nginx:nginx index.html robots.txt sitemap.xml 404.html /usr/share/nginx/html/
COPY css /usr/share/nginx/html/css
COPY js /usr/share/nginx/html/js
COPY assets /usr/share/nginx/html/assets

EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s CMD wget -q -O /dev/null http://127.0.0.1:${PORT}/healthz || exit 1
