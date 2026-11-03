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
- **Pop-out windows.** Drag a tab out of the window (or right-click it →
  *Open in new window*) and it moves into its own window, like a browser tab
  (the drag shows the tab and its last lines). To put it back, drag it onto
  the main window's tab bar, press *Move back*, or close the window.
  tmux's current window belongs to the session, so a pop-out attaches to a
  hidden companion session in the same group (`tmux new-session -t`, named
  `_mx_…`): same windows, its own current tab. Closing the window removes
  the companion; killing the session removes its companions too.
- **Links.** Ctrl/⌘+click any URL, or a bare `localhost:9004/plan` as agents
  print it; local ones are forwarded over SSH first.
- **Appearance.** Four themes (Dusk, a soft slate default; Midnight, the
  original crisp near-black; Mocha, warm charcoal; Daylight, light), six
  accent colours, terminal text size, and a colour per host (its sidebar
  tile), all from the gear button. Fonts (Manrope, JetBrains Mono) are
  bundled so it looks the same everywhere.
- **Port forwarding without commands.** Start a dev server in a pane and
  Muxscape notices `http://localhost:5173/` in its output: one click forwards
  it over the existing SSH connection and opens it in your browser. Clicking a
  `localhost` link in a terminal does the same. Forwards can be pinned per host.

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
`dev/docker/build-linux.sh` (outputs to `dist/`). The Windows installer can be
cross-compiled from Linux the same way: `dev/docker/build-windows.sh`
(outputs `dist/windows/Muxscape_<version>_x64-setup.exe`, unsigned).

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
- **Session groups.** Inside a group a window belongs to several sessions,
  so `select-window -t @5` switches whichever session tmux picks (the most
  recently used); Muxscape always qualifies it (`$3:@5`). And when a window
  is unlinked from *any* session (e.g. a companion is killed), tmux sends
  `%window-close` to clients whose session still has it, so that
  notification is treated as "re-read the window list", never as a close.
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

## Port forwarding

Equivalent to `ssh -L`, without typing it:

