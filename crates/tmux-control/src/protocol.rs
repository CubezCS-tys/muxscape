//! Streaming parser for the tmux control-mode protocol.
//!
//! The wire format is line based (`\n`). Two kinds of lines exist outside a
//! command block: notifications (`%name args...`) and the opening guard of a
//! command reply (`%begin <time> <cmdnum> <flags>`). A reply is closed by
//! `%end` or `%error` carrying the *same* time and command number; every line
//! in between is raw command output (unescaped, and it may itself start with
//! `%`). tmux never interleaves anything inside a block.
//!
//! Lines are handled as bytes because `%output` payloads may contain
//! arbitrary (including invalid or split) UTF-8.

use crate::escape::unescape_output;
use crate::ids::{PaneId, SessionId, WindowId};

/// A complete reply to one command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub time: u64,
    pub cmdnum: u64,
    /// Bit 0 is set when the command was sent by *this* client. Replies with
    /// flags 0 belong to commands tmux runs on our behalf (the initial
    /// `attach-session`/`new-session`, config file commands).
    pub flags: u32,
    pub success: bool,
    pub output: Vec<Vec<u8>>,
}

impl Response {
    pub fn from_this_client(&self) -> bool {
        self.flags & 1 == 1
    }

    /// Output lines decoded as (lossy) UTF-8.
    pub fn lines(&self) -> impl Iterator<Item = String> + '_ {
        self.output
            .iter()
            .map(|l| String::from_utf8_lossy(l).into_owned())
    }

    pub fn text(&self) -> String {
        self.lines().collect::<Vec<_>>().join("\n")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notification {
    Output { pane: PaneId, data: Vec<u8> },
    ExtendedOutput { pane: PaneId, age_ms: u64, data: Vec<u8> },
    Pause { pane: PaneId },
    Continue { pane: PaneId },
    PaneModeChanged { pane: PaneId },
    WindowPaneChanged { window: WindowId, pane: PaneId },
    WindowAdd { window: WindowId },
    WindowClose { window: WindowId },
    WindowRenamed { window: WindowId, name: String },
    UnlinkedWindowAdd { window: WindowId },
    UnlinkedWindowClose { window: WindowId },
    UnlinkedWindowRenamed { window: WindowId, name: String },
    LayoutChange {
        window: WindowId,
        layout: String,
        visible_layout: Option<String>,
        flags: Option<String>,
    },
    SessionChanged { session: SessionId, name: String },
    ClientSessionChanged { client: String, session: SessionId, name: String },
    ClientDetached { client: String },
    SessionRenamed { session: SessionId, name: String },
    SessionsChanged,
    SessionWindowChanged { session: SessionId, window: WindowId },
    PasteBufferChanged { name: String },
    PasteBufferDeleted { name: String },
    SubscriptionChanged {
        name: String,
        session: Option<SessionId>,
        window: Option<WindowId>,
        window_index: Option<u32>,
        pane: Option<PaneId>,
        value: String,
    },
    Message { text: String },
    ConfigError { text: String },
    /// The control client is exiting (detached, session killed, server exit).
    Exit { reason: Option<String> },
    /// A well-formed `%` line we do not understand (newer tmux). Kept so
    /// callers can log it; never fatal.
    Unknown { line: Vec<u8> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Notification(Notification),
    Response(Response),
    /// A line outside any block that is not a `%` line. This happens when a
    /// login shell prints something before tmux starts. Ignored but surfaced
    /// for diagnostics.
    Stray(Vec<u8>),
}

struct OpenBlock {
    time: u64,
    cmdnum: u64,
    flags: u32,
    output: Vec<Vec<u8>>,
}

/// Incremental parser. Feed it arbitrary chunks; it yields complete events.
pub struct Parser {
    buf: Vec<u8>,
    block: Option<OpenBlock>,
    at_start: bool,
}

/// DCS sequence sent by `tmux -CC` before the first line.
const CC_PREFIX: &[u8] = b"\x1bP1000p";

impl Default for Parser {
    fn default() -> Self {
        Self::new()
    }
}

impl Parser {
    pub fn new() -> Self {
        Parser { buf: Vec::new(), block: None, at_start: true }
    }

    /// True while inside a `%begin` block (useful for diagnostics).
    pub fn in_block(&self) -> bool {
        self.block.is_some()
    }

    pub fn feed(&mut self, data: &[u8], out: &mut Vec<Event>) {
        self.buf.extend_from_slice(data);
        let mut start = 0;
        while let Some(off) = memchr(b'\n', &self.buf[start..]) {
            let end = start + off;
            let mut line = &self.buf[start..end];
            if line.last() == Some(&b'\r') {
                line = &line[..line.len() - 1];
            }
            let line = line.to_vec();
            start = end + 1;
            self.line(line, out);
        }
        self.buf.drain(..start);
    }

    fn line(&mut self, mut line: Vec<u8>, out: &mut Vec<Event>) {
        if self.at_start {
            self.at_start = false;
            if line.starts_with(CC_PREFIX) {
                line.drain(..CC_PREFIX.len());
            }
        }

        if let Some(block) = &mut self.block {
            if let Some((kind, time, cmdnum, _flags)) = parse_guard(&line) {
                if (kind == Guard::End || kind == Guard::Error)
                    && time == block.time
                    && cmdnum == block.cmdnum
                {
                    let b = self.block.take().unwrap();
                    out.push(Event::Response(Response {
                        time: b.time,
                        cmdnum: b.cmdnum,
                        flags: b.flags,
                        success: kind == Guard::End,
                        output: b.output,
                    }));
                    return;
                }
            }
            block.output.push(line);
            return;
        }

        if let Some((Guard::Begin, time, cmdnum, flags)) = parse_guard(&line) {
            self.block = Some(OpenBlock { time, cmdnum, flags, output: Vec::new() });
            return;
        }

        if line.first() == Some(&b'%') {
            out.push(Event::Notification(parse_notification(&line)));
        } else if line.starts_with(b"\x1b\\") {
            // ST closing the -CC DCS after %exit; nothing to do.
        } else {
            out.push(Event::Stray(line));
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Guard {
    Begin,
    End,
    Error,
}

fn parse_guard(line: &[u8]) -> Option<(Guard, u64, u64, u32)> {
    let (kind, rest) = if let Some(r) = line.strip_prefix(b"%begin ") {
        (Guard::Begin, r)
    } else if let Some(r) = line.strip_prefix(b"%end ") {
        (Guard::End, r)
    } else if let Some(r) = line.strip_prefix(b"%error ") {
        (Guard::Error, r)
    } else {
        return None;
    };
    let s = std::str::from_utf8(rest).ok()?;
    let mut it = s.split(' ');
    let time = it.next()?.parse().ok()?;
    let cmdnum = it.next()?.parse().ok()?;
    let flags = it.next()?.parse().ok()?;
    if it.next().is_some() {
        return None;
    }
    Some((kind, time, cmdnum, flags))
}

fn memchr(needle: u8, hay: &[u8]) -> Option<usize> {
    hay.iter().position(|&b| b == needle)
}

/// Split off the next space-separated word.
fn word(s: &[u8]) -> (&[u8], &[u8]) {
    match memchr(b' ', s) {
        Some(i) => (&s[..i], &s[i + 1..]),
        None => (s, &[]),
    }
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// Parse one notification line (must start with `%`).
pub fn parse_notification(line: &[u8]) -> Notification {
    parse_notification_inner(line).unwrap_or_else(|| Notification::Unknown { line: line.to_vec() })
}

fn parse_notification_inner(line: &[u8]) -> Option<Notification> {
    use Notification as N;
    let (name, rest) = word(line);
    let n = match name {
        b"%output" => {
            let (pane, data) = word(rest);
            N::Output { pane: PaneId::parse_bytes(pane).ok()?, data: unescape_output(data) }
        }
        b"%extended-output" => {
            // %extended-output %<pane> <age> [future args...] : <data>
            let (pane, rest) = word(rest);
            let (age, mut rest) = word(rest);
            loop {
                let (w, r) = word(rest);
                rest = r;
                if w == b":" {
                    break;
                }
                if w.is_empty() && r.is_empty() {
                    return None;
                }
            }
            N::ExtendedOutput {
                pane: PaneId::parse_bytes(pane).ok()?,
                age_ms: std::str::from_utf8(age).ok()?.parse().ok()?,
                data: unescape_output(rest),
            }
        }
        b"%pause" => N::Pause { pane: PaneId::parse_bytes(rest).ok()? },
        b"%continue" => N::Continue { pane: PaneId::parse_bytes(rest).ok()? },
        b"%pane-mode-changed" => N::PaneModeChanged { pane: PaneId::parse_bytes(rest).ok()? },
        b"%window-pane-changed" => {
            let (w, p) = word(rest);
            N::WindowPaneChanged {
                window: WindowId::parse_bytes(w).ok()?,
                pane: PaneId::parse_bytes(p).ok()?,
            }
        }
        b"%window-add" => N::WindowAdd { window: WindowId::parse_bytes(rest).ok()? },
        b"%window-close" => N::WindowClose { window: WindowId::parse_bytes(rest).ok()? },
        b"%unlinked-window-add" => {
            N::UnlinkedWindowAdd { window: WindowId::parse_bytes(rest).ok()? }
        }
        b"%unlinked-window-close" => {
            N::UnlinkedWindowClose { window: WindowId::parse_bytes(rest).ok()? }
        }
        b"%window-renamed" => {
            let (w, name) = word(rest);
            N::WindowRenamed { window: WindowId::parse_bytes(w).ok()?, name: text(name) }
        }
        b"%unlinked-window-renamed" => {
            let (w, name) = word(rest);
            N::UnlinkedWindowRenamed { window: WindowId::parse_bytes(w).ok()?, name: text(name) }
        }
        b"%layout-change" => {
            let (w, rest) = word(rest);
            let (layout, rest) = word(rest);
            let (visible, flags) = word(rest);
            if layout.is_empty() {
                return None;
            }
            N::LayoutChange {
                window: WindowId::parse_bytes(w).ok()?,
                layout: text(layout),
                visible_layout: (!visible.is_empty()).then(|| text(visible)),
                flags: (!visible.is_empty()).then(|| text(flags)),
            }
        }
        b"%session-changed" => {
            let (s, name) = word(rest);
            N::SessionChanged { session: SessionId::parse_bytes(s).ok()?, name: text(name) }
        }
        b"%client-session-changed" => {
            let (c, rest) = word(rest);
            let (s, name) = word(rest);
            N::ClientSessionChanged {
                client: text(c),
                session: SessionId::parse_bytes(s).ok()?,
                name: text(name),
            }
        }
        b"%client-detached" => N::ClientDetached { client: text(rest) },
        b"%session-renamed" => {
            let (s, name) = word(rest);
            N::SessionRenamed { session: SessionId::parse_bytes(s).ok()?, name: text(name) }
        }
        b"%sessions-changed" => N::SessionsChanged,
        b"%session-window-changed" => {
            let (s, w) = word(rest);
            N::SessionWindowChanged {
                session: SessionId::parse_bytes(s).ok()?,
                window: WindowId::parse_bytes(w).ok()?,
            }
        }
        b"%paste-buffer-changed" => N::PasteBufferChanged { name: text(rest) },
        b"%paste-buffer-deleted" => N::PasteBufferDeleted { name: text(rest) },
        b"%subscription-changed" => {
            // %subscription-changed <name> $s <@w|-> <idx|-> <%p|-> ... : <value>
            let (name, rest) = word(rest);
            let (s, rest) = word(rest);
            let (w, rest) = word(rest);
            let (idx, rest) = word(rest);
            let (p, mut rest) = word(rest);
            loop {
                let (x, r) = word(rest);
                rest = r;
                if x == b":" {
                    break;
                }
                if x.is_empty() && r.is_empty() {
                    return None;
                }
            }
            let dash = |b: &[u8]| b == b"-";
            N::SubscriptionChanged {
                name: text(name),
                session: if dash(s) { None } else { Some(SessionId::parse_bytes(s).ok()?) },
                window: if dash(w) { None } else { Some(WindowId::parse_bytes(w).ok()?) },
                window_index: if dash(idx) {
                    None
                } else {
                    Some(std::str::from_utf8(idx).ok()?.parse().ok()?)
                },
                pane: if dash(p) { None } else { Some(PaneId::parse_bytes(p).ok()?) },
                value: text(rest),
            }
        }
        b"%message" => N::Message { text: text(rest) },
        b"%config-error" => N::ConfigError { text: text(rest) },
        b"%exit" => N::Exit { reason: (!rest.is_empty()).then(|| text(rest)) },
        _ => return None,
    };
    Some(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_all(input: &[u8]) -> Vec<Event> {
        let mut p = Parser::new();
        let mut out = Vec::new();
        p.feed(input, &mut out);
        out
    }

    fn notif(line: &str) -> Notification {
        parse_notification(line.as_bytes())
    }

    #[test]
    fn real_attach_transcript() {
        // Captured from tmux 3.4 (`tmux -C new-session` with no tty).
        let t = b"%begin 1793639927 265 0\n%end 1793639927 265 0\n%window-add @0\n\
%sessions-changed\n%session-changed $0 probe\n%begin 1793639927 271 1\n%end 1793639927 271 1\n\
%layout-change @0 a87d,100x30,0,0,0 a87d,100x30,0,0,0 *\n\
%output %0 echo \xc3\xa9\xe2\x82\xac\\015\\012\n";
        let ev = parse_all(t);
        assert_eq!(ev.len(), 7);
        match &ev[0] {
            Event::Response(r) => {
                assert!(r.success && !r.from_this_client() && r.output.is_empty())
            }
            e => panic!("{e:?}"),
        }
        assert_eq!(ev[1], Event::Notification(Notification::WindowAdd { window: WindowId(0) }));
        assert_eq!(
            ev[3],
            Event::Notification(Notification::SessionChanged {
                session: SessionId(0),
                name: "probe".into()
            })
        );
        match &ev[4] {
            Event::Response(r) => assert!(r.from_this_client()),
            e => panic!("{e:?}"),
        }
        assert_eq!(
            ev[5],
            Event::Notification(Notification::LayoutChange {
                window: WindowId(0),
                layout: "a87d,100x30,0,0,0".into(),
                visible_layout: Some("a87d,100x30,0,0,0".into()),
                flags: Some("*".into()),
            })
        );
        assert_eq!(
            ev[6],
            Event::Notification(Notification::Output {
                pane: PaneId(0),
                data: "echo é€\r\n".as_bytes().to_vec()
            })
        );
    }

    #[test]
    fn block_content_may_look_like_notifications() {
        let t = b"%begin 1 5 1\n%output %1 not really\n%end 1 4 1\n%end 2 5 1\nhi\n%end 1 5 1\n";
        let ev = parse_all(t);
        assert_eq!(ev.len(), 1);
        match &ev[0] {
            Event::Response(r) => {
                assert_eq!(r.cmdnum, 5);
                assert_eq!(
                    r.output,
                    vec![
                        b"%output %1 not really".to_vec(),
                        b"%end 1 4 1".to_vec(),
                        b"%end 2 5 1".to_vec(),
                        b"hi".to_vec()
                    ]
                );
            }
            e => panic!("{e:?}"),
        }
    }

    #[test]
    fn error_block() {
        let ev = parse_all(b"%begin 1578923149 270 1\nparse error: unknown command: abcdef\n%error 1578923149 270 1\n");
        match &ev[0] {
            Event::Response(r) => {
                assert!(!r.success);
                assert_eq!(r.text(), "parse error: unknown command: abcdef");
            }
            e => panic!("{e:?}"),
        }
    }

    #[test]
    fn arbitrary_chunking_gives_identical_events() {
        let t: &[u8] = b"\x1bP1000p%begin 1 1 0\n%end 1 1 0\n%output %3 a\\015\\012\xe2\x82\xac\n\
%begin 2 2 1\nline one\r\n\nline three\n%end 2 2 1\n%window-renamed @2 my window name\n%exit\n";
        let whole = parse_all(t);
        assert_eq!(whole.len(), 5);
        for chunk in 1..t.len() {
            let mut p = Parser::new();
            let mut out = Vec::new();
            for c in t.chunks(chunk) {
                p.feed(c, &mut out);
            }
            assert_eq!(out, whole, "chunk size {chunk}");
        }
        match &whole[2] {
            Event::Response(r) => assert_eq!(r.output, vec![b"line one".to_vec(), vec![], b"line three".to_vec()]),
            e => panic!("{e:?}"),
        }
        assert_eq!(whole[4], Event::Notification(Notification::Exit { reason: None }));
    }

    #[test]
    fn stray_lines_outside_blocks() {
        let ev = parse_all(b"Welcome to Ubuntu\n%sessions-changed\n");
        assert_eq!(ev[0], Event::Stray(b"Welcome to Ubuntu".to_vec()));
        assert_eq!(ev[1], Event::Notification(Notification::SessionsChanged));
    }

    #[test]
    fn notifications() {
        use Notification as N;
        assert_eq!(notif("%window-renamed @1 a b  c"), N::WindowRenamed { window: WindowId(1), name: "a b  c".into() });
        assert_eq!(notif("%window-renamed @1 "), N::WindowRenamed { window: WindowId(1), name: "".into() });
        assert_eq!(notif("%window-pane-changed @3 %9"), N::WindowPaneChanged { window: WindowId(3), pane: PaneId(9) });
        assert_eq!(notif("%session-window-changed $1 @4"), N::SessionWindowChanged { session: SessionId(1), window: WindowId(4) });
        assert_eq!(
            notif("%client-session-changed /dev/pts/3 $2 work stuff"),
            N::ClientSessionChanged { client: "/dev/pts/3".into(), session: SessionId(2), name: "work stuff".into() }
        );
        assert_eq!(notif("%exit detached"), N::Exit { reason: Some("detached".into()) });
        assert_eq!(notif("%pause %4"), N::Pause { pane: PaneId(4) });
        assert_eq!(notif("%continue %4"), N::Continue { pane: PaneId(4) });
        assert_eq!(
            notif("%layout-change @2 b25d,80x24,0,0{40x24,0,0,1,39x24,41,0,2} b25d,80x24,0,0{40x24,0,0,1,39x24,41,0,2} "),
            N::LayoutChange {
                window: WindowId(2),
                layout: "b25d,80x24,0,0{40x24,0,0,1,39x24,41,0,2}".into(),
                visible_layout: Some("b25d,80x24,0,0{40x24,0,0,1,39x24,41,0,2}".into()),
                flags: Some("".into()),
            }
        );
        // Old tmux: no visible layout / flags.
        assert_eq!(
            notif("%layout-change @2 a87d,100x30,0,0,0"),
            N::LayoutChange { window: WindowId(2), layout: "a87d,100x30,0,0,0".into(), visible_layout: None, flags: None }
        );
        assert_eq!(
            notif("%extended-output %5 1234 : abc\\012"),
            N::ExtendedOutput { pane: PaneId(5), age_ms: 1234, data: b"abc\n".to_vec() }
        );
        assert_eq!(
            notif("%extended-output %5 1234 future-arg : x : y"),
            N::ExtendedOutput { pane: PaneId(5), age_ms: 1234, data: b"x : y".to_vec() }
        );
        assert_eq!(
            notif("%subscription-changed cwd $1 @2 0 %3 : /home/dev"),
            N::SubscriptionChanged {
                name: "cwd".into(),
                session: Some(SessionId(1)),
                window: Some(WindowId(2)),
                window_index: Some(0),
                pane: Some(PaneId(3)),
                value: "/home/dev".into()
            }
        );
        assert_eq!(
            notif("%subscription-changed s $1 - - - : v"),
            N::SubscriptionChanged { name: "s".into(), session: Some(SessionId(1)), window: None, window_index: None, pane: None, value: "v".into() }
        );
        // Output payload may contain spaces.
        assert_eq!(notif("%output %1  two  spaces "), N::Output { pane: PaneId(1), data: b" two  spaces ".to_vec() });
    }

    #[test]
    fn malformed_and_unknown_notifications_do_not_panic() {
        for l in ["%output", "%output @1 x", "%window-add", "%window-add %1", "%layout-change @1", "%new-thing 1 2", "%", "%extended-output %1 12", "%pause"] {
            assert!(matches!(notif(l), Notification::Unknown { .. }), "{l}");
        }
    }
}
