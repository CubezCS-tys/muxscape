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
