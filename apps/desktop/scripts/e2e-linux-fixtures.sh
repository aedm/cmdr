# shellcheck shell=bash
# The Docker fixture stacks the Linux E2E container talks to: SMB, SFTP, and
# WebDAV. Sourced by `e2e-linux.sh`, never run on its own. It expects the
# caller's REPO_ROOT, DESKTOP_DIR, and log_info / log_warn / log_error.
#
# Every stack follows one lease model: take the machine-wide lease with holder
# $$ (this long-lived harness shell), probe the PUBLISHED port until the daemon
# accepts TCP, and check the stack's Docker network exists so the E2E container
# can join it and dial each service by name on its container port. CI runs
# `e2e-linux.sh` DIRECTLY (never through check.sh), so the orchestrator's lease
# never exists for that job; this script must own its own. Never down a stack
# another holder uses: the helper downs only at zero holders, and
# `release_fixture_leases` (from the caller's EXIT trap) gives ours back.
#
# Entry points: `start_fixture_stacks` (before the tests), `report_smb_post_flight`
# (after them), and `release_fixture_leases` (on exit).

# ── Shared ──────────────────────────────────────────────────────────────────

# Leases this run holds, space-separated stack names, for release_fixture_leases.
FIXTURE_LEASES_HELD=""

# acquire_fixture_lease takes this run's lease on stack $1 in mode $2, returning
# 1 when the Go helper is missing or fails (the caller then falls back to the
# fixture's own start.sh, lease-aware itself, and holds nothing to release).
acquire_fixture_lease() {
    local stack="$1" mode="$2"
    if ! command -v go &> /dev/null; then
        log_warn "'go' not found; proceeding without a cross-worktree $stack lease"
        return 1
    fi
    if (cd "$REPO_ROOT/scripts/check" && go run ./stack-lease acquire "$stack" "$$" "$mode"); then
        FIXTURE_LEASES_HELD="$FIXTURE_LEASES_HELD $stack"
        return 0
    fi
    log_warn "$stack lease helper failed; proceeding without a cross-worktree lease"
    return 1
}

# release_fixture_leases gives back only the leases this run acquired.
release_fixture_leases() {
    local stack
    for stack in $FIXTURE_LEASES_HELD; do
        (cd "$REPO_ROOT/scripts/check" && go run ./stack-lease release "$stack" "$$" 2>/dev/null) || true
    done
}

# wait_for_published_port returns 0 once compose project $1's service $2,
# container port $3, accepts TCP on its published host port, or 1 when
# $SECONDS reaches the absolute deadline $4 first. Docker's `running` comes
# well before the daemon binds. NEVER replace this with a blanket `sleep N`;
# see apps/desktop/test/CLAUDE.md "Testing principles".
wait_for_published_port() {
    local project="$1" service="$2" port="$3" deadline="$4"
    local host_port
    host_port=$(docker compose -p "$project" port "$service" "$port" 2>/dev/null | awk -F: '{print $NF}')
    [ -z "$host_port" ] && return 1
    while ! (exec 3<>"/dev/tcp/127.0.0.1/$host_port") 2>/dev/null; do
        [ $SECONDS -ge "$deadline" ] && return 1
        sleep 0.1
    done
    exec 3<&-
    exec 3>&-
    return 0
}

# wait_for_network waits up to $1 seconds for Docker network $2 (compose
# creates it with the stack), returning 1 if it never appears.
wait_for_network() {
    local timeout="$1" network="$2" i
    for ((i = 0; i < timeout; i++)); do
        docker network inspect "$network" > /dev/null 2>&1 && return 0
        sleep 1
    done
    docker network inspect "$network" > /dev/null 2>&1
}

# ── SMB ─────────────────────────────────────────────────────────────────────
# The E2E test container joins the smb-consumer_default network so it can reach
# smb-consumer-guest:445 and friends by container name (no host port mapping
# needed). Containers come from smb2's consumer test harness.

SMB_SERVERS_DIR="$DESKTOP_DIR/test/smb-servers"
SMB_NETWORK="smb-consumer_default"
SMB_E2E_SERVICES=(smb-consumer-guest smb-consumer-auth smb-consumer-50shares smb-consumer-unicode)

