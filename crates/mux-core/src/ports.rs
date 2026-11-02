//! "What is listening on this server, and what is it?"
//!
//! * [`scan_command`] gathers, in one exec and with nothing installed on the
//!   server: listening sockets (`/proc/net/tcp*`), which process owns each
//!   (socket inode → `/proc/<pid>/fd`), its command line and working
//!   directory, the tmux pane it runs under (parent chain → `pane_pid`), and
//!   Docker's published ports. Only the user's own processes can be
//!   attributed; everything else is still listed.
//! * [`probe`] asks each port what it serves, over the existing SSH
//!   connection: banner for server-first protocols, else one `GET /` read
//!   for status / title / server. Known databases are never poked.

use base64::Engine;
use mux_ssh::SshConnection;
use serde::Serialize;
use std::collections::HashMap;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// The tmux pane a process runs under.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PaneRef {
    pub session_id: String,
    pub session_name: String,
    pub window_id: String,
    pub window_index: u32,
    pub window_name: String,
    pub pane_id: String,
}

/// A listening TCP port on the server.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ListeningPort {
    pub port: u16,
    /// Bind address (`127.0.0.1`, `0.0.0.0`, `::`, …).
    pub address: String,
    /// Bound to all interfaces: reachable from other machines.
    pub public: bool,
    pub pid: Option<u32>,
    /// Short, recognisable name: `vite`, `uvicorn`, `npm run dev`, `postgres`.
    pub process: Option<String>,
    /// Full command line (truncated).
    pub command: Option<String>,
    /// Working directory, `~`-relative when under the home directory.
    pub cwd: Option<String>,
    pub pane: Option<PaneRef>,
    /// Docker container publishing this port: `name (image)`.
    pub container: Option<String>,
    /// Started by the user (own process, in tmux, or their container).
    pub mine: bool,
}

/// What a port answered.
#[derive(Debug, Clone, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct PortProbe {
    pub port: u16,
    /// Worth opening in a browser.
    pub web: bool,
    /// Speaks TLS (open it as https://).
    pub tls: bool,
    pub status: Option<u16>,
    /// Page `<title>`.
    pub title: Option<String>,
    /// What it is: `Vite`, `FastAPI`, `PostgreSQL`, `SSH`, `JSON API`…
    pub label: Option<String>,
    /// `Server` header (short).
    pub server: Option<String>,
    /// Redirect target for 3xx answers.
    pub redirect: Option<String>,
    /// Couldn't connect / no answer.
    pub error: Option<String>,
}

// ---------------------------------------------------------------- scan

/// `{TAB}` becomes a real tab (tmux/docker field separator) in [`scan_command`].
const SCAN_SCRIPT: &str = r##"echo "@@HOME $HOME"
echo @@TCP; cat /proc/net/tcp /proc/net/tcp6 2>/dev/null
echo @@FD; ls -l /proc/[0-9]*/fd 2>/dev/null | grep -e "socket:" -e "^/proc"
echo @@CWD; ls -l /proc/[0-9]*/cwd 2>/dev/null
echo @@PS; ps -eo pid=,ppid=,args= 2>/dev/null
echo @@TMUX; command -v tmux >/dev/null 2>&1 && tmux list-panes -a -F "#{pane_pid}{TAB}#{session_id}{TAB}#{session_name}{TAB}#{window_id}{TAB}#{window_index}{TAB}#{window_name}{TAB}#{pane_id}" 2>/dev/null
echo @@DOCKER; command -v docker >/dev/null 2>&1 && timeout 3 docker ps --format "{{.Names}}{TAB}{{.Image}}{TAB}{{.Ports}}" 2>/dev/null
echo @@END
"##;

/// One remote command that prints everything [`parse_scan`] needs. The
/// script travels base64-encoded, so no quoting reaches the remote shell.
pub fn scan_command() -> String {
    let script = SCAN_SCRIPT.replace("{TAB}", "\t");
    let b64 = base64::engine::general_purpose::STANDARD.encode(script);
    format!("echo {b64} | base64 -d | sh")
}

struct Socket {
    port: u16,
    address: String,
    inode: u64,
}

struct Pane {
    pid: u32,
    r: PaneRef,
}

