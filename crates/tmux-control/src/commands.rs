//! Command builders and parsers for structured query output.
//!
//! Queries use `-F` formats with `|` separated fields. Every field except the
//! last is an id, number or layout (which never contain `|`); the single
//! free-text field (a name) is always last and taken verbatim, so names
//! containing `|` or spaces are handled correctly.

use crate::escape::{hex_keys, quote};
use crate::ids::{PaneId, SessionId, WindowId};
use crate::layout::{LayoutCell, LayoutError};
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum QueryError {
    #[error("unexpected query output line: {0:?}")]
    BadLine(String),
    #[error(transparent)]
    Layout(#[from] LayoutError),
}

fn fields(line: &str, n: usize) -> Result<Vec<&str>, QueryError> {
    let v: Vec<&str> = line.splitn(n, '|').collect();
    if v.len() != n {
        return Err(QueryError::BadLine(line.to_string()));
    }
    Ok(v)
}

fn num<T: std::str::FromStr + Default>(s: &str, line: &str) -> Result<T, QueryError> {
    if s.is_empty() {
        // Unknown format variables expand to "" on older tmux versions.
        return Ok(T::default());
    }
    s.parse().map_err(|_| QueryError::BadLine(line.to_string()))
}

fn id<T: std::str::FromStr>(s: &str, line: &str) -> Result<T, QueryError> {
    s.parse().map_err(|_| QueryError::BadLine(line.to_string()))
}

fn flag(s: &str) -> bool {
    s == "1"
}

// ---------------------------------------------------------------- sessions

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SessionInfo {
    pub id: SessionId,
    pub name: String,
    pub windows: u32,
    /// Number of clients attached (including other terminals).
    pub attached: u32,
    pub created: u64,
    pub activity: u64,
}

pub const LIST_SESSIONS_FORMAT: &str =
    "#{session_id}|#{session_windows}|#{session_attached}|#{session_created}|#{session_activity}|#{session_name}";

/// `list-sessions` as a control-mode command.
pub fn list_sessions() -> String {
    format!("list-sessions -F {}", quote(LIST_SESSIONS_FORMAT))
}

pub fn parse_sessions<'a>(lines: impl IntoIterator<Item = &'a str>) -> Result<Vec<SessionInfo>, QueryError> {
    let mut v = Vec::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let f = fields(line, 6)?;
        v.push(SessionInfo {
            id: id(f[0], line)?,
            windows: num(f[1], line)?,
            attached: num(f[2], line)?,
            created: num(f[3], line)?,
            activity: num(f[4], line)?,
            name: f[5].to_string(),
        });
    }
    Ok(v)
}

// ----------------------------------------------------------------- windows

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowInfo {
    pub id: WindowId,
    pub index: u32,
    pub active: bool,
    pub zoomed: bool,
    pub bell: bool,
    pub activity: bool,
    pub layout: LayoutCell,
    pub visible_layout: LayoutCell,
    pub name: String,
}

pub const LIST_WINDOWS_FORMAT: &str = "#{window_id}|#{window_index}|#{window_active}|#{window_zoomed_flag}|\
#{window_bell_flag}|#{window_activity_flag}|#{window_layout}|#{window_visible_layout}|#{window_name}";

pub fn list_windows(session: SessionId) -> String {
    format!("list-windows -t {} -F {}", quote(&session.to_string()), quote(LIST_WINDOWS_FORMAT))
}

pub fn parse_windows<'a>(lines: impl IntoIterator<Item = &'a str>) -> Result<Vec<WindowInfo>, QueryError> {
    let mut v = Vec::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let f = fields(line, 9)?;
        let layout = LayoutCell::parse(f[6])?;
        let visible_layout = if f[7].is_empty() { layout.clone() } else { LayoutCell::parse(f[7])? };
        v.push(WindowInfo {
            id: id(f[0], line)?,
            index: num(f[1], line)?,
            active: flag(f[2]),
            zoomed: flag(f[3]),
            bell: flag(f[4]),
            activity: flag(f[5]),
            layout,
            visible_layout,
            name: f[8].to_string(),
        });
    }
    v.sort_by_key(|w| w.index);
    Ok(v)
}

// ------------------------------------------------------------------- panes

/// Everything needed to faithfully reconstruct a pane's terminal state in a
/// fresh emulator (cursor, screen mode, input modes).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct PaneInfo {
    pub id: PaneId,
    pub window: WindowId,
    pub index: u32,
    pub active: bool,
    pub width: u32,
    pub height: u32,
    pub cursor_x: u32,
    pub cursor_y: u32,
    pub cursor_visible: bool,
    pub alternate_on: bool,
    pub in_mode: bool,
    pub dead: bool,
    pub app_cursor_keys: bool,
    pub app_keypad: bool,
    pub insert_mode: bool,
    pub wrap_mode: bool,
    pub origin_mode: bool,
    pub mouse_standard: bool,
    pub mouse_button: bool,
    pub mouse_any: bool,
    pub mouse_sgr: bool,
    pub mouse_utf8: bool,
    pub scroll_region_upper: u32,
    pub scroll_region_lower: u32,
    pub history_size: u32,
    pub pid: u32,
    pub current_command: String,
}

