//! Live tests against the disposable docker servers from dev/test-server.
//! Run with: MUXSCAPE_LIVE=1 cargo test -p mux-ssh -- --test-threads=1
//! (skipped silently when MUXSCAPE_LIVE is not set).

use mux_ssh::*;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

fn live() -> bool {
    std::env::var("MUXSCAPE_LIVE").is_ok()
}

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

struct TestPrompter {
    decision: HostKeyDecision,
    host_prompts: AtomicUsize,
    passphrase: Option<String>,
}

#[async_trait::async_trait]
impl Prompter for TestPrompter {
    async fn unknown_host_key(&self, info: &HostKeyInfo) -> HostKeyDecision {
        assert!(info.fingerprint.starts_with("SHA256:"));
        self.host_prompts.fetch_add(1, Ordering::SeqCst);
        self.decision.clone()
    }
    async fn password(&self, _: &str) -> Option<String> {
        None
    }
    async fn key_passphrase(&self, _: &Path) -> Option<String> {
        self.passphrase.clone()
    }
    async fn keyboard_interactive(&self, _: &str, _: &str, _: &[(String, bool)]) -> Option<Vec<String>> {
        None
    }
}

fn prompter(d: HostKeyDecision) -> Arc<TestPrompter> {
    Arc::new(TestPrompter { decision: d, host_prompts: AtomicUsize::new(0), passphrase: None })
}

fn target(port: u16, key: &str) -> SshTarget {
    let mut t = SshTarget::new("127.0.0.1", port, "dev");
    t.identity_files = vec![repo().join("dev/keys").join(key)];
    t.identities_only = true;
    t
}

fn tmp_known_hosts(name: &str) -> KnownHosts {
    let p = std::env::temp_dir().join(format!("muxscape-kh-{}-{}", name, std::process::id()));
    let _ = std::fs::remove_file(&p);
    KnownHosts { path: p }
}

#[tokio::test]
async fn connect_exec_and_known_hosts_lifecycle() {
    if !live() {
        return;
    }
    let kh = tmp_known_hosts("lifecycle");
    let p = prompter(HostKeyDecision::AcceptAndSave);
    let c = SshConnection::connect(&target(2222, "test_ed25519"), &kh, p.clone()).await.unwrap();
    assert_eq!(p.host_prompts.load(Ordering::SeqCst), 1);
    let out = c.exec("tmux -V; echo err >&2; exit 3").await.unwrap();
    assert_eq!(out.stdout_str().trim(), "tmux 3.4");
    assert_eq!(out.stderr_str().trim(), "err");
    assert_eq!(out.status, Some(3));
    c.disconnect().await;

    // Second connection: key is known, no prompt.
    let p2 = prompter(HostKeyDecision::Reject);
    let c = SshConnection::connect(&target(2222, "test_ed25519"), &kh, p2.clone()).await.unwrap();
    assert_eq!(p2.host_prompts.load(Ordering::SeqCst), 0);
    c.disconnect().await;

    // Port 2223 is a different server with a different key, but the
    // known_hosts entry is per host:port so this is "unknown", not "changed".
    let p3 = prompter(HostKeyDecision::Reject);
    let err = SshConnection::connect(&target(2223, "test_ed25519"), &kh, p3).await.err().unwrap();
    assert!(matches!(err, SshError::HostKeyRejected(_)), "{err}");

    // Forge a mismatch: claim 2222's key belongs to 2223.
    let content = std::fs::read_to_string(&kh.path).unwrap();
    std::fs::write(&kh.path, content.replace("[127.0.0.1]:2222", "[127.0.0.1]:2223")).unwrap();
    let p4 = prompter(HostKeyDecision::AcceptAndSave);
    let err = SshConnection::connect(&target(2223, "test_ed25519"), &kh, p4.clone()).await.err().unwrap();
    assert!(matches!(err, SshError::HostKeyChanged { .. }), "{err}");
    assert_eq!(p4.host_prompts.load(Ordering::SeqCst), 0, "must never offer to accept a changed key");
}

