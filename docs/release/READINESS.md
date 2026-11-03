# Muxscape: first public alpha readiness

Audited tree: branch `alpha-launch` (= `main`, HEAD `4dea5e0`), 2026-11-03.
Evidence is cited as `path:line`. Every security finding is labelled **CONFIRMED** (traced in code) or **SUSPECTED** (needs a test to confirm).

Checks run during the audit:
- `cargo test -q` (non-live): all green.
- `cargo clippy -q` (default members): **1 warning** ("block may be rewritten with `?`"), so `-D warnings` in CI would fail today.
- `ui`: `tsc -b` is clean, and `vitest` passes 9 tests in 2 files.
- Live tests and the docker servers were not touched.

---

## 1. Packaging

### Windows installer (as configured)

**Build path.** `dev/docker/build-windows.sh` builds inside Docker on Linux:
- It runs `tauri build --runner cargo-xwin --target x86_64-pc-windows-msvc --bundles nsis` (lines 15-16) in `rust:1-trixie`, with clang-cl, lld and makensis (`dev/docker/windows-build.Dockerfile`).
- Output goes to `dist/windows/`: `Muxscape_0.1.0_x64-setup.exe` plus the bare `muxscape.exe`.
- The result is unsigned.
- `tauri.conf.json` has `"targets": "all"` (line 29). On a Windows CI runner this also builds an MSI, which needs WiX. Pass `--bundles nsis` explicitly there too.

**`src-tauri/tauri.conf.json`:**

| Item | Value | Notes |
|---|---|---|
| productName / version | `Muxscape` / `0.1.0` (l.3-4) | Same as the workspace `Cargo.toml` (`0.1.0`), `package.json` and `ui/package.json`. Nothing checks they stay in sync. |
| identifier | `dev.muxscape.app` (l.5) | **Problem.** The Tauri CLI warns about identifiers that end in `.app`: "conflicts with the application bundle extension on macOS" (the string is in `@tauri-apps/cli` 2.12.4). The identifier also names the app-data folder, so changing it after release orphans users' `hosts.json` and `vault.json`. Fix it before alpha. Also check you control `muxscape.dev`. |
| publisher / copyright / homepage / license | **not set** | Without `bundle.publisher`, Tauri falls back to the second identifier segment (`muxscape`). That shows in Apps & Features and in the exe's CompanyName. No LegalCopyright is set. |
| `bundle.windows` / `nsis` | **not set** | Tauri defaults apply: per-user install (`installMode: currentUser`, no UAC prompt, installs under `%LOCALAPPDATA%\Muxscape`), WebView2 `downloadBootstrapper` (needs internet only if WebView2 is missing; Win10 21H2+ and Win11 ship it), English UI, app icon reused for the installer. Make these explicit so they're reviewable. |
| icons | `src-tauri/icons/*` | Custom brand icon (not the Tauri default). `icon.ico` holds 6 sizes from 16 px up, plus icns and the Square*/StoreLogo set. Fine. |
| windows | main 1320×820, min 720×420, `dragDropEnabled:false` (l.12-22) | |
| console | `src-tauri/src/main.rs:2` uses `windows_subsystem="windows"` in release | No console window. It also means **no logs anywhere** in release builds (see §2). |
| source maps | `ui/vite.config.ts:9` sets `sourcemap: true` | `.map` files ship inside the app. Harmless for open source, but it adds size. |

**Where data lives** (`src-tauri/src/lib.rs:44-51`; `crates/mux-ssh/src/lib.rs:158-166`):

