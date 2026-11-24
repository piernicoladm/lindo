//! Narrow recognizers used both during discovery and immediately before Trash.
use super::{accessible, existing_dir};
use crate::policy;
use std::{
    ffi::OsStr,
    fs,
    io::{Read, Seek, SeekFrom},
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
};

pub(super) const HUB: &str = ".cache/huggingface/hub";
pub(super) const BACKUPS: &str = "Library/Application Support/MobileSync/Backup";

fn hex(s: &str, n: usize) -> bool {
    s.len() == n && s.bytes().all(|c| c.is_ascii_hexdigit())
}
fn device_id(id: &str) -> bool {
    hex(id, 40)
        || id
            .split_once('-')
            .is_some_and(|(a, b)| hex(a, 8) && hex(b, 16))
}
fn backup_id(name: &str) -> Option<(&str, bool)> {
    if device_id(name) {
        return Some((name, false));
    }
    let (prefix, time) = name.rsplit_once('-')?;
    let (id, date) = prefix.rsplit_once('-')?;
    if device_id(id)
        && date.len() == 8
        && time.len() == 6
        && date.bytes().chain(time.bytes()).all(|b| b.is_ascii_digit())
    {
        return Some((id, true));
    }
    None
}
fn plist_value(path: &Path, key: &str) -> Option<String> {
    if !accessible(path) {
        return None;
    }
    let input = policy::open_file(path, 0).ok()?;
    let meta = input.metadata().ok()?;
    if !meta.is_file() || meta.len() > 4 * 1024 * 1024 {
        return None;
    }
    let output = Command::new("/usr/bin/plutil")
        .args(["-extract", key, "raw", "-o", "-", "-"])
        .stdin(Stdio::from(input))
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}
pub(super) fn backup(home: &Path, path: &Path) -> Option<bool> {
    if path.parent()? != home.join(BACKUPS) || !existing_dir(path) {
        return None;
    }
    let (id, archived) = backup_id(path.file_name()?.to_str()?)?;
    if !plist_value(&path.join("Info.plist"), "Target Identifier")?.eq_ignore_ascii_case(id)
        || plist_value(&path.join("Status.plist"), "SnapshotState")? != "finished"
        || !["true", "false"]
            .contains(&plist_value(&path.join("Manifest.plist"), "IsEncrypted")?.as_str())
    {
        return None;
    }
    let db = path.join("Manifest.db");
    if !accessible(&db) || !policy::open_file(&db, 0).ok()?.metadata().ok()?.is_file() {
        return None;
    }
    Some(archived)
}
pub(super) fn model(home: &Path, path: &Path) -> bool {
    let Some(name) = path
        .file_name()
        .and_then(OsStr::to_str)
        .and_then(|n| n.strip_prefix("models--"))
    else {
        return false;
    };
    if path.parent() != Some(home.join(HUB).as_path())
        || !existing_dir(path)
        || !name.split("--").all(|s| {
            !s.is_empty()
                && !s.starts_with('.')
                && s.bytes()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'-'))
        })
    {
        return false;
    }
    let snaps = path.join("snapshots");
    existing_dir(&snaps)
        && policy::children(&snaps).is_ok_and(|mut dir| {
            dir.any(|n| {
                n.ok()
                    .and_then(|n| n.to_str().map(|s| hex(s, 40)))
                    .unwrap_or(false)
            })
        })
}
pub(crate) fn model_node(root: &Path, path: &Path, kind: u16) -> bool {
    let Ok(relative) = path.strip_prefix(root) else {
        return false;
    };
    if relative.as_os_str().is_empty() {
        return kind == libc::S_IFDIR;
    }
    let mut parts = relative.iter();
    match parts.next().and_then(OsStr::to_str) {
        Some("blobs") => match parts.next() {
            None => kind == libc::S_IFDIR,
            Some(hash) => {
                parts.next().is_none()
                    && kind == libc::S_IFREG
                    && hash.to_str().is_some_and(|s| hex(s, 40) || hex(s, 64))
            }
        },
        Some("snapshots") | Some(".no_exist") => {
            let Some(revision) = parts.next() else {
                return kind == libc::S_IFDIR;
            };
            revision.to_str().is_some_and(|s| hex(s, 40))
                && (parts.next().is_some() || kind == libc::S_IFDIR)
        }
        Some("refs") => kind == libc::S_IFDIR || (parts.next().is_some() && kind == libc::S_IFREG),
        _ => false,
    }
}
pub(crate) fn model_link(root: &Path, path: &Path) -> bool {
    if !path.starts_with(root.join("snapshots")) {
        return false;
    }
    let Ok(link) = fs::read_link(path) else {
        return false;
    };
    let mut resolved = path.parent().unwrap_or(root).to_owned();
    for part in link.components() {
        match part {
            Component::Normal(name) => resolved.push(name),
            Component::CurDir => {}
            Component::ParentDir if resolved != root => {
                resolved.pop();
            }
            _ => return false,
        }
    }
    resolved.parent() == Some(root.join("blobs").as_path())
        && resolved
            .file_name()
            .and_then(OsStr::to_str)
            .is_some_and(|s| hex(s, 40) || hex(s, 64))
        && accessible(&resolved)
        && policy::open_file(&resolved, 0).is_ok_and(|f| f.metadata().is_ok_and(|m| m.is_file()))
}
pub(super) fn installer(home: &Path, path: &Path) -> bool {
    if path.parent() != Some(home.join("Downloads").as_path()) || !accessible(path) {
        return false;
    }
    let Ok(mut file) = policy::open_file(path, 0) else {
        return false;
    };
    let Ok(meta) = file.metadata() else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    let (magic, offset) = match path
        .extension()
        .and_then(OsStr::to_str)
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "dmg" if meta.len() >= 512 => (b"koly", SeekFrom::End(-512)),
        "pkg" | "mpkg" => (b"xar!", SeekFrom::Start(0)),
        _ => return false,
    };
    let mut found = [0u8; 4];
    file.seek(offset).is_ok() && file.read_exact(&mut found).is_ok() && &found == magic
}
pub(crate) fn closed(path: &Path, directory: bool) -> Result<(), String> {
    let mut cmd = Command::new("/usr/sbin/lsof");
    cmd.args(["-n", "-P", "-Fp"]);
    if directory {
        cmd.arg("+D");
    }
    let result = cmd
        .arg(path)
        .output()
        .map_err(|_| "Verifica dei file in uso non disponibile.")?;
    if result.status.code() == Some(1) && result.stdout.is_empty() && result.stderr.is_empty() {
        Ok(())
    } else if !result.stdout.is_empty() {
        Err("Uno o più file sono aperti. Chiudi il programma che li utilizza.".into())
    } else {
        Err("Verifica dei file in uso incompleta. Ripeti la scansione.".into())
    }
}
pub(crate) fn model_locks(path: &Path) -> PathBuf {
    path.parent()
        .unwrap_or(path)
        .join(".locks")
        .join(path.file_name().unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        junk::{inventory, Scope},
        trash::{Identity, TreeKind},
    };
    use std::{os::unix::fs::symlink, sync::atomic::AtomicBool};

    fn plist(value: &str) -> String {
        format!(r#"<?xml version="1.0"?><plist version="1.0"><dict>{value}</dict></plist>"#)
    }
    #[test]
    fn completed_and_archived_backups_require_matching_metadata() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().canonicalize().unwrap();
        let id = "00008030-0123456789ABCDEF";
        let base = home.join(BACKUPS);
        let stop = AtomicBool::new(false);
        for (name, archived) in [
            (id.to_owned(), false),
            (format!("{id}-20260525-125905"), true),
        ] {
            let path = base.join(name);
            fs::create_dir_all(&path).unwrap();
            fs::write(
                path.join("Info.plist"),
                plist(&format!(
                    "<key>Target Identifier</key><string>{id}</string>"
                )),
            )
            .unwrap();
            fs::write(
                path.join("Status.plist"),
                plist("<key>SnapshotState</key><string>finished</string>"),
            )
            .unwrap();
            fs::write(
                path.join("Manifest.plist"),
                plist("<key>IsEncrypted</key><false/>"),
            )
            .unwrap();
            fs::write(path.join("Manifest.db"), b"fixture").unwrap();
            assert_eq!(backup(&home, &path), Some(archived));
        }
        assert_eq!(
            inventory(Scope::Backups, &home, &[], &stop)
                .unwrap()
                .rows
                .iter()
                .filter(|r| r.item.is_some())
                .count(),
            2
        );
        fs::write(
            base.join(id).join("Status.plist"),
            plist("<key>SnapshotState</key><string>started</string>"),
        )
        .unwrap();
        assert!(backup(&home, &base.join(id)).is_none());
        fs::rename(base.join(id), base.join("00008030-0000000000000000")).unwrap();
        assert!(backup(&home, &base.join("00008030-0000000000000000")).is_none());
    }

    #[test]
    fn repository_snapshot_permits_only_links_to_own_blobs() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().canonicalize().unwrap();
        let repo = home.join(HUB).join("models--example--model");
        let hash = "a".repeat(40);
        let revision = repo.join("snapshots").join(&hash);
        fs::create_dir_all(&revision).unwrap();
        fs::create_dir_all(repo.join("blobs")).unwrap();
        let blob = repo.join("blobs").join(&hash);
        fs::write(&blob, b"weights").unwrap();
        let link = revision.join("model.safetensors");
        symlink(Path::new("../../blobs").join(&hash), &link).unwrap();
        let stop = AtomicBool::new(false);
        assert!(model(&home, &repo));
        let (identity, size, _) = Identity::snapshot(&repo, TreeKind::Model, &stop, None).unwrap();
        assert_eq!(size, 7);
        assert!(identity.matches(&repo));
        assert!(inventory(Scope::Development, &home, &[], &stop)
            .unwrap()
            .rows
            .iter()
            .any(|r| r.path == repo && r.item.is_some()));
        assert!(closed(&repo, true).is_ok());
        let opened = fs::File::open(&blob).unwrap();
        assert!(closed(&repo, true).is_err());
        drop(opened);
        fs::remove_file(&link).unwrap();
        symlink("/etc/passwd", &link).unwrap();
        assert!(!identity.matches(&repo));
        assert!(Identity::snapshot(&repo, TreeKind::Model, &stop, None).is_err());
    }
    #[test]
    fn installers_require_signature_and_immediate_downloads_parent() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().canonicalize().unwrap();
        let base = home.join("Downloads");
        fs::create_dir_all(base.join("deep")).unwrap();
        fs::write(base.join("flat.pkg"), b"xar!test").unwrap();
        fs::write(base.join("misnamed.pkg"), b"test").unwrap();
        fs::write(base.join("deep/file.pkg"), b"xar!test").unwrap();
        let mut dmg = [0u8; 512];
        dmg[..4].copy_from_slice(b"koly");
        fs::write(base.join("disk.dmg"), dmg).unwrap();
        for name in ["flat.pkg", "disk.dmg"] {
            assert!(installer(&home, &base.join(name)))
        }
        for name in ["misnamed.pkg", "deep/file.pkg"] {
            assert!(!installer(&home, &base.join(name)))
        }
    }
}
