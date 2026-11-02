//! SSH port forwarding (`ssh -L`) without typing commands.
//!
//! * [`PortDetector`] watches terminal output for "a server just started"
//!   lines (`http://localhost:5173/`, `Listening on 0.0.0.0:8000`, …) so the
//!   UI can offer a one-click forward.
//! * [`crate::ports`] describes the server's listening ports.
//! * The forwarding itself lives in [`crate::core`]: a local listener per
//!   tunnel, one `direct-tcpip` channel per accepted connection, carried by
//!   the host's existing SSH connection (so it survives reconnects).

use crate::api::HostId;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

pub type TunnelId = u64;

/// A forward the user asked to keep: re-opened every time the host connects.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SavedForward {
    pub remote_port: u16,
    /// Preferred local port (defaults to the remote port).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_port: Option<u16>,
    /// Path to open in the browser, e.g. `/lab?token=…`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TunnelInfo {
    pub id: TunnelId,
    pub host_id: HostId,
    /// Always connected to from the server's side, so `localhost` there.
    pub remote_host: String,
    pub remote_port: u16,
    /// Bound on 127.0.0.1 (and ::1 when available) on this machine.
    pub local_port: u16,
    pub path: Option<String>,
    pub pinned: bool,
    /// Last forwarding problem (e.g. nothing listening on the server).
    pub error: Option<String>,
}

impl TunnelInfo {
    pub fn local_url(&self) -> String {
        format!("http://localhost:{}{}", self.local_port, self.path.as_deref().unwrap_or("/"))
    }
}

// ------------------------------------------------------------- detection

/// A port mentioned in terminal output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedPort {
    pub port: u16,
    /// Path + query from a URL, kept so tokens (Jupyter…) survive.
    pub path: Option<String>,
}

const LOCAL_HOSTS: &str = r"(?:localhost|127\.0\.0\.1|0\.0\.0\.0|\[::1?\])";

static URL_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(r#"(?i)\bhttps?://{LOCAL_HOSTS}:(\d{{1,5}})(/[^\s'"<>`)\]]*)?"#)).unwrap()
});
static HOSTPORT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r"(?i)(?:^|[\s(=@])(?://)?{LOCAL_HOSTS}:(\d{{1,5}})\b")).unwrap());
static PHRASE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(?:listening|running|serving|started|available|bound|server)\b[^\n]{0,40}?\bport\b[\s:=#]*(\d{4,5})\b",
    )
    .unwrap()
});
static LISTEN_COLON_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\blistening\s+(?:on|at)\s*:?\s*(?:\*|\[::\])?:(\d{4,5})\b").unwrap());

/// Finds ports in a pane's output stream. Feed it raw chunks as they arrive;
/// lines split across chunks and terminal escape sequences are handled.
#[derive(Default)]
pub struct PortDetector {
    /// Unterminated tail of the previous chunk (escape-stripped).
    partial: String,
    /// Escape sequence cut off at the end of the previous chunk.
    pending_esc: Vec<u8>,
}

/// Max bytes kept for an unterminated line; longer lines are scanned in pieces.
const MAX_PARTIAL: usize = 1024;

impl PortDetector {
    pub fn feed(&mut self, data: &[u8]) -> Vec<DetectedPort> {
        let mut bytes = std::mem::take(&mut self.pending_esc);
        bytes.extend_from_slice(data);
        let (text, rest) = strip_escapes(&bytes);
        self.pending_esc = rest;
        self.partial.push_str(&text);

        let mut found = Vec::new();
        // Scan complete lines; keep the last (unterminated) one for later.
        let cut = self.partial.rfind('\n').map(|i| i + 1).unwrap_or(0);
        if cut > 0 {
            let complete: String = self.partial.drain(..cut).collect();
            let lines: Vec<&str> = complete.trim_end_matches('\n').split('\n').collect();
            for (i, line) in lines.iter().enumerate() {
                // The following line (or the unterminated tail) lets a port
                // that was hard-wrapped at the pane edge be put back together.
                let next = lines.get(i + 1).copied().unwrap_or(self.partial.as_str());
                scan_line(line, next, &mut found);
            }
        }
        if self.partial.len() > MAX_PARTIAL {
            // Never wait forever on a line that doesn't end (progress bars).
            let line = std::mem::take(&mut self.partial);
            scan_line(&line, "", &mut found);
        }
        found
    }
}

fn valid_port(s: &str) -> Option<u16> {
    s.parse::<u32>().ok().filter(|p| (1..=65535).contains(p)).map(|p| p as u16)
}

