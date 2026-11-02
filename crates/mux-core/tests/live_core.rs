//! End-to-end tests of mux-core against real tmux over real SSH (the
//! disposable docker servers from dev/test-server/up.sh).
//!
//!   MUXSCAPE_LIVE=1 cargo test -p mux-core --test live_core -- --test-threads=1
//!
//! Skipped unless MUXSCAPE_LIVE is set. Nothing here is mocked: every
//! assertion is about state that came back from a remote tmux server.

use base64::Engine;
use mux_core::api::{Event, PromptAnswer, Request};
use mux_core::hosts::{Host, HostSource, HostStore};
use mux_core::ssh_config::SshConfig;
use mux_core::Core;
use mux_ssh::KnownHosts;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn live() -> bool {
    std::env::var("MUXSCAPE_LIVE").is_ok()
}

fn key() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../dev/keys/test_ed25519")
}

/// Out-of-band "other tmux client": plain OpenSSH, like a user's terminal.
fn ssh(port: u16, cmd: &str) -> String {
    let out = std::process::Command::new("ssh")
        .args(["-i", key().to_str().unwrap(), "-p", &port.to_string()])
        .args(["-o", "StrictHostKeyChecking=no", "-o", "UserKnownHostsFile=/dev/null", "-o", "LogLevel=ERROR"])
        .arg("dev@127.0.0.1")
        .arg(cmd)
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).into_owned()
}

struct Harness {
    core: Core,
    log: Arc<Mutex<Vec<Value>>>,
    host_id: String,
}

impl Harness {
    async fn new(port: u16) -> Harness {
        let kh = std::env::temp_dir().join(format!("muxscape-core-kh-{}-{port}", std::process::id()));
        let mut store = HostStore::open(None, SshConfig::default());
        let host = store
            .upsert(Host {
                id: String::new(),
                label: format!("test {port}"),
                folder: "Test".into(),
                hostname: "127.0.0.1".into(),
                port,
                user: Some("dev".into()),
                identity_files: vec![key().display().to_string()],
                identities_only: true,
                jump: None,
                auto_tmux: true,
                source: HostSource::Saved,
            })
            .unwrap();
        let (core, mut rx) = Core::new(store, KnownHosts { path: kh });
        let log = Arc::new(Mutex::new(Vec::new()));
        let l = log.clone();
        let c = core.clone();
        tokio::spawn(async move {
            while let Some(ev) = rx.recv().await {
                let v = serde_json::to_value(&ev).unwrap();
                if let Event::Prompt { prompt, .. } = &ev {
                    // Auto-accept host keys (a disposable test server).
                    let c = c.clone();
                    let p = *prompt;
                    tokio::spawn(async move {
                        let _ = c
                            .handle(Request::AnswerPrompt { prompt: p, answer: PromptAnswer::HostKey { accept: true, save: true } })
                            .await;
                    });
                }
                l.lock().unwrap().push(v);
            }
        });
        Harness { core, log, host_id: host.id }
    }

    async fn req(&self, r: Value) -> Value {
        let r: Request = serde_json::from_value(r).unwrap();
        self.core.handle(r).await.unwrap_or_else(|e| panic!("request failed: {e}"))
    }

    fn mark(&self) -> usize {
        self.log.lock().unwrap().len()
    }

