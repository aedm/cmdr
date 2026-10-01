#!/bin/sh
# Runs a single-node Garage and, beside it, bootstraps it: assign and apply a
# layout, import the fixture key, create the buckets, and allow the key on them.
# Idempotent: `restart: unless-stopped` and a persistent volume mean this runs
# again over a cluster that's already set up, and every step checks first.
#
# Env (the compose file sets all of them):
#   S3_ACCESS_KEY, S3_SECRET_KEY   the key to import. Garage insists on its own
#                                  format: `GK` + 24 hex, and a 64-hex secret.
#   S3_BUCKETS                     space-separated bucket names to ensure
set -e

: "${S3_ACCESS_KEY:?}" "${S3_SECRET_KEY:?}" "${S3_BUCKETS:?}"
READY=/tmp/fixture-ready

rm -f "$READY"
mkdir -p /var/lib/garage/meta /var/lib/garage/data

bootstrap() {
    # The CLI talks RPC to the local node; poll until it answers. ❌ No fixed sleep.
    tries=0
    until garage status > /dev/null 2>&1; do
        tries=$((tries + 1))
        if [ "$tries" -ge 300 ]; then
            echo "fixture: Garage never answered over RPC" >&2
            return 1
        fi
        sleep 0.1
    done

    # A layout with no role for this node leaves every S3 call refused. Version
    # 0 means nothing has been applied yet.
    if garage layout show 2> /dev/null | grep -q "Current cluster layout version: 0"; then
        node_id=$(garage node id -q | cut -d@ -f1)
        garage layout assign -z dc1 -c 1G "$node_id"
        garage layout apply --version 1
    fi

    if ! garage key info "$S3_ACCESS_KEY" > /dev/null 2>&1; then
        garage key import --yes -n cmdr-fixture "$S3_ACCESS_KEY" "$S3_SECRET_KEY"
    fi

    for bucket in $S3_BUCKETS; do
        if ! garage bucket info "$bucket" > /dev/null 2>&1; then
            garage bucket create "$bucket"
        fi
        # Re-granting is a no-op, so no check first.
        garage bucket allow --read --write --owner "$bucket" --key "$S3_ACCESS_KEY" > /dev/null
        echo "fixture: bucket $bucket ready"
    done
    touch "$READY"
}

bootstrap &

# PID 1 is the server itself, so `docker stop` signals it directly.
exec garage -c /etc/garage.toml server
