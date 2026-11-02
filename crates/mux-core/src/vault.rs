//! Encrypted vault for SSH passwords and key passphrases.
//!
//! Design (same shape as Termius' and Bitwarden's vaults):
//!
//! * The master password is stretched with **Argon2id** (64 MiB, 3 passes:
//!   above OWASP's minimum) and a random per-vault salt into a key-encryption
//!   key (KEK).
//! * A random 256-bit **vault key** encrypts every secret. The KEK only wraps
//!   the vault key, so changing the master password re-wraps 32 bytes and
//!   nothing else.
//! * Every encryption is **XChaCha20-Poly1305** (authenticated; RustCrypto,
//!   NCC-audited) with a fresh random 192-bit nonce, never reused (the
//!   MobaXterm CVE-2025-0714 mistake was a fixed IV). Each entry's slot name
//!   is bound in as associated data, so ciphertexts can't be swapped between
//!   entries, and any tampering fails decryption.
//! * The vault key lives in memory only while unlocked (zeroized on lock /
//!   drop). Secrets never leave the Rust core: the UI only sees metadata.
//! * "Remember on this device" (opt-in) also stores the vault key in the OS
//!   credential store (Windows Credential Manager / macOS Keychain), so the
//!   vault unlocks itself when the app starts. Anything running as the same
//!   OS user could read that, which the UI says.
//! * Forgetting the master password means the vault is unrecoverable, by
//!   design. It can be reset (deleted) and started again.

use base64::Engine;
use chacha20poly1305::aead::rand_core::RngCore;
use chacha20poly1305::aead::{Aead, AeadCore, KeyInit, OsRng, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use zeroize::Zeroizing;

const FORMAT: u32 = 1;
const KEY_AAD: &[u8] = b"muxscape-vault/v1/vault-key";
const CHECK_AAD: &[u8] = b"muxscape-vault/v1/check";
const CHECK_PLAIN: &[u8] = b"muxscape vault key ok";
/// Shortest accepted master password.
pub const MIN_MASTER_LEN: usize = 8;

/// Argon2id cost. Stored in the vault file, so it can be raised later
/// without breaking existing vaults.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct KdfParams {
    /// Memory in KiB.
    pub m: u32,
    /// Passes.
    pub t: u32,
    /// Lanes.
    pub p: u32,
}

impl KdfParams {
    /// Refuse costs a tampered file could use to hang or crash the app.
    fn check(&self) -> Result<()> {
        let ok = (1..=16).contains(&self.p)
            && (1..=16).contains(&self.t)
            && self.m >= 8 * self.p
            && self.m <= 1024 * 1024; // 1 GiB
        if ok { Ok(()) } else { Err(VaultError::Broken(format!("unreasonable key-derivation settings {self:?}"))) }
    }

    /// 64 MiB, 3 passes, 1 lane (OWASP minimum: 19 MiB, 2 passes).
    pub const DEFAULT: KdfParams = KdfParams { m: 64 * 1024, t: 3, p: 1 };
    /// Tests only: cheap, NOT secure.
    pub const TEST_ONLY_INSECURE: KdfParams = KdfParams { m: 64, t: 1, p: 1 };
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Sealed {
    nonce: String,
    ct: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct VaultFile {
    format: u32,
    kdf: KdfParams,
    salt: String,
    /// Vault key, encrypted with the master-password key.
    key: Sealed,
    /// Known value encrypted with the vault key (validates a remembered key).
    check: Sealed,
    /// The vault key is also in the OS credential store.
    #[serde(default)]
    remember: bool,
    /// Slot name → encrypted secret. Slot names are not secret
    /// (`password:user@host:port`, `passphrase:/path/to/key`).
    #[serde(default)]
    entries: BTreeMap<String, Sealed>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VaultError {
    NotSetUp,
    AlreadySetUp,
    Locked,
    WrongPassword,
    WeakPassword,
    /// The file exists but can't be used (unreadable, damaged, newer format).
    Broken(String),
    Io(String),
    OsStore(String),
}

impl std::fmt::Display for VaultError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VaultError::NotSetUp => write!(f, "the vault isn't set up yet"),
            VaultError::AlreadySetUp => write!(f, "a vault already exists"),
            VaultError::Locked => write!(f, "the vault is locked"),
            VaultError::WrongPassword => write!(f, "wrong master password"),
            VaultError::WeakPassword => write!(f, "the master password must be at least {MIN_MASTER_LEN} characters"),
            VaultError::Broken(e) => write!(f, "the vault file can't be read: {e}"),
            VaultError::Io(e) => write!(f, "could not save the vault: {e}"),
            VaultError::OsStore(e) => write!(f, "system credential store: {e}"),
        }
    }
}

