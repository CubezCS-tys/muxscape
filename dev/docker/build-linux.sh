#!/usr/bin/env bash
# Build the Linux desktop bundle in Docker (no system packages needed on the host).
set -euo pipefail
cd "$(dirname "$0")/../.."
BUNDLES=${BUNDLES:-deb}
docker build -q -t muxscape-tauri-build -f dev/docker/tauri-build.Dockerfile dev/docker >/dev/null
mkdir -p dist
docker run --rm \
  -v "$PWD":/src:ro -v muxscape-target:/target -v muxscape-cargo:/usr/local/cargo/registry \
  -v "$PWD/dist":/dist \
  -e CARGO_TARGET_DIR=/target \
  muxscape-tauri-build bash -ec '
    mkdir -p /work && cd /src && tar --exclude=./node_modules --exclude=./ui/node_modules --exclude=./target --exclude=./dist -cf - . | tar -C /work -xf -
    cd /work && npm install --no-audit --no-fund >/dev/null
    npx tauri build --bundles '"$BUNDLES"' 2>&1 | tail -25
    find /target/release/bundle -maxdepth 2 -type f \( -name "*.deb" -o -name "*.AppImage" -o -name "*.rpm" \) -exec cp {} /dist/ \;
    cp /target/release/muxscape /dist/muxscape-linux-x86_64
    chown -R '"$(id -u):$(id -g)"' /dist
  '
ls -la dist