/// Parse [`scan_command`] output. Hides sshd and local DNS resolvers
/// (systemd-resolved, Docker's embedded DNS): never what you want to forward.
pub fn parse_scan(out: &str) -> Vec<ListeningPort> {
    let mut section = "";
    let mut home = String::new();
    let mut sockets: Vec<Socket> = vec![];
    let mut inode_pid: HashMap<u64, u32> = HashMap::new();
    let mut fd_pid: Option<u32> = None;
    let mut cwd: HashMap<u32, String> = HashMap::new();
    let mut ps: HashMap<u32, (u32, String)> = HashMap::new();
    let mut panes: Vec<Pane> = vec![];
    let mut docker: Vec<(String, Vec<u16>)> = vec![];

    for line in out.lines() {
        if let Some(h) = line.strip_prefix("@@HOME ") {
            home = h.trim().to_string();
            continue;
        }
        if let Some(s) = line.strip_prefix("@@") {
            section = s.trim();
            continue;
        }
        match section {
            "TCP" => sockets.extend(parse_proc_tcp(line)),
            "FD" => {
                if let Some(p) = line.strip_prefix("/proc/").and_then(|r| r.strip_suffix("/fd:")) {
                    fd_pid = p.parse().ok();
                } else if let (Some(pid), Some((_, target))) = (fd_pid, line.split_once(" -> ")) {
                    if let Some(inode) = target.trim().strip_prefix("socket:[").and_then(|r| r.strip_suffix(']')) {
                        if let Ok(i) = inode.parse() {
                            inode_pid.entry(i).or_insert(pid);
                        }
                    }
                }
            }
            "CWD" => {
                if let Some((left, target)) = line.split_once(" -> ") {
                    let path = left.split_whitespace().last().unwrap_or("");
                    if let Some(pid) = path.strip_prefix("/proc/").and_then(|r| r.strip_suffix("/cwd")) {
                        if let Ok(pid) = pid.parse() {
                            cwd.insert(pid, target.trim().to_string());
                        }
                    }
                }
            }
            "PS" => {
                let t = line.trim_start();
                let Some((pid, rest)) = t.split_once(char::is_whitespace) else { continue };
                let rest = rest.trim_start();
                let (ppid, args) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
                if let (Ok(pid), Ok(ppid)) = (pid.parse(), ppid.parse()) {
                    ps.insert(pid, (ppid, args.trim().to_string()));
                }
            }
            "TMUX" => {
                let f: Vec<&str> = line.split('\t').collect();
                if f.len() == 7 {
                    if let (Ok(pid), Ok(idx)) = (f[0].parse(), f[4].parse()) {
                        panes.push(Pane {
                            pid,
                            r: PaneRef {
                                session_id: f[1].into(),
                                session_name: f[2].into(),
                                window_id: f[3].into(),
                                window_index: idx,
                                window_name: f[5].into(),
                                pane_id: f[6].into(),
                            },
                        });
                    }
                }
            }
            "DOCKER" => {
                let f: Vec<&str> = line.split('\t').collect();
                if f.len() == 3 {
                    docker.push((format!("{} ({})", f[0], f[1]), docker_host_ports(f[2])));
                }
            }
            _ => {}
        }
    }

    let mut ports: Vec<ListeningPort> = vec![];
    for s in sockets {
        if s.port == 22 || matches!(s.address.as_str(), "127.0.0.53" | "127.0.0.54" | "127.0.0.11") {
            continue;
        }
        let public = matches!(s.address.as_str(), "0.0.0.0" | "::");
        let pid = inode_pid.get(&s.inode).copied();
        match ports.iter_mut().find(|p| p.port == s.port) {
            // Same port on v4 and v6: one row; public if either is.
            Some(p) => {
                p.public |= public;
                if p.pid.is_none() {
                    p.pid = pid;
                }
            }
            None => ports.push(ListeningPort {
                port: s.port,
                address: s.address,
                public,
                pid,
                process: None,
                command: None,
                cwd: None,
                pane: None,
                container: None,
                mine: false,
            }),
        }
    }
    for p in &mut ports {
        if let Some(pid) = p.pid {
            if let Some((_, args)) = ps.get(&pid) {
                p.process = short_process(args);
                p.command = Some(truncate(args, 300));
            }
            p.cwd = cwd.get(&pid).map(|c| tilde(c, &home));
            // Walk up the parent chain to a tmux pane's shell.
            let mut cur = pid;
            for _ in 0..40 {
                if let Some(pane) = panes.iter().find(|x| x.pid == cur) {
                    p.pane = Some(pane.r.clone());
                    break;
                }
                match ps.get(&cur) {
                    Some((ppid, _)) if *ppid > 1 && *ppid != cur => cur = *ppid,
                    _ => break,
                }
            }
        }
        p.container = docker.iter().find(|(_, ports)| ports.contains(&p.port)).map(|(n, _)| n.clone());
        p.mine = p.pid.is_some() || p.container.is_some();
    }
    ports.sort_by_key(|p| p.port);
    ports
}