/// A port at the very end of a line whose next line starts with digits was
/// probably hard-wrapped by a TUI at the pane width (`…localhost:500` +
/// `0/app`). Join them when the result is still a valid port.
fn unwrap_port(line: &str, end: usize, digits: &str, next: &str) -> Option<(u16, Option<String>)> {
    if !line[end..].trim().is_empty() || digits.len() >= 5 {
        return None;
    }
    let n = next.trim_start();
    let more: String = n.chars().take_while(|c| c.is_ascii_digit()).collect();
    if more.is_empty() || digits.len() + more.len() > 5 {
        return None;
    }
    let port = valid_port(&format!("{digits}{more}"))?;
    let after = &n[more.len()..];
    let path = after
        .starts_with('/')
        .then(|| after.split(|c: char| c.is_whitespace() || "'\"<>`)]".contains(c)).next().unwrap_or(""))
        .map(|p| p.trim_end_matches(['.', ',', ';', ':']).to_string())
        .filter(|p| p.len() > 1);
    Some((port, path))
}

fn scan_line(line: &str, next: &str, out: &mut Vec<DetectedPort>) {
    let mut push = |port: u16, path: Option<String>| {
        // SSH itself is never what you want to open in a browser.
        if port == 22 {
            return;
        }
        if let Some(existing) = out.iter_mut().find(|d| d.port == port) {
            if existing.path.is_none() {
                existing.path = path;
            }
        } else {
            out.push(DetectedPort { port, path });
        }
    };
    for c in URL_RE.captures_iter(line) {
        let m = c.get(1).unwrap();
        if c.get(2).is_none() {
            if let Some((p, path)) = unwrap_port(line, m.end(), m.as_str(), next) {
                push(p, path);
                continue;
            }
        }
        if let Some(p) = valid_port(m.as_str()) {
            let path = c.get(2).map(|m| m.as_str().trim_end_matches(['.', ',', ';', ':']).to_string());
            push(p, path.filter(|s| !s.is_empty() && s != "/"));
        }
    }
    for c in HOSTPORT_RE.captures_iter(line) {
        let m = c.get(1).unwrap();
        match unwrap_port(line, m.end(), m.as_str(), next) {
            Some((p, path)) => push(p, path),
            None => {
                if let Some(p) = valid_port(m.as_str()) {
                    push(p, None);
                }
            }
        }
    }
    for re in [&*PHRASE_RE, &*LISTEN_COLON_RE] {
        for c in re.captures_iter(line) {
            if let Some(p) = valid_port(&c[1]) {
                push(p, None);
            }
        }
    }
}

/// Turn terminal output into plain text: drop CSI/OSC/other escape sequences
/// (cursor-forward becomes a space, since TUIs use it between words), map CR
/// to a line break. Returns the text and any escape sequence left incomplete
/// at the end of `b`.
fn strip_escapes(b: &[u8]) -> (String, Vec<u8>) {
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            0x1b => {
                let start = i;
                let Some(&kind) = b.get(i + 1) else { return (lossy(out), b[start..].to_vec()) };
                match kind {
                    b'[' => {
                        // CSI: parameters, then a final byte 0x40..=0x7e.
                        let mut j = i + 2;
                        while j < b.len() && !(0x40..=0x7e).contains(&b[j]) {
                            j += 1;
                        }
                        if j >= b.len() {
                            return (lossy(out), b[start..].to_vec());
                        }
                        if b[j] == b'C' {
                            out.push(b' ');
                        }
                        i = j + 1;
                    }
                    b']' | b'P' | b'_' | b'^' | b'X' => {
                        // String sequences end with BEL or ESC \.
                        let mut j = i + 2;
                        loop {
                            if j >= b.len() {
                                // Absurdly long/unterminated: give up on it.
                                if b.len() - start > 4096 {
                                    i = b.len();
                                    break;
                                }
                                return (lossy(out), b[start..].to_vec());
                            }
                            if b[j] == 0x07 {
                                i = j + 1;
                                break;
                            }
                            if b[j] == 0x1b && b.get(j + 1) == Some(&b'\\') {
                                i = j + 2;
                                break;
                            }
                            if b[j] == 0x1b && j + 1 >= b.len() {
                                return (lossy(out), b[start..].to_vec());
                            }
                            j += 1;
                        }
                    }
                    // Charset designations: ESC ( B etc.
                    b'(' | b')' | b'*' | b'+' => i += 3,
                    _ => i += 2,
                }
            }
            b'\r' => {
                // CRLF is one line break; a lone CR (redraw) starts a new line.
                if b.get(i + 1) != Some(&b'\n') {
                    out.push(b'\n');
                }
                i += 1;
            }
            c if c < 0x20 && c != b'\n' && c != b'\t' => i += 1,
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    (lossy(out), Vec::new())
}

