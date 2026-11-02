# Linux build environment for the Tauri desktop app (webkit2gtk 4.1).
# Usage: dev/docker/build-linux.sh  -> dist/ gets the .deb / AppImage
FROM rust:1-trixie
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
      libwebkit2gtk-4.1-dev libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev \
      nodejs npm file xdg-utils \
    && rm -rf /var/lib/apt/lists/*