fn parse_proc_tcp(line: &str) -> Option<Socket> {
    // sl local rem st tx:rx tr:when retrnsmt uid timeout inode
    let c: Vec<&str> = line.split_whitespace().collect();
    if c.len() < 10 || !c[0].ends_with(':') || c[3] != "0A" {
        return None;
    }
    let (addr_hex, port_hex) = c[1].split_once(':')?;
    let port = u16::from_str_radix(port_hex, 16).ok().filter(|p| *p > 0)?;
    let address = match addr_hex.len() {
        // Kernel order: each 32-bit word little-endian.
        8 => Ipv4Addr::from(u32::from_str_radix(addr_hex, 16).ok()?.to_le_bytes()).to_string(),
        32 => {
            let mut b = [0u8; 16];
            for w in 0..4 {
                let v = u32::from_str_radix(&addr_hex[w * 8..w * 8 + 8], 16).ok()?;
                b[w * 4..w * 4 + 4].copy_from_slice(&v.to_le_bytes());
            }
            let v6 = Ipv6Addr::from(b);
            // v4-mapped (::ffff:127.0.0.1) reads better as v4.
            v6.to_ipv4_mapped().map(|v4| v4.to_string()).unwrap_or_else(|| v6.to_string())
        }
        _ => return None,
    };
    Some(Socket { port, address, inode: c[9].parse().ok()? })
}

/// `0.0.0.0:8080->80/tcp, [::]:8080->80/tcp` → [8080]
fn docker_host_ports(s: &str) -> Vec<u16> {
    let mut v = vec![];
    for part in s.split(',') {
        let Some((host, _)) = part.trim().split_once("->") else { continue };
        let Some((_, port)) = host.rsplit_once(':') else { continue };
        let first = port.split('-').next().unwrap_or(port);
        if let Ok(p) = first.parse() {
            if !v.contains(&p) {
                v.push(p);
            }
        }
    }
    v
}

fn basename(p: &str) -> &str {
    p.rsplit('/').next().unwrap_or(p)
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(n).collect::<String>())
    }
}

fn tilde(path: &str, home: &str) -> String {
    if !home.is_empty() && home != "/" {
        if path == home {
            return "~".into();
        }
        if let Some(rest) = path.strip_prefix(home).filter(|r| r.starts_with('/')) {
            return format!("~{rest}");
        }
    }
    path.to_string()
}

/// A name you'd recognise: the script or module for interpreters
/// (`node …/vite` → `vite`, `python -m http.server` → `http.server`),
/// package-runner commands as typed (`npm run dev`), else the executable.
pub fn short_process(args: &str) -> Option<String> {
    let mut words = args.split_whitespace();
    let exe = basename(words.next()?).trim_end_matches(':');
    let rest: Vec<&str> = words.collect();
    let strip_ext = |s: &str| -> String {
        let b = basename(s);
        for ext in [".js", ".mjs", ".cjs", ".ts"] {
            if let Some(x) = b.strip_suffix(ext) {
                return x.to_string();
            }
        }
        b.to_string()
    };
    match exe {
        "npm" | "pnpm" | "yarn" | "npx" | "bunx" | "uv" | "poetry" | "pipenv" => {
            let mut v = vec![exe];
            v.extend(rest.iter().take(2).copied());
            Some(v.join(" "))
        }
        e if matches!(e, "node" | "nodejs" | "python" | "python2" | "python3" | "ruby" | "perl" | "java" | "bun" | "deno" | "php")
            || e.starts_with("python3.") =>
        {
            let mut i = 0;
            while i < rest.len() {
                let a = rest[i];
                if a == "-m" || a == "-jar" {
                    return rest.get(i + 1).map(|m| basename(m).to_string()).or(Some(exe.to_string()));
                }
                if matches!(a, "run" | "serve") && matches!(e, "bun" | "deno") {
                    i += 1;
                    continue;
                }
                if a.starts_with('-') {
                    i += 1;
                    continue;
                }
                return Some(strip_ext(a));
            }
            Some(exe.to_string())
        }
        e => Some(e.to_string()),
    }
}

