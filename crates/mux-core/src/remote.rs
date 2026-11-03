//! Remote shell commands. Everything runs through `sh -c '...'` so it works
//! whatever the user's login shell is (bash, zsh, fish...). Scripts contain
//! no single quotes or backslashes, and every dynamic argument is validated
//! to a safe character set, so no user input ever reaches a shell unquoted.

use crate::api::TmuxStatus;
use tmux_control::commands::{parse_sessions, SessionInfo, LIST_SESSIONS_FORMAT};
use tmux_control::SessionId;

/// Non-interactive SSH sessions often lack the PATH entries where tmux is
/// installed (Homebrew, ~/.local). UTF-8 and TERM matter if *we* end up
/// starting the tmux server: panes inherit this environment.
const PRELUDE: &str = "PATH=\"$PATH:/usr/local/bin:/opt/homebrew/bin:/home/linuxbrew/.linuxbrew/bin:$HOME/.local/bin:$HOME/bin\"; \
export PATH; [ -n \"$LANG\" ] || export LANG=C.UTF-8; TERM=xterm-256color; export TERM;";

fn sh(script: &str) -> String {
    sh_args(script, &[])
}

/// Script with positional arguments (`"$1"`...). Arguments are restricted to
/// characters that are inert inside single quotes in every common shell.
fn sh_args(script: &str, args: &[&str]) -> String {
    debug_assert!(!script.contains('\'') && !script.contains('\\'));
    let mut s = format!("sh -c '{PRELUDE} {script}' muxscape");
    for a in args {
        assert!(
            !a.is_empty() && a.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '$' | '@' | '%' | '_' | '-')),
            "unsafe remote argument {a:?}"
        );
        s.push_str(&format!(" '{a}'"));
    }
    s
}

