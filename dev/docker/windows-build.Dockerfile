# Cross-compile the Windows desktop app (x86_64-pc-windows-msvc) from Linux.
# Uses cargo-xwin (downloads the MSVC CRT + Windows SDK headers/libs) and
# makensis for the installer. Usage: dev/docker/build-windows.sh
FROM rust:1-trixie
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
      clang lld llvm nsis nodejs npm \
    && rm -rf /var/lib/apt/lists/*
# cargo-xwin drives clang-cl / lld-link / llvm-lib. Debian doesn't ship a clang-cl
# name, but clang picks its MSVC-compatible driver mode from argv[0], so a symlink works.
RUN ln -s "$(command -v clang)" /usr/local/bin/clang-cl \
    && clang-cl --version >/dev/null && lld-link --version >/dev/null && llvm-lib --version >/dev/null
RUN rustup target add x86_64-pc-windows-msvc && cargo install --locked cargo-xwin
