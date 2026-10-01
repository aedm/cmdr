# Garage: the S3 fixture with a different lineage (Rust, its own listing and
# ETag code) and NO conditional writes, so Cmdr can't quietly assume no-clobber
# PUTs work everywhere.
#
# ❗ The official image is `FROM scratch` with one binary, so there's no shell
# to run a bootstrap in. This copies that binary onto Alpine. Both pinned to an
# exact release: the cells assert on server behaviour, and a bump is a fixture
# change to review.
FROM dxflrs/garage:v2.4.1 AS garage

FROM alpine:3.24.2

COPY --from=garage /garage /usr/local/bin/garage
COPY garage.toml /etc/garage.toml
COPY entrypoint.sh /usr/local/bin/fixture-entrypoint.sh
RUN chmod +x /usr/local/bin/fixture-entrypoint.sh

EXPOSE 3900

# Healthy means "bootstrapped": layout applied, key imported, buckets allowed.
# Before that the S3 port is bound but answers every request with an error, and
# the lease adopts a stack on health.
HEALTHCHECK --interval=1s --timeout=3s --start-period=5s --retries=60 \
    CMD test -f /tmp/fixture-ready || exit 1

ENTRYPOINT ["/usr/local/bin/fixture-entrypoint.sh"]