- **Detection.** Pane and shell output is scanned (escape sequences stripped,
  lines reassembled across chunks) for `http://localhost:PORT/…`,
  `127.0.0.1:PORT`, `0.0.0.0:PORT`, `[::1]:PORT`, "listening on port N" and
  similar. Each port is offered once per host as a chip: **Open** forwards and
  opens the browser (keeping the URL path, so tokens like Jupyter's survive);
  **Forward** just forwards. Already-forwarded ports are never offered.
- **Links.** Clicking a `localhost` URL in a terminal forwards that port from
  the server the terminal belongs to, then opens it locally.
- **Ports panel** (`Ctrl+Shift+O` or the header's *Ports* button): a side
  panel listing everything listening on the server, grouped *Forwarded* /
  *Your services* / *Other services*, with a filter. Each port shows what it
  is and where it came from:
  - **what it serves**: page title and framework (Vite, Next.js, FastAPI,
    Swagger UI, Jupyter…), redirects, `https`; databases and other non-web
    services by name (PostgreSQL, Redis, SSH, SMTP…), offered as *Forward*
    rather than *Open*;
  - **what started it**: the process (`vite`, `uvicorn`, `npm run dev`), its
    folder, and the tmux session › window it runs in (click to jump there);
  - **public** when it listens on all interfaces (reachable from other
    machines).

  The scan is one remote command with nothing installed on the server:
  sockets from `/proc/net/tcp*`, owners via `/proc/*/fd`, command lines and
  parents via `ps`, working directories via `/proc/*/cwd`, panes by walking
  parent processes up to `tmux list-panes`, and `docker ps` for published
  container ports (`ports.rs`). Only the user's own processes can be
  attributed. Probing is read-only, over the SSH connection: a short wait for
  a server-first banner, then a single `GET /`. Well-known databases are
  never probed (Redis, for instance, logs HTTP requests as an attack).
  Forward by number there too (`5173`, or `8080:5173` for local:remote).
- **Wrapped output.** TUIs (coding agents included) hard-wrap long lines at
  the pane width; a port split across the wrap (`…localhost:500` + `0/app`)
  is put back together when the result is a valid port.
- **How it works.** Each forward is a local listener on `127.0.0.1` (and
  `::1`). Every accepted connection becomes a `direct-tcpip` channel to
  `localhost:PORT` on the server, over the host's existing SSH connection.
  The local port is the same as the remote one when it's free, otherwise the
  next free one. Listeners stay up while SSH reconnects; a browser request made
  during a reconnect is held (up to 20 s) and then served.
- **Pinned** forwards are saved per host in `hosts.json` (ssh-config hosts
  too) and re-opened whenever the host connects. Disconnecting a host closes
  its forwards; stopping one also unpins it.

## Security

- Host keys are verified against `~/.ssh/known_hosts` (shared with OpenSSH).
  Unknown keys require an explicit decision showing the SHA-256 fingerprint;
  **changed keys are refused outright** and never offered for acceptance.
- Authentication order mirrors OpenSSH: explicit identity files, then the
  agent (SSH_AUTH_SOCK; on Windows the OpenSSH agent pipe, then Pageant),
  default keys, keyboard-interactive, password. Passwords and key passphrases
  are only stored if you tick *Remember*, in the encrypted vault below; they
  are saved only after the login succeeds. One-time codes are never saved.
- **Password vault** (`vault.rs`, `vault.json` next to `hosts.json`):
  the master password is stretched with Argon2id (64 MiB, 3 passes, random
  salt; OWASP's minimum is 19 MiB / 2) into a key that wraps a random 256-bit
  vault key (replaced, and every entry re-encrypted, when the master password
  changes). Saved passwords are keyed by `user@host:port` *and* the jump
  chain; a saved one the server rejects is dropped once another works. Every secret is encrypted with XChaCha20-Poly1305 (RustCrypto,
  NCC-audited) using a fresh random nonce and its slot name as associated
  data, so entries can't be swapped and tampering is detected. The vault key
  stays in the Rust core's memory only while unlocked (zeroized on lock);
  secrets never reach the webview. Auto-lock after 8 hours idle. Optional
  "unlock automatically on this computer" keeps the vault key in Windows
  Credential Manager / macOS Keychain — convenient, but readable by any
  program running as the same user, as the UI says. A forgotten master
  password can't be recovered (by design); the vault can be reset.
- No user input reaches a remote shell unquoted (see `remote.rs`, tested
  against a real `/bin/sh`).
- The dev bridge binds to localhost and requires a token.
- Port forwards listen on loopback only: other machines on your network can't
  reach a forwarded service.

## Keyboard

All shortcuts avoid plain Ctrl keys so terminal programs keep them:
`Ctrl+Shift+T` new tab · `Ctrl+Tab`/`Ctrl+PgDn` next · `Alt+1…9` go to tab ·
`Ctrl+Shift+P` quick switcher · `Ctrl+Shift+D`/`E` split right/down ·
`Alt+Shift+Arrows` focus pane · `Ctrl+Shift+Z` zoom · `Ctrl+Shift+W` close
pane · `Ctrl+Shift+C`/`V` copy/paste · right-click copies a selection or pastes ·
`Ctrl+Shift+O` ports & forwarding.

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
instance re-attaches to the same session. `port_forwarding` starts an HTTP
server in a pane, checks it is detected (with its token path), that the scan
names its process, folder and tmux pane and the probe its title and server
(plus a dead port and sshd's banner), forwards it and
fetches through the tunnel (also 8 connections in parallel), scans ports,
reports a dead port, keeps serving across a killed sshd connection, and
restores a pinned forward after disconnect/connect. `mux-ssh/tests/live_ssh.rs`
covers host-key trust/rejection/mismatch, passphrase keys and ProxyJump.

`vault_saves_and_reuses_passwords` logs in with a password over real SSH:
asked once then never, reconnect after a killed connection without a prompt,
locked vault → master password (wrong one retried), declining falls back to
the server password, a changed server password is reported and replaced
(never by a typo), forget / host deletion; `vault_saves_key_passphrases` does
the same for an encrypted key. `ui/e2e/vault.mjs` covers the UI flow.

`ui/e2e/smoke.mjs` drives the real UI in headless Chromium through the dev
bridge against the same servers; `ui/e2e/popout.mjs` pops a tab out and checks both windows stay
independent and the main one keeps its tabs when it closes;
`ui/e2e/appearance.mjs` checks themes,
accent, text size, host colours and the tab-bar pane buttons; `ui/e2e/ports.mjs` does the same for port
forwarding (chip → browser tab, the Ports panel: titles, process, folder,
tmux window and jumping to it, pin/add/stop, clicking a terminal link).
The test keys in `dev/keys/` are not committed; create them with
`ssh-keygen -t ed25519 -N "" -f dev/keys/test_ed25519` and
`ssh-keygen -t ed25519 -N hunter2 -f dev/keys/test_ed25519_pass`.

## Status / roadmap

Phases 1–4 of the original plan are implemented and tested (see above).
Next up:

- Saved passwords/passphrases in the OS keychain (`keyring`, feature-gated).
- Remote (`-R`) and dynamic SOCKS (`-D`) forwards; optional auto-forward of
  detected ports.
- Per-host terminal settings (font, theme, scrollback), drag-to-reorder tabs.
- Flow control (`pause-after`) for very chatty panes on slow links.
- Host certificates (`@cert-authority`).
