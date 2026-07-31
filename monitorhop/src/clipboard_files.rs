//! Safe copy/paste of local files through the authenticated clipboard link.
//!
//! Clipboard managers expose file selections as `file://` URIs. We snapshot
//! those paths into a bounded tar archive, transfer the archive, and extract it
//! into a private cache directory on the receiving machine. Symlinks, special
//! files, absolute paths, and `..` components are rejected at both ends.

use std::{
    collections::BTreeSet,
    fs,
    io::Cursor,
    path::{Component, Path, PathBuf},
};

use thiserror::Error;
use url::Url;
use walkdir::WalkDir;

const MAX_ARCHIVE_SIZE: usize = monitorhop_proto::MAX_CLIPBOARD_TRANSFER_SIZE;

#[derive(Debug, Error)]
pub(crate) enum FileClipboardError {
    #[error("unsupported clipboard file URI: {0}")]
    Uri(String),
    #[error("clipboard file is unavailable: {0}")]
    Io(#[from] std::io::Error),
    #[error("clipboard selection contains an unsupported file type: {0}")]
    UnsupportedType(PathBuf),
    #[error("clipboard archive exceeds the {MAX_ARCHIVE_SIZE} byte safety limit")]
    TooLarge,
    #[error("clipboard archive contains an unsafe path")]
    UnsafePath,
    #[error("clipboard archive contains links or special files")]
    UnsafeEntry,
    #[error("clipboard archive is empty")]
    Empty,
}

fn uri_to_path(uri: &str) -> Result<PathBuf, FileClipboardError> {
    if let Ok(url) = Url::parse(uri) {
        if url.scheme() != "file" || url.host().is_some() {
            return Err(FileClipboardError::Uri(uri.to_owned()));
        }
        return url
            .to_file_path()
            .map_err(|_| FileClipboardError::Uri(uri.to_owned()));
    }
    let path = PathBuf::from(uri);
    if path.is_absolute() {
        Ok(path)
    } else {
        Err(FileClipboardError::Uri(uri.to_owned()))
    }
}

fn check_tree(path: &Path) -> Result<u64, FileClipboardError> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink()
        || !metadata.file_type().is_file() && !metadata.file_type().is_dir()
    {
        return Err(FileClipboardError::UnsupportedType(path.to_owned()));
    }
    if metadata.is_file() {
        return Ok(metadata.len());
    }
    let mut total = 0u64;
    for entry in WalkDir::new(path).follow_links(false) {
        let entry = entry.map_err(|e| FileClipboardError::Io(std::io::Error::other(e)))?;
        let metadata = entry
            .metadata()
            .map_err(|e| FileClipboardError::Io(std::io::Error::other(e)))?;
        if metadata.file_type().is_symlink()
            || (!metadata.file_type().is_file() && !metadata.file_type().is_dir())
        {
            return Err(FileClipboardError::UnsupportedType(entry.path().to_owned()));
        }
        if metadata.is_file() {
            total = total.saturating_add(metadata.len());
        }
    }
    Ok(total)
}

pub(crate) fn build_archive(uris: &[String]) -> Result<Vec<u8>, FileClipboardError> {
    if uris.is_empty() {
        return Err(FileClipboardError::Empty);
    }
    let roots: Vec<PathBuf> = uris
        .iter()
        .map(|u| uri_to_path(u))
        .collect::<Result<_, _>>()?;
    let total: u64 = roots
        .iter()
        .map(|p| check_tree(p))
        .try_fold(0u64, |acc, size| size.map(|s| acc.saturating_add(s)))?;
    if total > MAX_ARCHIVE_SIZE as u64 {
        return Err(FileClipboardError::TooLarge);
    }

    let mut bytes = Vec::new();
    {
        let mut builder = tar::Builder::new(&mut bytes);
        for root in roots {
            let name = root
                .file_name()
                .filter(|n| !n.is_empty())
                .ok_or_else(|| FileClipboardError::Uri(root.display().to_string()))?;
            let name = PathBuf::from(name);
            if root.is_file() {
                builder.append_path_with_name(&root, &name)?;
            } else {
                for entry in WalkDir::new(&root).follow_links(false) {
                    let entry =
                        entry.map_err(|e| FileClipboardError::Io(std::io::Error::other(e)))?;
                    let metadata = entry
                        .metadata()
                        .map_err(|e| FileClipboardError::Io(std::io::Error::other(e)))?;
                    if metadata.file_type().is_symlink()
                        || (!metadata.file_type().is_file() && !metadata.file_type().is_dir())
                    {
                        return Err(FileClipboardError::UnsupportedType(entry.path().to_owned()));
                    }
                    let relative = entry
                        .path()
                        .strip_prefix(&root)
                        .map_err(|_| FileClipboardError::UnsafePath)?;
                    let archive_name = name.join(relative);
                    if metadata.is_dir() {
                        builder.append_dir(&archive_name, entry.path())?;
                    } else {
                        builder.append_path_with_name(entry.path(), &archive_name)?;
                    }
                }
            }
        }
        builder.finish()?;
    }
    if bytes.is_empty() {
        return Err(FileClipboardError::Empty);
    }
    Ok(bytes)
}

