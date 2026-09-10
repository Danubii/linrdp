//! Safe, one-time import of pre-Fjern application state.
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

type Error = Box<dyn std::error::Error>;
const FILES: [&str; 2] = ["profiles.json", "known_hosts.json"];

pub fn migrate_legacy() -> Result<(), Error> {
    let base = config_base()?;
    migrate_at(&base)
}

fn config_base() -> Result<PathBuf, Error> {
    let base = match std::env::var_os("XDG_CONFIG_HOME").filter(|value| !value.is_empty()) {
        Some(path) => PathBuf::from(path),
        None => PathBuf::from(std::env::var_os("HOME").ok_or("HOME is not set")?).join(".config"),
    };
    if !base.is_absolute() {
        return Err("configuration directory must be absolute".into());
    }
    Ok(base)
}

fn migrate_at(base: &Path) -> Result<(), Error> {
    let legacy = base.join("linrdp");
    let target = base.join("fjern");
    if target.exists() || !legacy.exists() {
        return Ok(());
    }
    private_directory(&legacy)?;

    // Validate all state before creating anything visible at the target path.
    if legacy.join(FILES[0]).exists() {
        crate::profiles::Store::at(legacy.join(FILES[0])).load()?;
    }
    if legacy.join(FILES[1]).exists() {
        crate::trust_store::Store::at(legacy.join(FILES[1])).validate()?;
    }

    fs::create_dir_all(base)?;
    let staging = tempfile::Builder::new()
        .prefix(".fjern-migration-")
        .tempdir_in(base)?;
    fs::set_permissions(staging.path(), fs::Permissions::from_mode(0o700))?;
    for name in FILES {
        let source = legacy.join(name);
        if !source.exists() {
            continue;
        }
        let mut input = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&source)?;
        private_file(&input)?;
        let destination = staging.path().join(name);
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(destination)?;
        io::copy(&mut input, &mut output)?;
        output.sync_all()?;
    }
    let mut marker = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(staging.path().join(".migrated-from-linrdp"))?;
    marker.write_all(b"Imported by Fjern; the legacy directory was left untouched.\n")?;
    marker.sync_all()?;
    File::open(staging.path())?.sync_all()?;
    match fs::rename(staging.path(), &target) {
        Ok(()) => {
            let _ = staging.keep();
            File::open(base)?.sync_all()?;
            Ok(())
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn private_directory(path: &Path) -> Result<(), Error> {
    let directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(path)?;
    let metadata = directory.metadata()?;
    if metadata.permissions().mode() & 0o077 != 0 {
        return Err("legacy configuration directory must have private permissions (0700)".into());
    }
    Ok(())
}

fn private_file(file: &File) -> Result<(), Error> {
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.nlink() != 1 || metadata.permissions().mode() & 0o077 != 0 {
        return Err(
            "legacy configuration files must be private regular files (0600), without links".into(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn legacy(base: &Path) -> PathBuf {
        let path = base.join("linrdp");
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        path
    }

    fn write_private(path: &Path, bytes: &[u8]) {
        fs::write(path, bytes).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }

    #[test]
    fn no_legacy_state_is_a_noop() {
        let root = tempfile::tempdir().unwrap();
        migrate_at(root.path()).unwrap();
        assert!(!root.path().join("fjern").exists());
    }

    #[test]
    fn valid_state_is_copied_once_and_legacy_is_preserved() {
        let root = tempfile::tempdir().unwrap();
        let old = legacy(root.path());
        write_private(&old.join("profiles.json"), b"[]");
        write_private(
            &old.join("known_hosts.json"),
            b"{\"version\":1,\"hosts\":[]}",
        );
        migrate_at(root.path()).unwrap();
        let new = root.path().join("fjern");
        assert_eq!(fs::read(new.join("profiles.json")).unwrap(), b"[]");
        assert!(new.join(".migrated-from-linrdp").exists());
        assert!(old.join("profiles.json").exists());
        write_private(&old.join("profiles.json"), b"[broken");
        migrate_at(root.path()).unwrap();
        assert_eq!(fs::read(new.join("profiles.json")).unwrap(), b"[]");
    }

    #[test]
    fn an_existing_or_partial_target_wins_without_merging() {
        let root = tempfile::tempdir().unwrap();
        let old = legacy(root.path());
        write_private(&old.join("profiles.json"), b"[broken");
        fs::create_dir(root.path().join("fjern")).unwrap();
        migrate_at(root.path()).unwrap();
        assert!(!root.path().join("fjern/profiles.json").exists());
    }

    #[test]
    fn corrupt_or_unsafe_legacy_state_never_creates_target() {
        let root = tempfile::tempdir().unwrap();
        let old = legacy(root.path());
        write_private(&old.join("profiles.json"), b"[broken");
        assert!(migrate_at(root.path()).is_err());
        assert!(!root.path().join("fjern").exists());

        fs::set_permissions(&old, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(migrate_at(root.path()).is_err());
        assert!(!root.path().join("fjern").exists());
    }

    #[test]
    fn symlinked_files_are_rejected() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let old = legacy(root.path());
        let outside = root.path().join("profiles.json");
        write_private(&outside, b"[]");
        symlink(outside, old.join("profiles.json")).unwrap();
        assert!(migrate_at(root.path()).is_err());
        assert!(!root.path().join("fjern").exists());
    }
}