| Data | Windows | Linux |
|---|---|---|
| hosts.json, vault.json | `%APPDATA%\dev.muxscape.app\` (Roaming) | `~/.config/dev.muxscape.app/` |
| Webview storage (localStorage: open workspace, appearance prefs; `ui/src/store.ts:335`, `ui/src/prefs.ts:69`) | `%LOCALAPPDATA%\dev.muxscape.app\EBWebView` | `~/.local/share/dev.muxscape.app` |
| known_hosts | `%USERPROFILE%\.ssh\known_hosts` (shared with OpenSSH; created if missing) | `~/.ssh/known_hosts` |
| "Remember vault on this device" key | Windows Credential Manager, generic credential, service `Muxscape vault`, persisted with `CRED_PERSIST_ENTERPRISE` (roams with roaming profiles; keyring 3.6.3 `windows.rs:246`) | not supported (`vault.rs:474`) |
| Uploaded files | server-side `~/.cache/muxscape/uploads/` (never cleaned up) | same |

**Uninstall** (Tauri NSIS template, checked in the CLI binary):
- The uninstaller shows a "Delete the application data" checkbox that is **unchecked by default**.
- Only when ticked does it `RmDir /r "$APPDATA\${BUNDLEID}"` and `"$LOCALAPPDATA\${BUNDLEID}"`.
- So by default `hosts.json` and `vault.json` survive uninstall. Good for upgrades, but say so.
- The Credential Manager entry is **never** removed, even when the box is ticked. known_hosts entries also stay, which is correct because they're shared with OpenSSH.
- Recommendations:
  - Document this.
  - Optionally add an `NSIS_HOOK_POSTUNINSTALL` that runs `cmdkey /delete` for the `Muxscape vault` entries when the box is ticked.
  - Or make "Reset vault" the documented way to clear it first.

**First-run experience (no hosts, no `~/.ssh/config`):**
- Sidebar: "No hosts yet" with an *Add a host* button and the hint "Hosts from ~/.ssh/config appear here automatically" (`ui/src/components/Sidebar.tsx:232-237`).
- Main area: a welcome panel with three bullets and *Add a host* (`ui/src/components/Workspace.tsx:278-290`).
- With an existing `~/.ssh/config`, its concrete aliases appear under the "SSH config" folder (`crates/mux-core/src/hosts.rs:117-140`).
- Nothing is written until the user saves a host. The vault is created on demand.
- The first connection to a new server shows the "Unknown host key" dialog (`ui/src/components/Overlays.tsx:37-57`).
- **Gap:** the app never says which `~/.ssh/config` directives it ignores (ProxyCommand etc., see §3.4). A user whose hosts depend on ProxyCommand gets a confusing connection error.

### Linux packaging readiness
- **Build environment:** `dev/docker/tauri-build.Dockerfile` installs `libwebkit2gtk-4.1-dev libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev`. `build-linux.sh` defaults to `BUNDLES=deb` and also picks up AppImage and rpm output.
- **What a release needs:**
  1. Build on **ubuntu-22.04** (or older-glibc Debian), not trixie or 24.04. The `.deb` and AppImage inherit the build host's glibc and webkit2gtk-4.1 floor. trixie output won't run on Ubuntu 22.04.
  2. For AppImage, add `patchelf` and `file` to the image. Expect webkit2gtk rendering issues on some NVIDIA/Wayland setups; the documented workaround is `WEBKIT_DISABLE_DMABUF_RENDERER=1`.
  3. Add `bundle.linux.deb` metadata (section, depends are auto-detected).
  4. `libayatana-appindicator3-dev` isn't needed: there's no tray.
  5. The "remember vault" option is Windows/macOS only (`crates/mux-core/src/vault.rs:474`). On Linux add keyring's `sync-secret-service` (Secret Service) feature, or keep it documented as unsupported.
  6. The Linux webview cannot be bundled (system webkit2gtk), so list it in the README.
- **Status:** buildable today, but not release-grade. Ship Windows first as planned and offer Linux as "build from source" or an experimental `.deb`.

### Unsigned installer and SmartScreen
- **What users see:** an unsigned `setup.exe` downloaded from GitHub gets the Mark-of-the-Web. SmartScreen shows "Windows protected your PC", and users must click *More info → Run anyway*. Some corporate machines (SmartScreen enforced, WDAC/AppLocker) will block it outright.
- **Effect on reputation:** the warning never goes away for unsigned builds, because reputation is tied to the file hash and resets every release.
- **Defender false positives:** NSIS and Rust binaries occasionally trigger them. Submit releases to Microsoft's WDSI portal if that happens.
- **Options, cheapest first:**
  1. **Ship unsigned for the alpha and document it.** Add a README section with a screenshot of *More info → Run anyway*. Publish SHA-256 checksums alongside the release; GitHub release assets already show digests, but list them in the notes too. Build in CI so the binary is traceable to a commit (§5).
  2. **SignPath Foundation:** free code signing for qualifying OSS projects, with signing done in their CI integration. Requires an approved OSI licence (needs the LICENSE file, §4) and a public repo.
  3. **Azure Trusted Signing (Artifact Signing):** about USD 10/month (Basic), Microsoft-managed certificates, reputation builds on the publisher identity rather than per file. Identity validation is restricted by country and entity type; individuals were initially US/Canada only, so check current eligibility. Integrates through `bundle.windows.signCommand` (e.g. `trusted-signing-cli` or `signtool` with the dlib) or a `azure/trusted-signing-action` step in CI.
  4. **OV code-signing certificate** (Sectigo, SSL.com, Certum; about USD 200-400/yr, or less with Certum's open-source certificate). Since 2023 the keys must live on an HSM or cloud signer. SmartScreen reputation still has to build up over downloads. EV certificates no longer bypass SmartScreen instantly, so they're not worth the premium here.
- **Recommendation:** option 1 for the alpha. Start SignPath or Trusted Signing in parallel, before any wider launch.
- **Updater:** there is none (no `tauri-plugin-updater`). Alpha users must re-download manually. Adding the updater needs a Tauri update-signing key, separate from Authenticode.

---

## 2. Reliability

| Scenario | Behaviour (code) | Tested by |
|---|---|---|
| **SSH drop** | A monitor polls `is_closed()` every 500 ms (`crates/mux-core/src/core.rs:604-616`). Keepalive is 15 s × 3 (`crates/mux-ssh/src/lib.rs:262-266`), so a silent network drop is noticed after about 45 s; a RST immediately. `reconnect_loop` (`core.rs:618-675`) marks the host Reconnecting, drops plain shells (expected), and redials with backoff 1 s → 30 s cap, **forever** until the user disconnects. It then re-attaches each attachment by session id and closes those whose session vanished (`core.rs:679-710`). Pinned tunnels' listeners stay up, holding new connections for up to 20 s (`core.rs:1188-1200`). | `crates/mux-core/tests/live_core.rs` `acceptance` (pkill sshd mid-session; vim screen and background loop survive), `port_forwarding`, `vault_saves_and_reuses_passwords` (reconnect with saved password, no prompt) |
| **Control channel ends but SSH is fine** (tmux crashed, attach failed) | Re-attach after 300 ms (`core.rs:224-238`) | — |
| **Restore existing tmux sessions** | `AttachTarget::Auto` picks the most recently active non-pop-out session, else creates `main` (`core.rs:779-792`). A second attach to an already-shown session reuses the attachment (`core.rs:794-804`). On app start the UI reopens saved views matched by id *and* name (ids are reused after a tmux restart) (`ui/src/actions.ts:114-132`). `new-session -A` prevents duplicates (`crates/mux-core/src/remote.rs:93-97`). | `live_core.rs` `acceptance` ("a brand-new app instance finds the same session"), `new_and_auto_sessions_never_duplicate` |
| **Wrong credentials** | Order: explicit keys → agent → default keys → keyboard-interactive (≤5 rounds) → password (3 prompts). Then `AuthFailed("…(tried: …)")` (`crates/mux-ssh/src/lib.rs:401-476`), and the host goes to `Error` with that message (`core.rs:576-579`). A wrong typed password is never saved to the vault (`core.rs:598-601`, `1317-1363`). A saved password the server rejects is marked stale and dropped after the next successful login. | `live_ssh.rs` `passphrase_key_and_auth_failure`; `live_core.rs` vault tests |
| **Unreachable host** | TCP connect (including DNS) has a 15 s timeout, then "could not connect … timed out" (`lib.rs:282-288`). **SUSPECTED gap:** there's no timeout on the SSH handshake (`client::connect_stream`, `lib.rs:290`) or on the jump-host `direct-tcpip` open (`lib.rs:294-297`). A port that accepts TCP but never speaks SSH, or a hung bastion, can leave the host "Connecting" indefinitely while holding `conn.connecting` (`core.rs:560`). | not tested |
| **Server without tmux** | Discovery prints `MUXSCAPE-NOTMUX` (`remote.rs:35-40`). Auto-attach refuses (`core.rs:782-784`). The UI toasts and opens a plain PTY shell instead (`ui/src/actions.ts:77-80`). | `live_core.rs` `server_without_tmux_gets_plain_shell` (port 2224) |
| **Old tmux** | No version gate. Only tmux 3.3a and 3.4 are tested, and the README says < 3.0 is untested (README.md:176). **SUSPECTED:** tmux 2.x (e.g. CentOS 7) misbehaves in odd ways. Recommend a clear "tmux ≥ 3.0 required, opening a plain shell" fallback. | — |
| **Reconnect needs a human** (password not saved, unknown host key after "Trust once") | Each retry re-prompts. A prompt waits up to 600 s (`core.rs:1286`), then the attempt fails and the loop retries. On timeout the core never removes the UI dialog (`ui/src/store.ts:251` only adds), so a stale dialog can remain that does nothing when answered. Low. | — |
| **Logging** | `tracing` to stdout only (`src-tauri/src/lib.rs:36-38`), and the release exe has no console. Users can't send logs. **Should fix for alpha supportability:** add a rolling log file in the app log dir (e.g. `tauri-plugin-log` or `tracing-appender`) plus a "Open logs folder" menu item. | — |

**Config bug (CONFIRMED, low):** `ServerAliveInterval` is parsed (`crates/mux-core/src/ssh_config.rs:229,253`) but never copied into `SshTarget.keepalive_secs`. `hosts.rs:217-239` doesn't set it, so it always defaults to 15 s.

### Can a UI action kill remote processes unexpectedly?

Detach versus kill semantics are clean:
- Detaching means sending an empty line on the control channel (`crates/mux-core/src/control.rs:207-209`).
- Disconnect, app quit, host delete and connection loss **only detach** (`core.rs:712-751`, comment at `core.rs:725`; `src-tauri/src/lib.rs:7-8`).
- The live test asserts "Closing the app = detach, never kill" (`live_core.rs`, acceptance step 7).

| Action | Effect | Guard |
|---|---|---|
| Tab ×, middle-click on tab, tab menu "Close window" (`ui/src/components/TabBar.tsx:50,88,102`) | `kill-window`, which ends its processes | Confirm dialog "Processes running in it will be terminated" (`ui/src/actions.ts:161-167`) |
| `Ctrl+Shift+W` (`ui/src/shortcuts.ts:24`) | `kill-pane` | Confirm (`actions.ts:202-207`) |
| Sidebar "Kill session…" (`Sidebar.tsx:32`) | `kill-session` plus its `_mx_` pop-out companions (`remote.rs:102-111`) | Confirm with the window count (`actions.ts:99-103`) |
| Closing a plain-shell view (`ui/src/App.tsx:61`) | Ends the shell and its processes (no tmux to keep them) | **No confirm.** Low; add one when the shell has a foreground job, or always. |
| Closing a pop-out window | Kills only the hidden companion session, and only if its name contains `_mx_` (`remote.rs:192-197`). The windows live on. | Name check |
| "Detach" button / sidebar Detach (`App.tsx:56`, `Sidebar.tsx:31`) | Detach only | — |

Issues:
- **Destructive confirms autofocus the danger button** (`ui/src/components/Overlays.tsx:102`, `autoFocus={d.kind === "confirm"}`). A user typing in a terminal who hits `Ctrl+Shift+W` by accident and then Enter kills the pane. Recommend focusing *Cancel* for `danger` dialogs. **CONFIRMED, low-medium (UX/data loss).**
- A real user session whose name contains `_mx_` is hidden from auto-attach (`core.rs:786`). If it's in the same session group as a killed session, it's also killed (`remote.rs:106-108`). Edge case. Use a more specific marker (e.g. `__muxscape_popout_`) or tag companions with a tmux user option (`@muxscape-popout 1`) instead of a name substring.
- The Ports panel probes every listed port with `GET /` (`crates/mux-core/src/ports.rs:399-447`), skipping known databases. That's mostly harmless, but it is traffic the user didn't explicitly ask for. Document it.
- Remote tmux options such as `destroy-unattached on` in the user's `.tmux.conf` make *any* detach kill the session. That's inherent to tmux, but worth a FAQ line. **SUSPECTED,** untested.

---

## 3. Security

Severity scale: High / Medium / Low / Info.

### 3.1 Host-key verification
- **CONFIRMED (OK):** keys are checked against `~/.ssh/known_hosts` for every hop, jump hosts included (`crates/mux-ssh/src/lib.rs:190-226`, recursive connect at `lib.rs:293`).
  - Unknown key: a prompt shows host:port, algorithm and SHA-256 fingerprint, with *Cancel / Trust once / Trust & remember* (`ui/src/components/Overlays.tsx:37-57`).
  - Same-algorithm mismatch: refused with a MITM warning and never offered for acceptance (`lib.rs:201-204`; test `live_ssh.rs` "must never offer to accept a changed key").
  - Hashed entries (`|1|`) are supported by russh.
  - Host certificates are refused (`lib.rs:191-196`).
- **CONFIRMED, Medium: algorithm-switch downgrade to TOFU.**
  - russh's `check_known_hosts_path` returns `Ok(false)` ("unknown") when known_hosts holds keys for this host only under *other* algorithms (russh 0.65 `keys/known_hosts.rs:33-39`). Muxscape then shows the normal first-connect prompt (`lib.rs:200, 208`).
  - So a MITM presents, say, an ECDSA or RSA key for a host recorded only with ed25519, and the user sees the ordinary "Unknown host key" dialog. That contradicts the README claim that "changed keys are refused outright" (README.md:238-240).
  - OpenSSH at least orders its host-key algorithms to prefer known types and warns that other key types are already known.
  - **Fix:** call `known_host_keys_path` yourself. If any entry exists for host:port but none matches the presented algorithm, either refuse like a mismatch or show a distinct red warning listing the known fingerprints. Also set `client::Config.preferred.key` to put the already-known algorithms first. Add a live test.
- **CONFIRMED, Low: known_hosts parsing gaps (russh).**
  - `@revoked` and `@cert-authority` markers are ignored, because the first token never matches a host.
  - Wildcard host patterns (`*.corp`) aren't matched.
  - Lines are split on single spaces.
  - A matching line whose key fails to parse makes the whole lookup `Err`, which Muxscape treats as "unknown → prompt" (`lib.rs:205-206`) instead of failing closed.
  - The **global** known_hosts files (`/etc/ssh/ssh_known_hosts`, `%ProgramData%\ssh\ssh_known_hosts`) and `UserKnownHostsFile`, `StrictHostKeyChecking` and `HostKeyAlias` from ssh_config are not consulted. Managed fleets therefore get TOFU prompts where OpenSSH would verify silently or refuse.
  - **Fix:** document this now. Later, read the global files and fail closed on lookup errors.
- **SUSPECTED, Low:** if russh negotiates `*-cert-v01` host-key algorithms by default, servers that present host certificates fail to connect ("host certificates are not supported yet") rather than falling back to the plain key. Test against a server with `HostCertificate` set.

### 3.2 Credential storage (vault)
- **CONFIRMED (good design), `crates/mux-core/src/vault.rs`:**
  - Argon2id with 64 MiB, t=3, p=1 and a random 16-byte salt wraps a random 256-bit vault key (`vault.rs:159-166`, `268-299`).
  - Every entry is XChaCha20-Poly1305 with a random 192-bit nonce and the slot name as AAD (`vault.rs:155-184`).
  - KDF parameters from the file are bounds-checked (`vault.rs:55-61`).
  - Changing the password re-keys every entry (`vault.rs:334-381`).
  - The key is zeroized on lock, with auto-lock after 8 h idle (`core.rs:78, 165-179`).
  - Minimum master length is 8 (`vault.rs:39`).
  - Saves are atomic (tmp + rename) and `0600` on Unix (`vault.rs:244-266`). On Windows the file inherits the per-user `%APPDATA%` ACL.
- **CONFIRMED: secrets never reach the webview.**
  - No `Event` variant carries a secret (`crates/mux-core/src/api.rs:176-200`). `VaultStatus` is metadata only (`vault.rs:126-137`). `PromptRequest` carries no secrets (`api.rs:204-212`).
  - Secrets travel only webview → core, when the user types them, wrapped in `SecretString` so `Debug` redacts them (`api.rs:15-23`).
  - Vault slot names (`password:user@host:port`, key paths) are visible to the UI and stored in cleartext. That's by design.
- **CONFIRMED, Low/Info: "Remember on this device".**
  - Stores the raw vault key in Windows Credential Manager or the macOS Keychain (`vault.rs:470-503`). Any same-user process can read it (the UI says so).
  - On Windows, keyring uses `CRED_PERSIST_ENTERPRISE` and `vault.json` sits in Roaming AppData. In domain environments with roaming profiles, "this device" actually follows the user to other machines.
  - **Fix:** document it, or use `CRED_PERSIST_LOCAL_MACHINE` (keyring has no option for this; a small custom Credential Manager call would be needed).
- **Info:** `hosts.json` is written with default permissions (`crates/mux-core/src/hosts.rs:105-114`). It holds no secrets, but it does hold hostnames and usernames. Consider `0600` on Unix.

### 3.3 Auth prompts
- **CONFIRMED (OK):**
  - Prompts are core → UI events with a 600 s timeout (`core.rs:1281-1289`).
  - Keyboard-interactive answers are never saved (`core.rs:1404-1415`).
  - Password and passphrase "remember" only commits after a successful login (`core.rs:598-601`).
  - Field types honour the server's `echo` flag (`Overlays.tsx:68`).
  - Server-supplied `instructions` and prompt labels render as React text, not HTML (no `dangerouslySetInnerHTML` anywhere in `ui/src`).
- **SUSPECTED, Low:**
  - Before falling back to password, every default key plus every agent identity is offered (`lib.rs:406-439`). Users with many agent keys may hit the server's `MaxAuthTries` ("Too many authentication failures") before reaching password auth. The same happens with OpenSSH, but there users can set `IdentitiesOnly` per host. Here `identities_only` exists for saved hosts, so it's mostly fine. Document it.

### 3.4 ssh_config parsing (`crates/mux-core/src/ssh_config.rs`)
- **Honoured:**
  - `Host` with wildcards and negation, `Include` (globs, ~, relative to `~/.ssh`, depth ≤ 8), first-value-wins.
  - `HostName` (with `%h`), `User`, `Port`, `IdentityFile` (accumulating; `%d %u %r %h %p ~` expanded), `IdentitiesOnly`, `ProxyJump` (chains, `none`) (`ssh_config.rs:160-254`).
- **Parsed but ignored:** `ServerAliveInterval` (see §2).
- **Silently ignored** (CONFIRMED):
  - `Match` blocks are treated as never matching (`ssh_config.rs:165`).
  - **`ProxyCommand`**, `HostKeyAlias`, `UserKnownHostsFile`, `GlobalKnownHostsFile`, `StrictHostKeyChecking`, `CertificateFile`, `IdentityAgent`, `ForwardAgent`, `LocalForward`/`RemoteForward`, `PreferredAuthentications`, `ConnectTimeout`, `AddressFamily`.
- **ProxyCommand:**
  - **Not supported, and not executed.** That's good from a "the app runs arbitrary local commands" standpoint.
  - But a host that depends on it (AWS SSM, `cloudflared access ssh`, corporate bastion scripts) is dialled **directly** at its `HostName`. That's confusing at best. At worst it connects to a different machine that answers on that name, though host-key checking still applies.
  - **Fix (Medium, UX/security):** when a resolved host has `ProxyCommand` (or `Match` blocks are present), mark the host "unsupported: uses ProxyCommand" and refuse to dial. List the unsupported directives in the README.

### 3.5 File uploads
- **CONFIRMED (OK):**
  - The remote path is fixed: `$HOME/.cache/muxscape/uploads/<base>.<ext>`, created under `umask 077`, so the directory is 0700 and files are 0600 (`crates/mux-core/src/remote.rs:113-120`).
  - The name is generated: the stem is sanitised to `[A-Za-z0-9_-]` (40 chars max) plus a UTC timestamp and 16 random bits. The extension is alphanumeric only, 10 chars max (`remote.rs:124-152`).
  - Arguments are additionally asserted to a safe charset (`remote.rs:22-33`), and tests cover hostile names (`remote.rs:247-262`).
  - Maximum size is 50 MB after base64 decode (`core.rs:444-448`).
  - Data travels on stdin of an exec channel, so no SFTP is needed (`crates/mux-ssh/src/lib.rs:339-355`).
  - Live test: `uploads_land_byte_exact_and_private`. E2E: `ui/e2e/uploads.mjs`.
- **Low:**
  - The path printed by the server is typed into the terminal unquoted (`ui/src/actions.ts:594`). It breaks if `$HOME` contains spaces. A hostile server could make it type arbitrary text, but only into its own shell, so there's no privilege gain.
  - Uploads are never cleaned up on the server.
  - On the devserver the 50 MB upload is about 67 MB of base64, which exceeds the WebSocket 64 MiB cap (`crates/mux-devserver/src/main.rs:97`). That only affects dev.
  - The 16-bit random suffix plus one-second timestamp could collide and overwrite with `cat >`. Negligible; use `set -C` (noclobber) or more random bits.

### 3.6 Port forwarding
- **CONFIRMED (OK):**
  - Listeners bind only `127.0.0.1` and, best-effort, `::1` (`core.rs:1418-1446`). There's no option for `0.0.0.0` (README claim verified).
  - The remote end is always `localhost:<port>` on the server (`core.rs:1068, 1174`).
  - The URL `path` must start with `/` (`core.rs:1071`).
  - Pinned forwards persist in `hosts.json` and reopen on connect (`core.rs:1114-1132`).
- **SUSPECTED, Low:**
  - If `::1:<port>` is already taken by another local process while `127.0.0.1:<port>` is free, Muxscape still uses that port (the v6 bind failure is ignored, `core.rs:1437`).
  - The UI then opens `http://localhost:<port><path>` (`ui/src/actions.ts:268-270`). Browsers usually try `::1` first, so the request, including tokens in the path such as Jupyter's `?token=`, can go to the *other* local process.
  - **Fix:** open `http://127.0.0.1:port`, or treat `EADDRINUSE` on `::1` as "port busy" and move to the next port.
