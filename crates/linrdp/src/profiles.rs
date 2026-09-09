//! Non-secret saved connection profiles.
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::PathBuf,
};

const MAX_FILE: u64 = 256 * 1024;
const MAX_PROFILES: usize = 100;
const MAX_TEXT: usize = 1024;

type Error = Box<dyn std::error::Error>;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub name: String,
    pub computer: String,
    pub user: String,
    pub port: u16,
    pub size: Option<String>,
    pub dynamic_resolution: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub h264: bool,
    pub clipboard: bool,
    pub ca: Option<String>,
    pub fingerprint: Option<String>,
}

fn is_false(value: &bool) -> bool {
    !*value
}

impl Profile {
    pub fn arguments(&self) -> Vec<String> {
        let mut args = vec!["connect".into(), self.computer.clone()];
        if self.port != 3389 {
            args.push(self.port.to_string());
        }
        args.extend(["--user".into(), self.user.clone()]);
        if let Some(size) = &self.size {
            args.extend(["--size".into(), size.clone()]);
        }
        args.extend([
            "--dynamic-resolution".into(),
            if self.dynamic_resolution { "on" } else { "off" }.into(),
            "--clipboard".into(),
            if self.clipboard { "on" } else { "off" }.into(),
        ]);
        if self.h264 {
            args.extend(["--graphics".into(), "h264".into()]);
        }
        if let Some(ca) = &self.ca {
            args.extend(["--ca".into(), ca.clone()]);
        } else if let Some(fingerprint) = &self.fingerprint {
            args.extend(["--cert-sha256".into(), fingerprint.clone()]);
        }
        args
    }

    pub fn validate(&self) -> Result<(), Error> {
        for (label, value, allow_empty) in [
            ("profile name", self.name.as_str(), false),
            ("computer", self.computer.as_str(), false),
            ("user", self.user.as_str(), false),
            ("size", self.size.as_deref().unwrap_or(""), true),
            ("CA path", self.ca.as_deref().unwrap_or(""), true),
            (
                "certificate fingerprint",
                self.fingerprint.as_deref().unwrap_or(""),
                true,
            ),
        ] {
            if (!allow_empty && value.trim().is_empty())
                || value.len() > MAX_TEXT
                || value.chars().any(char::is_control)
            {
                return Err(format!("invalid {label}").into());
            }
        }
        if self.ca.is_some() && self.fingerprint.is_some() {
            return Err("choose either a CA file or a certificate fingerprint".into());
        }
        crate::Options::parse(&self.arguments())?;
        Ok(())
    }
}

pub struct Store {
    path: PathBuf,
}

impl Store {
    pub fn discover() -> Result<Self, Error> {
        let base = match std::env::var_os("XDG_CONFIG_HOME") {
            Some(path) if !path.is_empty() => PathBuf::from(path),
            _ => PathBuf::from(std::env::var_os("HOME").ok_or("HOME is not set")?).join(".config"),
        };
        if !base.is_absolute() {
            return Err("XDG_CONFIG_HOME must be an absolute path".into());
        }
        Ok(Self::at(base.join("linrdp").join("profiles.json")))
    }

    fn at(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn load(&self) -> Result<Vec<Profile>, Error> {
        let file = match OpenOptions::new().read(true).open(&self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        if file.metadata()?.len() > MAX_FILE {
            return Err("saved profile file exceeds 256 KiB".into());
        }
        let mut bytes = Vec::new();
        file.take(MAX_FILE + 1).read_to_end(&mut bytes)?;
        let profiles: Vec<Profile> = serde_json::from_slice(&bytes)?;
        validate_all(&profiles)?;
        Ok(profiles)
    }

    pub fn save(&self, profiles: &[Profile]) -> Result<(), Error> {
        validate_all(profiles)?;
        let directory = self.path.parent().ok_or("invalid profile path")?;
        fs::create_dir_all(directory)?;
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
        let bytes = serde_json::to_vec_pretty(profiles)?;
        if bytes.len() as u64 > MAX_FILE {
            return Err("saved profile file exceeds 256 KiB".into());
        }
        let mut temporary = tempfile::Builder::new()
            .prefix(".profiles-")
            .tempfile_in(directory)?;
        temporary
            .as_file_mut()
            .set_permissions(fs::Permissions::from_mode(0o600))?;
        temporary.write_all(&bytes)?;
        temporary.as_file_mut().sync_all()?;
        temporary.persist(&self.path).map_err(|error| error.error)?;
        fs::set_permissions(&self.path, fs::Permissions::from_mode(0o600))?;
        if let Ok(directory) = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY)
            .open(directory)
        {
            directory.sync_all()?;
        }
        Ok(())
    }
}

fn validate_all(profiles: &[Profile]) -> Result<(), Error> {
    if profiles.len() > MAX_PROFILES {
        return Err("at most 100 saved profiles are supported".into());
    }
    for profile in profiles {
        profile.validate()?;
    }
    for (index, profile) in profiles.iter().enumerate() {
        if profiles[..index]
            .iter()
            .any(|saved| saved.name == profile.name)
        {
            return Err(format!("duplicate profile name: {}", profile.name).into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(name: &str) -> Profile {
        Profile {
            name: name.into(),
            computer: "rdp.example".into(),
            user: "LAB\\tester".into(),
            port: 3390,
            size: Some("1280x800".into()),
            dynamic_resolution: true,
            h264: false,
            clipboard: false,
            ca: Some("/tmp/lab.pem".into()),
            fingerprint: None,
        }
    }

    #[test]
    fn profile_arguments_reuse_cli_validation() {
        let saved = profile("Lab");
        let parsed = crate::Options::parse(&saved.arguments()).unwrap();
        assert_eq!(parsed.host, "rdp.example");
        assert_eq!(parsed.port, 3390);
        assert_eq!(parsed.size, Some((1280, 800)));
        assert!(parsed.dynamic_resolution);
        assert!(!parsed.clipboard);

        let mut invalid = saved.clone();
        invalid.size = Some("9000x800".into());
        assert!(invalid.validate().is_err());
        invalid = saved;
        invalid.fingerprint = Some("00".repeat(32));
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn store_roundtrips_atomically_with_private_permissions() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::at(directory.path().join("linrdp/profiles.json"));
        store.save(&[profile("Lab")]).unwrap();
        assert_eq!(store.load().unwrap(), [profile("Lab")]);
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
    fn store_rejects_secrets_duplicates_and_unbounded_input() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::at(directory.path().join("profiles.json"));
        fs::write(
            &store.path,
            r#"[{"name":"x","computer":"h","user":"u","port":3389,"size":null,"dynamic_resolution":true,"clipboard":true,"ca":null,"fingerprint":null,"password":"secret"}]"#,
        )
        .unwrap();
        assert!(store.load().is_err());
        assert!(store.save(&[profile("same"), profile("same")]).is_err());
        let mut long = profile("long");
        long.computer = "x".repeat(MAX_TEXT + 1);
        assert!(long.validate().is_err());
    }
}