    /// Wait for an event after position `from` matching `pred`.
    async fn wait(&self, from: usize, what: &str, pred: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if let Some(v) = self.log.lock().unwrap()[from..].iter().find(|v| pred(v)) {
                return v.clone();
            }
            if Instant::now() > deadline {
                let tail: Vec<String> = self.log.lock().unwrap()[from..]
                    .iter()
                    .map(|v| v["event"].as_str().unwrap_or("").to_string())
                    .collect();
                panic!("timed out waiting for {what}; events since mark: {tail:?}");
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    async fn wait_state(&self, from: usize, att: u64, what: &str, pred: impl Fn(&Value) -> bool) -> Value {
        self.wait(from, what, |v| v["event"] == "sessionState" && v["attachment"] == att && pred(&v["state"]))
            .await["state"]
            .clone()
    }

    /// Concatenated output (snapshot + live) for a pane after `from`.
    fn pane_text(&self, from: usize, att: u64, pane: &str) -> String {
        let mut s = Vec::new();
        for v in &self.log.lock().unwrap()[from..] {
            if (v["event"] == "paneOutput" || v["event"] == "paneSnapshot") && v["attachment"] == att && v["pane"] == pane {
                if v["event"] == "paneSnapshot" {
                    s.clear();
                }
                s.extend(base64::engine::general_purpose::STANDARD.decode(v["data"].as_str().unwrap()).unwrap());
            }
        }
        String::from_utf8_lossy(&s).into_owned()
    }

    /// Render everything the GUI would have written into the pane's terminal
    /// through a real VT emulator and return the visible screen text.
    fn pane_screen(&self, from: usize, att: u64, pane: &str) -> String {
        let mut parser: Option<vt100::Parser> = None;
        for v in &self.log.lock().unwrap()[from..] {
            if v["attachment"] != att || v["pane"] != pane {
                continue;
            }
            let data = || base64::engine::general_purpose::STANDARD.decode(v["data"].as_str().unwrap()).unwrap();
            if v["event"] == "paneSnapshot" {
                let mut p = vt100::Parser::new(v["rows"].as_u64().unwrap() as u16, v["cols"].as_u64().unwrap() as u16, 0);
                p.process(&data());
                parser = Some(p);
            } else if v["event"] == "paneOutput" {
                if let Some(p) = parser.as_mut() {
                    p.process(&data());
                }
            }
        }
        parser.map(|p| p.screen().contents()).unwrap_or_default()
    }

    async fn wait_screen(&self, from: usize, att: u64, pane: &str, needle: &str) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !self.pane_screen(from, att, pane).contains(needle) {
            assert!(Instant::now() < deadline, "screen of {pane} never showed {needle:?}:\n{}", self.pane_screen(from, att, pane));
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    async fn wait_text(&self, from: usize, att: u64, pane: &str, needle: &str) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !self.pane_text(from, att, pane).contains(needle) {
            assert!(Instant::now() < deadline, "pane {pane} never showed {needle:?}; got {:?}", self.pane_text(from, att, pane));
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

fn session_id(port: u16, name: &str) -> String {
    ssh(port, &format!("tmux display -p -t {name}: '#{{session_id}}'")).trim().to_string()
}

fn b64(s: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(s)
}

async fn acceptance(port: u16) {
    let name = format!("accept{port}");
    ssh(port, &format!("tmux kill-session -t {name} 2>/dev/null; tmux new-session -d -s {name} -x 120 -y 40"));
    for i in 1..20 {
        ssh(port, &format!("tmux new-window -d -t {name}: -n w{i}"));
    }
    // Window 17 has content before we ever attach.
    ssh(port, &format!("tmux send-keys -t {name}:17 'seq 1 300; echo before-attach' Enter"));
    let sid = session_id(port, &name);
    assert!(sid.starts_with('$'));

    let h = Harness::new(port).await;
    let r = h.req(json!({"cmd": "connect", "hostId": h.host_id})).await;
    assert_eq!(r["tmux"]["kind"], "available");
    assert!(r["sessions"].as_array().unwrap().iter().any(|s| s["name"] == name.as_str()));

    // 1-2. Attach: all 20 windows appear as tabs.
    let m = h.mark();
    let a = h.req(json!({"cmd": "attach", "hostId": h.host_id, "target": {"kind": "existing", "session": sid}, "cols": 120, "rows": 40})).await;
    let att = a["attachment"].as_u64().unwrap();
    let st = h.wait_state(m, att, "20 windows", |s| s["windows"].as_array().unwrap().len() == 20).await;
    let windows = st["windows"].as_array().unwrap().clone();
    assert_eq!(windows[17]["name"], "w17");
    // Every pane gets a snapshot; window 17's includes pre-existing output
    // and scrollback.
    let attach_mark = m;
    let w17 = windows[17]["id"].as_str().unwrap().to_string();
    let p17 = windows[17]["layout"]["value"].as_str().unwrap().to_string();
    h.wait(m, "snapshot of window 17", |v| v["event"] == "paneSnapshot" && v["pane"] == p17.as_str()).await;
    let text = h.pane_text(m, att, &p17);
    assert!(text.contains("before-attach"), "{text}");
    // History lines come without trailing padding.
    assert!(text.contains("\r\n150\r\n151\r\n"), "scrollback should be restored: {:?}", &text[..text.len().min(600)]);

    // 3. Click window 17 directly.
    let m = h.mark();
    h.req(json!({"cmd": "selectWindow", "attachment": att, "window": w17})).await;
    h.wait_state(m, att, "w17 active", |s| s["windows"][17]["active"] == true).await;
    assert_eq!(ssh(port, &format!("tmux display -p -t {name}: '#{{window_index}}'")).trim(), "17");

    // 4. Interactive work in that window, including a full-screen app.
    let m = h.mark();
    h.req(json!({"cmd": "input", "attachment": att, "pane": p17, "data": b64("echo hello-$((6*7))\r")})).await;
    h.wait_text(m, att, &p17, "hello-42").await;
    h.req(json!({"cmd": "input", "attachment": att, "pane": p17, "data": b64("vi -u NONE /tmp/muxscape-vi.txt\r")})).await;
    h.wait_text(m, att, &p17, "\x1b[?1049h").await; // app switched to the alternate screen
    h.req(json!({"cmd": "input", "attachment": att, "pane": p17, "data": b64("iviml\u{e9}ine \u{20ac}\x1b")})).await;
    h.wait_screen(attach_mark, att, &p17, "vimléine €").await; // UTF-8 both ways, via a real VT emulator

    // 5. Split panes and drag a divider.
    let m = h.mark();
    h.req(json!({"cmd": "splitPane", "attachment": att, "pane": p17, "horizontal": true})).await;
    let st = h.wait_state(m, att, "split", |s| s["windows"][17]["layout"]["type"] == "horizontal").await;
    let lay = &st["windows"][17]["layout"];
    let left_w = lay["value"][0]["width"].as_u64().unwrap();
    let new_pane = lay["value"][1]["value"].as_str().unwrap().to_string();
    h.wait(m, "snapshot of new pane", |v| v["event"] == "paneSnapshot" && v["pane"] == new_pane.as_str()).await;
    let m = h.mark();
    h.req(json!({"cmd": "moveDivider", "attachment": att, "window": w17, "path": [], "index": 0, "delta": -10})).await;
    h.wait_state(m, att, "divider moved", |s| s["windows"][17]["layout"]["value"][0]["width"].as_u64() == Some(left_w - 10)).await;
    let tmux_w = ssh(port, &format!("tmux display -p -t {p17} '#{{pane_width}}'"));
    assert_eq!(tmux_w.trim().parse::<u64>().unwrap(), left_w - 10, "real tmux pane resized");

    // 8. Changes from an ordinary tmux client show up.
    let m = h.mark();
    ssh(port, &format!("tmux new-window -d -t {name}: -n from-outside; tmux rename-window -t {name}:3 renamed-outside"));
    h.wait_state(m, att, "external changes", |s| {
        let w = s["windows"].as_array().unwrap();
        w.len() == 21 && w.iter().any(|w| w["name"] == "from-outside") && w[3]["name"] == "renamed-outside"
    })
    .await;
    let m = h.mark();
    ssh(port, &format!("tmux kill-window -t {name}:from-outside"));
    h.wait_state(m, att, "external close", |s| s["windows"].as_array().unwrap().len() == 20).await;

    // Resize from the GUI propagates to tmux.
    let m = h.mark();
    h.req(json!({"cmd": "resize", "attachment": att, "cols": 100, "rows": 30})).await;
    h.wait_state(m, att, "resized", |s| s["windows"][17]["layout"]["width"] == 100 && s["windows"][17]["layout"]["height"] == 30).await;

    // 6. A long-running process, then the transport dies abruptly.
    h.req(json!({"cmd": "input", "attachment": att, "pane": new_pane, "data": b64("while true; do date +%s > /tmp/tick-$PPID; sleep 1; done\r")})).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let m = h.mark();
    // Kill only our sshd connection processes (the tmux server is separate).
    let container = if port == 2222 { "muxscape-t34" } else { "muxscape-t33" };
    std::process::Command::new("docker").args(["exec", container, "pkill", "-f", "sshd: dev"]).status().unwrap();
    h.wait(m, "connection lost", |v| v["event"] == "attachmentStatus" && v["live"] == false).await;
    h.wait(m, "reconnecting", |v| v["event"] == "connection" && v["state"] == "reconnecting").await;
    h.wait(m, "reconnected", |v| v["event"] == "connection" && v["state"] == "connected").await;
    h.wait(m, "live again", |v| v["event"] == "attachmentStatus" && v["live"] == true).await;
    // 7. Same session, same layout, processes still running, vim restored
    //    on the alternate screen.
    let st = h.wait_state(m, att, "restored", |s| s["windows"].as_array().unwrap().len() == 20).await;
    assert_eq!(st["windows"][17]["layout"]["type"], "horizontal");
    h.wait(m, "vim pane snapshot", |v| v["event"] == "paneSnapshot" && v["pane"] == p17.as_str()).await;
    let t = h.pane_text(m, att, &p17);
    assert!(t.contains("\x1b[?1049h"), "restored on the alternate screen");
    let screen = h.pane_screen(m, att, &p17);
    assert!(screen.starts_with("vimléine €"), "vim screen restored after reconnect:\n{screen}");
    let t1 = ssh(port, "cat /tmp/tick-* | sort | tail -1");
    tokio::time::sleep(Duration::from_millis(2100)).await;
    let t2 = ssh(port, "cat /tmp/tick-* | sort | tail -1");
    assert_ne!(t1.trim(), t2.trim(), "background loop kept running through the disconnect");

    // Closing the app = detach, never kill.
    let m = h.mark();
    h.req(json!({"cmd": "disconnect", "hostId": h.host_id})).await;
    h.wait(m, "detached", |v| v["event"] == "attachmentClosed").await;
    assert!(ssh(port, "tmux ls -F '#{session_name}'").lines().any(|l| l == name));

    // 7 (again). A brand-new app instance finds the same session.
    let h2 = Harness::new(port).await;
    let m = h2.mark();
    let a = h2.req(json!({"cmd": "attach", "hostId": h2.host_id, "target": {"kind": "existing", "session": sid}, "cols": 100, "rows": 30})).await;
    let att2 = a["attachment"].as_u64().unwrap();
    h2.wait_state(m, att2, "same windows", |s| s["windows"].as_array().unwrap().len() == 20 && s["windows"][17]["layout"]["type"] == "horizontal").await;

    // Tabs: create, rename, close; panes: close.
    let m = h2.mark();
    h2.req(json!({"cmd": "newWindow", "attachment": att2})).await;
    let st = h2.wait_state(m, att2, "new window", |s| s["windows"].as_array().unwrap().len() == 21).await;
    let last = st["windows"][20]["id"].as_str().unwrap().to_string();
    h2.req(json!({"cmd": "renameWindow", "attachment": att2, "window": last, "name": "it's \"quoted\" ; done"})).await;
    h2.wait_state(m, att2, "renamed", |s| s["windows"][20]["name"] == "it's \"quoted\" ; done").await;
    let m = h2.mark();
    h2.req(json!({"cmd": "killWindow", "attachment": att2, "window": last})).await;
    h2.wait_state(m, att2, "closed tab", |s| s["windows"].as_array().unwrap().len() == 20).await;
    let m = h2.mark();
    h2.req(json!({"cmd": "killPane", "attachment": att2, "pane": new_pane})).await;
    h2.wait_state(m, att2, "pane closed", |s| s["windows"][17]["layout"]["type"] == "pane").await;

    ssh(port, &format!("tmux kill-session -t {name}"));
    let m = h2.mark();
    h2.wait(m, "session ended", |v| v["event"] == "attachmentClosed" && v["attachment"] == att2).await;
}

#[tokio::test]
async fn acceptance_tmux_34() {
    if live() {
        acceptance(2222).await;
    }
}

#[tokio::test]
async fn acceptance_tmux_33() {
    if live() {
        acceptance(2223).await;
    }
}

#[tokio::test]
async fn new_and_auto_sessions_never_duplicate() {
    if !live() {
        return;
    }
    let port = 2222;
    ssh(port, "tmux kill-session -t fresh 2>/dev/null");
    let h = Harness::new(port).await;
    let m = h.mark();
    let a = h.req(json!({"cmd": "attach", "hostId": h.host_id, "target": {"kind": "new", "name": "fresh"}, "cols": 80, "rows": 24})).await;
    let att = a["attachment"].as_u64().unwrap();
    let st = h.wait_state(m, att, "fresh session", |s| s["name"] == "fresh" && !s["windows"].as_array().unwrap().is_empty()).await;
    let sid = st["id"].as_str().unwrap().to_string();
    // Attaching again to the same session reuses the attachment.
    let b = h.req(json!({"cmd": "attach", "hostId": h.host_id, "target": {"kind": "existing", "session": sid}, "cols": 80, "rows": 24})).await;
    assert_eq!(b["attachment"].as_u64().unwrap(), att);
    // "New" with an existing name attaches instead of duplicating.
    let before = ssh(port, "tmux ls | wc -l");
    let h2 = Harness::new(port).await;
    let m = h2.mark();
    let c = h2.req(json!({"cmd": "attach", "hostId": h2.host_id, "target": {"kind": "new", "name": "fresh"}, "cols": 80, "rows": 24})).await;
    let st = h2.wait_state(m, c["attachment"].as_u64().unwrap(), "same session", |s| s["name"] == "fresh").await;
    assert_eq!(st["id"].as_str().unwrap(), sid);
    assert_eq!(ssh(port, "tmux ls | wc -l"), before);
    // Rename session from the GUI with awkward characters.
    h.req(json!({"cmd": "renameSession", "attachment": att, "name": "fresh 'two'"})).await;
    let m = h.mark();
    h.wait(m, "sessions list", |v| {
        v["event"] == "sessions" && v["sessions"].as_array().unwrap().iter().any(|s| s["name"] == "fresh 'two'")
    })
    .await;
    h.req(json!({"cmd": "killSession", "hostId": h.host_id, "session": sid})).await;
    h.wait(m, "closed", |v| v["event"] == "attachmentClosed" && v["attachment"] == att).await;
}

#[tokio::test]
async fn server_without_tmux_gets_plain_shell() {
    if !live() {
        return;
    }
    let h = Harness::new(2224).await;
    let r = h.req(json!({"cmd": "connect", "hostId": h.host_id})).await;
    assert_eq!(r["tmux"]["kind"], "notInstalled");
    let r = h.req(json!({"cmd": "openShell", "hostId": h.host_id, "cols": 80, "rows": 24})).await;
    let sh = r["shell"].as_u64().unwrap();
    let m = h.mark();
    h.req(json!({"cmd": "shellInput", "shell": sh, "data": b64("echo plain-$((40+2)); tput cols\r")})).await;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let all: String = h.log.lock().unwrap()[m..]
            .iter()
            .filter(|v| v["event"] == "shellOutput")
            .map(|v| String::from_utf8_lossy(&base64::engine::general_purpose::STANDARD.decode(v["data"].as_str().unwrap()).unwrap()).into_owned())
            .collect();
        if all.contains("plain-42") && all.contains("\r\n80\r\n") {
            break;
        }
        assert!(Instant::now() < deadline, "{all:?}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    h.req(json!({"cmd": "shellInput", "shell": sh, "data": b64("exit\r")})).await;
    h.wait(m, "shell closed", |v| v["event"] == "shellClosed").await;
}

/// HTTP GET through a local tunnel port; returns the whole response.
async fn http_get(local_port: u16, path: &str) -> std::io::Result<String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut s = tokio::net::TcpStream::connect(("127.0.0.1", local_port)).await?;
    s.write_all(format!("GET {path} HTTP/1.0\r\nHost: localhost\r\n\r\n").as_bytes()).await?;
    let mut out = String::new();
    tokio::time::timeout(Duration::from_secs(10), s.read_to_string(&mut out)).await??;
    Ok(out)
}

fn tunnels_in(v: &Value) -> Vec<Value> {
    v["tunnels"].as_array().cloned().unwrap_or_default()
}

#[tokio::test]
async fn port_forwarding() {
    if !live() {
        return;
    }
    let port = 2222;
    let remote = 18573u16;
    // A tiny HTTP server that announces itself like a dev server would.
    ssh(port, "pkill -f 'perl /tmp/[m]uxscape-srv'; cat > /tmp/muxscape-srv.pl <<'PL'
use IO::Socket::INET;
my $port = shift;
my $s = IO::Socket::INET->new(LocalAddr => '127.0.0.1', LocalPort => $port, Listen => 10, ReuseAddr => 1) or die \"listen: $!\";
$| = 1;
print \"Server listening on http://localhost:$port/app?token=xyz\\n\";
while (my $c = $s->accept) { my $r = <$c>; print $c \"HTTP/1.0 200 OK\\r\\nContent-Type: text/html\\r\\nServer: MuxTest/1.0\\r\\n\\r\\n<title>Muxscape test $port</title>hello-from-$port $r\"; close $c; }
PL
tmux kill-session -t fwd 2>/dev/null; tmux new-session -d -s fwd -x 120 -y 30");
    let sid = session_id(port, "fwd");

    let h = Harness::new(port).await;
    h.req(json!({"cmd": "connect", "hostId": h.host_id})).await;
    let m = h.mark();
    let a = h.req(json!({"cmd": "attach", "hostId": h.host_id, "target": {"kind": "existing", "session": sid}, "cols": 120, "rows": 30})).await;
    let att = a["attachment"].as_u64().unwrap();
    let st = h.wait_state(m, att, "session", |s| !s["windows"].as_array().unwrap().is_empty()).await;
    let pane = st["windows"][0]["layout"]["value"].as_str().unwrap().to_string();
    h.wait(m, "snapshot", |v| v["event"] == "paneSnapshot" && v["pane"] == pane.as_str()).await;

    // 1. The server's banner in the pane is detected, token path included.
    let m = h.mark();
    h.req(json!({"cmd": "input", "attachment": att, "pane": pane, "data": b64(&format!("perl /tmp/muxscape-srv.pl {remote}\r"))})).await;
    let d = h.wait(m, "port detected", |v| v["event"] == "portDetected" && v["port"] == remote).await;
    assert_eq!(d["hostId"], h.host_id.as_str());
    assert_eq!(d["path"], "/app?token=xyz");

    // 2. One request forwards it; real HTTP goes through the SSH channel.
    let m = h.mark();
    let t = h.req(json!({"cmd": "openTunnel", "hostId": h.host_id, "remotePort": remote, "path": d["path"]})).await;
    let tid = t["id"].as_u64().unwrap();
    let local = t["localPort"].as_u64().unwrap() as u16;
    assert_eq!(t["path"], "/app?token=xyz");
    h.wait(m, "tunnels event", |v| v["event"] == "tunnels" && tunnels_in(v).iter().any(|t| t["id"] == tid)).await;
    let body = http_get(local, "/app?token=xyz").await.unwrap();
    assert!(body.contains(&format!("hello-from-{remote} GET /app?token=xyz")), "{body}");
    // Many parallel connections (a browser opens several).
    let gets: Vec<_> = (0..8).map(|i| tokio::spawn(http_get(local, if i % 2 == 0 { "/a" } else { "/b" }))).collect();
    for g in gets {
        assert!(g.await.unwrap().unwrap().contains("hello-from-"));
    }

    // 3. Port scan sees it, and knows what started it and where.
    let scan = h.req(json!({"cmd": "scanPorts", "hostId": h.host_id})).await;
    let ports = scan["ports"].as_array().unwrap();
    assert!(!ports.iter().any(|p| p["port"] == 22), "sshd is hidden");
    let me = ports.iter().find(|p| p["port"] == remote).unwrap_or_else(|| panic!("{scan}"));
    assert_eq!(me["process"], "muxscape-srv.pl", "{me}");
    assert_eq!(me["command"], format!("perl /tmp/muxscape-srv.pl {remote}").as_str());
    assert_eq!(me["cwd"], "~");
    assert_eq!(me["address"], "127.0.0.1");
    assert_eq!(me["public"], false);
    assert_eq!(me["mine"], true);
    assert_eq!(me["pane"]["sessionName"], "fwd", "{me}");
    assert_eq!(me["pane"]["windowIndex"], 0);
    assert_eq!(me["pane"]["paneId"], pane.as_str());
    // ...and what it serves (one read-only GET), plus a dead port and a
    // real server-first banner (sshd on 22).
    let pr = h
        .req(json!({"cmd": "probePorts", "hostId": h.host_id,
            "targets": [{"port": remote, "process": "muxscape-srv.pl"}, {"port": 18998}, {"port": 22}]}))
        .await;
    let probes = pr["probes"].as_array().unwrap();
    let web = probes.iter().find(|p| p["port"] == remote).unwrap();
    assert_eq!(web["web"], true, "{pr}");
    assert_eq!(web["status"], 200);
    assert_eq!(web["title"], format!("Muxscape test {remote}").as_str());
    assert_eq!(web["server"], "MuxTest/1.0");
    let dead = probes.iter().find(|p| p["port"] == 18998).unwrap();
    assert_eq!(dead["error"], "not accepting connections", "{pr}");
    let sshd = probes.iter().find(|p| p["port"] == 22).unwrap();
    assert_eq!((sshd["label"].as_str(), sshd["web"].as_bool()), (Some("SSH"), Some(false)), "{pr}");
    // Forwarding the same port again reuses the tunnel; no second detection.
    let again = h.req(json!({"cmd": "openTunnel", "hostId": h.host_id, "remotePort": remote})).await;
    assert_eq!(again["id"].as_u64(), Some(tid));
    assert_eq!(again["path"], "/app?token=xyz");

    // 4. Nothing listening: the browser's connection is closed and the
    //    tunnel says why.
    let m = h.mark();
    let dead = h.req(json!({"cmd": "openTunnel", "hostId": h.host_id, "remotePort": 18999})).await;
    let dead_id = dead["id"].as_u64().unwrap();
    let r = http_get(dead["localPort"].as_u64().unwrap() as u16, "/").await;
    assert!(r.map(|s| s.is_empty()).unwrap_or(true), "no response from a dead port");
    h.wait(m, "dead-port error", |v| {
        v["event"] == "tunnels"
            && tunnels_in(v).iter().any(|t| t["id"] == dead_id && t["error"].as_str().is_some_and(|e| e.contains("nothing is listening")))
    })
    .await;
    h.req(json!({"cmd": "closeTunnel", "tunnel": dead_id})).await;

    // 5. The tunnel survives the SSH connection dropping: same local port,
    //    works again once reconnected.
    let m = h.mark();
    std::process::Command::new("docker").args(["exec", "muxscape-t34", "pkill", "-f", "sshd: dev"]).status().unwrap();
    h.wait(m, "reconnecting", |v| v["event"] == "connection" && v["state"] == "reconnecting").await;
    // A request made mid-reconnect is held, then served.
    let during = tokio::spawn(http_get(local, "/during"));
    h.wait(m, "reconnected", |v| v["event"] == "connection" && v["state"] == "connected").await;
    let body = during.await.unwrap().unwrap();
    assert!(body.contains("GET /during"), "request during reconnect: {body}");

    // 6. Pinned forwards come back on the next connect.
    h.req(json!({"cmd": "pinTunnel", "tunnel": tid, "pinned": true})).await;
    let m = h.mark();
    h.req(json!({"cmd": "disconnect", "hostId": h.host_id})).await;
    h.wait(m, "tunnels closed on disconnect", |v| v["event"] == "tunnels" && tunnels_in(v).is_empty()).await;
    assert!(http_get(local, "/").await.is_err(), "local port released after disconnect");
    let m = h.mark();
    h.req(json!({"cmd": "connect", "hostId": h.host_id})).await;
    let ev = h.wait(m, "pinned tunnel restored", |v| {
        v["event"] == "tunnels" && tunnels_in(v).iter().any(|t| t["remotePort"] == remote && t["pinned"] == true)
    })
    .await;
    let restored = tunnels_in(&ev).into_iter().find(|t| t["remotePort"] == remote).unwrap();
    assert_eq!(restored["path"], "/app?token=xyz");
    let local2 = restored["localPort"].as_u64().unwrap() as u16;
    assert!(http_get(local2, "/back").await.unwrap().contains("GET /back"));

    // 7. Stopping it frees the port and unpins it.
    let id2 = restored["id"].as_u64().unwrap();
    h.req(json!({"cmd": "closeTunnel", "tunnel": id2})).await;
    assert!(h.req(json!({"cmd": "listTunnels"})).await.as_array().unwrap().is_empty());
    assert!(http_get(local2, "/").await.is_err());
    h.req(json!({"cmd": "disconnect", "hostId": h.host_id})).await;
    h.req(json!({"cmd": "connect", "hostId": h.host_id})).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(h.req(json!({"cmd": "listTunnels"})).await.as_array().unwrap().is_empty(), "unpinned on stop");

    ssh(port, "pkill -f 'perl /tmp/[m]uxscape-srv'; tmux kill-session -t fwd");
}
