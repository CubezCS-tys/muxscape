#!/usr/bin/env bash
# Starts disposable test servers bound to localhost only:
#   127.0.0.1:2222  ubuntu 24.04 + tmux 3.4   (user dev)
#   127.0.0.1:2223  debian 12    + tmux 3.3a  (user dev)
#   127.0.0.1:2224  ubuntu 24.04, NO tmux     (user dev)
set -euo pipefail
cd "$(dirname "$0")"
cat ../keys/test_ed25519.pub ../keys/test_ed25519_pass.pub > authorized_keys
docker build -q -t muxscape-test:tmux34 . >/dev/null
docker build -q -t muxscape-test:tmux33 --build-arg BASE=debian:12 . >/dev/null
docker build -q -t muxscape-test:notmux --build-arg WITH_TMUX=0 . >/dev/null
docker network inspect muxscape-net >/dev/null 2>&1 || docker network create muxscape-net >/dev/null
for spec in "muxscape-t34 2222 tmux34" "muxscape-t33 2223 tmux33" "muxscape-notmux 2224 notmux"; do
  set -- $spec
  docker rm -f "$1" >/dev/null 2>&1 || true
  docker run -d --name "$1" --hostname "$1" --network muxscape-net -p 127.0.0.1:$2:22 "muxscape-test:$3" >/dev/null
  echo "$1 -> 127.0.0.1:$2"
done