pub const LIST_PANES_FORMAT: &str = "#{pane_id}|#{window_id}|#{pane_index}|#{pane_active}|#{pane_width}|#{pane_height}|\
#{cursor_x}|#{cursor_y}|#{cursor_flag}|#{alternate_on}|#{pane_in_mode}|#{pane_dead}|#{keypad_cursor_flag}|\
#{keypad_flag}|#{insert_flag}|#{wrap_flag}|#{origin_flag}|#{mouse_standard_flag}|#{mouse_button_flag}|\
#{mouse_any_flag}|#{mouse_sgr_flag}|#{mouse_utf8_flag}|#{scroll_region_upper}|#{scroll_region_lower}|\
#{history_size}|#{pane_pid}|#{pane_current_command}";
const PANE_FIELDS: usize = 27;

/// All panes of all windows in a session.
pub fn list_panes_session(session: SessionId) -> String {
    format!("list-panes -s -t {} -F {}", quote(&session.to_string()), quote(LIST_PANES_FORMAT))
}

/// A single pane (`list-panes -t %N` would list its whole window).
pub fn describe_pane(pane: PaneId) -> String {
    format!("display-message -p -t {} {}", quote(&pane.to_string()), quote(LIST_PANES_FORMAT))
}

pub fn parse_panes<'a>(lines: impl IntoIterator<Item = &'a str>) -> Result<Vec<PaneInfo>, QueryError> {
    let mut v = Vec::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let f = fields(line, PANE_FIELDS)?;
        v.push(PaneInfo {
            id: id(f[0], line)?,
            window: id(f[1], line)?,
            index: num(f[2], line)?,
            active: flag(f[3]),
            width: num(f[4], line)?,
            height: num(f[5], line)?,
            cursor_x: num(f[6], line)?,
            cursor_y: num(f[7], line)?,
            cursor_visible: f[8] != "0",
            alternate_on: flag(f[9]),
            in_mode: flag(f[10]),
            dead: flag(f[11]),
            app_cursor_keys: flag(f[12]),
            app_keypad: flag(f[13]),
            insert_mode: flag(f[14]),
            wrap_mode: f[15] != "0",
            origin_mode: flag(f[16]),
            mouse_standard: flag(f[17]),
            mouse_button: flag(f[18]),
            mouse_any: flag(f[19]),
            mouse_sgr: flag(f[20]),
            mouse_utf8: flag(f[21]),
            scroll_region_upper: num(f[22], line)?,
            scroll_region_lower: num(f[23], line)?,
            history_size: num(f[24], line)?,
            pid: num(f[25], line)?,
            current_command: f[26].to_string(),
        });
    }
    Ok(v)
}

// ---------------------------------------------------------------- builders

pub fn t(target: impl ToString) -> String {
    quote(&target.to_string())
}

/// Tell tmux how big our viewport is (in cells). With this set, a control
/// client participates in window sizing like any other client.
pub fn refresh_client_size(cols: u32, rows: u32) -> String {
    format!("refresh-client -C {}x{}", cols.max(1), rows.max(1))
}

pub fn select_window(w: WindowId) -> String {
    format!("select-window -t {}", t(w))
}

/// Create a window at the end of the session (or right after `after`).
pub fn new_window(session: SessionId, after: Option<WindowId>, name: Option<&str>) -> String {
    let mut s = match after {
        Some(w) => format!("new-window -a -t {}", t(w)),
        // "$1:" targets the session with no index: next free index.
        None => format!("new-window -t {}", quote(&format!("{session}:"))),
    };
    if let Some(n) = name {
        s.push_str(" -n ");
        s.push_str(&quote(n));
    }
    s
}

pub fn kill_window(w: WindowId) -> String {
    format!("kill-window -t {}", t(w))
}

pub fn rename_window(w: WindowId, name: &str) -> String {
    format!("rename-window -t {} {}", t(w), quote(name))
}

/// `horizontal` = new pane to the right (`-h`), otherwise below (`-v`).
pub fn split_pane(p: PaneId, horizontal: bool) -> String {
    format!("split-window {} -t {}", if horizontal { "-h" } else { "-v" }, t(p))
}

pub fn kill_pane(p: PaneId) -> String {
    format!("kill-pane -t {}", t(p))
}