impl std::error::Error for VaultError {}

type Result<T> = std::result::Result<T, VaultError>;

/// What the UI may know about the vault (no secrets).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct VaultStatus {
    pub exists: bool,
    pub unlocked: bool,
    pub remember: bool,
    pub remember_supported: bool,
    /// Saved slot names (metadata only).
    pub entries: Vec<String>,
    /// Set when the file exists but can't be used.
    pub error: Option<String>,
}

pub struct Vault {
    path: Option<PathBuf>,
    kdf_for_new: KdfParams,
    file: Option<VaultFile>,
    broken: Option<String>,
    key: Option<Zeroizing<[u8; 32]>>,
}

fn b64e(b: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(b)
}

fn b64d(s: &str) -> Result<Vec<u8>> {
    base64::engine::general_purpose::STANDARD.decode(s).map_err(|e| VaultError::Broken(e.to_string()))
}

fn entry_aad(slot: &str) -> Vec<u8> {
    format!("muxscape-vault/v1/entry/{slot}").into_bytes()
}

fn derive(master: &str, salt: &[u8], kdf: KdfParams) -> Result<Zeroizing<[u8; 32]>> {
    kdf.check()?;
    let params = argon2::Params::new(kdf.m, kdf.t, kdf.p, Some(32)).map_err(|e| VaultError::Broken(e.to_string()))?;
    let argon = argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
    let mut out = Zeroizing::new([0u8; 32]);
    argon.hash_password_into(master.as_bytes(), salt, out.as_mut()).map_err(|e| VaultError::Broken(e.to_string()))?;
    Ok(out)
}

fn seal(key: &[u8; 32], aad: &[u8], plain: &[u8]) -> Sealed {
    let cipher = XChaCha20Poly1305::new(key.into());
    let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
    let ct = cipher.encrypt(&nonce, Payload { msg: plain, aad }).expect("encryption cannot fail for in-memory buffers");
    Sealed { nonce: b64e(&nonce), ct: b64e(&ct) }
}

/// `None`: wrong key, wrong slot, or tampered data (indistinguishable, by design).
fn unseal(key: &[u8; 32], aad: &[u8], s: &Sealed) -> Result<Option<Zeroizing<Vec<u8>>>> {
    let nonce = b64d(&s.nonce)?;
    if nonce.len() != 24 {
        return Err(VaultError::Broken("bad nonce".into()));
    }
    let ct = b64d(&s.ct)?;
    let cipher = XChaCha20Poly1305::new(key.into());
    Ok(cipher.decrypt(XNonce::from_slice(&nonce), Payload { msg: &ct, aad }).ok().map(Zeroizing::new))
}

fn random_key() -> Zeroizing<[u8; 32]> {
    let mut k = Zeroizing::new([0u8; 32]);
    OsRng.fill_bytes(k.as_mut());
    k
}

impl Vault {
    /// `path: None` keeps the vault in memory (tests, dev server).
    pub fn open(path: Option<PathBuf>, kdf_for_new: KdfParams) -> Vault {
        let mut v = Vault { path, kdf_for_new, file: None, broken: None, key: None };
        if let Some(p) = &v.path {
            match std::fs::read(p) {
                Ok(bytes) => match serde_json::from_slice::<VaultFile>(&bytes) {
                    Ok(f) if f.format == FORMAT => match f.kdf.check() {
                        Ok(()) => v.file = Some(f),
                        Err(e) => v.broken = Some(e.to_string()),
                    },
                    Ok(f) => v.broken = Some(format!("format {} is newer than this version of Muxscape", f.format)),
                    Err(e) => v.broken = Some(e.to_string()),
                },
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => v.broken = Some(e.to_string()),
            }
        }
        v
    }

