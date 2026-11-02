//! Encoding and decoding rules of the control-mode wire format.

/// Decode the payload of a `%output`/`%extended-output` line.
///
/// tmux (control.c) writes every byte `< 0x20` and the backslash as `\ooo`
/// (three octal digits); every other byte — including bytes >= 0x80 that make
/// up UTF-8 sequences, which may be split or even invalid — is passed through
/// untouched. The result is therefore raw bytes, not a `String`.
///
/// Malformed escapes (which tmux never produces) are passed through literally
/// rather than dropped, so nothing is ever silently lost.
pub fn unescape_output(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut i = 0;
    while i < data.len() {
        let b = data[i];
        if b == b'\\' {
            if let Some(v) = octal3(&data[i + 1..]) {
                out.push(v);
                i += 4;
                continue;
            }
        }
        out.push(b);
        i += 1;
    }
    out
}

fn octal3(s: &[u8]) -> Option<u8> {
    if s.len() < 3 {
        return None;
    }
    let mut v: u32 = 0;
    for &c in &s[..3] {
        if !(b'0'..=b'7').contains(&c) {
            return None;
        }
        v = v * 8 + (c - b'0') as u32;
    }
    u8::try_from(v).ok()
}

/// Quote an arbitrary string as a single tmux command argument.
///
/// tmux's command parser follows shell-like rules: inside single quotes
/// nothing is special, and adjacent quoted pieces concatenate, so `'` is
/// written as `'\''`. Control mode is line-based, so a raw newline would end
/// the command; newlines and other control characters are therefore replaced
/// by spaces (tmux object names cannot usefully contain them anyway).
pub fn quote(arg: &str) -> String {
    let mut out = String::with_capacity(arg.len() + 2);
    out.push('\'');
    for ch in arg.chars() {
        match ch {
            '\'' => out.push_str("'\\''"),
            c if (c as u32) < 0x20 || c == '\u{7f}' => out.push(' '),
            c => out.push(c),
        }
    }
    out.push('\'');
    out
}

/// Encode bytes for `send-keys -H`, which takes each key as a hex byte.
/// Returns space-separated chunks so callers can split large pastes.
pub fn hex_keys(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 3);
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 {
            s.push(' ');
        }
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0xf) as usize] as char);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unescapes_control_bytes_and_backslash() {
        assert_eq!(unescape_output(b"ls /\\015\\015\\012"), b"ls /\r\r\n");
        assert_eq!(unescape_output(b"a\\134b"), b"a\\b");
        assert_eq!(unescape_output(b"\\033[1;31mred\\033[0m"), b"\x1b[1;31mred\x1b[0m");
        assert_eq!(unescape_output(b"\\000"), b"\0");
    }

    #[test]
    fn passes_high_bytes_through_even_if_not_utf8() {
        // "é€" followed by a truncated UTF-8 sequence.
        let raw = b"\xc3\xa9\xe2\x82\xac\xe2\x82";
        assert_eq!(unescape_output(raw), raw);
    }

    #[test]
    fn malformed_escapes_are_kept_literally() {
        assert_eq!(unescape_output(b"\\"), b"\\");
        assert_eq!(unescape_output(b"\\01"), b"\\01");
        assert_eq!(unescape_output(b"\\9xx"), b"\\9xx");
        assert_eq!(unescape_output(b"\\777"), b"\\777"); // > 255
    }

    #[test]
    fn roundtrip_against_tmux_encoder() {
        // Reimplementation of the encoder in tmux control.c.
        fn encode(data: &[u8]) -> Vec<u8> {
            let mut o = Vec::new();
            for &b in data {
                if b < b' ' || b == b'\\' {
                    o.extend_from_slice(format!("\\{:03o}", b).as_bytes());
                } else {
                    o.push(b);
                }
            }
            o
        }
        let all: Vec<u8> = (0u8..=255).chain(b"\\\\\\0".iter().copied()).collect();
        assert_eq!(unescape_output(&encode(&all)), all);
    }

    #[test]
    fn quoting() {
        assert_eq!(quote("simple"), "'simple'");
        assert_eq!(quote("it's"), "'it'\\''s'");
        assert_eq!(quote("a;b $HOME #{x}"), "'a;b $HOME #{x}'");
        assert_eq!(quote("two\nlines"), "'two lines'");
        assert_eq!(quote(""), "''");
    }

    #[test]
    fn hex() {
        assert_eq!(hex_keys(b"a\r\x1b"), "61 0d 1b");
        assert_eq!(hex_keys(b""), "");
    }
}