- **Info:**
  - Any local process or user can use a forwarded loopback port, same as `ssh -L`.
  - Link clicks only auto-forward `localhost`, `127.0.0.1`, `0.0.0.0` and `[::1]` URLs, and only on Ctrl/⌘+click (`ui/src/terminals.ts:169-174`, `actions.ts:407-422`).

### 3.7 Dev server (`crates/mux-devserver`)
- **CONFIRMED: not shipped.**
  - `src-tauri/Cargo.toml` depends only on `mux-core` and `mux-ssh`.
  - `tauri build` builds the `muxscape` package only.
  - The devserver is a separate binary crate in `default-members` (root `Cargo.toml:5`).
- **CONFIRMED (OK):** binds to `127.0.0.1` only (`main.rs:88`) and requires `?token=` on `/ws` (`main.rs:94-96`). The random token is 24 alphanumeric characters (`main.rs:76-79`).
- **CONFIRMED, Medium (contributors only): predictable token, no Origin check.**
  - `dev/run-devserver.sh:10` defaults `MUXSCAPE_TOKEN=devtoken`, and the README example uses `secret` (README.md:72-73).
  - WebSockets aren't covered by the same-origin policy, and `ws()` doesn't check `Origin`.
  - So while a contributor runs the bridge, **any web page in their browser** can connect to `ws://127.0.0.1:7878/ws?token=devtoken` and drive SSH with their keys and agent.
  - **Fix:** reject upgrades whose `Origin` isn't `http://127.0.0.1:<port>` or `http://localhost:<port>`. Drop the fixed default token from the script; let e2e read the printed token or set a random one. Compare tokens in constant time.
