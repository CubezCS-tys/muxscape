//! SSH host management: saved hosts (organised in folders) plus hosts read
//! live from `~/.ssh/config`.
//!
//! Saved hosts live in `<config dir>/hosts.json`. Secrets are never written
//! there; see [`crate::secrets`].

use crate::ssh_config::{parse_hop, SshConfig};
use mux_ssh::SshTarget;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Host {
    pub id: String,
    pub label: String,
    /// Folder path, `/`-separated ("Work/Production"). Empty = top level.
    #[serde(default)]
    pub folder: String,
    pub hostname: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default)]
    pub user: Option<String>,
    #[serde(default)]
    pub identity_files: Vec<String>,
    #[serde(default)]
    pub identities_only: bool,
    /// Jump host: id of another host, or an OpenSSH `ProxyJump` spec.
    #[serde(default)]
    pub jump: Option<String>,
    /// Attach to tmux automatically on connect (otherwise show sessions).
    #[serde(default = "yes")]
    pub auto_tmux: bool,
    #[serde(default)]
    pub source: HostSource,
}

fn default_port() -> u16 {
    22
}
fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum HostSource {
    #[default]
    Saved,
    /// Read-only, comes from `~/.ssh/config` (alias).
    SshConfig { alias: String },
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct HostFile {
    #[serde(default)]
    hosts: Vec<Host>,
    #[serde(default)]
    folders: Vec<String>,
}

pub struct HostStore {
    path: Option<PathBuf>,
    file: HostFile,
    ssh_config: SshConfig,
    home: PathBuf,
    local_user: String,
}

pub fn local_user() -> String {
    std::env::var("USER").or_else(|_| std::env::var("USERNAME")).unwrap_or_else(|_| "root".into())
}

impl HostStore {
    /// `path = None` keeps everything in memory (tests, dev server).
    pub fn open(path: Option<PathBuf>, ssh_config: SshConfig) -> HostStore {
        let file = path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| match serde_json::from_str(&s) {
                Ok(f) => Some(f),
                Err(e) => {
                    tracing::error!("hosts.json is invalid, starting empty: {e}");
                    None
                }
            })
            .unwrap_or_default();
        HostStore {
            path,
            file,
            ssh_config,
            home: mux_ssh::home_dir().unwrap_or_default(),
            local_user: local_user(),
        }
    }

    fn save(&self) -> anyhow::Result<()> {
        let Some(p) = &self.path else { return Ok(()) };
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = p.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(&self.file)?)?;
        std::fs::rename(tmp, p)?;
        Ok(())
    }

    /// Saved hosts followed by `~/.ssh/config` hosts not already saved.
    pub fn list(&self) -> Vec<Host> {
        let mut v = self.file.hosts.clone();
        for alias in self.ssh_config.aliases() {
            if v.iter().any(|h| matches!(&h.source, HostSource::SshConfig { alias: a } if *a == alias)) {
                continue;
            }
            let r = self.ssh_config.resolve(&alias, &self.home, &self.local_user);
            v.push(Host {
                id: format!("sshconfig:{alias}"),
                label: alias.clone(),
                folder: "SSH config".into(),
                hostname: r.hostname,
                port: r.port,
                user: r.user,
                identity_files: r.identity_files,
                identities_only: r.identities_only,
                jump: r.proxy_jump,
                auto_tmux: true,
                source: HostSource::SshConfig { alias },
            });
        }
        v
    }

    pub fn folders(&self) -> Vec<String> {
        let mut f = self.file.folders.clone();
        for h in self.list() {
            if !h.folder.is_empty() && !f.contains(&h.folder) {
                f.push(h.folder);
            }
        }
        f
    }

    pub fn get(&self, id: &str) -> Option<Host> {
        self.list().into_iter().find(|h| h.id == id)
    }

    /// Insert or update. Saving an ssh-config host stores an editable copy.
    pub fn upsert(&mut self, mut host: Host) -> anyhow::Result<Host> {
        if host.id.is_empty() || host.id.starts_with("sshconfig:") {
            host.id = new_id();
            host.source = HostSource::Saved;
        }
        anyhow::ensure!(!host.hostname.trim().is_empty(), "hostname is required");
        if host.label.trim().is_empty() {
            host.label = host.hostname.clone();
        }
        match self.file.hosts.iter_mut().find(|h| h.id == host.id) {
            Some(h) => *h = host.clone(),
            None => self.file.hosts.push(host.clone()),
        }
        if !host.folder.is_empty() && !self.file.folders.contains(&host.folder) {
            self.file.folders.push(host.folder.clone());
        }
        self.save()?;
        Ok(host)
    }

    pub fn remove(&mut self, id: &str) -> anyhow::Result<()> {
        self.file.hosts.retain(|h| h.id != id);
        self.save()
    }

    pub fn add_folder(&mut self, folder: &str) -> anyhow::Result<()> {
        let f = folder.trim().trim_matches('/').to_string();
        if !f.is_empty() && !self.file.folders.contains(&f) {
            self.file.folders.push(f);
        }
        self.save()
    }

    /// Build the transport target, resolving jump hosts recursively.
    pub fn target(&self, host: &Host) -> anyhow::Result<SshTarget> {
        self.target_depth(host, 0)
    }

    fn target_depth(&self, host: &Host, depth: u32) -> anyhow::Result<SshTarget> {
        anyhow::ensure!(depth < 8, "jump host chain too long (loop?)");
        let mut t = SshTarget::new(
            host.hostname.clone(),
            host.port,
            host.user.clone().unwrap_or_else(|| self.local_user.clone()),
        );
        t.identity_files = host.identity_files.iter().map(|f| expand(f, &self.home)).collect();
        t.identities_only = host.identities_only;
        if let Some(j) = host.jump.as_deref().filter(|j| !j.is_empty()) {
            // A chain "a,b" means: reach a, then b, then the host.
            let mut prev: Option<SshTarget> = None;
            for hop in j.split(',') {
                let mut ht = self.hop_target(hop.trim(), depth)?;
                if let Some(p) = prev.take() {
                    ht.jump = Some(Box::new(p));
                }
                prev = Some(ht);
            }
            t.jump = prev.map(Box::new);
        }
        Ok(t)
    }

    fn hop_target(&self, hop: &str, depth: u32) -> anyhow::Result<SshTarget> {
        if let Some(h) = self.get(hop) {
            return self.target_depth(&h, depth + 1);
        }
        // An ssh_config alias or a literal [user@]host[:port].
        let (user, host, port) = parse_hop(hop);
        let r = self.ssh_config.resolve(&host, &self.home, &self.local_user);
        let h = Host {
            id: String::new(),
            label: host.clone(),
            folder: String::new(),
            hostname: r.hostname,
            port: port.unwrap_or(r.port),
            user: user.or(r.user),
            identity_files: r.identity_files,
            identities_only: r.identities_only,
            jump: r.proxy_jump,
            auto_tmux: false,
            source: HostSource::Saved,
        };
        self.target_depth(&h, depth + 1)
    }
}