# probe_smb_ports returns 0 if every required service's published port 445
# accepts TCP within $1 seconds (one deadline across all four), otherwise 1.
probe_smb_ports() {
    local timeout="${1:-10}"
    local deadline=$((SECONDS + timeout))
    local service
    for service in "${SMB_E2E_SERVICES[@]}"; do
        if ! wait_for_published_port smb-consumer "$service" 445 "$deadline"; then
            log_warn "  ! $service did not accept TCP on :445 within ${timeout}s"
            return 1
        fi
    done
    return 0
}

start_smb_containers() {
    # The lease first. It coexists with the short-lived inner start.sh's
    # "manual" holder (the helper is idempotent per holder).
    acquire_fixture_lease smb e2e || true

    # Check that ALL four required containers are running. A prior `minimal` or
    # `core` invocation leaves guest+auth up but not 50shares/unicode, so a
    # guest-only check falsely reports "already running" and tests that need
    # the other two fail with "Cannot reach smb-consumer-50shares".
    #
    # ALSO: "running" per `docker compose ps` only means the container is
    # alive; smbd inside may be hung, OOM-killed, or still loading. We always
    # follow the running-check with an active TCP probe; if it fails, we
    # reconcile the SMB stack rather than letting the E2E run hit "Cannot reach"
    # errors mid-test. See the case study in
    # apps/desktop/test/CLAUDE.md "Testing principles".
    local running service
    running=$(docker compose -p smb-consumer ps --services --filter status=running 2>/dev/null || true)
    local all_running=true
    for service in "${SMB_E2E_SERVICES[@]}"; do
        if ! echo "$running" | grep -q "^${service}$"; then
            all_running=false
            break
        fi
    done

    if $all_running; then
        log_info "SMB containers already running; verifying smbd reachability..."
        if probe_smb_ports 10; then
            log_info "SMB containers healthy"
        else
            # Running-but-not-serving. NEVER blanket-`down` the shared stack:
            # a sibling worktree's suite may be mid-run against it. Reconcile
            # instead — `up -d` the e2e services under the lock (additive, no
            # down, no force-recreate). If other leases are live, the sick stack
            # is the first-comer's to manage; the probe below retries.
            log_warn "SMB containers running but not serving; reconciling (no down)"
            if command -v go &> /dev/null && (cd "$REPO_ROOT/scripts/check" && go run ./stack-lease reconcile smb e2e); then
                : # reconciled under the lock
            else
                # Fallback (Go missing / helper broken): legacy down + restart.
                log_warn "SMB reconcile helper unavailable; falling back to legacy down + restart"
                docker compose -p smb-consumer down > /dev/null 2>&1 || true
                "$SMB_SERVERS_DIR/start.sh" e2e
            fi
        fi
    else
        log_info "Starting SMB containers (e2e)..."
        "$SMB_SERVERS_DIR/start.sh" e2e
    fi

    if ! wait_for_network 10 "$SMB_NETWORK"; then
        log_error "SMB network '$SMB_NETWORK' not found after starting containers"
        exit 1
    fi

    # Final confirmation banner. Surfaces in the failing-test output (per the
    # checker's filter) so an agent reading a failed run knows whether SMB
    # came up healthy or not, without spelunking container state.
    if probe_smb_ports 30; then
        log_info "SMB e2e stack ready: all 4 containers accepting TCP on :445"
    else
        log_error "SMB e2e stack NOT ready after restart; aborting before tests"
        docker compose -p smb-consumer ps
        for service in "${SMB_E2E_SERVICES[@]}"; do
            log_warn "--- last 30 lines of $service log ---"
            docker compose -p smb-consumer logs --tail=30 "$service" || true
        done
        exit 1
    fi
}

# report_smb_post_flight: did the consumer containers survive the run? The
# pre-flight probe confirms TCP at start; this one tells us whether the same
# containers are still serving when the test phase exits. Diverging results
# (pre-flight OK, post-flight FAIL) point at containers dying mid-run (memory
# pressure, smbd crash) vs Cmdr-side bugs. Diagnostic only: it never fails, so
# it can't mask the test result.
report_smb_post_flight() {
    local service state
    if probe_smb_ports 5; then
        log_info "SMB post-flight: all 4 containers still accepting TCP on :445"
    else
        log_warn "SMB post-flight: at least one container is no longer accepting TCP, likely died mid-run"
        for service in "${SMB_E2E_SERVICES[@]}"; do
            state=$(docker compose -p smb-consumer ps --format '{{.State}} {{.Status}}' "$service" 2>/dev/null | head -1)
            log_warn "  $service: ${state:-unknown}"
        done
    fi
    return 0
}