pub fn select_pane(p: PaneId) -> String {
    format!("select-pane -t {}", t(p))
}

pub fn toggle_zoom(p: PaneId) -> String {
    format!("resize-pane -Z -t {}", t(p))
}

pub fn select_layout(w: WindowId, layout: &LayoutCell) -> String {
    format!("select-layout -t {} {}", t(w), quote(&layout.to_layout_string()))
}

pub fn list_window_pane_order(w: WindowId) -> String {
    format!("list-panes -t {} -F '#{{pane_id}}'", t(w))
}

pub fn rename_session(s: SessionId, name: &str) -> String {
    format!("rename-session -t {} {}", t(s), quote(name))
}

pub fn kill_session(s: SessionId) -> String {
    format!("kill-session -t {}", t(s))
}

/// Feed raw bytes to a pane as if typed. Large inputs are split into
/// several commands to keep each control-mode line reasonably short.
pub fn send_bytes(p: PaneId, bytes: &[u8]) -> Vec<String> {
    bytes
        .chunks(256)
        .map(|c| format!("send-keys -t {} -H {}", t(p), hex_keys(c)))
        .collect()
}

/// Capture pane contents with escape sequences.
/// `history_lines`: how many lines of scrollback before the visible screen.
/// `alternate`: capture the alternate screen instead (when vim etc. is up).
/// The result is the history (if any) followed by exactly `height` lines.
pub fn capture_pane(p: PaneId, start: i64, end: Option<i64>, alternate: bool) -> String {
    let mut s = format!("capture-pane -p -e -N -t {} -S {}", t(p), start);
    if let Some(e) = end {
        s.push_str(&format!(" -E {e}"));
    }
    if alternate {
        s.push_str(" -a -q");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sessions_with_awkward_names() {
        let lines = ["$0|5|1|1757329400|1757330000|0", "$4|1|0|1|2|my | weird name ", "", "$5|1|0|1|2|"];
        let s = parse_sessions(lines).unwrap();
        assert_eq!(s.len(), 3);
        assert_eq!(s[1].name, "my | weird name ");
        assert_eq!(s[1].id, SessionId(4));
        assert_eq!(s[2].name, "");
        assert!(parse_sessions(["garbage"]).is_err());
        assert!(parse_sessions(["@1|1|1|1|1|x"]).is_err());
    }

    #[test]
    fn windows_sorted_by_index() {
        let l1 = "a87d,100x30,0,0,0";
        let lines = [
            format!("@3|7|0|0|0|1|{l1}|{l1}|later"),
            format!("@1|2|1|1|0|0|{l1}||first|with pipe"),
        ];
        let w = parse_windows(lines.iter().map(|s| s.as_str())).unwrap();
        assert_eq!(w[0].id, WindowId(1));
        assert_eq!(w[0].name, "first|with pipe");
        assert!(w[0].active && w[0].zoomed);
        assert_eq!(w[1].index, 7);
        assert!(w[1].activity);
    }

    #[test]
    fn panes_tolerate_missing_vars() {
        let mut line = "%4|@1|0|1|80|24|5|6|1|0|0|0|0|0|0|1|0|0|0|0|0|0|0|23|100|4242|bash".to_string();
        let p = &parse_panes([line.as_str()]).unwrap()[0];
        assert_eq!(p.id, PaneId(4));
        assert_eq!((p.width, p.height, p.cursor_x, p.cursor_y), (80, 24, 5, 6));
        assert_eq!(p.history_size, 100);
        assert_eq!(p.current_command, "bash");
        // Older tmux: some flags empty.
        line = "%4|@1|0|1|80|24|5|6||0|0|0||||||||||||23|0|1|".to_string();
        let p = &parse_panes([line.as_str()]).unwrap()[0];
        assert!(p.cursor_visible && p.wrap_mode && !p.mouse_any);
    }

    #[test]
    fn builders_quote_targets() {
        // `$1` must never reach a shell or tmux unquoted.
        assert_eq!(list_windows(SessionId(1)).split(' ').nth(2), Some("'$1'"));
        assert_eq!(new_window(SessionId(2), None, Some("it's")), "new-window -t '$2:' -n 'it'\\''s'");
        assert_eq!(new_window(SessionId(2), Some(WindowId(5)), None), "new-window -a -t '@5'");
        assert_eq!(split_pane(PaneId(3), true), "split-window -h -t '%3'");
        assert_eq!(send_bytes(PaneId(1), b"hi\r"), vec!["send-keys -t '%1' -H 68 69 0d"]);
        assert_eq!(send_bytes(PaneId(1), &[b'x'; 600]).len(), 3);
        assert!(send_bytes(PaneId(1), b"").is_empty());
        assert_eq!(refresh_client_size(0, 50), "refresh-client -C 1x50");
    }
}
