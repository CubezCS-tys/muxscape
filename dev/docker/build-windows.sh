#!/usr/bin/env bash
# Build the Windows installer (NSIS .exe) in Docker, cross-compiled from Linux.
# Output: dist/windows/  (Muxscape_<ver>_x64-setup.exe and the bare muxscape.exe)
set -euo pipefail
cd "$(dirname "$0")/../.."
docker build -q -t muxscape-windows-build -f dev/docker/windows-build.Dockerfile dev/docker >/dev/null
mkdir -p dist/windows
docker run --rm \
  -v "$PWD":/src:ro -v muxscape-target-win:/target -v muxscape-cargo-win:/usr/local/cargo/registry \
  -v muxscape-xwin-cache:/root/.cache/cargo-xwin \
  -v "$PWD/dist/windows":/dist \
  -e CARGO_TARGET_DIR=/target \
  muxscape-windows-build bash -ec '
    mkdir -p /work && cd /src && tar --exclude=./node_modules --exclude=./ui/node_modules --exclude=./target --exclude=./dist -cf - . | tar -C /work -xf -
    cd /work && npm install --no-audit --no-fund >/dev/null
    npx tauri build --runner cargo-xwin --target x86_64-pc-windows-msvc --bundles nsis 2>&1 | tail -25
    T=/target/x86_64-pc-windows-msvc/release
    cp $T/bundle/nsis/*.exe /dist/
    cp $T/muxscape.exe /dist/muxscape.exe
    chown -R '"$(id -u):$(id -g)"' /dist
  '
ls -la dist/windows