# ── SFTP and WebDAV ─────────────────────────────────────────────────────────
# The server specs (`server-ops-sftp.spec.ts`, `server-ops-webdav.spec.ts`) add a
# real server through the sheet and move bytes to and from it. Each stack is
# leased in its `e2e` mode (one server each).
SERVER_STACKS=(
    # stack   compose project   service                 container port   fixture dir
    "sftp     sftp-fixture      sftp-fixture-openssh    22               sftp-servers"
    "webdav   webdav-fixture    webdav-fixture-apache   80               webdav-servers"
)

start_server_stacks() {
    local entry stack project service port fixture_dir
    for entry in "${SERVER_STACKS[@]}"; do
        read -r stack project service port fixture_dir <<< "$entry"
        if ! acquire_fixture_lease "$stack" e2e; then
            log_warn "Starting $stack through $fixture_dir/start.sh e2e"
            "$DESKTOP_DIR/test/$fixture_dir/start.sh" e2e
        fi
        if wait_for_published_port "$project" "$service" "$port" $((SECONDS + 60)); then
            log_info "$stack e2e stack ready: $service accepting TCP"
        else
            log_error "$stack e2e stack NOT ready; aborting before tests"
            docker compose -p "$project" ps
            docker compose -p "$project" logs --tail=30 "$service" || true
            exit 1
        fi
        if ! docker network inspect "${project}_default" > /dev/null 2>&1; then
            log_error "$stack network '${project}_default' not found after starting the stack"
            exit 1
        fi
    done
}

# ── What the E2E container needs ────────────────────────────────────────────

# start_fixture_stacks brings every stack up and sets the two word-split
# argument strings each E2E `docker run` passes:
#   - FIXTURE_DOCKER_ARGS: one `--network` per stack (several on one `docker
#     run` need Docker 25+, API 1.44; this repo is on 29), plus `--privileged`,
#     which `mount -t cifs` inside the container needs (SYS_ADMIN alone is
#     blocked by Docker's default seccomp profile, which denies `mount`).
#   - FIXTURE_ENV_ARGS: where the app and the specs dial each server. Inside
#     the Docker networks, compose's service name is the host, on the container
#     port. SMB's are read by `virtual_smb_hosts.rs`, the servers' by
#     `e2e-shared/server-fixtures.ts`.
start_fixture_stacks() {
    start_smb_containers
    start_server_stacks

    local entry project
    FIXTURE_DOCKER_ARGS="--network $SMB_NETWORK"
    for entry in "${SERVER_STACKS[@]}"; do
        read -r _ project _ _ _ <<< "$entry"
        FIXTURE_DOCKER_ARGS="$FIXTURE_DOCKER_ARGS --network ${project}_default"
    done
    FIXTURE_DOCKER_ARGS="$FIXTURE_DOCKER_ARGS --privileged"

    FIXTURE_ENV_ARGS="-e SMB_E2E_GUEST_HOST=smb-consumer-guest -e SMB_E2E_GUEST_PORT=445 -e SMB_E2E_AUTH_HOST=smb-consumer-auth -e SMB_E2E_AUTH_PORT=445 -e SMB_E2E_50SHARES_HOST=smb-consumer-50shares -e SMB_E2E_50SHARES_PORT=445 -e SMB_CONSUMER_50SHARES_PORT=445 -e SMB_E2E_UNICODE_HOST=smb-consumer-unicode -e SMB_E2E_UNICODE_PORT=445 -e SMB_CONSUMER_UNICODE_PORT=445"
    FIXTURE_ENV_ARGS="$FIXTURE_ENV_ARGS -e SFTP_E2E_HOST=sftp-fixture-openssh -e SFTP_E2E_PORT=22 -e WEBDAV_E2E_HOST=webdav-fixture-apache -e WEBDAV_E2E_PORT=80"
}
