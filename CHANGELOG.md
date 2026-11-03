# Changelog

All notable changes to Muxscape. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [0.1.0-alpha.1] - unreleased

First public alpha (Windows x64).

### Added
- SSH client with saved hosts, folders, `~/.ssh/config` hosts (`Include`,
  `IdentityFile`, `ProxyJump`), keys, ssh-agent (OpenSSH agent pipe and
  Pageant on Windows), keyboard-interactive and passwords.
- Host-key verification against OpenSSH's `known_hosts`, with a
  fingerprint prompt for unknown hosts.
- tmux sessions as a graphical workspace: windows as tabs, panes as splits,
  resizing by dragging dividers, rename, zoom, close, quick switcher.
- Persistent workspaces: automatic reconnect and workspace restore after
  dropped connections and app restarts. Plain shells for servers without tmux.
- Pop-out windows: drag a tab into its own window and back.
- Port forwarding: detection of dev servers in terminal output, one-click
  forwarding, Ports panel (what's listening, what it is, which pane started
  it), pinned forwards, `localhost` links forwarded on Ctrl+click.
- Optional encrypted password vault (Argon2id + XChaCha20-Poly1305), with
  optional unlock via Windows Credential Manager.
- Paste screenshots and drop files into a terminal (uploaded to the server,
  path typed).
- Find in pane (`Ctrl+Shift+F`).
- Themes (Dusk, Midnight, Mocha, Daylight), accent colours, text size, host
  colours.