- **Info:** the token is printed to stdout and placed in the URL, which ends up in browser history. Acceptable for a dev tool.

### 3.8 Tauri capabilities and CSP
- **CSP** (`src-tauri/tauri.conf.json:24-26`): `default-src 'self' ipc: http://ipc.localhost; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self' data:`.
  - No `unsafe-eval` or inline scripts, and `connect-src` falls back to `default-src`. Good.
  - `'unsafe-inline'` styles are needed by xterm and React inline styles. Acceptable.
- **Capabilities** (`src-tauri/capabilities/default.json`), applied to `main` and `popout-*`: `core:default`, clipboard read/write text, **read image**, `opener:default`, `core:webview:allow-create-webview-window`, `core:window:allow-close`.
  - `opener:default` (plugin 2.7.2) restricts `openUrl` to `http(s)://`, `mailto:` and `tel:`. That's **CONFIRMED** from the plugin's `permissions/default.toml`, so terminal-supplied `file:`, `ms-*` or `javascript:` URLs can't be launched. It also grants `allow-reveal-item-in-dir`, which is unused and could be dropped.
  - Clipboard read (text and image) is needed for paste and screenshot upload.
- **App commands:** only `mux_request` and `mux_send` exist (`src-tauri/src/lib.rs:63`). They expose the whole core API, so any script executing in the webview has full SSH power. That makes XSS the critical boundary.
  - No HTML sinks were found: no `dangerouslySetInnerHTML`, `innerHTML` or `eval` in `ui/src`.
  - Server-controlled strings (tmux names, probe page titles, prompt texts) render as React text.