// --------------------------------------------------------------- probe

/// Services recognised without connecting (never send them HTTP: Redis, for
/// one, logs that as an attack). Matched by process, then by port.
pub fn known_service(port: u16, process: Option<&str>) -> Option<&'static str> {
    let by_process = process.and_then(|p| {
        let p = p.to_ascii_lowercase();
        Some(match () {
            _ if p.starts_with("postgres") => "PostgreSQL",
            _ if p.starts_with("redis") => "Redis",
            _ if p.starts_with("valkey") => "Valkey",
            _ if p == "mysqld" || p == "mariadbd" || p.starts_with("mysqld") => "MySQL",
            _ if p == "mongod" || p == "mongos" => "MongoDB",
            _ if p == "memcached" => "Memcached",
            _ if p == "sshd" => "SSH",
            _ => return None,
        })
    });
    by_process.or(match port {
        5432 => Some("PostgreSQL"),
        6379 => Some("Redis"),
        3306 => Some("MySQL"),
        27017 => Some("MongoDB"),
        11211 => Some("Memcached"),
        9092 => Some("Kafka"),
        2181 => Some("ZooKeeper"),
        5672 => Some("RabbitMQ"),
        1433 => Some("SQL Server"),
        1521 => Some("Oracle"),
        _ => None,
    })
}

const PROBE_TIMEOUT: Duration = Duration::from_millis(2500);
const MAX_RESPONSE: usize = 64 * 1024;

/// Ask `localhost:port` on the server what it is. Read-only: at most a
/// banner read and one `GET /`.
pub async fn probe(ssh: &SshConnection, port: u16, process: Option<&str>) -> PortProbe {
    let mut r = PortProbe { port, ..Default::default() };
    if let Some(label) = known_service(port, process) {
        r.label = Some(label.into());
        return r;
    }
    let origin = SocketAddr::from((Ipv4Addr::LOCALHOST, 0));
    let mut ch = match tokio::time::timeout(PROBE_TIMEOUT, ssh.open_direct_tcpip("localhost", port, origin)).await {
        Ok(Ok(c)) => c,
        Ok(Err(_)) => {
            r.error = Some("not accepting connections".into());
            return r;
        }
        Err(_) => {
            r.error = Some("timed out".into());
            return r;
        }
    };
    // Server-first protocols greet us; HTTP servers wait for a request.
    let mut buf = vec![0u8; 4096];
    if let Ok(Ok(n)) = tokio::time::timeout(Duration::from_millis(300), ch.read(&mut buf)).await {
        if n > 0 {
            r.label = Some(classify_banner(&buf[..n]));
            return r;
        }
        r.error = Some("closed the connection".into());
        return r;
    }
    let req = "GET / HTTP/1.0\r\nHost: localhost\r\nUser-Agent: Muxscape\r\nAccept: text/html,*/*\r\nConnection: close\r\n\r\n";
    if ch.write_all(req.as_bytes()).await.is_err() {
        r.error = Some("closed the connection".into());
        return r;
    }
    let _ = ch.flush().await;
    let mut resp = Vec::new();
    let _ = tokio::time::timeout(PROBE_TIMEOUT, async {
        loop {
            match ch.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    resp.extend_from_slice(&buf[..n]);
                    if resp.len() >= MAX_RESPONSE {
                        break;
                    }
                }
            }
        }
    })
    .await;
    let _ = ch.shutdown().await;
    classify_response(&mut r, &resp);
    r
}

/// Name a server-first protocol from its greeting.
pub fn classify_banner(b: &[u8]) -> String {
    let text = String::from_utf8_lossy(&b[..b.len().min(200)]);
    if text.starts_with("SSH-") {
        "SSH".into()
    } else if b.len() > 5 && b[4] == 10 && text.contains("mysql") || text.contains("mysql_native_password") {
        "MySQL".into()
    } else if text.starts_with("220") {
        if text.to_ascii_lowercase().contains("ftp") { "FTP".into() } else { "SMTP".into() }
    } else if text.starts_with("+OK") {
        "POP3".into()
    } else if text.starts_with("* OK") {
        "IMAP".into()
    } else {
        "TCP service".into()
    }
}

