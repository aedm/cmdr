#!/bin/bash
# Run cmdr-s3's live-provider cells against real accounts (R2, Hetzner, GCS,
# DigitalOcean Spaces): what each provider enforces, how multipart behaves,
# and throughput. Never part of `pnpm check` or CI; the cells skip without
# CMDR_S3_LIVE=1.
#
# Usage:
#   ./live.sh                      # every provider, every live cell, then the sweep
#   ./live.sh r2,gcs               # only these providers
#   ./live.sh all live_batch       # every provider, only cells matching a filter
#
# Credentials come from David's sops store through the `secret` CLI and go to
# the test process as env vars only. ❗ Never echo them.
#
# Each cell deletes what it wrote under `cmdr-live/<run>/`; the last step
# sweeps anything a crashed run left. The buckets themselves stay.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../../.." && pwd)"

only="${1:-all}"
filter="${2:-live_}"

export CMDR_S3_LIVE=1
if [ "$only" != "all" ]; then
    export CMDR_S3_LIVE_ONLY="$only"
fi

# Cloudflare R2: the account ID comes from the API token, so it isn't written
# down here.
cf_token="$(secret CLOUDFLARE_API_TOKEN)"
CMDR_S3_LIVE_R2_ACCOUNT="$(curl -fsS -H "Authorization: Bearer $cf_token" \
    https://api.cloudflare.com/client/v4/accounts | jq -r '.result[0].id')"
unset cf_token
export CMDR_S3_LIVE_R2_ACCOUNT
export CMDR_S3_LIVE_R2_KEY_ID="$(secret R2_S3_TEST_ACCESS_KEY_ID)"
export CMDR_S3_LIVE_R2_SECRET="$(secret R2_S3_TEST_SECRET_ACCESS_KEY)"
export CMDR_S3_LIVE_R2_BUCKET="${CMDR_S3_LIVE_R2_BUCKET:-cmdr-s3-test}"

# Hetzner Object Storage. HEL1 has been degraded; NBG1 is the default.
export CMDR_S3_LIVE_HETZNER_LOCATION="${CMDR_S3_LIVE_HETZNER_LOCATION:-nbg1}"
export CMDR_S3_LIVE_HETZNER_KEY_ID="$(secret HETZNER_S3_ACCESS_KEY_ID)"
export CMDR_S3_LIVE_HETZNER_SECRET="$(secret HETZNER_S3_SECRET_ACCESS_KEY)"
export CMDR_S3_LIVE_HETZNER_BUCKET="${CMDR_S3_LIVE_HETZNER_BUCKET:-cmdr-s3-test-d0e600}"

# Google Cloud Storage, through the XML API with HMAC keys.
export CMDR_S3_LIVE_GCS_KEY_ID="$(secret GCS_S3_ACCESS_KEY_ID)"
export CMDR_S3_LIVE_GCS_SECRET="$(secret GCS_S3_SECRET_ACCESS_KEY)"
export CMDR_S3_LIVE_GCS_BUCKET="$(secret GCS_S3_TEST_BUCKET)"

# DigitalOcean Spaces.
export CMDR_S3_LIVE_SPACES_REGION="$(secret DO_SPACES_REGION)"
export CMDR_S3_LIVE_SPACES_KEY_ID="$(secret DO_SPACES_ACCESS_KEY_ID)"
export CMDR_S3_LIVE_SPACES_SECRET="$(secret DO_SPACES_SECRET_ACCESS_KEY)"
export CMDR_S3_LIVE_SPACES_BUCKET="$(secret DO_SPACES_TEST_BUCKET)"

cd "$REPO_ROOT"
# One cell at a time: the cells share buckets, and the throughput numbers mean
# nothing with other cells on the same link.
cargo test -p cmdr-s3 --lib "$filter" -- --test-threads=1 --nocapture
cargo test -p cmdr-s3 --lib live_cleanup_removes_every_leftover -- --nocapture
