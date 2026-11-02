//! Rebuild a pane's terminal state in a fresh emulator.
//!
//! When we attach to an existing session (or reconnect), each pane already
//! has content: scrollback, a visible screen, maybe a full-screen app on the
//! alternate screen, and input modes (application cursor keys, mouse
//! reporting...). This module turns `capture-pane` output plus the pane's
//! state flags into a byte stream that, written to a blank terminal of the
//! pane's size, reproduces that state. Live `%output` then continues on top.
//!
//! Captures used (all with `-p -e -N`):
//! * history: `-S -<n> -E -1`             (normal-screen scrollback)
//! * screen:  default range               (what is currently displayed: the
//!   alternate screen when `alternate_on`)
//! * saved:   `-a -q` (only when `alternate_on`; the normal screen hidden
//!   behind the full-screen app)

use crate::commands::PaneInfo;

pub struct Captures<'a> {
    pub history: &'a [Vec<u8>],
    pub screen: &'a [Vec<u8>],
    /// Normal screen while the alternate screen is active.
    pub saved_normal: Option<&'a [Vec<u8>]>,
}

fn push_lines(out: &mut Vec<u8>, lines: &[&Vec<u8>]) {
    for (i, l) in lines.iter().enumerate() {
        if i > 0 {
            out.extend_from_slice(b"\r\n");
        }
        out.extend_from_slice(l);
    }
}

/// Pad/trim a captured screen to exactly `height` rows so it lands at the
/// right place regardless of trailing-empty-line behaviour.
fn screen_rows(lines: &[Vec<u8>], height: usize) -> Vec<&Vec<u8>> {
    static EMPTY: Vec<u8> = Vec::new();
    let mut v: Vec<&Vec<u8>> = lines.iter().take(height).collect();
    while v.len() < height {
        v.push(&EMPTY);
    }
    v
}

pub fn build(p: &PaneInfo, cap: &Captures) -> Vec<u8> {
    let height = p.height.max(1) as usize;
    let mut out = Vec::new();
    // Reset to a known state (RIS would also reset modes we set below, and
    // the emulator is fresh anyway, so just clear attributes and home).
    out.extend_from_slice(b"\x1b[0m\x1b[H\x1b[2J");

    let normal: &[Vec<u8>] = if p.alternate_on { cap.saved_normal.unwrap_or(&[]) } else { cap.screen };
    // History then the normal screen, as one continuous stream: with N >= H
    // lines separated by CRLF, the last H lines end up exactly on screen and
    // everything above lands in the emulator's scrollback.
    let mut lines: Vec<&Vec<u8>> = cap.history.iter().collect();
    lines.extend(screen_rows(normal, height));
    push_lines(&mut out, &lines);

    if p.alternate_on {
        out.extend_from_slice(b"\x1b[0m\x1b[?1049h\x1b[H\x1b[2J");
        push_lines(&mut out, &screen_rows(cap.screen, height));
    }

    out.extend_from_slice(b"\x1b[0m");
    // Scroll region (DECSTBM homes the cursor, so before positioning).
    let full = p.scroll_region_upper == 0 && (p.scroll_region_lower + 1 >= p.height || p.scroll_region_lower == 0);
    if !full {
        out.extend_from_slice(format!("\x1b[{};{}r", p.scroll_region_upper + 1, p.scroll_region_lower + 1).as_bytes());
    }
    let mut row = p.cursor_y + 1;
    if p.origin_mode {
        out.extend_from_slice(b"\x1b[?6h");
        row = p.cursor_y.saturating_sub(p.scroll_region_upper) + 1;
    }
    out.extend_from_slice(format!("\x1b[{};{}H", row, p.cursor_x + 1).as_bytes());

    let mut mode = |on: bool, seq: &[u8]| {
        if on {
            out.extend_from_slice(seq);
        }
    };
    mode(!p.cursor_visible, b"\x1b[?25l");
    mode(p.app_cursor_keys, b"\x1b[?1h");
    mode(p.app_keypad, b"\x1b=");
    mode(p.insert_mode, b"\x1b[4h");
    mode(!p.wrap_mode, b"\x1b[?7l");
    mode(p.mouse_standard, b"\x1b[?1000h");
    mode(p.mouse_button, b"\x1b[?1002h");
    mode(p.mouse_any, b"\x1b[?1003h");
    mode(p.mouse_utf8, b"\x1b[?1005h");
    mode(p.mouse_sgr, b"\x1b[?1006h");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(h: u32) -> PaneInfo {
        PaneInfo { width: 10, height: h, cursor_visible: true, wrap_mode: true, scroll_region_lower: h - 1, ..Default::default() }
    }

    fn lines(v: &[&str]) -> Vec<Vec<u8>> {
        v.iter().map(|s| s.as_bytes().to_vec()).collect()
    }

    #[test]
    fn normal_screen_with_history() {
        let mut p = pane(3);
        p.cursor_x = 2;
        p.cursor_y = 1;
        let h = lines(&["old1", "old2"]);
        let s = lines(&["a", "b"]); // shorter than height: padded
        let out = build(&p, &Captures { history: &h, screen: &s, saved_normal: None });
        let txt = String::from_utf8(out).unwrap();
        assert_eq!(txt, "\x1b[0m\x1b[H\x1b[2Jold1\r\nold2\r\na\r\nb\r\n\x1b[0m\x1b[2;3H");
    }

    #[test]
    fn alternate_screen_and_modes() {
        let mut p = pane(2);
        p.alternate_on = true;
        p.app_cursor_keys = true;
        p.mouse_button = true;
        p.mouse_sgr = true;
        p.cursor_visible = false;
        let s = lines(&["VIM", "~"]);
        let n = lines(&["$ vim", ""]);
        let out = String::from_utf8(build(&p, &Captures { history: &[], screen: &s, saved_normal: Some(&n) })).unwrap();
        let alt = out.find("\x1b[?1049h").unwrap();
        assert!(out[..alt].contains("$ vim"));
        assert!(out[alt..].contains("VIM\r\n~"));
        assert!(out.ends_with("\x1b[1;1H\x1b[?25l\x1b[?1h\x1b[?1002h\x1b[?1006h"));
    }

    #[test]
    fn scroll_region_and_origin() {
        let mut p = pane(10);
        p.scroll_region_upper = 2;
        p.scroll_region_lower = 7;
        p.origin_mode = true;
        p.cursor_y = 4;
        let out = String::from_utf8(build(&p, &Captures { history: &[], screen: &[], saved_normal: None })).unwrap();
        assert!(out.contains("\x1b[3;8r\x1b[?6h\x1b[3;1H"), "{out:?}");
    }

    #[test]
    fn screen_longer_than_height_is_trimmed() {
        let p = pane(1);
        let s = lines(&["x", "extra"]);
        let out = String::from_utf8(build(&p, &Captures { history: &[], screen: &s, saved_normal: None })).unwrap();
        assert!(!out.contains("extra"));
    }
}