fn expand(p: &str, home: &Path) -> PathBuf {
    match p.strip_prefix("~/") {
        Some(rest) => home.join(rest),
        None => PathBuf::from(p),
    }
}

pub fn new_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let t = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
    let c = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("h{:x}{:x}", t, c)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(label: &str, hostname: &str) -> Host {
        Host {
            id: String::new(),
            label: label.into(),
            folder: "Work/Prod".into(),
            hostname: hostname.into(),
            port: 22,
            user: Some("me".into()),
            identity_files: vec![],
            identities_only: false,
            jump: None,
            auto_tmux: true,
            source: HostSource::Saved,
        }
    }

    #[test]
    fn persist_and_merge_with_ssh_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hosts.json");
        let cfg = SshConfig::parse_str("Host cfg1\n  HostName 10.0.0.1\n  User u\n", Path::new("/h"));
        let mut s = HostStore::open(Some(path.clone()), cfg.clone());
        let a = s.upsert(host("web", "web.example.com")).unwrap();
        assert!(!a.id.is_empty());
        let list = s.list();
        assert_eq!(list.len(), 2);
        assert_eq!(list[1].id, "sshconfig:cfg1");
        assert_eq!(list[1].hostname, "10.0.0.1");
        assert_eq!(s.folders(), vec!["Work/Prod", "SSH config"]);

        // Reopen from disk.
        let s2 = HostStore::open(Some(path), cfg);
        assert_eq!(s2.get(&a.id).unwrap().label, "web");

        // Editing an ssh-config host saves a copy (new id).
        let mut s2 = s2;
        let mut c = s2.get("sshconfig:cfg1").unwrap();
        c.label = "edited".into();
        let saved = s2.upsert(c).unwrap();
        assert_ne!(saved.id, "sshconfig:cfg1");
        assert_eq!(saved.source, HostSource::Saved);
    }

    #[test]
    fn jump_chains() {
        let cfg = SshConfig::parse_str("Host bastion\n  HostName 1.2.3.4\n  User jb\n  Port 2222\n", Path::new("/h"));
        let mut s = HostStore::open(None, cfg);
        let mut inner = host("inner", "10.1.1.1");
        inner.jump = Some("bastion,ops@mid:2200".into());
        let t = s.target(&inner).unwrap();
        let mid = t.jump.as_ref().unwrap();
        assert_eq!((mid.host.as_str(), mid.port, mid.user.as_str()), ("mid", 2200, "ops"));
        let b = mid.jump.as_ref().unwrap();
        assert_eq!((b.host.as_str(), b.port, b.user.as_str()), ("1.2.3.4", 2222, "jb"));

        // Jump referencing a saved host by id, and loop detection.
        let j = s.upsert(host("j", "j.example")).unwrap();
        let mut h = host("x", "x.example");
        h.jump = Some(j.id.clone());
        assert_eq!(s.target(&h).unwrap().jump.unwrap().host, "j.example");
        let mut lp = s.get(&j.id).unwrap();
        lp.jump = Some(j.id.clone());
        s.upsert(lp).unwrap();
        assert!(s.target(&h).is_err());
    }

    #[test]
    fn validation() {
        let mut s = HostStore::open(None, SshConfig::default());
        assert!(s.upsert(host("x", " ")).is_err());
        let h = s.upsert(host("", "only.host")).unwrap();
        assert_eq!(h.label, "only.host");
    }
}