- **SUSPECTED, Low:**
  - xterm's default OSC 8 hyperlink handler (no `linkHandler` option is set in `ui/src/terminals.ts:205-216`) calls `confirm()` and then `window.open()`. How WebView2 handles that `window.open` (new webview window, or blocked) isn't verified.
  - **Fix:** set `linkHandler` to route through `openLink`, which goes through the opener's scope.

### 3.9 Remote command construction (`crates/mux-core/src/remote.rs`)
- **CONFIRMED (OK):**
  - Every script is a constant without `'` or `\` (debug-asserted, `remote.rs:23`), wrapped in `sh -c '…'`.
  - Every dynamic value is a positional argument asserted (with a hard `assert!`, also in release) to `[A-Za-z0-9$@%_-]` (`remote.rs:22-33`).
  - Session names for creation are sanitised (`remote.rs:74-85`).
  - Session ids are typed `SessionId` (`$N`).
  - Upload names are generated (§3.5).
  - The tmux-level commands sent over the control channel use `quote()`, which single-quotes and strips control characters and newlines (`crates/tmux-control/src/escape.rs:43-61`). Ids are typed.
  - The port scan is a constant script sent as `echo <base64> | base64 -d | sh` (`crates/mux-core/src/ports.rs:93-97`).
  - Tests run generated commands through a real `/bin/sh` (`remote.rs:227-245`).
  - **No shell-injection path found.**
- **Low:** the `assert!` in `sh_args` panics the async task (Tokio contains the panic) if a future caller passes an unsafe argument. Returning `Result` would be friendlier, but there's no exploit.

### 3.10 Other
- **Info:** tmux session and window renames use `quote()`, which turns newlines and control characters into spaces, so they can't inject commands. Window names that contain `#{…}` may be format-expanded by tmux in some contexts. Cosmetic only.
- **Info:** `pagehide` → `closePopout` from a pop-out (`ui/src/actions.ts:39-41`) and the Rust `WindowEvent::Destroyed` hook (`src-tauri/src/lib.rs:66-75`) both remove only `_mx_` companions. That's safe.