    pub fn status(&self) -> VaultStatus {
        VaultStatus {
            exists: self.file.is_some() || self.broken.is_some(),
            unlocked: self.key.is_some(),
            remember: self.file.as_ref().is_some_and(|f| f.remember),
            remember_supported: os_store::SUPPORTED,
            entries: self.file.as_ref().map(|f| f.entries.keys().cloned().collect()).unwrap_or_default(),
            error: self.broken.clone(),
        }
    }

    pub fn is_unlocked(&self) -> bool {
        self.key.is_some()
    }

    pub fn has(&self, slot: &str) -> bool {
        self.file.as_ref().is_some_and(|f| f.entries.contains_key(slot))
    }

    fn file(&self) -> Result<&VaultFile> {
        if let Some(e) = &self.broken {
            return Err(VaultError::Broken(e.clone()));
        }
        self.file.as_ref().ok_or(VaultError::NotSetUp)
    }

    fn key(&self) -> Result<&[u8; 32]> {
        self.file()?;
        self.key.as_deref().ok_or(VaultError::Locked)
    }

    fn save(&self) -> Result<()> {
        let (Some(path), Some(f)) = (&self.path, &self.file) else { return Ok(()) };
        let io = |e: std::io::Error| VaultError::Io(e.to_string());
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(io)?;
        }
        let tmp = path.with_extension("json.tmp");
        let data = serde_json::to_vec_pretty(f).map_err(|e| VaultError::Io(e.to_string()))?;
        {
            use std::io::Write;
            let mut opts = std::fs::OpenOptions::new();
            opts.write(true).create(true).truncate(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                opts.mode(0o600);
            }
            let mut fh = opts.open(&tmp).map_err(io)?;
            fh.write_all(&data).map_err(io)?;
            fh.sync_all().map_err(io)?;
        }
        std::fs::rename(&tmp, path).map_err(io)
    }

    /// Set up a new vault (unlocked afterwards).
    pub fn create(&mut self, master: &str, remember: bool) -> Result<()> {
        if self.file.is_some() || self.broken.is_some() {
            return Err(VaultError::AlreadySetUp);
        }
        if master.chars().count() < MIN_MASTER_LEN {
            return Err(VaultError::WeakPassword);
        }
        let mut salt = [0u8; 16];
        OsRng.fill_bytes(&mut salt);
        let kek = derive(master, &salt, self.kdf_for_new)?;
        let vkey = random_key();
        self.file = Some(VaultFile {
            format: FORMAT,
            kdf: self.kdf_for_new,
            salt: b64e(&salt),
            key: seal(&kek, KEY_AAD, vkey.as_ref()),
            check: seal(&vkey, CHECK_AAD, CHECK_PLAIN),
            remember: false,
            entries: BTreeMap::new(),
        });
        self.key = Some(vkey);
        if let Err(e) = self.save() {
            self.file = None;
            self.key = None;
            return Err(e);
        }
        if remember {
            self.set_remember(true)?;
        }
        Ok(())
    }

    pub fn unlock(&mut self, master: &str) -> Result<()> {
        let f = self.file()?;
        let kek = derive(master, &b64d(&f.salt)?, f.kdf)?;
        let plain = unseal(&kek, KEY_AAD, &f.key)?.ok_or(VaultError::WrongPassword)?;
        let key: [u8; 32] = plain.as_slice().try_into().map_err(|_| VaultError::Broken("bad vault key".into()))?;
        self.key = Some(Zeroizing::new(key));
        Ok(())
    }

    /// Unlock with the key remembered in the OS store. `Ok(false)` when
    /// nothing usable is remembered.
    pub fn unlock_remembered(&mut self) -> Result<bool> {
        let f = self.file()?;
        if !f.remember || !os_store::SUPPORTED {
            return Ok(false);
        }
        let Some(raw) = os_store::load(&f.salt)? else { return Ok(false) };
        let Ok(key) = <[u8; 32]>::try_from(raw.as_slice()) else { return Ok(false) };
        let key = Zeroizing::new(key);
        if unseal(&key, CHECK_AAD, &f.check)?.as_deref().map(|v| v.as_slice()) != Some(CHECK_PLAIN) {
            return Ok(false);
        }
        self.key = Some(key);
        Ok(true)
    }

    pub fn lock(&mut self) {
        self.key = None; // Zeroizing wipes it
    }

    /// New master password **and a new vault key**: every entry is
    /// re-encrypted, so an old copy of the file plus the old (perhaps
    /// leaked) master password can't open anything saved from now on.
    pub fn change_password(&mut self, old: &str, new: &str) -> Result<()> {
        if new.chars().count() < MIN_MASTER_LEN {
            return Err(VaultError::WeakPassword);
        }
        let f = self.file()?.clone();
        let kek = derive(old, &b64d(&f.salt)?, f.kdf)?;
        let old_key = unseal(&kek, KEY_AAD, &f.key)?.ok_or(VaultError::WrongPassword)?;
        let old_key: Zeroizing<[u8; 32]> =
            Zeroizing::new(old_key.as_slice().try_into().map_err(|_| VaultError::Broken("bad vault key".into()))?);
        let new_key = random_key();
        let mut entries = BTreeMap::new();
        for (slot, sealed) in &f.entries {
            let aad = entry_aad(slot);
            let plain = unseal(&old_key, &aad, sealed)?
                .ok_or_else(|| VaultError::Broken(format!("entry {slot} failed authentication")))?;
            entries.insert(slot.clone(), seal(&new_key, &aad, &plain));
        }
        let mut salt = [0u8; 16];
        OsRng.fill_bytes(&mut salt);
        let kdf = self.kdf_for_new;
        let new_kek = derive(new, &salt, kdf)?;
        let next = VaultFile {
            format: FORMAT,
            kdf,
            salt: b64e(&salt),
            key: seal(&new_kek, KEY_AAD, new_key.as_ref()),
            check: seal(&new_key, CHECK_AAD, CHECK_PLAIN),
            remember: f.remember,
            entries,
        };
        // Remembered key first (keyed by salt): if that fails nothing changed.
        if f.remember {
            os_store::store(&next.salt, &new_key)?;
        }
        let prev = self.file.replace(next);
        if let Err(e) = self.save() {
            self.file = prev;
            if f.remember {
                let _ = os_store::delete(&b64e(&salt));
            }
            return Err(e);
        }
        self.key = Some(new_key);
        if f.remember {
            os_store::delete(&f.salt)?;
        }
        Ok(())
    }

    pub fn set_remember(&mut self, on: bool) -> Result<()> {
        if on && !os_store::SUPPORTED {
            return Err(VaultError::OsStore("not available on this system yet".into()));
        }
        let salt = self.file()?.salt.clone();
        if on {
            let key = Zeroizing::new(*self.key()?);
            os_store::store(&salt, &key)?;
            self.file.as_mut().unwrap().remember = true;
            if let Err(e) = self.save() {
                // Don't leave a key the file doesn't know about.
                self.file.as_mut().unwrap().remember = false;
                let _ = os_store::delete(&salt);
                return Err(e);
            }
            Ok(())
        } else {
            os_store::delete(&salt)?;
            self.file.as_mut().unwrap().remember = false;
            self.save()
        }
    }

    pub fn get(&self, slot: &str) -> Result<Option<Zeroizing<String>>> {
        let key = self.key()?;
        let Some(s) = self.file()?.entries.get(slot) else { return Ok(None) };
        let plain = unseal(key, &entry_aad(slot), s)?
            .ok_or_else(|| VaultError::Broken(format!("entry {slot} failed authentication")))?;
        let text = String::from_utf8(plain.to_vec()).map_err(|_| VaultError::Broken("entry is not text".into()))?;
        Ok(Some(Zeroizing::new(text)))
    }

    pub fn put(&mut self, slot: &str, secret: &str) -> Result<()> {
        let key = *self.key()?;
        let key = Zeroizing::new(key);
        let sealed = seal(&key, &entry_aad(slot), secret.as_bytes());
        self.file.as_mut().unwrap().entries.insert(slot.to_string(), sealed);
        self.save()
    }

    /// Works while locked (it only deletes ciphertext).
    pub fn remove(&mut self, slot: &str) -> Result<bool> {
        self.file()?;
        let removed = self.file.as_mut().unwrap().entries.remove(slot).is_some();
        if removed {
            self.save()?;
        }
        Ok(removed)
    }

    /// Delete the vault (forgotten master password, or starting over).
    pub fn reset(&mut self) -> Result<()> {
        // Delete the file regardless, but report a key left in the OS store.
        let mut store_err = None;
        if let Some(f) = &self.file {
            if f.remember {
                store_err = os_store::delete(&f.salt).err();
            }
        }
        if let Some(p) = &self.path {
            match std::fs::remove_file(p) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(VaultError::Io(e.to_string())),
            }
        }
        self.file = None;
        self.broken = None;
        self.key = None;
        match store_err {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}