/// Fill a probe from a (possibly empty) answer to `GET /`.
pub fn classify_response(r: &mut PortProbe, resp: &[u8]) {
    if resp.is_empty() {
        r.error = Some("no answer".into());
        return;
    }
    if !resp.starts_with(b"HTTP/") {
        if resp[0] == 0x15 || resp[0] == 0x16 {
            // TLS alert/handshake: an https server.
            r.web = true;
            r.tls = true;
            r.label = Some("HTTPS".into());
        } else if resp.len() >= 9 && resp[3] == 0x04 {
            // HTTP/2 SETTINGS frame first: gRPC or h2c.
            r.label = Some("gRPC / HTTP/2".into());
        } else {
            r.label = Some("TCP service".into());
        }
        return;
    }
    r.web = true;
    let (head, body) = match find(resp, b"\r\n\r\n") {
        Some(i) => (&resp[..i], &resp[i + 4..]),
        None => (resp, &resp[resp.len()..]),
    };
    let head = String::from_utf8_lossy(head);
    let body_s = String::from_utf8_lossy(body);
    let mut lines = head.lines();
    r.status = lines.next().and_then(|l| l.split_whitespace().nth(1)).and_then(|c| c.parse().ok());
    let mut headers: HashMap<String, String> = HashMap::new();
    for l in lines {
        if let Some((k, v)) = l.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }
    r.server = headers.get("server").map(|s| truncate(s, 40));
    if r.status.is_some_and(|s| (300..400).contains(&s)) {
        r.redirect = headers.get("location").map(|s| truncate(s, 80));
    }
    r.title = extract_title(&body_s);
    let ctype = headers.get("content-type").map(|s| s.to_ascii_lowercase()).unwrap_or_default();
    if body_s.contains("plain HTTP request was sent to HTTPS port") {
        r.tls = true;
    }
    let server = r.server.clone().unwrap_or_default().to_ascii_lowercase();
    let powered = headers.get("x-powered-by").map(|s| s.to_ascii_lowercase()).unwrap_or_default();
    let title = r.title.clone().unwrap_or_default().to_ascii_lowercase();
    let redirect = r.redirect.clone().unwrap_or_default();
    let label = match () {
        _ if body_s.contains("/@vite/client") => "Vite",
        _ if body_s.contains("__NEXT_DATA__") || body_s.contains("/_next/") || powered.contains("next.js") => "Next.js",
        _ if body_s.contains("swagger-ui") || title.contains("swagger") => "Swagger UI",
        _ if title.contains("jupyter") || body_s.contains("jupyter-config-data") => "Jupyter",
        _ if redirect.starts_with("/lab") || redirect.starts_with("/tree") || redirect.contains("/login?next=%2Flab") => "Jupyter",
        _ if body_s.contains("ng-version=") => "Angular",
        _ if body_s.contains("data-sveltekit") || body_s.contains("__sveltekit") => "SvelteKit",
        _ if body_s.contains("streamlit") => "Streamlit",
        _ if body_s.contains("gradio") => "Gradio",
        _ if body_s.contains(r#"{"detail":"Not Found"}"#) => "FastAPI",
        _ if server.starts_with("uvicorn") => "Uvicorn",
        _ if server.starts_with("gunicorn") => "Gunicorn",
        _ if server.starts_with("werkzeug") => "Flask",
        _ if server.starts_with("tornado") => "Tornado",
        _ if server.starts_with("simplehttp") || server.starts_with("basehttp") => "Python http.server",
        _ if powered.contains("express") => "Express",
        _ if powered.contains("php") => "PHP",
        _ if server.starts_with("nginx") => "nginx",
        _ if server.starts_with("apache") => "Apache",
        _ if server.starts_with("caddy") => "Caddy",
        _ if ctype.contains("json") => "JSON API",
        _ => "",
    };
    r.label = (!label.is_empty()).then(|| label.to_string());
}

fn find(h: &[u8], n: &[u8]) -> Option<usize> {
    h.windows(n.len()).position(|w| w == n)
}

fn extract_title(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let start = lower.find("<title")?;
    let open_end = start + lower[start..].find('>')? + 1;
    let end = open_end + lower[open_end..].find("</title")?;
    let raw = &html[open_end..end];
    let t = raw
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#x27;", "'");
    let t = t.split_whitespace().collect::<Vec<_>>().join(" ");
    (!t.is_empty()).then(|| truncate(&t, 80))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCAN: &str = "@@HOME /home/dev
@@TCP
  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 0100007F:1435 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 5001 1 0 100 0 0 10 0
   1: 00000000:0016 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 5002 1 0 100 0 0 10 0
   2: 0B00007F:9567 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 5003 1 0 100 0 0 10 0
   3: 00000000:1F41 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 5004 1 0 100 0 0 10 0
   4: 00000000:1538 00000000:0000 0A 00000000:00000000 00:00000000 00000000   999        0 5005 1 0 100 0 0 10 0
   5: 0100007F:1435 0100007F:A2B4 01 00000000:00000000 00:00000000 00000000  1000        0 5006 1 0 20 4 30 10 -1
   6: 00000000:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 5008 1 0 100 0 0 10 0
   0: 00000000000000000000000000000000:1F41 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 5007 1 0 100 0 0 10 0
@@FD
/proc/400/fd:
lrwx------ 1 dev dev 64 Nov  2 21:00 20 -> socket:[5001]
lrwx------ 1 dev dev 64 Nov  2 21:00 21 -> socket:[9999]
/proc/500/fd:
lrwx------ 1 dev dev 64 Nov  2 21:00 3 -> socket:[5004]
lrwx------ 1 dev dev 64 Nov  2 21:00 4 -> socket:[5007]
@@CWD
lrwxrwxrwx 1 dev dev 0 Nov  2 21:00 /proc/400/cwd -> /home/dev/plan-ui
lrwxrwxrwx 1 dev dev 0 Nov  2 21:00 /proc/500/cwd -> /srv/api
@@PS
    1     0 /sbin/init
  100     1 tmux new -s work
  200   100 -bash
  300   200 npm run dev
  400   300 node /home/dev/plan-ui/node_modules/.bin/vite --port 5173
  500     1 /home/dev/.venv/bin/python3 -m uvicorn app.main:app --port 8001
@@TMUX
200\t$1\twork\t@2\t0\tfrontend\t%3
@@DOCKER
qdrant\tqdrant/qdrant:latest\t0.0.0.0:6333->6333/tcp, [::]:6333->6333/tcp
web\tnginx:1.27\t0.0.0.0:8080-8081->80-81/tcp
@@END
";

    #[test]
    fn scan_attributes_ports() {
        let p = parse_scan(&SCAN.replace("\\t", "\t"));
        let ports: Vec<u16> = p.iter().map(|x| x.port).collect();
        // 22 (sshd) and 127.0.0.11 (docker DNS) hidden; non-LISTEN skipped.
        assert_eq!(ports, vec![5173, 5432, 8001, 8080]);

        let vite = &p[0];
        assert_eq!(vite.address, "127.0.0.1");
        assert!(!vite.public);
        assert_eq!(vite.pid, Some(400));
        assert_eq!(vite.process.as_deref(), Some("vite"));
        assert_eq!(vite.cwd.as_deref(), Some("~/plan-ui"));
        assert!(vite.command.as_deref().unwrap().contains("--port 5173"));
        let pane = vite.pane.as_ref().expect("found through node → npm → bash");
        assert_eq!((pane.session_name.as_str(), pane.window_index, pane.window_name.as_str()), ("work", 0, "frontend"));
        assert_eq!(pane.pane_id, "%3");
        assert!(vite.mine);

        // Someone else's process: listed, public, not attributed.
        let pg = &p[1];
        assert!(pg.public && pg.pid.is_none() && pg.process.is_none() && !pg.mine);

        // v4 + v6 merged; not in tmux; cwd outside home.
        let api = &p[2];
        assert_eq!(api.process.as_deref(), Some("uvicorn"));
        assert_eq!(api.cwd.as_deref(), Some("/srv/api"));
        assert!(api.pane.is_none() && api.public);

        let web = &p[3];
        assert_eq!(web.container.as_deref(), Some("web (nginx:1.27)"));
        assert!(web.mine);
    }

    #[test]
    fn process_names() {
        let n = |s: &str| short_process(s).unwrap();
        assert_eq!(n("node /x/node_modules/.bin/vite --port 5173"), "vite");
        assert_eq!(n("node /x/node_modules/next/dist/bin/next.js dev"), "next");
        assert_eq!(n("/usr/bin/python3.12 -m http.server 8000"), "http.server");
        assert_eq!(n("python manage.py runserver"), "manage.py");
        assert_eq!(n("npm run dev"), "npm run dev");
        assert_eq!(n("uv run uvicorn main:app"), "uv run uvicorn");
        assert_eq!(n("java -Xmx1g -jar /opt/app/server.jar"), "server.jar");
        assert_eq!(n("nginx: master process /usr/sbin/nginx"), "nginx");
        assert_eq!(n("/usr/lib/postgresql/16/bin/postgres -D /var/lib"), "postgres");
        assert_eq!(n("bun run dev.ts"), "dev");
    }

    #[test]
    fn known_services_are_not_probed() {
        assert_eq!(known_service(5432, None), Some("PostgreSQL"));
        assert_eq!(known_service(7000, Some("redis-server")), Some("Redis"));
        assert_eq!(known_service(5173, Some("vite")), None);
        assert_eq!(known_service(8080, None), None);
    }

    fn classify(resp: &str) -> PortProbe {
        let mut r = PortProbe::default();
        classify_response(&mut r, resp.as_bytes());
        r
    }

    #[test]
    fn http_answers() {
        let vite = classify("HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\r\n<!doctype html><html><head><script type=\"module\" src=\"/@vite/client\"></script><title>Plan\n  dashboard &amp; more</title></head>");
        assert!(vite.web && !vite.tls);
        assert_eq!(vite.status, Some(200));
        assert_eq!(vite.title.as_deref(), Some("Plan dashboard & more"));
        assert_eq!(vite.label.as_deref(), Some("Vite"));

        let fastapi = classify("HTTP/1.1 404 Not Found\r\nserver: uvicorn\r\ncontent-type: application/json\r\n\r\n{\"detail\":\"Not Found\"}");
        assert_eq!((fastapi.status, fastapi.label.as_deref(), fastapi.server.as_deref()), (Some(404), Some("FastAPI"), Some("uvicorn")));

        let redirect = classify("HTTP/1.1 302 Found\r\nLocation: /login?next=%2F\r\nServer: TornadoServer/6.4\r\n\r\n");
        assert_eq!(redirect.redirect.as_deref(), Some("/login?next=%2F"));
        assert_eq!(redirect.label.as_deref(), Some("Tornado"));
        let jupyter = classify("HTTP/1.1 302 Found\r\nLocation: /lab?token=4f1c\r\nServer: TornadoServer/6.4\r\n\r\n");
        assert_eq!(jupyter.label.as_deref(), Some("Jupyter"));

        let json = classify("HTTP/1.0 200 OK\r\nContent-Type: application/json\r\n\r\n{\"ok\":true}");
        assert_eq!(json.label.as_deref(), Some("JSON API"));

        let https = classify("HTTP/1.1 400 Bad Request\r\nServer: nginx\r\n\r\n<html><center>The plain HTTP request was sent to HTTPS port</center>");
        assert!(https.tls && https.web);

        let mut tls = PortProbe::default();
        classify_response(&mut tls, &[0x15, 0x03, 0x01, 0x00, 0x02, 0x02, 0x46]);
        assert!(tls.tls && tls.web);

        let mut h2 = PortProbe::default();
        classify_response(&mut h2, &[0, 0, 0x12, 0x04, 0, 0, 0, 0, 0, 0, 3]);
        assert_eq!(h2.label.as_deref(), Some("gRPC / HTTP/2"));
        assert!(!h2.web);

        assert_eq!(classify("").error.as_deref(), Some("no answer"));
    }

    #[test]
    fn banners() {
        assert_eq!(classify_banner(b"SSH-2.0-OpenSSH_9.6\r\n"), "SSH");
        assert_eq!(classify_banner(b"220 mail.example.com ESMTP Postfix\r\n"), "SMTP");
        assert_eq!(classify_banner(b"220 (vsFTPd 3.0.5)\r\n"), "FTP");
        assert_eq!(classify_banner(b"J\0\0\0\n8.0.36\0....mysql_native_password\0"), "MySQL");
        assert_eq!(classify_banner(b"hello"), "TCP service");
    }

    #[test]
    fn scan_script_is_transport_safe() {
        let c = scan_command();
        assert!(c.starts_with("echo ") && c.ends_with(" | base64 -d | sh"));
        let b64 = c.trim_start_matches("echo ").trim_end_matches(" | base64 -d | sh");
        assert!(b64.chars().all(|ch| ch.is_ascii_alphanumeric() || "+/=".contains(ch)));
    }
}