---

## 4. Open-source readiness

### Licence
- **There is no `LICENSE` file in the repo** (`git ls-files` shows none). The workspace sets `license = "MIT"` (`Cargo.toml:11`), but neither `package.json` has a `license` field, and the README has no licence section.
- MIT requires the copyright notice and licence text to accompany copies, so a public repo or binary without them is effectively "all rights reserved" plus a contradictory manifest. **Blocker.** Add `LICENSE` (MIT, "Copyright (c) 2026 <name>"), plus `"license": "MIT"` in both `package.json` files, and `repository`/`authors` in `Cargo.toml`.

### Rust dependencies
Source: `cargo metadata --format-version 1` gives 659 packages; 653 are reachable via normal or build edges across all targets.

| Licence (as declared) | Crates |
|---|---|
| MIT and/or Apache-2.0 (incl. "MIT OR Apache-2.0", "Apache-2.0 OR MIT", "MIT/Apache-2.0", plain MIT, plain Apache-2.0) | ~565 |
| Unicode-3.0 (icu_*, zerovec, yoke, tinystr, litemap, writeable, zerotrie, potential_utf, …) | 19 (incl. unicode-ident `(MIT OR Apache-2.0) AND Unicode-3.0`) |
| Zlib / Unlicense / ISC / BSD-2 / BSD-3 / 0BSD / CC0 options | ~50 |
| **MPL-2.0**: `cssparser`, `cssparser-macros`, `selectors`, `dtoa-short` (Tauri HTML/CSP tooling), `option-ext` (via `directories`) | 5 |
| **BSL-1.0** (Boost): `clipboard-win`, `error-code` (Windows clipboard) | 2 (+`ryu` as an Apache-2.0 OR BSL-1.0 option) |
| LGPL-2.1+ *as one option*: `r-efi` (`MIT OR Apache-2.0 OR LGPL-2.1-or-later`), UEFI only, not compiled for Windows or Linux | 2 |
| GPL-only / unknown / missing licence | **none** |

**Verdict: no conflict with distributing Muxscape under MIT.**
- MPL-2.0 is file-level copyleft. Using unmodified crates in a larger work is fine; the obligation is to tell recipients where the MPL source is available (crates.io / upstream) and keep notices. Don't patch those files without publishing the changes.
- BSL-1.0 needs no attribution for binary distribution.
- Unicode-3.0 requires including its notice with the data and software.
- Apache-2.0 requires reproducing licence text and any NOTICE files.

### npm dependencies
- **Production** (`ui/package-lock.json`, bundled into the app):
  - MIT: react, react-dom, scheduler, zustand, @xterm/xterm, addon-search, addon-unicode11, csstype, @types/react.
  - MIT OR Apache-2.0: @tauri-apps/api, plugin-clipboard-manager, plugin-opener.
  - **OFL-1.1**: `@fontsource-variable/manrope`, `@fontsource/jetbrains-mono`. The fonts are embedded in `ui/dist` and thus in the installer.
- **Dev only:** MIT, ISC, Apache-2.0, BSD-3-Clause, and `caniuse-lite` (CC-BY-4.0, build-time only, not shipped). The root `package.json` has only `@tauri-apps/cli` (Apache-2.0 OR MIT).
- OFL-1.1 allows bundling with software of any licence. The OFL text and font copyright **must be distributed with the fonts**. Vite copies only the font files, not their LICENSE.

### Obligations checklist
1. `LICENSE` (MIT) at the repo root, and include it in the installer.
2. `THIRD_PARTY_LICENSES` (or `.html`) generated in CI and shipped in the bundle. In Tauri, add it to `bundle.resources` and add an *About → Licences* link.
   - Rust: `cargo about generate` (or `cargo-bundle-licenses`).
   - npm: `npx license-checker-rseidelsohn --production` (or `generate-license-file`) over `ui/`.
   - Include the OFL-1.1 text for Manrope and JetBrains Mono, the Unicode-3.0 notice, Apache-2.0 text and NOTICEs, and a line on where the MPL-2.0 sources are available.
3. Add `cargo deny check licenses advisories` in CI with an allow-list: MIT, Apache-2.0, Apache-2.0 WITH LLVM-exception, BSD-2/3-Clause, ISC, Zlib, Unicode-3.0, BSL-1.0, MPL-2.0, CC0-1.0, 0BSD, Unlicense, MIT-0. That way a GPL dependency can't sneak in.

### Secrets / personal data scan (`git ls-files`, 104 files, plus history of `main`)
- **No private keys, API tokens, cloud keys or real passwords** in tracked files or in `main`'s history (searched for `BEGIN … PRIVATE`, `ghp_`, `AKIA`, `sk-`, `xox`, `password=`, `token=`).
  - The only "secrets" are documented test fixtures: `devtoken` / `secret` (dev bridge), `dev:devpass` in `dev/test-server/Dockerfile:10`, and Jupyter-style `?token=xyz` in tests.
- **No personal paths** (`/home/yassine`, `C:\Users\…`) in tracked files or `main`'s history.
  - The only private IPs are RFC1918 test literals in unit tests (`hosts.rs:306-333`, `tunnels.rs:373`).
  - `dev/hosts.dev.json` uses an `@REPO@` placeholder and `127.0.0.1` docker ports.
