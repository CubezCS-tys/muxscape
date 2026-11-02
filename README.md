# Muxscape

**The SSH workspace that never closes.**

Muxscape is a desktop SSH client (saved hosts, folders, `~/.ssh/config`,
keys, agents, jump hosts) whose terminals are *real tmux sessions on the
server*, presented as ordinary GUI tabs and split panes. tmux stays invisible:
no prefix keys, no status bar — but close the laptop, lose Wi‑Fi, or quit the
app, and every shell, editor and long-running job keeps running. Reconnect and
the workspace is rebuilt from the server's actual state.

- **Tabs are tmux windows.** Click window 17 directly. `+` runs `new-window`,
  double-click renames, `×` kills it. Scroll the strip or press
  `Ctrl+Shift+P` to fuzzy-jump among dozens of windows.
- **Panes are tmux panes**, each rendered by its own xterm.js terminal at
  tmux's exact cell geometry. Split, focus, zoom, close, and drag the
  dividers with the mouse — every action is a real tmux command.
- **tmux is the source of truth.** Changes made from any other tmux client
  (another laptop, a plain `ssh` + `tmux attach`) show up immediately.
- **Servers without tmux** get a normal PTY shell.

## Running it

### Desktop app (your laptop)

Prerequisites: [Rust](https://rustup.rs), Node.js ≥ 20, and the
[Tauri system dependencies](https://tauri.app/start/prerequisites/) for your OS
(macOS: Xcode command line tools; Windows: WebView2 + MSVC build tools; Linux:
`libwebkit2gtk-4.1-dev` and friends).

```sh
npm install          # also installs ui/
npm run dev          # Tauri dev build with hot-reloading UI
npm run build        # release bundle (.dmg / .msi / .deb / .AppImage)
```

Hosts from `~/.ssh/config` show up automatically under **SSH config**; add
others with `+`. Known hosts are shared with OpenSSH (`~/.ssh/known_hosts`).
Saved hosts are stored in the app config directory (`hosts.json`, no secrets).

On Linux you can also build without installing any system packages:
`dev/docker/build-linux.sh` (outputs to `dist/`).

### In a browser (development / headless testing)

`crates/mux-devserver` serves the same UI and exposes the same core over a
token-protected WebSocket bound to `127.0.0.1`:

```sh
(cd ui && npm run build)
MUXSCAPE_TOKEN=secret cargo run -p mux-devserver
# open http://127.0.0.1:7878/?token=secret
```

## Architecture

```
┌──────────────────────────── desktop app (Tauri) ────────────────────────────┐
│  ui/ React + TypeScript + xterm.js                                          │
│   Sidebar (hosts/folders/sessions) · TabBar (windows) · Workspace (panes)   │
│        │  mux_request (async)   mux_send (ordered keystrokes)   ▲ mux-event │
├────────┼────────────────────────────────────────────────────────┼───────────┤
│  src-tauri/  thin shell: hosts the core, forwards requests/events           │
├─────────────────────────────────────────────────────────────────────────────┤
│  crates/mux-core     hosts · ssh_config · connection manager · reconnect    │
│                      AttachmentTask (one per attached tmux session)         │
│  crates/mux-ssh      russh transport: known_hosts, keys/agent/password,     │
│                      ProxyJump, exec / exec-stream / PTY channels           │
│  crates/tmux-control I/O-free protocol: parser, escaping, layouts, model,   │
│                      command builders, pane snapshot reconstruction         │
└─────────────────────────────────────────────────────────────────────────────┘
              │ one SSH connection per server, multiplexed channels
              ▼
   server: sshd ─┬─ exec  `tmux list-sessions …`          (discovery)
                 ├─ exec  `tmux -u -C attach -t $3`       (one per open session)
                 └─ pty   login shell                     (servers without tmux)
```

The five product modules map onto the code like this:

| Module | Where |
|---|---|
| SSH host & credential management | `mux-core/src/hosts.rs`, `ssh_config.rs`; prompts in the UI |
| SSH connection lifecycle & transport | `mux-ssh`, `mux-core/src/core.rs` |
| tmux Control Mode integration | `tmux-control`, `mux-core/src/control.rs`, `remote.rs` |
| Terminal rendering & workspace | `ui/src` (`terminals.ts`, `components/Workspace.tsx`, `TabBar.tsx`) |
| Session restoration & sync | `mux-core/src/attachment.rs`, `tmux-control/src/snapshot.rs` |

Nothing is installed on servers: only `sshd` and (optionally) `tmux`.

## Control Mode: what it gives us, and the hard parts

`tmux -C` turns a tmux client into a line protocol on stdin/stdout. Commands
go in as lines; every command gets exactly one `%begin … %end|%error` reply,
in order; asynchronous `%notifications` report output and structural changes.
Findings that shaped the design (verified against tmux 3.3a and 3.4 sources
and live servers):

- **Attached to one session.** A control client streams the panes of *one*
  session (all of its windows). Muxscape opens one control channel per
  session you have open, all multiplexed over the host's single SSH
  connection; the session list comes from cheap `exec` queries and is
  refreshed on `%sessions-changed`.
- **No TTY needed.** `tmux -C` works over a plain SSH exec channel (verified),
  so there's no terminal-in-a-terminal and no DCS wrapping (`-CC`).
- **Framing is byte-level.** `%output` payloads escape bytes `< 0x20` and `\`
  as octal and pass everything else raw, so lines may contain split or
  invalid UTF-8. The parser works on bytes; text is decoded only where tmux
  guarantees text. Lines *inside* a reply block may themselves start with
  `%`; only the guard with the same time+number closes a block. Replies
  flagged `0` (the attach itself, config commands) are not ours and are
  skipped when matching replies FIFO.
- **Ordering is the hardest part.** A reply and the notifications around it
  form a single ordered stream. Restoring a pane (`capture-pane`) or reloading
  the layout (`list-windows`) must be applied at exactly the point in that
  stream where tmux produced it, or stale `%layout-change`/`%output` lines get
  applied on top of newer state. The control client therefore delivers
  "tagged" replies *in-stream*, and pane snapshots are requested as an atomic
  batch (one write: `display` + three `capture-pane`s). tmux drains its whole
  command queue before servicing pane output again, so the batch observes a
  single instant; output before the reply is dropped (it's in the capture)
  and output after it is applied live.
- **Restoring a pane faithfully** needs more than text: scrollback,
  the visible screen, the *alternate* screen when vim/htop is running (with
  the normal screen saved behind it), cursor, scroll region and input modes
  (application cursor keys, mouse reporting…). `snapshot.rs` rebuilds all of
  that as a byte stream for a blank emulator. Gotcha: with zero history,
  `capture-pane -S -n -E -1` returns screen line 0, so `history_size` from the
  same batch decides whether to use it.
- **Sizes.** A control client doesn't affect window sizes until it sends
  `refresh-client -C WxH`. The GUI measures its area in xterm cells, sends
  that, and renders whatever layout tmux decides (which may be smaller if
  another client constrains it) — panes are positioned at exact cell offsets
  and the 1-cell gaps between them become the draggable dividers.
- **Resizing by dragging.** A divider drag is applied as a rewritten layout
  string (`select-layout`, checksum included), absorbing the change in the
  panes adjacent to the divider. Because `select-layout` assigns panes in
  pane-list order, Muxscape first checks that order matches the layout and
  otherwise falls back to `resize-pane -L/-R/-U/-D`.
- **Input** goes through `send-keys -H` (hex), which is byte-exact for any
  key sequence, UTF-8, mouse reports and pastes.
- **Quoting.** Commands are quoted for tmux's parser (`'…'\''…'`), and
  newline-free. Remote shell commands go through `sh -c '…'` with ids passed
  as positional arguments, because `$3` (a session id) inside the script
  would be expanded by `sh`.
- **Limitations.** tmux sanitises some characters in window names (`$` is
  stored escaped). Output tmux generates itself (copy mode, choose-tree) isn't
  sent to control clients — Muxscape scrolls and selects in xterm.js instead.
  Very old tmux (< 3.0) is untested.

## Persistence and reconnects

- Quitting the app or calling **Detach** only ends the control client; the
  session and its processes are untouched.
- A watchdog per host notices a dead transport (keepalives every 15 s). The
  host goes to *reconnecting*, retries with backoff, then re-attaches every
  session it had by id (after checking it still exists). Attachment ids stay
  the same, so the UI simply receives fresh snapshots and the layout reappears.
- On restart, the UI reopens the sessions you had open (matched by id and
  name, since ids are reused after a tmux server restart).

## Security

- Host keys are verified against `~/.ssh/known_hosts` (shared with OpenSSH).
  Unknown keys require an explicit decision showing the SHA-256 fingerprint;
  **changed keys are refused outright** and never offered for acceptance.
- Authentication order mirrors OpenSSH: explicit identity files, then the
  agent (SSH_AUTH_SOCK; on Windows the OpenSSH agent pipe, then Pageant),
  default keys, keyboard-interactive, password. Passphrases and passwords are
  asked for at connect time and never written to disk.
- No user input reaches a remote shell unquoted (see `remote.rs`, tested
  against a real `/bin/sh`).
- The dev bridge binds to localhost and requires a token.

## Keyboard

All shortcuts avoid plain Ctrl keys so terminal programs keep them:
`Ctrl+Shift+T` new tab · `Ctrl+Tab`/`Ctrl+PgDn` next · `Alt+1…9` go to tab ·
`Ctrl+Shift+P` quick switcher · `Ctrl+Shift+D`/`E` split right/down ·
`Alt+Shift+Arrows` focus pane · `Ctrl+Shift+Z` zoom · `Ctrl+Shift+W` close
pane · `Ctrl+Shift+C`/`V` copy/paste · right-click copies a selection or pastes.

## Tests

```sh
cargo test                        # protocol, layout, model, ssh_config, hosts, quoting
(cd ui && npm test)               # layout geometry / fuzzy matching

# Live tests against disposable docker servers — never your real tmux:
dev/test-server/up.sh             # 127.0.0.1:2222 tmux 3.4, :2223 tmux 3.3a, :2224 no tmux
MUXSCAPE_LIVE=1 cargo test -- --test-threads=1
```

The live suite (`crates/mux-core/tests/live_core.rs`) runs the acceptance
scenario end to end over real SSH against real tmux, on both tmux versions:
a session with 20 windows appears as 20 tabs; window 17 is selected directly;
typing works, including vim with UTF-8, checked through a real VT emulator;
panes are split and a divider is dragged (verified via `tmux display`);
windows created/renamed/killed from a plain `ssh` client show up; the sshd
connection is killed mid-session and Muxscape reconnects, finds the layout and
the vim screen intact and the background loop still running; a fresh app
instance re-attaches to the same session. `mux-ssh/tests/live_ssh.rs` covers
host-key trust/rejection/mismatch, passphrase keys and ProxyJump.

`ui/e2e/smoke.mjs` drives the real UI in headless Chromium through the dev
bridge against the same servers.

## Status / roadmap

Phases 1–4 of the original plan are implemented and tested (see above).
Next up:

- Saved passwords/passphrases in the OS keychain (`keyring`, feature-gated).
- Port forwarding UI (the transport already multiplexes channels).
- Per-host terminal settings (font, theme, scrollback), drag-to-reorder tabs.
- Flow control (`pause-after`) for very chatty panes on slow links.
- Host certificates (`@cert-authority`).
