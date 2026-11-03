# Contributing to Muxscape

Thanks for helping. Muxscape is an alpha; bug reports with clear steps are as
valuable as code.

## Reporting a bug

Please include: your OS and Muxscape version, the server's OS and
`tmux -V`, what you did, what you expected and what happened. Screenshots help.
Never paste passwords, private keys or vault files. For security issues, see
[SECURITY.md](SECURITY.md) instead of opening a public issue.

## Development setup

Prerequisites: Rust (stable), Node.js 20+, and the
[Tauri prerequisites](https://tauri.app/start/prerequisites/) for your OS.

```sh
npm install            # root + ui/
npm run dev            # desktop app with hot-reloading UI
cargo test             # unit tests (no servers needed)
(cd ui && npm test)    # UI unit tests
```

The UI also runs in a browser against the real core, which is how most UI
work and the end-to-end tests are done:

```sh
(cd ui && npm run build)
MUXSCAPE_TOKEN=secret cargo run -p mux-devserver   # http://127.0.0.1:7878/?token=secret
```

The dev server is for development only. While it runs, anything that knows
the token can drive your SSH connections, so pick a random token and don't
leave it running.

### Live tests (real SSH + tmux)

Live tests run against disposable Docker containers, never your own tmux:

```sh
ssh-keygen -t ed25519 -N "" -f dev/keys/test_ed25519
ssh-keygen -t ed25519 -N hunter2 -f dev/keys/test_ed25519_pass
dev/test-server/up.sh                         # :2222 tmux 3.4, :2223 tmux 3.3a, :2224 no tmux
MUXSCAPE_LIVE=1 cargo test -- --test-threads=1
```

Browser end-to-end tests (`ui/e2e/*.mjs`, Playwright) drive the dev server
against the same containers: start the dev server with `dev/run-devserver.sh`,
then `node ui/e2e/smoke.mjs`. Several suites expect a freshly started dev
server.

## Pull requests

- Keep changes focused; one feature or fix per PR.
- tmux is the source of truth: don't cache session state the server can tell
  you.
- Add or update tests: a unit test for logic, a live test for anything that
  talks to SSH or tmux, an e2e test for UI flows.
- Anything that builds a remote shell command must go through the quoting
  helpers in `crates/mux-core/src/remote.rs`.
- Secrets never go to the webview.
- The repo isn't formatted with default `rustfmt` settings; please don't
  reformat whole files.

By contributing you agree that your contributions are licensed under the MIT
licence.