- **Gitignored and verified untracked:** `dev/keys/` (it holds 4 test keys locally; correctly not committed), `dev/test-server/authorized_keys`, `/target`, `node_modules/`, `/ui/dist`, `dist/`, `src-tauri/gen/`, `*.log`, `.playwright/`.
- **Git metadata exposes two personal emails** as commit authors: 9 commits by `CubezCS-tys <(personal email removed)` and 7 by `(personal email removed)`. Publishing `main` publishes them. If that's unwanted, rewrite authors to the GitHub `noreply` address *before* the first public push. Rewriting history is a destructive step to do deliberately.
- **Other branches:** local branch `overnight` contains files not in `main` (`mcp.rs`, `socks.rs`, `ci.rs`, `ROADMAP.md`, …). Don't `git push --all`.
- **Tracked build artifact:** `ui/tsconfig.tsbuildinfo`. Add it to `.gitignore` and untrack it.
- **`dev/`** is fine to publish. It holds docker build scripts and disposable localhost-only test servers (`dev/test-server/up.sh` binds `127.0.0.1`).

### README accuracy (`README.md`)
- **Line 53:** "release bundle (.dmg / .msi / .deb / .AppImage)". On Windows `targets:"all"` produces MSI *and* NSIS, but the documented and tested installer is the NSIS `setup.exe` (line 63).
- **Lines 236-240:** "changed keys are refused outright". Not true for a key of a *different algorithm* (§3.1). Fix the code or soften the claim.
- **Line 262:** "The dev bridge binds to localhost and requires a token". True, but the default token is fixed and there's no Origin check (§3.7).
- **Lines 321-331 (roadmap):**
  - "Saved passwords/passphrases in the OS keychain (`keyring`, feature-gated)" is stale: the vault plus keyring "remember" is already implemented (lines 246-259).
  - "Phases 1-4 of the original plan" refers to an internal document.
- **Missing:**
  - Licence section.
  - A Windows install section (SmartScreen, WebView2, where data lives, uninstall).
  - Supported / unsupported ssh_config directives (ProxyCommand!).
  - Minimum tmux version.
  - Linux "remember" unsupported.
  - Uploads feature (paste screenshot / drop file → `~/.cache/muxscape/uploads`) and Find (`Ctrl+Shift+F`) in the feature list. The shortcuts list (lines 268-273) omits `Ctrl+Shift+F`, which is in `ui/src/shortcuts.ts:61`.
  - The e2e list omits `ui/e2e/find.mjs` and `uploads.mjs`.
- **Line 278:** `cargo test` excludes `src-tauri` (by design, `Cargo.toml:4-5`). Worth a word.
- **Missing contributor docs:** `CONTRIBUTING.md`, `SECURITY.md` (how to report vulnerabilities; essential for an SSH client), and issue templates.

---

## 5. CI and release automation (recommended)

There's no `.github/` directory today. Suggested `.github/workflows/ci.yml` (on push and PR):

```yaml
jobs:
  rust:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with: { components: clippy, rustfmt }
      - uses: Swatinem/rust-cache@v2
      - run: cargo fmt --all --check            # after one `cargo fmt` commit
      - run: cargo clippy --all-targets -- -D warnings   # default members (no webkit needed); fix the 1 current warning first
      - run: cargo test                          # live tests self-skip without MUXSCAPE_LIVE
      - uses: EmbarkStudios/cargo-deny-action@v2  # licences + RustSec advisories (deny.toml)

  ui:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
        with: { node-version: 22, cache: npm, cache-dependency-path: ui/package-lock.json }
      - run: npm ci --prefix ui                  # NOT root `npm install` (its postinstall runs a non-ci install)
      - run: npm --prefix ui run typecheck
      - run: npm --prefix ui test
      - run: npm --prefix ui run build

  tauri-linux-check:                           # keeps src-tauri compiling + clippy-clean
    runs-on: ubuntu-22.04
    steps:
      - uses: actions/checkout@v4
      - run: sudo apt-get update && sudo apt-get install -y libwebkit2gtk-4.1-dev librsvg2-dev libxdo-dev patchelf
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - run: mkdir -p ui/dist && cargo clippy -p muxscape -- -D warnings
```

`.github/workflows/release.yml` (on tag `v*`, or `workflow_dispatch`):

```yaml
jobs:
  windows:
    runs-on: windows-latest
    permissions: { contents: write }
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
        with: { node-version: 22 }
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - run: npm ci --ignore-scripts && npm ci --prefix ui
      - run: node scripts/check-versions.mjs   # Cargo.toml == tauri.conf.json == package.json(s) == tag
      - uses: tauri-apps/tauri-action@v0
        env: { GITHUB_TOKEN: "${{ secrets.GITHUB_TOKEN }}" }
        with:
          tagName: ${{ github.ref_name }}
          releaseName: "Muxscape ${{ github.ref_name }} (alpha)"
          releaseDraft: true
          prerelease: true
          args: --bundles nsis
      # later: signing step (SignPath / Trusted Signing) before upload, and SHA256SUMS.txt
  linux:            # optional/experimental for alpha
    runs-on: ubuntu-22.04
    # apt deps as above + tauri-action with args: --bundles deb,appimage
```

Notes:
- The `windows-latest` build is preferable to the cross-compile for releases: native MSVC, and no cargo-xwin SDK-licence ambiguity. Keep `dev/docker/build-windows.sh` for local builds.
- Add `dependabot.yml` for cargo, npm (`/ui`) and github-actions.
- **What can't run in plain CI, and how it could:**
  - The live suite (`MUXSCAPE_LIVE=1`) and the `ui/e2e/*.mjs` Playwright tests need the docker SSH and tmux servers. GitHub's `ubuntu-latest` runners have Docker, so a **nightly or manual workflow** can do it:
    1. `ssh-keygen` the two test keys into `dev/keys/`.
    2. `dev/test-server/up.sh`.
    3. `MUXSCAPE_LIVE=1 cargo test -- --test-threads=1`.
    4. `npx playwright install chromium`.
    5. `dev/run-devserver.sh &`.
    6. `node ui/e2e/*.mjs`.
  - `services:` containers are a poor fit, because the images are built from `dev/test-server/Dockerfile` and the tests `docker exec … pkill` into them. Plain `docker run` steps are simpler.
  - The live tests also need ports 2222-2224 and 7878 free, which is true on fresh runners.
  - **Not automatable:** SmartScreen behaviour, WebView2-missing installs and real network-drop or sleep scenarios. Keep a short manual Windows smoke checklist (install → add host → TOFU prompt → attach → kill the network for 60 s → reconnect → uninstall) per release.