pub fn discover_command() -> String {
    sh(&format!(
        "if command -v tmux >/dev/null 2>&1; then echo \"MUXSCAPE-TMUX $(tmux -V)\"; \
         tmux list-sessions -F \"{LIST_SESSIONS_FORMAT}\" 2>&1; else echo MUXSCAPE-NOTMUX; fi"
    ))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discovery {
    pub tmux: TmuxStatus,
    pub sessions: Vec<SessionInfo>,
    /// Unexpected output (errors), for display.
    pub error: Option<String>,
}

pub fn parse_discovery(stdout: &str) -> Discovery {
    let mut tmux = TmuxStatus::Unknown;
    let mut lines = Vec::new();
    for l in stdout.lines() {
        if let Some(v) = l.strip_prefix("MUXSCAPE-TMUX ") {
            tmux = TmuxStatus::Available { version: v.trim().to_string() };
        } else if l.trim() == "MUXSCAPE-NOTMUX" {
            tmux = TmuxStatus::NotInstalled;
        } else if matches!(tmux, TmuxStatus::Available { .. }) {
            lines.push(l);
        }
    }
    // "no server running on ..." / "error connecting to ..." = zero sessions.
    let (good, bad): (Vec<&str>, Vec<&str>) = lines.into_iter().partition(|l| l.starts_with('$'));
    let bad: Vec<&str> = bad
        .into_iter()
        .filter(|l| !l.contains("no server running") && !l.contains("error connecting to") && !l.trim().is_empty())
        .collect();
    let sessions = parse_sessions(good).unwrap_or_default();
    Discovery { tmux, sessions, error: (!bad.is_empty()).then(|| bad.join("\n")) }
}

/// Safe session name for creation (can be renamed to anything later via the
/// control channel, which needs no shell quoting).
pub fn sanitize_session_name(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') { c } else { '_' })
        .take(64)
        .collect();
    if s.is_empty() {
        "main".into()
    } else {
        s
    }
}

pub fn attach_command(session: SessionId) -> String {
    // The id ("$3") travels as a positional argument: inside the script a
    // literal $3 would be expanded by sh.
    sh_args("exec tmux -u -C attach-session -t \"$1\"", &[&session.to_string()])
}

pub fn new_session_command(name: &str) -> String {
    let n = sanitize_session_name(name);
    // -A: attach if it already exists instead of failing/duplicating.
    sh_args("exec tmux -u -C new-session -A -s \"$1\"", &[&n])
}

/// Kill a session, and the hidden pop-out companions grouped with it (they
/// share its windows: left alone they'd keep those processes running
/// invisibly).
pub fn kill_session_command(session: SessionId) -> String {
    sh_args(
        "g=$(tmux display -p -t \"$1\" \"#{session_group}\" 2>/dev/null); \
         tmux kill-session -t \"$1\" || exit 1; [ -n \"$g\" ] || exit 0; \
         tmux list-sessions -F \"#{session_group} #{session_id} #{session_name}\" 2>/dev/null | \
         while read -r gg id name; do [ \"$gg\" = \"$g\" ] || continue; \
         case \"$name\" in *_mx_*) tmux kill-session -t \"$id\";; esac; done; exit 0",
        &[&session.to_string()],
    )
}

/// Marks the hidden companion sessions behind pop-out windows.
pub const POPOUT_MARK: &str = "_mx_";

pub fn is_popout_name(name: &str) -> bool {
    name.contains(POPOUT_MARK)
}

/// A companion session in `target`'s group: the same windows, but its own
/// current window, so a second app window can show a different tab (tmux's
/// current window is per session, not per client). Prints the new id.
pub fn new_popout_session_command(target: SessionId, name: &str) -> String {
    assert!(is_popout_name(name));
    sh_args(
        "tmux new-session -d -P -F \"#{session_id}\" -t \"$1\" -s \"$2\"",
        &[&target.to_string(), name],
    )
}

/// Remove a pop-out companion: only ever a session with the marker in its
/// name, so a mix-up can never kill a real session. Its windows live on in
/// the original session.
pub fn kill_popout_session_command(session: SessionId) -> String {
    sh_args(
        "case \"$(tmux display -p -t \"$1\" \"#{session_name}\" 2>/dev/null)\" in *_mx_*) tmux kill-session -t \"$1\";; esac",
        &[&session.to_string()],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_parsing() {
        let d = parse_discovery("MUXSCAPE-TMUX tmux 3.4\n$0|5|1|10|20|0\n$3|1|0|11|21|work stuff\n");
        assert_eq!(d.tmux, TmuxStatus::Available { version: "tmux 3.4".into() });
        assert_eq!(d.sessions.len(), 2);
        assert_eq!(d.sessions[1].name, "work stuff");
        assert!(d.error.is_none());

        let d = parse_discovery("motd junk\nMUXSCAPE-TMUX tmux 3.3a\nno server running on /tmp/tmux-1000/default\n");
        assert!(d.sessions.is_empty() && d.error.is_none());

        let d = parse_discovery("MUXSCAPE-NOTMUX\n");
        assert_eq!(d.tmux, TmuxStatus::NotInstalled);

        let d = parse_discovery("MUXSCAPE-TMUX tmux 3.4\nsome failure\n");
        assert_eq!(d.error.as_deref(), Some("some failure"));
    }

    #[test]
    fn session_ids_are_positional_args() {
        let c = attach_command(SessionId(3));
        assert!(c.ends_with("attach-session -t \"$1\"' muxscape '$3'"), "{c}");
    }

    #[test]
    fn real_sh_receives_the_literal_id() {
        // Run the generated command through a real /bin/sh with tmux
        // replaced by echo to see what tmux would receive.
        let c = attach_command(SessionId(7)).replace("exec tmux", "exec echo");
        let out = std::process::Command::new("sh").arg("-c").arg(&c).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "-u -C attach-session -t $7");
    }

    #[test]
    fn commands_are_shell_safe() {
        assert_eq!(sanitize_session_name("my session's; rm -rf"), "my_session_s__rm_-rf");
        assert_eq!(sanitize_session_name(""), "main");
        for c in [attach_command(SessionId(3)), new_session_command("x'y"), discover_command(), kill_session_command(SessionId(1))] {
            // one single-quoted script plus single-quoted safe arguments
            assert_eq!(c.matches('\'').count() % 2, 0, "{c}");
            assert!(c.starts_with("sh -c '"));
        }
    }
}
