use super::Result;
use linrdp_proto::clipboard::{self as wire, FileDescriptor, FileRequest};
use std::{
    collections::BTreeSet,
    fs::{File, OpenOptions},
    os::unix::fs::{FileExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::Arc,
};
pub struct Inventory {
    pub descriptors: Vec<FileDescriptor>,
    files: Vec<Option<File>>,
}
impl Inventory {
    pub fn collect(paths: &[PathBuf]) -> Result<Self> {
        if paths.is_empty() || paths.len() > wire::MAX_FILES {
            return Err("clipboard file count exceeds limit".into());
        }
        let mut out = Self {
            descriptors: Vec::new(),
            files: Vec::new(),
        };
        for path in paths {
            let name = path
                .file_name()
                .and_then(|s| s.to_str())
                .ok_or("clipboard file name is not UTF-8")?
                .to_owned();
            wire::safe_name(&name)?;
            let metadata = std::fs::symlink_metadata(path)?;
            if metadata.file_type().is_symlink() {
                return Err("copying symbolic links is not supported".into());
            }
            if metadata.is_dir() {
                let handle = OpenOptions::new()
                    .read(true)
                    .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_NONBLOCK)
                    .open(path)?;
                let dir = cap_std::fs::Dir::from_std_file(handle);
                out.push(name.clone(), None)?;
                out.walk(&dir, Path::new(""), &name, 0)?;
            } else if metadata.is_file() {
                let file = OpenOptions::new()
                    .read(true)
                    .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
                    .open(path)?;
                out.push(name, Some(file))?;
            } else {
                return Err("only regular files and directories can be copied".into());
            }
        }
        let descriptors = wire::encode_files(&out.descriptors)?;
        wire::decode_files(&descriptors)?; // duplicate names and aggregate size budget
        Ok(out)
    }
    fn walk(
        &mut self,
        root: &cap_std::fs::Dir,
        rel: &Path,
        name: &str,
        depth: usize,
    ) -> Result<()> {
        if depth >= 32 {
            return Err("clipboard directory nesting exceeds limit".into());
        }
        for entry in root.read_dir(if rel.as_os_str().is_empty() {
            Path::new(".")
        } else {
            rel
        })? {
            let entry = entry?;
            let ty = entry.file_type()?;
            let leaf = entry
                .file_name()
                .into_string()
                .map_err(|_| "clipboard file name is not UTF-8")?;
            let path = rel.join(&leaf);
            let name = format!("{name}\\{leaf}");
            wire::safe_name(&name)?;
            if ty.is_dir() {
                self.push(name.clone(), None)?;
                self.walk(root, &path, &name, depth + 1)?;
            } else if ty.is_file() {
                use cap_std::fs::OpenOptionsExt;
                let mut options = cap_std::fs::OpenOptions::new();
                options
                    .read(true)
                    .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
                self.push(name, Some(root.open_with(&path, &options)?.into_std()))?;
            } else {
                return Err("symbolic links and special files cannot be copied".into());
            }
        }
        Ok(())
    }
    fn push(&mut self, name: String, file: Option<File>) -> Result<()> {
        if self.files.len() >= wire::MAX_FILES {
            return Err("clipboard file count exceeds limit".into());
        }
        let size = if let Some(f) = &file {
            let m = f.metadata()?;
            if !m.is_file() {
                return Err("clipboard source changed type".into());
            }
            m.len()
        } else {
            0
        };
        self.descriptors.push(FileDescriptor {
            name,
            size,
            directory: file.is_none(),
        });
        self.files.push(file);
        Ok(())
    }
    pub fn read(&self, r: FileRequest) -> Result<Vec<u8>> {
        let f = self
            .files
            .get(r.index as usize)
            .and_then(Option::as_ref)
            .ok_or("invalid clipboard file index")?;
        let original = &self.descriptors[r.index as usize];
        if f.metadata()?.len() != original.size {
            return Err("clipboard file changed; copy it again".into());
        }
        if r.size_only {
            if r.offset != 0 || r.count != 8 {
                return Err("invalid file size request".into());
            }
            return Ok(original.size.to_le_bytes().to_vec());
        }
        if r.count > 1024 * 1024 || r.offset > original.size {
            return Err("clipboard read range exceeds limit".into());
        }
        let size = (u64::from(r.count)).min(original.size - r.offset) as usize;
        let mut out = vec![0; size];
        let mut done = 0;
        while done < size {
            let n = f.read_at(&mut out[done..], r.offset + done as u64)?;
            if n == 0 {
                return Err("clipboard source truncated during copy".into());
            }
            done += n;
        }
        Ok(out)
    }
}
pub struct Download {
    pub dir: Arc<tempfile::TempDir>,
    pub files: Vec<FileDescriptor>,
    pub roots: Vec<PathBuf>,
    pub index: usize,
    pub offset: u64,
    file: Option<File>,
}
impl Download {
    pub fn new(files: Vec<FileDescriptor>) -> Result<Self> {
        let dir = Arc::new(
            tempfile::Builder::new()
                .prefix("linrdp-clipboard-")
                .tempdir()?,
        );
        let mut roots = BTreeSet::new();
        // Validate all path relationships before any transfer starts.
        let mut types = std::collections::BTreeMap::new();
        for f in &files {
            let parts = wire::safe_name(&f.name)?;
            let mut prefix = String::new();
            for (index, part) in parts.iter().enumerate() {
                if !prefix.is_empty() {
                    prefix.push('\\');
                }
                prefix.push_str(part);
                let directory = index + 1 < parts.len() || f.directory;
                let value = (prefix.clone(), directory);
                if let Some(previous) = types.insert(prefix.to_lowercase(), value.clone())
                    && previous != value
                {
                    return Err("clipboard paths have conflicting types or capitalization".into());
                }
            }
            roots.insert(parts[0].to_owned());
        }
        for f in &files {
            let p = join(dir.path(), &f.name)?;
            if f.directory {
                std::fs::create_dir_all(&p)?;
            } else {
                if let Some(parent) = p.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                OpenOptions::new().write(true).create_new(true).open(&p)?;
            }
        }
        let roots = roots.into_iter().map(|p| dir.path().join(p)).collect();
        Ok(Self {
            dir,
            files,
            roots,
            index: 0,
            offset: 0,
            file: None,
        })
    }
    pub fn next(&mut self, stream: u32) -> Result<Option<Vec<u8>>> {
        while let Some(f) = self.files.get(self.index) {
            if f.directory || self.offset == f.size {
                self.file = None;
                self.index += 1;
                self.offset = 0;
                continue;
            }
            if self.file.is_none() {
                self.file = Some(
                    OpenOptions::new()
                        .write(true)
                        .open(join(self.dir.path(), &f.name)?)?,
                );
            }
            return Ok(Some(wire::file_request(FileRequest {
                stream,
                index: self.index as u32,
                size_only: false,
                offset: self.offset,
                count: (f.size - self.offset).min(65536) as u32,
            })));
        }
        Ok(None)
    }
    pub fn append(&mut self, b: &[u8]) -> Result<()> {
        let f = self
            .files
            .get(self.index)
            .ok_or("unexpected clipboard file data")?;
        if b.is_empty() || b.len() > 65536 || b.len() as u64 > f.size - self.offset {
            return Err("invalid clipboard file response size".into());
        }
        use std::io::{Seek, SeekFrom, Write};
        let file = self.file.as_mut().ok_or("clipboard file is not open")?;
        file.seek(SeekFrom::Start(self.offset))?;
        file.write_all(b)?;
        self.offset += b.len() as u64;
        Ok(())
    }
}
fn join(root: &Path, name: &str) -> Result<PathBuf> {
    let mut out = root.to_owned();
    for p in wire::safe_name(name)? {
        out.push(p);
    }
    Ok(out)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn directories_and_binary_ranges_roundtrip() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir(temp.path().join("folder")).unwrap();
        std::fs::write(temp.path().join("folder/data.bin"), [0, 255, 1, 2]).unwrap();
        let inv = Inventory::collect(&[temp.path().join("folder")]).unwrap();
        assert_eq!(inv.descriptors.len(), 2);
        let index = inv.descriptors.iter().position(|f| !f.directory).unwrap();
        let bytes = inv
            .read(FileRequest {
                stream: 1,
                index: index as u32,
                size_only: false,
                offset: 1,
                count: 2,
            })
            .unwrap();
        assert_eq!(bytes, [255, 1]);
        let mut down = Download::new(inv.descriptors.clone()).unwrap();
        while down.next(1).unwrap().is_some() {
            let n = down.files[down.index].size - down.offset;
            let b = inv
                .read(FileRequest {
                    stream: 1,
                    index: down.index as u32,
                    size_only: false,
                    offset: down.offset,
                    count: n as u32,
                })
                .unwrap();
            down.append(&b).unwrap();
        }
        assert_eq!(
            std::fs::read(down.dir.path().join("folder/data.bin")).unwrap(),
            [0, 255, 1, 2]
        );
    }
    #[test]
    fn symlinks_and_changed_files_are_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let p = temp.path().join("file");
        std::fs::write(&p, "old").unwrap();
        std::os::unix::fs::symlink(&p, temp.path().join("link")).unwrap();
        assert!(Inventory::collect(&[temp.path().join("link")]).is_err());
        let inv = Inventory::collect(std::slice::from_ref(&p)).unwrap();
        std::fs::write(p, "longer").unwrap();
        assert!(
            inv.read(FileRequest {
                stream: 1,
                index: 0,
                size_only: true,
                offset: 0,
                count: 8
            })
            .is_err()
        );
    }
    #[test]
    fn conflicting_ancestors_are_rejected_in_both_orders() {
        let directory = FileDescriptor {
            name: "A".into(),
            directory: true,
            size: 0,
        };
        let child = FileDescriptor {
            name: "a\\child".into(),
            directory: false,
            size: 1,
        };
        assert!(Download::new(vec![directory.clone(), child.clone()]).is_err());
        assert!(Download::new(vec![child.clone(), directory]).is_err());
        let file = FileDescriptor {
            name: "a".into(),
            directory: false,
            size: 1,
        };
        assert!(Download::new(vec![file.clone(), child.clone()]).is_err());
        assert!(Download::new(vec![child, file]).is_err());
    }
}
