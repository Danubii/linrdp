//! Explicitly approved certificate pins, scoped to the exact host and port.
use crate::tls::pin::Fingerprint;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs::{self, DirBuilder, File, OpenOptions},
    io::{self, Read, Write},
    net::IpAddr,
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

type Error = Box<dyn std::error::Error>;
const MAX_BYTES: u64 = 256 * 1024;
const MAX_HOSTS: usize = 1024;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    host: String,
    port: u16,
    fingerprint: String,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct KnownHosts {
    version: u8,
    hosts: Vec<Entry>,
}

pub struct Store {
    path: PathBuf,
}
impl Store {
    pub fn discover() -> Result<Self, Error> {
        let config = match std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
            Some(path) => PathBuf::from(path),
            None => {
                PathBuf::from(std::env::var_os("HOME").ok_or("HOME is not set")?).join(".config")
            }
        };
        if !config.is_absolute() {
            return Err("certificate trust configuration directory must be absolute".into());
        }
        Ok(Self {
            path: config.join("linrdp/known_hosts.json"),
        })
    }

    pub fn get(&self, host: &str, port: u16) -> Result<Option<Fingerprint>, Error> {
        let host = normalize(host, port)?;
        let saved = self.read()?;
        saved
            .hosts
            .iter()
            .find(|entry| entry.host == host && entry.port == port)
            .map(|entry| entry.fingerprint.parse().map_err(Error::from))
            .transpose()
    }

    /// Store a user's approval. A changed pin must never replace an existing one.
    pub fn remember(&self, host: &str, port: u16, pin: Fingerprint) -> Result<(), Error> {
        let host = normalize(host, port)?;
        let directory = self
            .path
            .parent()
            .ok_or("missing certificate trust directory")?;
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(directory)?;
        let directory_file = open_directory(directory)?;
        directory_file.set_permissions(fs::Permissions::from_mode(0o700))?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(directory.join(".known_hosts.lock"))?;
        private_regular_file(&lock)?;
        // File::lock is stable since Rust 1.89. Keep a separate lock inode:
        // locking the JSON file itself would lose mutual exclusion on rename.
        lock.lock()?;
        let mut saved = self.read()?;
        if let Some(entry) = saved
            .hosts
            .iter()
            .find(|entry| entry.host == host && entry.port == port)
        {
            if entry.fingerprint.parse::<Fingerprint>()? == pin {
                return Ok(());
            }
            return Err(format!("certificate for {host}:{port} differs from the remembered fingerprint; existing trust was not changed").into());
        }
        if saved.hosts.len() >= MAX_HOSTS {
            return Err("certificate trust store has reached 1024 hosts".into());
        }
        saved.hosts.push(Entry {
            host,
            port,
            fingerprint: pin.to_string(),
        });
        saved
            .hosts
            .sort_by(|a, b| (&a.host, a.port).cmp(&(&b.host, b.port)));
        let mut bytes = serde_json::to_vec_pretty(&saved)?;
        bytes.push(b'\n');
        if bytes.len() as u64 > MAX_BYTES {
            return Err("certificate trust store exceeds 256 KiB".into());
        }
        let mut temporary = tempfile::Builder::new()
            .prefix(".known-hosts-")
            .tempfile_in(directory)?;
        temporary
            .as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))?;
        temporary.write_all(&bytes)?;
        temporary.as_file().sync_all()?;
        temporary.persist(&self.path).map_err(|error| error.error)?;
        directory_file.sync_all()?;
        Ok(())
    }

    fn read(&self) -> Result<KnownHosts, Error> {
        let empty = || KnownHosts {
            version: 1,
            hosts: Vec::new(),
        };
        let directory = self
            .path
            .parent()
            .ok_or("missing certificate trust directory")?;
        match open_directory(directory) {
            Ok(file) => {
                if file.metadata()?.permissions().mode() & 0o077 != 0 {
                    return Err(
                        "certificate trust directory must have private permissions (0700)".into(),
                    );
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(empty()),
            Err(error) => return Err(error.into()),
        }
        let file = match OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&self.path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(empty()),
            Err(error) => return Err(error.into()),
        };
        private_regular_file(&file)?;
        if file.metadata()?.len() > MAX_BYTES {
            return Err("certificate trust store exceeds 256 KiB".into());
        }
        let mut bytes = Vec::new();
        file.take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err("certificate trust store exceeds 256 KiB".into());
        }
        let mut saved: KnownHosts = serde_json::from_slice(&bytes)?;
        if saved.version != 1 || saved.hosts.len() > MAX_HOSTS {
            return Err("unsupported certificate trust store version or too many hosts".into());
        }
        let mut keys = BTreeSet::new();
        for entry in &mut saved.hosts {
            entry.host = normalize(&entry.host, entry.port)?;
            entry.fingerprint = entry.fingerprint.parse::<Fingerprint>()?.to_string();
            if !keys.insert((entry.host.clone(), entry.port)) {
                return Err("duplicate host and port in certificate trust store".into());
            }
        }
        Ok(saved)
    }
}

