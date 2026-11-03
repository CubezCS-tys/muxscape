//! A pragmatic OpenSSH `ssh_config` reader.
//!
//! Implements the parts that matter for picking a server from a list:
//! `Host` blocks with wildcards and negation, `Include` (with globs),
//! first-value-wins semantics, `IdentityFile` accumulation, `%h/%u/%p/%r/%d/~`
//! expansion and `ProxyJump`. `Match` blocks are skipped (treated as not
//! matching) rather than misinterpreted.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ResolvedHost {
    pub alias: String,
    pub hostname: String,
    pub port: u16,
    pub user: Option<String>,
    pub identity_files: Vec<String>,
    pub identities_only: bool,
    /// Raw `ProxyJump` value (comma separated `[user@]host[:port]` hops).
    pub proxy_jump: Option<String>,
    /// `ProxyCommand` (not supported: hosts that need it are refused rather
    /// than dialled directly). `none` counts as unset.
    pub proxy_command: Option<String>,
    pub server_alive_interval: Option<u64>,
}

#[derive(Debug, Clone)]
struct Block {
    /// None = top-level (applies to all hosts) ; Some(patterns) = Host line;
    /// `Match` blocks are represented as a pattern list that never matches.
    patterns: Option<Vec<String>>,
    options: Vec<(String, String)>,
}

#[derive(Debug, Clone, Default)]
pub struct SshConfig {
    blocks: Vec<Block>,
}

fn tokenize(line: &str) -> Option<(String, String)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    // "Key value", "Key=value" or "Key = value".
    let idx = line.find(|c: char| c.is_whitespace() || c == '=')?;
    let key = line[..idx].to_ascii_lowercase();
    let rest = line[idx..].trim_start();
    let rest = rest.strip_prefix('=').unwrap_or(rest).trim();
    Some((key, rest.to_string()))
}

/// Split respecting double quotes.
fn words(v: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut any = false;
    for c in v.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                any = true;
            }
            c if c.is_whitespace() && !quoted => {
                if any || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            c => cur.push(c),
        }
    }
    if any || !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// OpenSSH-style glob: `*` and `?`.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti, mut star, mut mark) = (0, 0, None, 0);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi].eq_ignore_ascii_case(&t[ti])) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

fn patterns_match(patterns: &[String], host: &str) -> bool {
    let mut matched = false;
    for p in patterns {
        if let Some(neg) = p.strip_prefix('!') {
            if glob_match(neg, host) {
                return false;
            }
        } else if glob_match(p, host) {
            matched = true;
        }
    }
    matched
}

fn expand_tilde(p: &str, home: &Path) -> String {
    if p == "~" {
        home.display().to_string()
    } else if let Some(rest) = p.strip_prefix("~/") {
        home.join(rest).display().to_string()
    } else {
        p.to_string()
    }
}

impl SshConfig {
    /// Load `~/.ssh/config` (missing file = empty config).
    pub fn load_default() -> SshConfig {
        match mux_ssh::home_dir() {
            Some(h) => Self::load(&h.join(".ssh").join("config"), &h),
            None => SshConfig::default(),
        }
    }

    pub fn load(path: &Path, home: &Path) -> SshConfig {
        let mut cfg = SshConfig::default();
        cfg.blocks.push(Block { patterns: None, options: vec![] });
        cfg.read_file(path, home, 0);
        cfg
    }

    pub fn parse_str(s: &str, home: &Path) -> SshConfig {
        let mut cfg = SshConfig::default();
        cfg.blocks.push(Block { patterns: None, options: vec![] });
        cfg.read_str(s, home, 0);
        cfg
    }

    fn read_file(&mut self, path: &Path, home: &Path, depth: u32) {
        if depth > 8 {
            return;
        }
        if let Ok(s) = std::fs::read_to_string(path) {
            self.read_str(&s, home, depth);
        }
    }

