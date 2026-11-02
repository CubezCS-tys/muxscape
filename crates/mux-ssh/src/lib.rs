//! SSH connection lifecycle and transport.
//!
//! One [`SshConnection`] is one authenticated SSH transport to one server.
//! Everything Muxscape does on a server is multiplexed over it as channels:
//! short `exec` queries (tmux discovery), long-lived control-mode channels
//! (one per attached tmux session) and plain PTY shells.
//!
//! Interaction that needs a human (unknown host keys, passwords, key
//! passphrases, keyboard-interactive prompts) goes through the [`Prompter`]
//! trait so the UI layer decides how to ask.

use russh::client::{self, AuthResult, Handle, KeyboardInteractiveAuthResponse};
use russh::keys::agent::client::AgentClient;
use russh::keys::{HashAlg, PrivateKeyWithHashAlg, PublicKey, PublicKeyOrCertificate};
use russh::Disconnect;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub use russh::{ChannelMsg, ChannelReadHalf, ChannelStream, ChannelWriteHalf};
pub type Msg = client::Msg;
pub type Channel = russh::Channel<client::Msg>;

/// Where and how to connect. Built by the host-management layer from saved
/// hosts and/or `~/.ssh/config`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SshTarget {
    pub host: String,
    pub port: u16,
    pub user: String,
    /// Explicit identity files, tried before agent and default keys.
    #[serde(default)]
    pub identity_files: Vec<PathBuf>,
    /// Only use `identity_files` (OpenSSH `IdentitiesOnly`).
    #[serde(default)]
    pub identities_only: bool,
    #[serde(default = "yes")]
    pub use_agent: bool,
    /// Hop through this host first (OpenSSH `ProxyJump`).
    #[serde(default)]
    pub jump: Option<Box<SshTarget>>,
    #[serde(default)]
    pub keepalive_secs: Option<u64>,
}

fn yes() -> bool {
    true
}

impl SshTarget {
    pub fn new(host: impl Into<String>, port: u16, user: impl Into<String>) -> Self {
        SshTarget {
            host: host.into(),
            port,
            user: user.into(),
            identity_files: vec![],
            identities_only: false,
            use_agent: true,
            jump: None,
            keepalive_secs: None,
        }
    }