/// Slot for a server password (`user@host[:port]`, as `SshTarget::display`).
pub fn password_slot(target: &str) -> String {
    format!("password:{target}")
}

/// Slot for a private key's passphrase.
pub fn passphrase_slot(key_path: &std::path::Path) -> String {
    format!("passphrase:{}", key_path.display())
}

/// The OS credential store, for "remember on this device".
mod os_store {
    use super::{Result, VaultError};
    use zeroize::Zeroizing;

    pub const SUPPORTED: bool = cfg!(any(windows, target_os = "macos"));

    #[cfg(any(windows, target_os = "macos"))]
    fn entry(salt: &str) -> Result<keyring::Entry> {
        // One entry per vault (keyed by its salt), so test/dev vaults never
        // collide with the real one.
        keyring::Entry::new("Muxscape vault", salt).map_err(|e| VaultError::OsStore(e.to_string()))
    }

    #[cfg(any(windows, target_os = "macos"))]
    pub fn store(salt: &str, key: &[u8; 32]) -> Result<()> {
        entry(salt)?.set_secret(key).map_err(|e| VaultError::OsStore(e.to_string()))
    }

    #[cfg(any(windows, target_os = "macos"))]
    pub fn load(salt: &str) -> Result<Option<Zeroizing<Vec<u8>>>> {
        match entry(salt)?.get_secret() {
            Ok(v) => Ok(Some(Zeroizing::new(v))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(VaultError::OsStore(e.to_string())),
        }
    }

    #[cfg(any(windows, target_os = "macos"))]
    pub fn delete(salt: &str) -> Result<()> {
        match entry(salt)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(VaultError::OsStore(e.to_string())),
        }
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    pub fn store(_: &str, _: &[u8; 32]) -> Result<()> {
        Err(VaultError::OsStore("not available on this system yet".into()))
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    pub fn load(_: &str) -> Result<Option<Zeroizing<Vec<u8>>>> {
        Ok(None)
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    pub fn delete(_: &str) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FAST: KdfParams = KdfParams::TEST_ONLY_INSECURE;

    fn temp() -> (tempfile::TempDir, PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("vault.json");
        (d, p)
    }

    #[test]
    fn create_save_reopen_unlock() {
        let (_d, p) = temp();
        let mut v = Vault::open(Some(p.clone()), FAST);
        assert!(!v.status().exists);
        assert_eq!(v.create("short", false), Err(VaultError::WeakPassword));
        v.create("correct horse battery", false).unwrap();
        v.put("password:dev@example.com", "s3cret pw é").unwrap();
        v.put("passphrase:/home/me/.ssh/id_ed25519", "phrase").unwrap();
        assert_eq!(v.get("password:dev@example.com").unwrap().unwrap().as_str(), "s3cret pw é");

        // On disk: no secret, no master password, unique nonces.
        let raw = std::fs::read_to_string(&p).unwrap();
        assert!(!raw.contains("s3cret") && !raw.contains("phrase\"") && !raw.contains("correct horse"));
        let f: VaultFile = serde_json::from_str(&raw).unwrap();
        let mut nonces: Vec<&str> = f.entries.values().map(|s| s.nonce.as_str()).collect();
        nonces.extend([f.key.nonce.as_str(), f.check.nonce.as_str()]);
        let n = nonces.len();
        nonces.sort();
        nonces.dedup();
        assert_eq!(nonces.len(), n, "every encryption uses a fresh nonce");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
        }

        // A fresh process: locked until the master password is given.
        let mut v2 = Vault::open(Some(p), FAST);
        let st = v2.status();
        assert!(st.exists && !st.unlocked);
        assert_eq!(st.entries, vec!["passphrase:/home/me/.ssh/id_ed25519", "password:dev@example.com"]);
        assert_eq!(v2.get("password:dev@example.com"), Err(VaultError::Locked));
        assert_eq!(v2.unlock("wrong password!"), Err(VaultError::WrongPassword));
        assert!(!v2.is_unlocked());
        v2.unlock("correct horse battery").unwrap();
        assert_eq!(v2.get("passphrase:/home/me/.ssh/id_ed25519").unwrap().unwrap().as_str(), "phrase");
        assert_eq!(v2.get("password:nope").unwrap(), None);
        v2.lock();
        assert_eq!(v2.get("password:dev@example.com"), Err(VaultError::Locked));
    }

    #[test]
    fn same_secret_twice_encrypts_differently() {
        let mut v = Vault::open(None, FAST);
        v.create("correct horse battery", false).unwrap();
        v.put("password:a", "same").unwrap();
        v.put("password:b", "same").unwrap();
        let f = v.file.as_ref().unwrap();
        assert_ne!(f.entries["password:a"].ct, f.entries["password:b"].ct);
    }

    #[test]
    fn tampering_and_swapping_are_detected() {
        let mut v = Vault::open(None, FAST);
        v.create("correct horse battery", false).unwrap();
        v.put("password:a@x", "alpha").unwrap();
        v.put("password:b@x", "bravo").unwrap();

        // Flip one ciphertext bit.
        let mut t = Vault { path: None, kdf_for_new: FAST, file: v.file.clone(), broken: None, key: v.key.clone() };
        let e = t.file.as_mut().unwrap().entries.get_mut("password:a@x").unwrap();
        let mut ct = b64d(&e.ct).unwrap();
        ct[0] ^= 1;
        e.ct = b64e(&ct);
        assert!(matches!(t.get("password:a@x"), Err(VaultError::Broken(_))));

        // Move b's ciphertext into a's slot: authentication fails (AAD).
        let mut s = Vault { path: None, kdf_for_new: FAST, file: v.file.clone(), broken: None, key: v.key.clone() };
        let b = s.file.as_ref().unwrap().entries["password:b@x"].clone();
        s.file.as_mut().unwrap().entries.insert("password:a@x".into(), b);
        assert!(matches!(s.get("password:a@x"), Err(VaultError::Broken(_))));

        // A tampered wrapped key reads as a wrong password, never garbage.
        let mut w = Vault { path: None, kdf_for_new: FAST, file: v.file.clone(), broken: None, key: None };
        let k = &mut w.file.as_mut().unwrap().key;
        let mut ct = b64d(&k.ct).unwrap();
        ct[5] ^= 0x80;
        k.ct = b64e(&ct);
        assert_eq!(w.unlock("correct horse battery"), Err(VaultError::WrongPassword));
    }

    #[test]
    fn change_password_keeps_entries() {
        let (_d, p) = temp();
        let mut v = Vault::open(Some(p.clone()), FAST);
        v.create("first password", false).unwrap();
        v.put("password:x", "kept").unwrap();
        assert_eq!(v.change_password("not it at all", "second password"), Err(VaultError::WrongPassword));
        assert_eq!(v.change_password("first password", "short"), Err(VaultError::WeakPassword));
        v.change_password("first password", "second password").unwrap();
        let mut v2 = Vault::open(Some(p.clone()), FAST);
        assert_eq!(v2.unlock("first password"), Err(VaultError::WrongPassword));
        v2.unlock("second password").unwrap();
        assert_eq!(v2.get("password:x").unwrap().unwrap().as_str(), "kept");
    }

    #[test]
    fn change_password_rotates_the_vault_key() {
        // An old backup + the old master password must not open new data.
        let (_d, p) = temp();
        let mut v = Vault::open(Some(p.clone()), FAST);
        v.create("first password", false).unwrap();
        v.put("password:x", "before").unwrap();
        let backup = std::fs::read(&p).unwrap();
        let mut old = Vault::open(None, FAST);
        old.file = serde_json::from_slice(&backup).unwrap();
        old.unlock("first password").unwrap();
        let old_key = *old.key.as_deref().unwrap();

        v.change_password("first password", "second password").unwrap();
        v.put("password:y", "after").unwrap();
        assert_eq!(v.get("password:x").unwrap().unwrap().as_str(), "before", "entries survive");
        let now: VaultFile = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
        for (slot, s) in &now.entries {
            assert!(unseal(&old_key, &entry_aad(slot), s).unwrap().is_none(), "old key can't read {slot}");
        }
        assert!(unseal(&old_key, CHECK_AAD, &now.check).unwrap().is_none());
    }

    #[test]
    fn absurd_kdf_settings_are_refused() {
        let (_d, p) = temp();
        let mut v = Vault::open(Some(p.clone()), FAST);
        v.create("correct horse battery", false).unwrap();
        let mut f: serde_json::Value = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
        f["kdf"]["m"] = serde_json::json!(4_000_000_000u32);
        std::fs::write(&p, serde_json::to_vec(&f).unwrap()).unwrap();
        let mut v2 = Vault::open(Some(p), FAST);
        assert!(v2.status().error.unwrap().contains("unreasonable"));
        assert!(matches!(v2.unlock("correct horse battery"), Err(VaultError::Broken(_))));
    }

    #[test]
    fn remove_and_reset() {
        let (_d, p) = temp();
        let mut v = Vault::open(Some(p.clone()), FAST);
        v.create("correct horse battery", false).unwrap();
        v.put("password:x", "1").unwrap();
        v.lock();
        assert!(v.remove("password:x").unwrap(), "removing works while locked");
        assert!(!v.has("password:x"));
        v.reset().unwrap();
        assert!(!p.exists());
        assert!(!v.status().exists);
        v.create("another password", false).unwrap();
    }

    #[test]
    fn broken_file_is_never_overwritten() {
        let (_d, p) = temp();
        std::fs::write(&p, b"{ not json").unwrap();
        let mut v = Vault::open(Some(p.clone()), FAST);
        let st = v.status();
        assert!(st.exists && st.error.is_some());
        assert_eq!(v.create("correct horse battery", false), Err(VaultError::AlreadySetUp));
        assert!(matches!(v.unlock("x"), Err(VaultError::Broken(_))));
        assert_eq!(std::fs::read(&p).unwrap(), b"{ not json");
        // Only an explicit reset removes it.
        v.reset().unwrap();
        v.create("correct horse battery", false).unwrap();
    }

    #[test]
    fn default_cost_meets_owasp() {
        let d = KdfParams::DEFAULT;
        assert!(d.m >= 19 * 1024 && d.t >= 2);
        // And the stored parameters are what's used to unlock.
        let mut v = Vault::open(None, FAST);
        v.create("correct horse battery", false).unwrap();
        assert_eq!(v.file.as_ref().unwrap().kdf, FAST);
    }

    #[test]
    fn remember_reports_support() {
        let mut v = Vault::open(None, FAST);
        v.create("correct horse battery", false).unwrap();
        if !os_store::SUPPORTED {
            assert!(matches!(v.set_remember(true), Err(VaultError::OsStore(_))));
            assert!(!v.status().remember_supported);
            assert!(!v.unlock_remembered().unwrap());
        }
    }
}