fn normalize(host: &str, port: u16) -> Result<String, Error> {
    if port == 0 || host.is_empty() || host.len() > 253 || !host.is_ascii() || host.trim() != host {
        return Err("invalid certificate trust host or port".into());
    }
    rustls::pki_types::ServerName::try_from(host.to_owned())
        .map_err(|_| "invalid certificate trust host")?;
    Ok(match host.parse::<IpAddr>() {
        Ok(ip) => ip.to_string(),
        Err(_) => host.to_ascii_lowercase(),
    })
}
fn open_directory(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(path)
}
fn private_regular_file(file: &File) -> Result<(), Error> {
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.nlink() != 1 || metadata.permissions().mode() & 0o077 != 0 {
        return Err(
            "certificate trust files must be private regular files (0600), without links".into(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn store(directory: &tempfile::TempDir) -> Store {
        Store {
            path: directory.path().join("linrdp/known_hosts.json"),
        }
    }
    fn pin(byte: &str) -> Fingerprint {
        byte.repeat(32).parse().unwrap()
    }
    fn write(store: &Store, data: &[u8]) {
        let directory = store.path.parent().unwrap();
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(directory)
            .unwrap();
        fs::write(&store.path, data).unwrap();
        fs::set_permissions(&store.path, fs::Permissions::from_mode(0o600)).unwrap();
    }
    #[test]
    fn reload_scopes_hosts_ports_and_normalizes_dns_and_ip() {
        let directory = tempfile::tempdir().unwrap();
        let first = store(&directory);
        assert_eq!(first.get("RDP.example", 3389).unwrap(), None);
        first.remember("RDP.example", 3389, pin("aa")).unwrap();
        first
            .remember("2001:0db8:0:0:0:0:0:1", 3390, pin("bb"))
            .unwrap();
        let reloaded = store(&directory);
        assert_eq!(reloaded.get("rdp.EXAMPLE", 3389).unwrap(), Some(pin("aa")));
        assert_eq!(reloaded.get("rdp.example", 3390).unwrap(), None);
        assert_eq!(reloaded.get("other.example", 3389).unwrap(), None);
        assert_eq!(reloaded.get("rdp.example.", 3389).unwrap(), None);
        assert_eq!(reloaded.get("2001:db8::1", 3390).unwrap(), Some(pin("bb")));
        assert!(reloaded.get("[2001:db8::1]", 3390).is_err());
    }
    #[test]
    fn repeated_approval_is_idempotent_and_changed_pin_never_overwrites() {
        let directory = tempfile::tempdir().unwrap();
        let store = store(&directory);
        store.remember("rdp.example", 3389, pin("aa")).unwrap();
        let original = fs::read(&store.path).unwrap();
        store.remember("RDP.example", 3389, pin("aa")).unwrap();
        assert_eq!(fs::read(&store.path).unwrap(), original);
        assert!(store.remember("rdp.example", 3389, pin("bb")).is_err());
        assert_eq!(fs::read(&store.path).unwrap(), original);
        assert_eq!(
            fs::metadata(&store.path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(store.path.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }
    #[test]
    fn concurrent_instances_preserve_every_entry() {
        let directory = tempfile::tempdir().unwrap();
        let barrier = std::sync::Barrier::new(8);
        std::thread::scope(|scope| {
            for index in 0..8 {
                let instance = store(&directory);
                let barrier = &barrier;
                scope.spawn(move || {
                    barrier.wait();
                    instance
                        .remember(&format!("host-{index}.example"), 3389, pin("aa"))
                        .unwrap();
                });
            }
        });
        let loaded = store(&directory);
        for index in 0..8 {
            assert_eq!(
                loaded.get(&format!("host-{index}.example"), 3389).unwrap(),
                Some(pin("aa"))
            );
        }
    }
    #[test]
    fn rejects_corrupt_oversized_unknown_duplicate_and_invalid_entries() {
        let directory = tempfile::tempdir().unwrap();
        let store = store(&directory);
        for bytes in [
            b"{broken".to_vec(),
            b"{\"version\":2,\"hosts\":[]}".to_vec(),
            b"{\"version\":1,\"hosts\":[],\"extra\":true}".to_vec(),
            vec![b' '; MAX_BYTES as usize + 1],
        ] {
            write(&store, &bytes);
            assert!(store.get("host", 3389).is_err());
            assert!(store.remember("host", 3389, pin("aa")).is_err());
            assert_eq!(fs::read(&store.path).unwrap(), bytes);
        }
        let fingerprint = "aa".repeat(32);
        for hosts in [
            serde_json::json!([{"host":"host", "port":0, "fingerprint":fingerprint}]),
            serde_json::json!([{"host":"host", "port":3389, "fingerprint":"invalid"}]),
            serde_json::json!([{"host":"host", "port":3389, "fingerprint":fingerprint, "extra":true}]),
            serde_json::json!([
                {"host":"host", "port":3389, "fingerprint":fingerprint},
                {"host":"HOST", "port":3389, "fingerprint":fingerprint}
            ]),
        ] {
            write(
                &store,
                &serde_json::to_vec(&serde_json::json!({"version":1,"hosts":hosts})).unwrap(),
            );
            assert!(store.get("host", 3389).is_err());
        }
        for host in ["", " bad", "bad/path", "user@host", "☃.example"] {
            assert!(store.get(host, 3389).is_err());
        }
        assert!(store.get("host", 0).is_err());
    }
    #[test]
    fn rejects_symlinks_and_nonprivate_files() {
        let directory = tempfile::tempdir().unwrap();
        let store = store(&directory);
        store.remember("host", 3389, pin("aa")).unwrap();
        fs::set_permissions(&store.path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(store.get("host", 3389).is_err());
        assert!(store.remember("other", 3389, pin("aa")).is_err());
        let target = directory.path().join("target");
        fs::rename(&store.path, &target).unwrap();
        std::os::unix::fs::symlink(&target, &store.path).unwrap();
        assert!(store.get("host", 3389).is_err());
        assert!(store.remember("other", 3389, pin("aa")).is_err());
    }
}
