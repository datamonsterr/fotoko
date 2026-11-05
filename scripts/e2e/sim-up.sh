#!/usr/bin/env bash
# Start the simulated stack for the web e2e and write its addresses to $FOTOKO_E2E_DIR/env.sh
# (default scripts/e2e/.sim/env.sh). Stop it with sim-down.sh.
#
#   broker (fotoko-sim-broker)  <-  fotoko-local (API)  <-  gateway (static app + API proxy)
#        ^                              ^
#   2 x fotoko-pi-agent (sim)      payos-mock (hosted checkout page, signed webhooks)
#
# Environment:
#   FOTOKO_E2E_DIR         state directory (logs, pids, printer output)
#   FOTOKO_E2E_SKIP_BUILD  1 = use the binaries and web export of the last run
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
sim="${FOTOKO_E2E_DIR:-$here/.sim}"

# A previous run that was not stopped.
bash "$here/sim-down.sh" >/dev/null

free_port() {
    node -e 'const s = require("node:net").createServer().listen(0, "127.0.0.1", () => { console.log(s.address().port); s.close(); });'
}

rm -rf "$sim/logs" "$sim/pids" "$sim/pi" "$sim/objects"
mkdir -p "$sim/logs" "$sim/pids"

broker_port="$(free_port)"
api_port="$(free_port)"
mock_port="$(free_port)"
if [ "${FOTOKO_E2E_SKIP_BUILD:-0}" = 1 ] && [ -f "$sim/ports" ]; then
    # The web export has the gateway address built in, so a reused export needs the same port.
    gateway_port="$(cat "$sim/ports")"
else
    gateway_port="$(free_port)"
fi
gateway_url="http://127.0.0.1:$gateway_port"
api_url="http://127.0.0.1:$api_port"
mock_url="http://127.0.0.1:$mock_port"

checksum_key=e2e-checksum-key
client_id=e2e-client-id
api_key=e2e-api-key

cd "$root"

if [ "${FOTOKO_E2E_SKIP_BUILD:-0}" != 1 ]; then
    echo "sim-up: building the Rust binaries"
    cargo build -p fotoko-e2e -p fotoko-api -p fotoko-pi-agent --features fotoko-api/local \
        --bin fotoko-sim-broker --bin fotoko-payos-mock --bin fotoko-local --bin fotoko-pi-agent
    echo "sim-up: exporting the web app (local mode, API $gateway_url)"
    rm -rf "$sim/dist-web"
    CI=1 EXPO_PUBLIC_FOTOKO_MODE=local EXPO_PUBLIC_API_URL="$gateway_url" \
        pnpm --filter @fotoko/mobile exec expo export -p web --output-dir "$sim/dist-web" \
        >"$sim/logs/web-build.log" 2>&1 || {
        tail -n 40 "$sim/logs/web-build.log" >&2
        exit 1
    }
    echo "$gateway_port" >"$sim/ports"
fi

bin="$(cargo metadata --format-version 1 --no-deps | node -e 'let s = ""; process.stdin.on("data", (d) => (s += d)).on("end", () => console.log(JSON.parse(s).target_directory));')/debug"

start() {
    local name="$1"
    shift
    "$@" </dev/null >"$sim/logs/$name.log" 2>&1 &
    echo "$!" >"$sim/pids/$name.pid"
}

alive() {
    kill -0 "$(cat "$sim/pids/$1.pid")" 2>/dev/null
}

fail() {
    echo "sim-up: $1" >&2
    tail -n 30 "$sim/logs/$2.log" >&2 || true
    bash "$here/sim-down.sh" >/dev/null || true
    exit 1
}

wait_http() { # name url
    for _ in $(seq 1 300); do
        alive "$1" || fail "$1 exited" "$1"
        if curl -fsS -o /dev/null "$2" 2>/dev/null; then return 0; fi
        sleep 0.1
    done
    fail "$1 did not answer on $2" "$1"
}