    fn read_str(&mut self, s: &str, home: &Path, depth: u32) {
        for line in s.lines() {
            let Some((key, value)) = tokenize(line) else { continue };
            match key.as_str() {
                "host" => self.blocks.push(Block { patterns: Some(words(&value)), options: vec![] }),
                "match" => self.blocks.push(Block { patterns: Some(vec![]), options: vec![] }),
                "include" => {
                    // Includes are processed in place, inheriting the current
                    // block context (as OpenSSH does).
                    for pat in words(&value) {
                        let pat = expand_tilde(&pat, home);
                        let full = if Path::new(&pat).is_absolute() {
                            PathBuf::from(&pat)
                        } else {
                            home.join(".ssh").join(&pat)
                        };
                        for f in glob_files(&full) {
                            self.read_file(&f, home, depth + 1);
                        }
                    }
                }
                _ => {
                    if let Some(b) = self.blocks.last_mut() {
                        b.options.push((key, value));
                    }
                }
            }
        }
    }

    /// Concrete (non-wildcard) aliases, in file order, for the host list.
    pub fn aliases(&self) -> Vec<String> {
        let mut v = Vec::new();
        for b in &self.blocks {
            for p in b.patterns.iter().flatten() {
                if !p.contains(['*', '?', '!']) && !v.contains(p) {
                    v.push(p.clone());
                }
            }
        }
        v
    }

    pub fn resolve(&self, alias: &str, home: &Path, local_user: &str) -> ResolvedHost {
        let mut r = ResolvedHost { alias: alias.to_string(), ..Default::default() };
        let mut hostname: Option<String> = None;
        let mut port: Option<u16> = None;
        let mut identities_only: Option<bool> = None;
        let mut proxy_jump: Option<String> = None;
        let mut proxy_command: Option<String> = None;
        let mut alive: Option<u64> = None;
        for b in &self.blocks {
            let applies = match &b.patterns {
                None => true,
                Some(p) => patterns_match(p, alias),
            };
            if !applies {
                continue;
            }
            for (k, v) in &b.options {
                let first = words(v).into_iter().next().unwrap_or_default();
                match k.as_str() {
                    "hostname" if hostname.is_none() => hostname = Some(first),
                    "user" if r.user.is_none() => r.user = Some(first),
                    "port" if port.is_none() => port = first.parse().ok(),
                    "identityfile" => r.identity_files.push(first),
                    "identitiesonly" if identities_only.is_none() => {
                        identities_only = Some(first.eq_ignore_ascii_case("yes"))
                    }
                    "proxyjump" if proxy_jump.is_none() && proxy_command.is_none() => proxy_jump = Some(first),
                    // First of ProxyJump / ProxyCommand wins, as in OpenSSH.
                    "proxycommand" if proxy_command.is_none() && proxy_jump.is_none() => proxy_command = Some(v.trim().to_string()),
                    "serveraliveinterval" if alive.is_none() => alive = first.parse().ok(),
                    _ => {}
                }
            }
        }
        r.proxy_command = proxy_command.filter(|c| !c.eq_ignore_ascii_case("none"));
        let host = hostname.unwrap_or_else(|| alias.to_string()).replace("%h", alias);
        r.port = port.unwrap_or(22);
        r.hostname = host;
        let user = r.user.clone().unwrap_or_else(|| local_user.to_string());
        r.identity_files = r
            .identity_files
            .iter()
            .map(|f| {
                let f = f
                    .replace("%d", &home.display().to_string())
                    .replace("%u", local_user)
                    .replace("%r", &user)
                    .replace("%h", &r.hostname)
                    .replace("%p", &r.port.to_string());
                expand_tilde(&f, home)
            })
            .collect();
        r.identities_only = identities_only.unwrap_or(false);
        r.proxy_jump = proxy_jump.filter(|p| !p.eq_ignore_ascii_case("none"));
        r.server_alive_interval = alive;
        r
    }
}

/// Minimal glob for Include: `*`/`?` in the final path component only.
fn glob_files(p: &Path) -> Vec<PathBuf> {
    let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if !name.contains(['*', '?']) {
        return vec![p.to_path_buf()];
    }
    let dir = p.parent().unwrap_or(Path::new("."));
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|f| f.file_name().is_some_and(|n| glob_match(&name, &n.to_string_lossy())))
        .collect();
    v.sort();
    v
}

