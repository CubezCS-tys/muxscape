# Launch copy (drafts)

Everything here describes what's on `main` today. Edit freely; nothing has
been posted anywhere.

## Product description (one paragraph)

Muxscape is a desktop SSH client for people who work on remote Linux
machines. Its terminals are real tmux sessions on your servers, shown as
ordinary tabs and split panes you can click, drag and rename, with no prefix
keys to remember. Because everything runs in tmux on the server, closing the
app, losing Wi-Fi or restarting your laptop doesn't stop anything: Muxscape
reconnects and rebuilds the workspace exactly as tmux has it. It also handles
the chores around remote work: saved hosts and your `~/.ssh/config`, host-key
checks, one-click port forwarding for dev servers, and a quick switcher for
when you have twenty windows open.

## GitHub "About" description

> Desktop SSH client whose terminals are real tmux sessions: windows as tabs, panes as splits, persistent across disconnects. Rust + Tauri.

Topics: `ssh`, `tmux`, `terminal`, `ssh-client`, `tauri`, `rust`,
`xterm-js`, `remote-development`, `developer-tools`.

## README introduction

The README now opens with this (see `README.md`):

> **Your servers. Your terminals. One workspace.**
>
> If you work on remote Linux machines, you probably live in tmux: it's the
> only way a build, a dev server or a coding agent survives your laptop
> closing. But twenty tmux windows behind a prefix key is a memory test, and
> every SSH client treats tmux as just another program running in a terminal.
>
> Muxscape is a desktop SSH client (saved hosts, folders, `~/.ssh/config`,
> keys, agents, jump hosts) whose terminals are *real tmux sessions on the
> server*, presented as ordinary GUI tabs and split panes. …

## Launch post

**Title options**

- Show HN: Muxscape, an SSH client where tmux windows are just tabs
- I built a GUI for tmux over SSH because I kept losing track of 20 windows

**Body**

I spend most of my day SSH'd into Linux boxes: a couple of work servers, a
VPS, a machine at home. Everything that matters runs in tmux, because that's
the only way a long build, a dev server or a coding agent survives my laptop
going to sleep.

The problem is that I'd end up with twenty tmux windows per server and no
idea which number held what. I'd cycle through them with the prefix key,
forget which session the logs were in, and copy-paste `ssh -L` commands every
time something started on a new port.

I wanted the persistence of tmux with the convenience of a normal graphical
terminal, so I built Muxscape.

- It connects over plain SSH and drives the server's own tmux through
  control mode. Nothing gets installed on the server.
- Each tmux window is a tab and each pane is a split. You can click window
  17, drag a divider, double-click to rename, and fuzzy-find any window with
  Ctrl+Shift+P.
- tmux stays the source of truth. If you attach from a plain terminal you
  see the same session, and changes show up both ways.
- Close the app or drop off Wi-Fi and everything keeps running. Muxscape
  reconnects by itself and rebuilds the tabs and panes.
- When a dev server prints `localhost:5173`, one click forwards it over the
  same SSH connection and opens it in your browser.

It's a Rust core (russh plus a tmux control-mode client) inside a Tauri app,
with xterm.js drawing each pane at tmux's exact size. The test suite runs it
against real SSH and tmux servers in Docker (tmux 3.3 and 3.4), including
killing the connection mid-session and checking that everything comes back.

It's an early alpha, Windows only for now (that's what I use day to day).
Linux and macOS should be possible with Tauri but I haven't tested them yet.
It's free and will be MIT-licensed.

I'd love feedback from people who live in tmux. What breaks, what's missing,
and what would make you switch (or not)?

[link to the website / repo]

## Release notes: v0.1.0-alpha.1 (draft)

**Muxscape 0.1.0 alpha 1: first public alpha (Windows x64)**

Muxscape is a desktop SSH client whose terminals are real tmux sessions on
your servers, shown as tabs and split panes.

Highlights:
- tmux windows as tabs and panes as splits: resize by dragging, rename,
  zoom, close, quick switcher (`Ctrl+Shift+P`), find in pane (`Ctrl+Shift+F`).
- Persistent workspaces: automatic reconnect and restore after dropped
  connections and app restarts.
- Saved hosts, folders and `~/.ssh/config` hosts (`Include`, `IdentityFile`,
  `ProxyJump`), keys, ssh-agent, Pageant.
- One-click port forwarding for dev servers, plus a Ports panel.
- Optional encrypted password vault.
- Paste screenshots or drop files to upload them to the server.

Before you install:
- The installer isn't code-signed yet, so SmartScreen will warn. Choose
  *More info → Run anyway*, and check the SHA-256 below.
- Servers need SSH and, for persistence, tmux (3.3+ tested).
- Known limitations: no `ProxyCommand` or `Match` support in ssh_config yet.
  See the README.

SHA-256: `<filled in by the release workflow>`
