# Alpha release checklist

Work through this in order. Items marked **(approval)** change something
public or are hard to undo: do them only when the maintainer says so.

## 1. Decide (before building the release candidate)

- [ ] Final bundle identifier. `dev.muxscape.app` names the app-data folder
      (`%APPDATA%\dev.muxscape.app`) and Tauri warns about the `.app` suffix.
      Changing it later needs a data migration. See READINESS.md blocker 4.
- [ ] Copyright holder for `LICENSE` (currently "The Muxscape contributors").
- [ ] Commit author emails: keep them, or rewrite to the GitHub noreply
      address **before** the first public push (a history rewrite). Blocker 9.
- [ ] Website domain (placeholder `https://muxscape.dev` in
      `website/src/site.config.ts`).

## 2. Fix the release blockers (READINESS.md §6)

- [ ] Host key of a new type: show a distinct warning with the known
      fingerprints, or refuse. Add a live test.
- [ ] `ProxyCommand` hosts: refuse with "not supported yet" instead of
      dialling `HostName` directly.
- [ ] Third-party licences file shipped with the app (fonts are OFL-1.1;
      Apache-2.0, MPL-2.0 and Unicode notices).
- [ ] Publisher, copyright and homepage in `src-tauri/tauri.conf.json`.
- [ ] Clippy warning; untrack `ui/tsconfig.tsbuildinfo`.

## 3. Build the release candidate

- [ ] Versions agree: `Cargo.toml`, `src-tauri/tauri.conf.json`, both
      `package.json` files, `CHANGELOG.md`.
- [ ] `cargo test`, `(cd ui && npm test && npm run typecheck)`.
- [ ] Live suite against the Docker servers:
      `MUXSCAPE_LIVE=1 cargo test -- --test-threads=1`.
- [ ] Browser suites (`ui/e2e/*.mjs`), each on a fresh dev server.
- [ ] Installer built on a clean machine: the `release.yml` workflow (draft),
      or `dev/docker/build-windows.sh` locally. Record the SHA-256.

## 4. Manual test on Windows (fresh user account or VM if possible)

- [ ] Install. SmartScreen shows the expected warning and *Run anyway* works.
- [ ] First run with no hosts: the welcome screen explains what to do.
- [ ] Hosts from `~/.ssh/config` appear. Add a host by hand.
- [ ] Unknown host: the fingerprint prompt appears. *Trust* saves it to
      `known_hosts`.
- [ ] Wrong password: a clear error, and it can be retried.
- [ ] Unreachable host: a clear error within ~15 s.
- [ ] Server without tmux: a plain shell opens.
- [ ] Open an existing tmux session with many windows: tabs, splits, rename,
      drag a divider, zoom, close a pane (confirms first).
- [ ] Disconnect the network for 60 s: "reconnecting", then the same
      workspace, with background jobs still running.
- [ ] Quit and reopen the app: sessions reopen.
- [ ] Start a dev server: the port chip appears, *Open* works, the Ports
      panel shows it.
- [ ] Paste a screenshot (Win+Shift+S, Ctrl+V) and drop a file on a pane.
- [ ] Vault: save a password, quit, reopen, unlock, connect without typing it.
- [ ] Uninstall. App removed. Data folder is left in place (as documented).

## 5. Publish (approval)

- [ ] Make the repository public. Push `main` only, never `--all` (the
      `overnight` branch is unreleased work).
- [ ] Publish the draft GitHub release with notes from `docs/release/LAUNCH.md`
      and the SHA-256.
- [ ] Set `downloadUrl` and `githubUrl` in `website/src/site.config.ts`,
      rebuild and deploy the website.
- [ ] Set the GitHub About text and topics.
- [ ] Post the launch announcement.