    /// `user@host:port` for display and logs.
    pub fn display(&self) -> String {
        if self.port == 22 {
            format!("{}@{}", self.user, self.host)
        } else {
            format!("{}@{}:{}", self.user, self.host, self.port)
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HostKeyInfo {
    pub host: String,
    pub port: u16,
    pub algorithm: String,
    /// `SHA256:...` as printed by OpenSSH.
    pub fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostKeyDecision {
    /// Trust and record in known_hosts.
    AcceptAndSave,
    /// Trust for this connection only.
    AcceptOnce,
    Reject,
}

impl HostKeyDecision {
    pub fn from_accept(save: bool) -> Self {
        if save {
            HostKeyDecision::AcceptAndSave
        } else {
            HostKeyDecision::AcceptOnce
        }
    }
}

/// Asks the human. Every method may return `None`/`Reject` to cancel.
#[async_trait::async_trait]
pub trait Prompter: Send + Sync {
    async fn unknown_host_key(&self, info: &HostKeyInfo) -> HostKeyDecision;
    async fn password(&self, target: &str) -> Option<String>;
    async fn key_passphrase(&self, key_path: &Path) -> Option<String>;
    async fn keyboard_interactive(
        &self,
        target: &str,
        instructions: &str,
        prompts: &[(String, bool)],
    ) -> Option<Vec<String>>;
}

#[derive(Debug, thiserror::Error)]
pub enum SshError {
    #[error("could not connect to {target}: {source}")]
    Connect { target: String, source: std::io::Error },
    #[error(
        "HOST KEY MISMATCH for {}:{} — the server presented {} {} which differs from known_hosts line {line}. \
         This could be a man-in-the-middle attack. Connection refused.",
        .info.host, .info.port, .info.algorithm, .info.fingerprint
    )]
    HostKeyChanged { info: HostKeyInfo, line: usize },
    #[error("host key for {}:{} was not accepted", .0.host, .0.port)]
    HostKeyRejected(HostKeyInfo),
    #[error("authentication failed for {0} (tried: {1})")]
    AuthFailed(String, String),
    #[error("ssh: {0}")]
    Russh(#[from] russh::Error),
    #[error("{0}")]
    Other(String),
}

pub type Result<T, E = SshError> = std::result::Result<T, E>;

/// Known-hosts file location. Defaults to the user's OpenSSH file so keys
/// accepted in Muxscape and in `ssh` are shared.
#[derive(Debug, Clone)]
pub struct KnownHosts {
    pub path: PathBuf,
}

impl KnownHosts {
    pub fn openssh_default() -> Option<Self> {
        home_dir().map(|h| KnownHosts { path: h.join(".ssh").join("known_hosts") })
    }
}

pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

struct ClientHandler {
    host: String,
    port: u16,
    known_hosts: KnownHosts,
    prompter: Arc<dyn Prompter>,
    /// Set when we reject a key so connect() can report *why*.
    failure: Arc<Mutex<Option<SshError>>>,
}

fn key_info(host: &str, port: u16, key: &PublicKey) -> HostKeyInfo {
    HostKeyInfo {
        host: host.to_string(),
        port,
        algorithm: key.algorithm().to_string(),
        fingerprint: key.fingerprint(HashAlg::Sha256).to_string(),
    }
}

impl client::Handler for ClientHandler {
    type Error = SshError;

    async fn check_server_key(&mut self, key: &PublicKeyOrCertificate) -> Result<bool, SshError> {
        let PublicKeyOrCertificate::PublicKey { key, .. } = key else {
            // Host certificates need a CA trust store; not supported yet.
            *self.failure.lock().unwrap() =
                Some(SshError::Other("host certificates are not supported yet".into()));
            return Ok(false);
        };
        let info = key_info(&self.host, self.port, key);
        match russh::keys::check_known_hosts_path(&self.host, self.port, key, &self.known_hosts.path) {
            Ok(true) => return Ok(true),
            Ok(false) => {}
            Err(russh::keys::Error::KeyChanged { line }) => {
                *self.failure.lock().unwrap() = Some(SshError::HostKeyChanged { info, line });
                return Ok(false);
            }
            // Missing/unreadable file: treat as unknown host.
            Err(e) => tracing::debug!("known_hosts: {e}"),
        }
        match self.prompter.unknown_host_key(&info).await {
            HostKeyDecision::AcceptAndSave => {
                if let Err(e) = russh::keys::known_hosts::learn_known_hosts_path(
                    &self.host,
                    self.port,
                    key,
                    &self.known_hosts.path,
                ) {
                    tracing::warn!("could not save host key: {e}");
                }
                Ok(true)
            }
            HostKeyDecision::AcceptOnce => Ok(true),
            HostKeyDecision::Reject => {
                *self.failure.lock().unwrap() = Some(SshError::HostKeyRejected(info));
                Ok(false)
            }
        }
    }
}

/// Result of a one-shot remote command.
#[derive(Debug, Clone)]
pub struct ExecOutput {
    pub status: Option<u32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl ExecOutput {
    pub fn stdout_str(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }
    pub fn stderr_str(&self) -> String {
        String::from_utf8_lossy(&self.stderr).into_owned()
    }
    pub fn success(&self) -> bool {
        self.status == Some(0)
    }
}

pub struct SshConnection {
    handle: Handle<ClientHandler>,
    target: SshTarget,
    /// Keeps the jump host's transport alive for as long as we use it.
    _jump: Option<Box<SshConnection>>,
}

impl SshConnection {
    pub async fn connect(
        target: &SshTarget,
        known_hosts: &KnownHosts,
        prompter: Arc<dyn Prompter>,
    ) -> Result<SshConnection> {
        let keepalive = Duration::from_secs(target.keepalive_secs.unwrap_or(15));
        let config = Arc::new(client::Config {
            keepalive_interval: Some(keepalive),
            keepalive_max: 3,
            inactivity_timeout: None,
            nodelay: true,
            ..Default::default()
        });
        let failure = Arc::new(Mutex::new(None));
        let handler = ClientHandler {
            host: target.host.clone(),
            port: target.port,
            known_hosts: known_hosts.clone(),
            prompter: prompter.clone(),
            failure: failure.clone(),
        };

        let (handle, jump) = match &target.jump {
            None => {
                let addr = (target.host.as_str(), target.port);
                let tcp = tokio::time::timeout(Duration::from_secs(15), tokio::net::TcpStream::connect(addr))
                    .await
                    .map_err(|_| SshError::Connect {
                        target: target.display(),
                        source: std::io::Error::new(std::io::ErrorKind::TimedOut, "timed out"),
                    })?
                    .map_err(|e| SshError::Connect { target: target.display(), source: e })?;
                let _ = tcp.set_nodelay(true);
                (client::connect_stream(config, tcp, handler).await, None)
            }
            Some(jump_target) => {
                let jump = Box::pin(SshConnection::connect(jump_target, known_hosts, prompter.clone())).await?;
                let ch = jump
                    .handle
                    .channel_open_direct_tcpip(target.host.clone(), target.port as u32, "127.0.0.1", 0)
                    .await?;
                (client::connect_stream(config, ch.into_stream(), handler).await, Some(Box::new(jump)))
            }
        };
        let mut handle = match handle {
            Ok(h) => h,
            Err(e) => {
                // Prefer the specific reason recorded by the handler.
                return Err(failure.lock().unwrap().take().unwrap_or(e));
            }
        };
        authenticate(&mut handle, target, prompter.as_ref()).await?;
        Ok(SshConnection { handle, target: target.clone(), _jump: jump })
    }

    pub fn target(&self) -> &SshTarget {
        &self.target
    }

    pub fn is_closed(&self) -> bool {
        self.handle.is_closed()
    }

    /// Run a command and collect its output.
    pub async fn exec(&self, command: &str) -> Result<ExecOutput> {
        let mut ch = self.handle.channel_open_session().await?;
        ch.exec(true, command).await?;
        let mut out = ExecOutput { status: None, stdout: vec![], stderr: vec![] };
        while let Some(msg) = ch.wait().await {
            match msg {
                ChannelMsg::Data { data } => out.stdout.extend_from_slice(&data),
                ChannelMsg::ExtendedData { data, ext: 1 } => out.stderr.extend_from_slice(&data),
                ChannelMsg::ExitStatus { exit_status } => out.status = Some(exit_status),
                ChannelMsg::Close => break,
                _ => {}
            }
        }
        Ok(out)
    }

    /// Start a command and return its stdin/stdout as a byte stream (no PTY).
    /// Used for tmux control mode.
    pub async fn exec_stream(&self, command: &str) -> Result<ChannelStream<client::Msg>> {
        let ch = self.handle.channel_open_session().await?;
        ch.exec(true, command).await?;
        Ok(ch.into_stream())
    }

    /// Interactive login shell with a PTY (servers without tmux, or when the
    /// user asks for a plain terminal).
    pub async fn open_shell(&self, cols: u32, rows: u32) -> Result<Channel> {
        let ch = self.handle.channel_open_session().await?;
        ch.request_pty(true, "xterm-256color", cols, rows, 0, 0, &[]).await?;
        ch.request_shell(true).await?;
        Ok(ch)
    }

    pub async fn disconnect(&self) {
        let _ = self.handle.disconnect(Disconnect::ByApplication, "bye", "en").await;
    }
}

fn default_identity_files() -> Vec<PathBuf> {
    let Some(home) = home_dir() else { return vec![] };
    ["id_ed25519", "id_ecdsa", "id_rsa"].iter().map(|n| home.join(".ssh").join(n)).collect()
}

async fn authenticate(handle: &mut Handle<ClientHandler>, target: &SshTarget, prompter: &dyn Prompter) -> Result<()> {
    let user = target.user.clone();
    let mut tried: Vec<String> = vec![];
    let rsa_hash = handle.best_supported_rsa_hash().await.ok().flatten().flatten();

    // 1. Explicit identity files.
    let mut files = target.identity_files.clone();
    if !target.identities_only {
        for f in default_identity_files() {
            if !files.contains(&f) {
                files.push(f);
            }
        }
    }
    let (explicit, defaults) = files.split_at(target.identity_files.len());

    let try_files = |list: &[PathBuf]| list.iter().filter(|p| p.exists()).cloned().collect::<Vec<_>>();

    for path in try_files(explicit) {
        if try_key_file(handle, &user, &path, rsa_hash, prompter, &mut tried).await? {
            return Ok(());
        }
    }

    // 2. Agent.
    if target.use_agent && !target.identities_only {
        if let Some(ok) = try_agent(handle, &user, rsa_hash, &mut tried).await {
            if ok {
                return Ok(());
            }
        }
    }

    // 3. Default key files.
    for path in try_files(defaults) {
        if try_key_file(handle, &user, &path, rsa_hash, prompter, &mut tried).await? {
            return Ok(());
        }
    }

    // 4. Keyboard-interactive, then password.
    tried.push("keyboard-interactive".into());
    let mut resp = handle.authenticate_keyboard_interactive_start(user.clone(), None::<String>).await?;
    let mut rounds = 0;
    loop {
        match resp {
            KeyboardInteractiveAuthResponse::Success => return Ok(()),
            KeyboardInteractiveAuthResponse::Failure { .. } => break,
            KeyboardInteractiveAuthResponse::InfoRequest { instructions, prompts, .. } => {
                rounds += 1;
                if rounds > 5 {
                    break;
                }
                let p: Vec<(String, bool)> = prompts.iter().map(|p| (p.prompt.clone(), p.echo)).collect();
                let answers = if p.is_empty() {
                    vec![]
                } else {
                    match prompter.keyboard_interactive(&target.display(), &instructions, &p).await {
                        Some(a) => a,
                        None => break,
                    }
                };
                resp = handle.authenticate_keyboard_interactive_respond(answers).await?;
            }
        }
    }

    tried.push("password".into());
    for _ in 0..3 {
        let Some(pw) = prompter.password(&target.display()).await else { break };
        if handle.authenticate_password(user.clone(), pw).await?.success() {
            return Ok(());
        }
    }
    Err(SshError::AuthFailed(target.display(), tried.join(", ")))
}

async fn try_key_file(
    handle: &mut Handle<ClientHandler>,
    user: &str,
    path: &Path,
    rsa_hash: Option<HashAlg>,
    prompter: &dyn Prompter,
    tried: &mut Vec<String>,
) -> Result<bool> {
    tried.push(path.display().to_string());
    let key = match russh::keys::load_secret_key(path, None) {
        Ok(k) => k,
        Err(russh::keys::Error::KeyIsEncrypted) => {
            let mut loaded = None;
            for _ in 0..3 {
                let Some(pass) = prompter.key_passphrase(path).await else { break };
                match russh::keys::load_secret_key(path, Some(&pass)) {
                    Ok(k) => {
                        loaded = Some(k);
                        break;
                    }
                    Err(e) => tracing::info!("passphrase for {}: {e}", path.display()),
                }
            }
            match loaded {
                Some(k) => k,
                None => return Ok(false),
            }
        }
        Err(e) => {
            tracing::info!("skipping key {}: {e}", path.display());
            return Ok(false);
        }
    };
    let res = handle
        .authenticate_publickey(user, PrivateKeyWithHashAlg::new(Arc::new(key), rsa_hash))
        .await?;
    Ok(res.success())
}

#[cfg(unix)]
async fn try_agent(
    handle: &mut Handle<ClientHandler>,
    user: &str,
    rsa_hash: Option<HashAlg>,
    tried: &mut Vec<String>,
) -> Option<bool> {
    let mut agent = AgentClient::connect_env().await.ok()?;
    agent_auth(handle, user, rsa_hash, &mut agent, tried).await
}

#[cfg(windows)]
async fn try_agent(
    handle: &mut Handle<ClientHandler>,
    user: &str,
    rsa_hash: Option<HashAlg>,
    tried: &mut Vec<String>,
) -> Option<bool> {
    // Windows OpenSSH agent first, then Pageant.
    if let Ok(mut agent) = AgentClient::connect_named_pipe(r"\\.\pipe\openssh-ssh-agent").await {
        if let Some(true) = agent_auth(handle, user, rsa_hash, &mut agent, tried).await {
            return Some(true);
        }
    }
    let mut agent = AgentClient::connect_pageant().await.ok()?;
    agent_auth(handle, user, rsa_hash, &mut agent, tried).await
}

async fn agent_auth<S>(
    handle: &mut Handle<ClientHandler>,
    user: &str,
    rsa_hash: Option<HashAlg>,
    agent: &mut AgentClient<S>,
    tried: &mut Vec<String>,
) -> Option<bool>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Unpin + 'static,
{
    let ids = agent.request_identities().await.ok()?;
    for id in ids {
        let russh::keys::agent::AgentIdentity::PublicKey { key, comment } = id else { continue };
        tried.push(format!("agent:{comment}"));
        let hash = if key.algorithm().is_rsa() { rsa_hash } else { None };
        match handle.authenticate_publickey_with(user, key, hash, agent).await {
            Ok(AuthResult::Success) => return Some(true),
            Ok(_) => {}
            Err(e) => tracing::info!("agent auth: {e}"),
        }
    }
    Some(false)
}