/// Parse one `ProxyJump` hop: `[user@]host[:port]`.
pub fn parse_hop(s: &str) -> (Option<String>, String, Option<u16>) {
    let (user, rest) = match s.rsplit_once('@') {
        Some((u, r)) => (Some(u.to_string()), r),
        None => (None, s),
    };
    // [v6::addr]:port
    if let Some(r) = rest.strip_prefix('[') {
        if let Some((h, p)) = r.split_once(']') {
            return (user, h.to_string(), p.strip_prefix(':').and_then(|p| p.parse().ok()));
        }
    }
    match rest.rsplit_once(':') {
        Some((h, p)) if !h.contains(':') => (user, h.to_string(), p.parse().ok()),
        _ => (user, rest.to_string(), None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CFG: &str = r#"
# global
ServerAliveInterval 30

Host github.com
    HostName github.com
    User git
    IdentityFile ~/.ssh/github_deploy

Host prod-* !prod-legacy
  User deploy
  IdentityFile ~/.ssh/%r_key
  ProxyJump bastion

Host prod-web prod-db prod-legacy
  HostName %h.example.com
  Port=2200

Host bastion
  HostName 203.0.113.9
  User = jumper

Match host foo
  User nope

Host *
  User fallback
  IdentitiesOnly yes
"#;

    fn cfg() -> SshConfig {
        SshConfig::parse_str(CFG, Path::new("/home/me"))
    }

    #[test]
    fn aliases_skip_wildcards() {
        assert_eq!(cfg().aliases(), vec!["github.com", "prod-web", "prod-db", "prod-legacy", "bastion"]);
    }

    #[test]
    fn resolve_first_value_wins() {
        let h = cfg().resolve("github.com", Path::new("/home/me"), "me");
        assert_eq!(h.hostname, "github.com");
        assert_eq!(h.user.as_deref(), Some("git"));
        assert_eq!(h.identity_files, vec!["/home/me/.ssh/github_deploy"]);
        assert_eq!(h.server_alive_interval, Some(30));
        assert!(h.identities_only);
        assert_eq!(h.port, 22);
    }

    #[test]
    fn wildcards_negation_and_tokens() {
        let h = cfg().resolve("prod-web", Path::new("/home/me"), "me");
        assert_eq!(h.hostname, "prod-web.example.com");
        assert_eq!(h.port, 2200);
        assert_eq!(h.user.as_deref(), Some("deploy"));
        assert_eq!(h.identity_files, vec!["/home/me/.ssh/deploy_key"]);
        assert_eq!(h.proxy_jump.as_deref(), Some("bastion"));
        // Negated pattern excludes prod-legacy from the prod-* block.
        let l = cfg().resolve("prod-legacy", Path::new("/home/me"), "me");
        assert_eq!(l.user.as_deref(), Some("fallback"));
        assert_eq!(l.proxy_jump, None);
        // Match block ignored, unknown host gets defaults.
        let u = cfg().resolve("foo", Path::new("/home/me"), "me");
        assert_eq!((u.hostname.as_str(), u.user.as_deref()), ("foo", Some("fallback")));
    }

    #[test]
    fn includes_with_globs() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        std::fs::create_dir_all(home.join(".ssh/conf.d")).unwrap();
        std::fs::write(home.join(".ssh/config"), "Include conf.d/*.conf\nHost main\n  User a\n").unwrap();
        std::fs::write(home.join(".ssh/conf.d/10-x.conf"), "Host inc1\n  Port 1022\n").unwrap();
        std::fs::write(home.join(".ssh/conf.d/ignored.txt"), "Host nope\n").unwrap();
        let c = SshConfig::load(&home.join(".ssh/config"), home);
        assert_eq!(c.aliases(), vec!["inc1", "main"]);
        assert_eq!(c.resolve("inc1", home, "me").port, 1022);
    }

    #[test]
    fn globbing() {
        assert!(glob_match("*.example.com", "a.example.com"));
        assert!(glob_match("web?", "web1"));
        assert!(!glob_match("web?", "web12"));
        assert!(glob_match("*", ""));
        assert!(glob_match("PROD-*", "prod-x"));
    }

    #[test]
    fn hops() {
        assert_eq!(parse_hop("bastion"), (None, "bastion".into(), None));
        assert_eq!(parse_hop("u@b:2222"), (Some("u".into()), "b".into(), Some(2222)));
        assert_eq!(parse_hop("[::1]:22"), (None, "::1".into(), Some(22)));
        assert_eq!(parse_hop("fe80::1"), (None, "fe80::1".into(), None));
    }
}