wait_log() { # name text
    for _ in $(seq 1 300); do
        alive "$1" || fail "$1 exited" "$1"
        if grep -q "$2" "$sim/logs/$1.log"; then return 0; fi
        sleep 0.1
    done
    fail "$1 never logged '$2'" "$1"
}

wait_port() { # name port
    for _ in $(seq 1 300); do
        alive "$1" || fail "$1 exited" "$1"
        if (exec 3<>"/dev/tcp/127.0.0.1/$2") 2>/dev/null; then return 0; fi
        sleep 0.1
    done
    fail "$1 did not listen on $2" "$1"
}

echo "sim-up: starting"
start broker env FOTOKO_BROKER_ADDR="127.0.0.1:$broker_port" "$bin/fotoko-sim-broker"
wait_port broker "$broker_port"

start api env \
    FOTOKO_LOCAL_ADDR="127.0.0.1:$api_port" \
    FOTOKO_LOCAL_PUBLIC_URL="$gateway_url" \
    FOTOKO_LOCAL_DIR="$sim/objects" \
    FOTOKO_LOCAL_SECRET=e2e-url-secret \
    FOTOKO_LOCAL_PRINTERS=stand-001,stand-002 \
    FOTOKO_MQTT_URL="mqtt://127.0.0.1:$broker_port" \
    PAYOS_BASE_URL="$mock_url" \
    PAYOS_CLIENT_ID="$client_id" PAYOS_API_KEY="$api_key" PAYOS_CHECKSUM_KEY="$checksum_key" \
    PUBLIC_APP_URL="$gateway_url" \
    "$bin/fotoko-local"
wait_http api "$api_url/healthz"

start payos-mock env \
    PAYOS_MOCK_ADDR="127.0.0.1:$mock_port" PAYOS_MOCK_PUBLIC_URL="$mock_url" \
    FOTOKO_API_URL="$api_url" \
    PAYOS_CLIENT_ID="$client_id" PAYOS_API_KEY="$api_key" PAYOS_CHECKSUM_KEY="$checksum_key" \
    "$bin/fotoko-payos-mock"
wait_http payos-mock "$mock_url/healthz"

for printer in stand-001 stand-002; do
    mkdir -p "$sim/pi/$printer/out" "$sim/pi/$printer/state"
    cat >"$sim/pi/$printer/agent.toml" <<TOML
printer_id = "$printer"
[mqtt]
host = "127.0.0.1"
port = $broker_port
tls = false
keep_alive_secs = 5
[printer]
backend = "sim"
[sim]
out_dir = "$sim/pi/$printer/out"
paper = 200
[storage]
state_dir = "$sim/pi/$printer/state"
allow_insecure_http = true
[agent]
heartbeat_secs = 5
TOML
    start "pi-$printer" "$bin/fotoko-pi-agent" --config "$sim/pi/$printer/agent.toml"
done
wait_log pi-stand-001 "connected to broker"
wait_log pi-stand-002 "connected to broker"

start gateway env GATEWAY_PORT="$gateway_port" API_URL="$api_url" STATIC_DIR="$sim/dist-web" \
    node "$here/gateway.mjs"
wait_http gateway "$gateway_url/healthz"

cat >"$sim/env.sh" <<ENV
export FOTOKO_E2E_WEB_URL="$gateway_url"
export FOTOKO_E2E_API_URL="$api_url"
export FOTOKO_E2E_PAYOS_URL="$mock_url"
export FOTOKO_E2E_SIM_OUT_STAND_001="$sim/pi/stand-001/out"
export FOTOKO_E2E_SIM_OUT_STAND_002="$sim/pi/stand-002/out"
export FOTOKO_E2E_PI_PID_STAND_002="$sim/pids/pi-stand-002.pid"
ENV
echo "sim-up: ready, web $gateway_url (source $sim/env.sh)"
