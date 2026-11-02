#!/usr/bin/env bash
# Run the browser dev bridge against the docker test servers.
set -euo pipefail
cd "$(dirname "$0")/.."
SCRATCH=${SCRATCH:-$(mktemp -d)}
[ -f "$SCRATCH/hosts.json" ] || sed "s|@REPO@|$PWD|g" dev/hosts.dev.json > "$SCRATCH/hosts.json"
export MUXSCAPE_HOSTS_FILE="$SCRATCH/hosts.json"
export MUXSCAPE_KNOWN_HOSTS="$SCRATCH/known_hosts"
export MUXSCAPE_SSH_CONFIG=${MUXSCAPE_SSH_CONFIG:-none}
export MUXSCAPE_TOKEN=${MUXSCAPE_TOKEN:-devtoken}
exec cargo run -q -p mux-devserver