---

## 6. Release blockers

### Must fix before the public alpha

| # | Blocker | Why | Fix | Effort |
|---|---|---|---|---|
| 1 | **No LICENSE file** | The MIT declaration in `Cargo.toml:11` isn't a grant without the text. It also blocks SignPath and other OSS programs. | Add `LICENSE` (MIT + copyright), and `license` in both `package.json` files | 10 min |
| 2 | **No third-party notices in the bundle** | OFL-1.1 fonts, Apache-2.0, Unicode-3.0 and MPL-2.0 notice obligations (§4) | Generate `THIRD_PARTY_LICENSES` with cargo-about plus an npm licence tool, ship it via `bundle.resources`, link it from the app | 2-3 h |
| 3 | **Host-key algorithm switch silently degrades to TOFU** (§3.1, CONFIRMED, Medium) | Bypasses the "changed keys are refused" guarantee the README advertises. Easy for an active MITM. | When known_hosts has entries for host:port of other types, refuse (or show a strong, distinct warning with the known fingerprints). Prefer known algorithms. Add a live test. | 0.5-1 day |
| 4 | **Bundle identifier `dev.muxscape.app`** | Tauri warns about `.app` identifiers. It's the app-data directory name, so it can't change after users have data without a migration. | Choose the final id now (e.g. `dev.muxscape.desktop` or `io.github.<user>.muxscape`) and set `bundle.publisher`, `copyright`, `homepage` | 30 min |
| 5 | **Unsigned installer, undocumented** | SmartScreen "Windows protected your PC" with no explanation looks like malware | README install section (SmartScreen steps, WebView2, data locations, uninstall leaves data and the Credential Manager entry), SHA-256 checksums in the release notes | 1-2 h |
| 6 | **ProxyCommand / Match / global known_hosts silently ignored** (§3.4, CONFIRMED) | ProxyCommand hosts get dialled directly at `HostName`. Managed-fleet users get TOFU prompts OpenSSH wouldn't show. | Refuse to dial hosts that use ProxyCommand ("not supported yet"). Document supported and unsupported directives. | 2-4 h |
| 7 | **Release built on a dev machine, no CI** | Users can't tie a binary to a commit, and nothing gates regressions | The `ci.yml` + `release.yml` above (draft prerelease from `windows-latest`). Fix the single clippy warning. | 0.5 day |
| 8 | **README inaccuracies** (§4) | A security claim that's false today, a stale roadmap, missing install/licence/limitations sections | Edit the README. Add `SECURITY.md` with a private reporting channel. | 1-2 h |
| 9 | **Personal emails in commit authors; `overnight` branch** | Publishing `main` exposes both addresses. `push --all` would publish unreleased work. | Decide: rewrite to the GitHub noreply address before the first public push, or accept it. Push `main` only. | 15 min (rewriting is a deliberate, destructive step) |

### Should fix soon (early alpha iterations)

| # | Issue | Why | Fix | Effort |
|---|---|---|---|---|
| 10 | No log file in release builds (`src-tauri/src/lib.rs:36-38`, `main.rs:2`) | Alpha bug reports will have no diagnostics | `tauri-plugin-log` (or tracing-appender) to the app log dir, plus "Open logs folder" | 2 h |
| 11 | Devserver: fixed `devtoken` default, no `Origin` check (§3.7) | Any website can drive a contributor's SSH while the bridge runs | Check `Origin`, use a random token by default, compare in constant time | 1 h |
| 12 | No handshake / jump-open timeout (§2, SUSPECTED) | A host can hang in "Connecting" forever | `tokio::time::timeout` around `connect_stream` up to the host-key callback, and around `channel_open_direct_tcpip` | 2 h + test |
| 13 | Danger confirms autofocus the destructive button (`Overlays.tsx:102`) | Enter after an accidental `Ctrl+Shift+W` kills a pane | Focus Cancel for danger dialogs. Optionally confirm closing plain shells (`App.tsx:61`). | 30 min |
| 14 | `localhost` URL vs. a busy `::1` port (§3.6, SUSPECTED) | Tokenised URLs can go to the wrong local process | Open `127.0.0.1`, or treat a v6 `EADDRINUSE` as busy | 1 h |
| 15 | Code signing | SmartScreen friction, corporate blocking | Apply to SignPath Foundation or Azure Trusted Signing. Wire `signCommand` into the release workflow. | 1-2 days, mostly waiting |
| 16 | No auto-updater | Alpha users stay on old builds | `tauri-plugin-updater` + update key + `latest.json` from tauri-action | 0.5-1 day |
| 17 | Explicit `bundle.windows.nsis` config, version sync check | Make install mode, WebView2 mode and languages reviewable. Avoid version drift across 4 files. | Add config and a `scripts/check-versions.mjs` | 1 h |
| 18 | Uninstaller leaves the Credential Manager key | Leftover secret material (the vault key) after "delete app data" | NSIS post-uninstall hook (`cmdkey`) or document "Reset vault before uninstalling" | 1-2 h |
| 19 | Old-tmux handling, `ServerAliveInterval` ignored, `_mx_` name marker, upload path unquoted, upload cleanup, xterm OSC 8 `linkHandler`, `@revoked` / known_hosts lookup errors fail open | Low-severity correctness and hardening (§2, §3) | Small targeted fixes | 0.5-1 day total |
| 20 | Linux release (glibc floor, AppImage deps, Secret Service "remember") | Linux users | Build on ubuntu-22.04 in CI; keyring `sync-secret-service` | 1 day |
| 21 | Nightly live + e2e workflow | The most valuable tests currently run only on a dev box | Workflow per §5 | 0.5 day |
| 22 | Housekeeping | — | Untrack `ui/tsconfig.tsbuildinfo`; consider `sourcemap: false` for release; `CONTRIBUTING.md`, issue templates, dependabot, cargo-deny | 1-2 h |