fn lossy(v: Vec<u8>) -> String {
    String::from_utf8_lossy(&v).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detect(chunks: &[&[u8]]) -> Vec<DetectedPort> {
        let mut d = PortDetector::default();
        chunks.iter().flat_map(|c| d.feed(c)).collect()
    }

    fn ports(chunks: &[&[u8]]) -> Vec<u16> {
        detect(chunks).into_iter().map(|d| d.port).collect()
    }

    #[test]
    fn common_dev_servers() {
        // Vite (with colours), Next, Django, Flask, uvicorn, Rails, http.server
        let vite = b"\x1b[32m\xe2\x9e\x9c\x1b[39m  \x1b[1mLocal\x1b[22m:   \x1b[36mhttp://localhost:\x1b[1m5173\x1b[22m/\x1b[39m\r\n";
        assert_eq!(ports(&[vite]), vec![5173]);
        assert_eq!(ports(&[b"  - Local:        http://localhost:3000\n"]), vec![3000]);
        assert_eq!(ports(&[b"Starting development server at http://127.0.0.1:8000/\n"]), vec![8000]);
        assert_eq!(ports(&[b" * Running on http://127.0.0.1:5000\n"]), vec![5000]);
        assert_eq!(ports(&[b"INFO:     Uvicorn running on http://0.0.0.0:8080 (Press CTRL+C to quit)\n"]), vec![8080]);
        assert_eq!(ports(&[b"* Listening on http://[::1]:3001\n"]), vec![3001]);
        assert_eq!(ports(&[b"Serving HTTP on 0.0.0.0 port 8001 (http://0.0.0.0:8001/) ...\n"]), vec![8001]);
        assert_eq!(ports(&[b"Server listening on port 4000\n"]), vec![4000]);
        assert_eq!(ports(&[b"listening on :9090\n"]), vec![9090]);
        assert_eq!(ports(&[b"api ready at localhost:7000\n"]), vec![7000]);
    }

    #[test]
    fn keeps_url_path_for_tokens() {
        let d = detect(&[b"    http://127.0.0.1:8888/lab?token=abc123def\n"]);
        assert_eq!(d, vec![DetectedPort { port: 8888, path: Some("/lab?token=abc123def".into()) }]);
        // Trailing punctuation isn't part of the URL; a bare "/" isn't a path.
        let d = detect(&[b"open http://localhost:3000/docs. Then http://localhost:4000/\n"]);
        assert_eq!(d[0].path.as_deref(), Some("/docs"));
        assert_eq!(d[1], DetectedPort { port: 4000, path: None });
    }

    #[test]
    fn split_chunks_and_escapes() {
        // Line and escape sequence both cut mid-way.
        assert_eq!(ports(&[b"Local: http://local", b"host:51", b"73/\x1b[3", b"9m\n"]), vec![5173]);
        // TUIs separating words with cursor-forward.
        assert_eq!(ports(&[b"Server\x1b[1Clistening\x1b[1Con\x1b[1Cport\x1b[1C8123\n"]), vec![8123]);
        // OSC 8 hyperlink: the URL in the escape is dropped, the text is scanned.
        assert_eq!(ports(&[b"\x1b]8;;http://localhost:1111\x07http://localhost:2222\x1b]8;;\x07\n"]), vec![2222]);
        // A line that never ends still gets scanned eventually.
        let mut long = b"x".repeat(1100);
        long.extend_from_slice(b" http://localhost:6006 ");
        long.extend(b"y".repeat(1100));
        assert_eq!(ports(&[&long]), vec![6006]);
    }

    #[test]
    fn rejoins_ports_wrapped_at_the_pane_edge() {
        // An agent's TUI hard-wrapping a long line, with indentation.
        let d = detect(&[b"  \xe2\x8e\xbf  Server running at http://localhost:500\r\n     0/app?t=1 (pid 42)\r\n"]);
        assert_eq!(d, vec![DetectedPort { port: 5000, path: Some("/app?t=1".into()) }]);
        assert_eq!(ports(&[b"api ready at localhost:30\n00\n"]), vec![3000]);
        // Next line still arriving: the unterminated tail is used.
        assert_eq!(ports(&[b"http://localhost:51\n73", b"\n"]), vec![5173]);
        // A real line end followed by numbers that can't extend the port.
        assert_eq!(ports(&[b"http://localhost:5000\n2024-01-01 started\n"]), vec![5000]);
        assert_eq!(ports(&[b"http://localhost:3000\n12 requests\n"]), vec![3000]);
        // Text after the port on the same line: not a wrap.
        assert_eq!(ports(&[b"http://localhost:500 ok\n0\n"]), vec![500]);
    }

    #[test]
    fn ignores_non_servers() {
        assert!(ports(&[b"ssh dev@localhost:22\n"]).is_empty());
        assert!(ports(&[b"see https://example.com:8443/x and 10.0.0.5:8080\n"]).is_empty());
        assert!(ports(&[b"export port 80 stuff; error code 5173\n"]).is_empty());
        assert!(ports(&[b"http://localhost:99999/\n"]).is_empty());
        assert!(ports(&[b"timestamp 12:34:56 at host:1234\n"]).is_empty());
        // Unterminated line is held back until the newline arrives.
        let mut d = PortDetector::default();
        assert!(d.feed(b"http://localhost:5555").is_empty());
        assert_eq!(d.feed(b"\n")[0].port, 5555);
    }
}