fn cache_root() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(std::env::temp_dir)
        .join("monitorhop/clipboard")
}

fn safe_entry_path(
    root: &Path,
    entry: &mut tar::Entry<'_, Cursor<&[u8]>>,
) -> Result<PathBuf, FileClipboardError> {
    let path = entry
        .path()
        .map_err(|_| FileClipboardError::UnsafePath)?
        .into_owned();
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(FileClipboardError::UnsafePath);
    }
    let kind = entry.header().entry_type();
    if kind.is_symlink() || kind.is_hard_link() || !kind.is_file() && !kind.is_dir() {
        return Err(FileClipboardError::UnsafeEntry);
    }
    Ok(root.join(path))
}

pub(crate) fn extract_archive(
    bytes: &[u8],
    transfer_id: u64,
) -> Result<Vec<String>, FileClipboardError> {
    let result = extract_archive_inner(bytes, transfer_id);
    if result.is_err() {
        let _ = fs::remove_dir_all(cache_root().join(format!("{transfer_id:016x}")));
    }
    result
}

fn extract_archive_inner(
    bytes: &[u8],
    transfer_id: u64,
) -> Result<Vec<String>, FileClipboardError> {
    if bytes.is_empty() || bytes.len() > MAX_ARCHIVE_SIZE {
        return Err(FileClipboardError::TooLarge);
    }
    let root = cache_root();
    fs::create_dir_all(&root)?;
    let staging = root.join(format!("{transfer_id:016x}"));
    if staging.exists() {
        return Err(FileClipboardError::Io(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "clipboard staging directory already exists",
        )));
    }
    fs::create_dir(&staging)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&staging, fs::Permissions::from_mode(0o700))?;
    }

    let mut archive = tar::Archive::new(Cursor::new(bytes));
    let mut top_level = BTreeSet::new();
    for item in archive.entries()? {
        let mut entry = item?;
        let output = safe_entry_path(&staging, &mut entry)?;
        if let Some(Component::Normal(name)) = entry.path()?.components().next() {
            top_level.insert(staging.join(name));
        }
        entry.unpack(&output)?;
    }
    if top_level.is_empty() {
        let _ = fs::remove_dir_all(&staging);
        return Err(FileClipboardError::Empty);
    }
    top_level
        .into_iter()
        .map(|p| {
            Url::from_file_path(p)
                .map(|u| u.to_string())
                .map_err(|_| FileClipboardError::Uri("staged path".into()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn archive_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("hello.txt");
        fs::File::create(&file)
            .unwrap()
            .write_all(b"hello")
            .unwrap();
        let uri = Url::from_file_path(&file).unwrap().to_string();
        let archive = build_archive(&[uri]).unwrap();
        let transfer_id = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;
        let files = extract_archive(&archive, transfer_id).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(
            fs::read(Url::parse(&files[0]).unwrap().to_file_path().unwrap()).unwrap(),
            b"hello"
        );
    }

    #[test]
    fn archive_rejects_parent_path() {
        let mut bytes = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut bytes);
            let mut header = tar::Header::new_gnu();
            header.set_path("safe.txt").unwrap();
            header.as_mut_bytes()[..14].copy_from_slice(b"../escape.txt\0");
            header.set_size(1);
            header.set_cksum();
            builder.append(&header, &b"x"[..]).unwrap();
            builder.finish().unwrap();
        }
        assert!(matches!(
            extract_archive(&bytes, 0xfeed_beef),
            Err(FileClipboardError::UnsafePath)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn archive_rejects_symlink_source() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.txt");
        fs::write(&target, b"secret").unwrap();
        let link = dir.path().join("link.txt");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let uri = Url::from_file_path(&link).unwrap().to_string();
        assert!(matches!(
            build_archive(&[uri]),
            Err(FileClipboardError::UnsupportedType(_))
        ));
    }
}