#[tokio::test]
async fn passphrase_key_and_auth_failure() {
    if !live() {
        return;
    }
    let kh = tmp_known_hosts("pass");
    let p = Arc::new(TestPrompter {
        decision: HostKeyDecision::AcceptOnce,
        host_prompts: AtomicUsize::new(0),
        passphrase: Some("hunter2".into()),
    });
    let c = SshConnection::connect(&target(2222, "test_ed25519_pass"), &kh, p).await.unwrap();
    assert!(c.exec("true").await.unwrap().success());

    let mut t = target(2222, "does-not-exist");
    t.user = "dev".into();
    let err = SshConnection::connect(&t, &kh, prompter(HostKeyDecision::AcceptOnce)).await.err().unwrap();
    assert!(matches!(err, SshError::AuthFailed(..)), "{err}");
}
#[tokio::test]
async fn jump_host() {
    if !live() {
        return;
    }
    // Jump through the 2222 container to the 2223 container's sshd, which
    // is only reachable by name on the private docker network.
    let kh = tmp_known_hosts("jump");
    let mut t = target(22, "test_ed25519");
    t.host = "muxscape-t33".into();
    t.jump = Some(Box::new(target(2222, "test_ed25519")));
    let c = SshConnection::connect(&t, &kh, prompter(HostKeyDecision::AcceptOnce)).await.unwrap();
    assert_eq!(c.exec("hostname").await.unwrap().stdout_str().trim(), "muxscape-t33");
}

/// Records what the host-key prompt was shown.
struct SeeingPrompter(std::sync::Mutex<Vec<HostKeyInfo>>, HostKeyDecision);

#[async_trait::async_trait]
impl Prompter for SeeingPrompter {
    async fn unknown_host_key(&self, info: &HostKeyInfo) -> HostKeyDecision {
        self.0.lock().unwrap().push(info.clone());
        self.1.clone()
    }
    async fn password(&self, _: &str) -> Option<String> {
        None
    }
    async fn key_passphrase(&self, _: &Path) -> Option<String> {
        None
    }
    async fn keyboard_interactive(&self, _: &str, _: &str, _: &[(String, bool)]) -> Option<Vec<String>> {
        None
    }
}

#[tokio::test]
async fn host_key_types_known_hosts() {
    if !live() {
        return;
    }
    let port = 2222;
    let scan = std::process::Command::new("ssh-keyscan").args(["-p", &port.to_string(), "127.0.0.1"]).output().unwrap();
    let scan = String::from_utf8_lossy(&scan.stdout).to_string();
    let rsa_line = scan.lines().find(|l| l.contains(" ssh-rsa ")).expect("server has an RSA key").to_string();

    // 1. Only the server's RSA key is recorded: we ask for RSA first, so a
    //    server with several keys isn't mistaken for a new host.
    let kh = tmp_known_hosts("rsa-only");
    std::fs::write(&kh.path, format!("{rsa_line}\n")).unwrap();
    let p = Arc::new(SeeingPrompter(Default::default(), HostKeyDecision::Reject));
    let c = SshConnection::connect(&target(port, "test_ed25519"), &kh, p.clone()).await.expect("connects with the known RSA key");
    c.disconnect().await;
    assert!(p.0.lock().unwrap().is_empty(), "no prompt for a known host");

    // 2. Only a key of a type the server doesn't offer is recorded (a P-521
    //    key that isn't the server's): the prompt must say so.
    let dir = std::env::temp_dir().join(format!("muxscape-fakekey-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let fake = dir.join("fake");
    assert!(std::process::Command::new("ssh-keygen").args(["-q", "-t", "ecdsa", "-b", "521", "-N", ""]).arg("-f").arg(&fake).status().unwrap().success());
    let fake_pub = std::fs::read_to_string(dir.join("fake.pub")).unwrap();
    let kh = tmp_known_hosts("other-type");
    std::fs::write(&kh.path, format!("[127.0.0.1]:{port} {}", fake_pub.split_whitespace().take(2).collect::<Vec<_>>().join(" ") + "\n")).unwrap();
    let p = Arc::new(SeeingPrompter(Default::default(), HostKeyDecision::Reject));
    let r = SshConnection::connect(&target(port, "test_ed25519"), &kh, p.clone()).await;
    assert!(r.is_err(), "rejected");
    let seen = p.0.lock().unwrap().clone();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].known_other.len(), 1, "{:?}", seen[0]);
    assert!(seen[0].known_other[0].starts_with("ecdsa-sha2-nistp521 SHA256:"), "{:?}", seen[0].known_other);

    // 3. A brand-new host: an ordinary prompt, no warning.
    let kh = tmp_known_hosts("new");
    let p = Arc::new(SeeingPrompter(Default::default(), HostKeyDecision::AcceptOnce));
    let c = SshConnection::connect(&target(port, "test_ed25519"), &kh, p.clone()).await.unwrap();
    c.disconnect().await;
    assert!(p.0.lock().unwrap()[0].known_other.is_empty());
}
