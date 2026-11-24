use crate::{model::Mode, policy, trash::Identity};
use std::collections::HashMap;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Clone)]
pub(super) struct Roots {
    pub apps: PathBuf,
    pub caches: PathBuf,
    pub logs: PathBuf,
}
impl Default for Roots {
    fn default() -> Self {
        Self {
            apps: "/Applications".into(),
            caches: "/Library/Caches".into(),
            logs: "/Library/Logs".into(),
        }
    }
}
#[derive(Clone)]
pub(super) struct Owner {
    pub app: PathBuf,
    pub id: String,
    info: Identity,
}
#[derive(Clone)]
pub(super) struct Candidate {
    pub path: PathBuf,
    pub owner: Owner,
    pub identity: Identity,
    pub cache: bool,
    pub bytes: u64,
    pub uid: u32,
}
#[derive(Default)]
pub(super) struct Inventory {
    pub items: Vec<Candidate>,
    pub skipped: usize,
    pub partial: bool,
}
fn vendor_id(id: &str) -> bool {
    let lower = id.to_ascii_lowercase();
    id.len() <= 200
        && !lower.starts_with("com.apple.")
        && lower != "com.piernicolademaria.lindo"
        && id.split('.').count() >= 3
        && id
            .split('.')
            .all(|p| !p.is_empty() && p.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-'))
}
fn archive(name: &str) -> bool {
    let compressed = name
        .strip_suffix(".gz")
        .or_else(|| name.strip_suffix(".bz2"));
    let base = compressed.unwrap_or(name);
    if compressed.is_some() && base.ends_with(".log") {
        return true;
    }
    base.rsplit_once(".log.").is_some_and(|(prefix, n)| {
        !prefix.is_empty()
            && n.len() <= 6
            && n.as_bytes()
                .first()
                .is_some_and(|b| matches!(b, b'1'..=b'9'))
            && n.bytes().all(|b| b.is_ascii_digit())
    })
}
fn cache_body(branch: &[&str]) -> bool {
    match branch {
        ["fsCachedData", file] => {
            file.len() == 36
                && file.bytes().enumerate().all(|(i, b)| {
                    if matches!(i, 8 | 13 | 18 | 23) {
                        b == b'-'
                    } else {
                        b.is_ascii_hexdigit()
                    }
                })
        }
        ["Cache", "Cache_Data", file] => file.rsplit_once('_').is_some_and(|(hash, stream)| {
            hash.len() == 16
                && hash.bytes().all(|b| b.is_ascii_hexdigit())
                && matches!(stream, "0" | "1")
        }),
        _ => false,
    }
}
fn matches_scope(roots: &Roots, owner: &Owner, path: &Path, cache: bool) -> bool {
    let root = if cache { &roots.caches } else { &roots.logs };
    let Some(parts) = path
        .strip_prefix(root.join(&owner.id))
        .ok()
        .and_then(|p| p.iter().map(|s| s.to_str()).collect::<Option<Vec<_>>>())
    else {
        return false;
    };
    if parts
        .iter()
        .any(|s| s.starts_with('.') || s.chars().any(char::is_control))
    {
        return false;
    }
    if cache {
        return cache_body(&parts);
    }
    match parts.as_slice() {
        [name] | ["Archive" | "Archives" | "archive" | "archives", name] => archive(name),
        _ => false,
    }
}
fn allowed_file(
    roots: &Roots,
    owner: &Owner,
    path: &Path,
    cache: bool,
) -> Result<std::fs::Metadata, String> {
    if !matches_scope(roots, owner, path, cache)
        || policy::protected(path, Mode::Space, None)
        || !policy::local_path(path).unwrap_or(false)
    {
        return Err("Percorso escluso.".into());
    }
    let m = path.symlink_metadata().map_err(|e| e.to_string())?;
    if !m.is_file()
        || m.nlink() != 1
        || (m.uid() != unsafe { libc::geteuid() } && m.uid() != 0)
        || std::os::macos::fs::MetadataExt::st_flags(&m)
            & (libc::UF_IMMUTABLE | libc::SF_IMMUTABLE | 0x00080000)
            != 0
    {
        return Err("File protetto o non attribuibile.".into());
    }
    if !cache {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_secs();
        if m.mtime() < 0 || now.saturating_sub(m.mtime() as u64) < 7 * 86400 {
            return Err("Log modificato negli ultimi sette giorni.".into());
        }
    }
    Ok(m)
}
pub(super) fn validate(roots: &Roots, item: &Candidate) -> Result<(), String> {
    if !vendor_id(&item.owner.id)
        || item.owner.app.parent() != Some(roots.apps.as_path())
        || !item
            .owner
            .info
            .matches(&item.owner.app.join("Contents/Info.plist"))
        || policy::app_identifier(&item.owner.app).ok().as_ref() != Some(&item.owner.id)
    {
        return Err("Attribuzione all’app cambiata.".into());
    }
    let current = allowed_file(roots, &item.owner, &item.path, item.cache)?;
    if !item.identity.matches(&item.path) || !item.identity.matches_metadata(&current) {
        return Err("File cambiato. Ripeti l’analisi.".into());
    }
    Ok(())
}
pub(super) fn scan(roots: &Roots, cancel: &AtomicBool) -> Result<Inventory, String> {
    let _guard = policy::LocalOnly::enter().map_err(|e| e.to_string())?;
    let mut out = Inventory::default();
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut entries = 0;
    let mut budget = || {
        entries += 1;
        entries <= 20_000 && Instant::now() < deadline && !cancel.load(Ordering::Acquire)
    };
    let mut apps = HashMap::<String, Option<Owner>>::new();
    for app in policy::children(&roots.apps).map_err(|e| e.to_string())? {
        if !budget() {
            out.partial = true;
            return Ok(out);
        }
        let Ok(name) = app else {
            out.skipped += 1;
            continue;
        };
        let app = roots.apps.join(name);
        if app
            .extension()
            .is_none_or(|s| !s.eq_ignore_ascii_case("app"))
        {
            continue;
        }
        let Ok(id) = policy::app_identifier(&app) else {
            continue;
        };
        if !vendor_id(&id) {
            continue;
        }
        let Ok(info) = Identity::read(&app.join("Contents/Info.plist")) else {
            continue;
        };
        let owner = Owner {
            app,
            id: id.clone(),
            info,
        };
        apps.entry(id)
            .and_modify(|old| *old = None)
            .or_insert(Some(owner));
    }
    for (cache, root) in [(true, &roots.caches), (false, &roots.logs)] {
        let Ok(top) = policy::children(root) else {
            out.partial = true;
            continue;
        };
        for name in top {
            if !budget() {
                out.partial = true;
                return Ok(out);
            }
            let Ok(name) = name else {
                out.skipped += 1;
                continue;
            };
            let Some(Some(owner)) = name.to_str().and_then(|n| apps.get(n)) else {
                out.skipped += 1;
                continue;
            };
            let base = root.join(name);
            let allowed = if cache {
                &["fsCachedData", "Cache/Cache_Data"][..]
            } else {
                &["", "Archive", "Archives", "archive", "archives"][..]
            };
            for branch in allowed {
                let dir = base.join(branch);
                let stream = match policy::children(&dir) {
                    Ok(d) => d,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(_) => {
                        out.skipped += 1;
                        continue;
                    }
                };
                for leaf in stream {
                    if !budget() || out.items.len() >= 750 {
                        out.partial = true;
                        return Ok(out);
                    }
                    let Ok(leaf) = leaf else {
                        out.skipped += 1;
                        continue;
                    };
                    let path = dir.join(leaf);
                    let Ok(m) = allowed_file(roots, owner, &path, cache) else {
                        out.skipped += 1;
                        continue;
                    };
                    let Ok(identity) = Identity::read(&path) else {
                        out.skipped += 1;
                        continue;
                    };
                    if !identity.matches_metadata(&m) {
                        out.skipped += 1;
                        continue;
                    }
                    out.items.push(Candidate {
                        path,
                        owner: owner.clone(),
                        identity,
                        cache,
                        uid: m.uid(),
                        bytes: m.len(),
                    });
                }
            }
        }
    }
    out.items.sort_unstable_by(|a, b| a.path.cmp(&b.path));
    let mut nodes = std::collections::HashSet::new();
    out.items.retain(|item| nodes.insert(item.identity.node()));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    pub(super) fn fixture(base: &Path) -> Roots {
        let roots = Roots {
            apps: base.join("Applications"),
            caches: base.join("Library/Caches"),
            logs: base.join("Library/Logs"),
        };
        let info = roots.apps.join("Example.app/Contents/Info.plist");
        std::fs::create_dir_all(info.parent().unwrap()).unwrap();
        std::fs::write(info,br#"<?xml version="1.0"?><plist version="1.0"><dict><key>CFBundleIdentifier</key><string>org.lindo.fixture</string></dict></plist>"#).unwrap();
        for path in [&roots.caches, &roots.logs] {
            std::fs::create_dir_all(path.join("org.lindo.fixture")).unwrap();
        }
        roots
    }
    fn put(path: &Path, old: bool) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"synthetic").unwrap();
        if old {
            std::fs::File::open(path)
                .unwrap()
                .set_modified(SystemTime::now() - Duration::from_secs(9 * 86400))
                .unwrap();
        }
    }
    #[test]
    fn exact_bundle_cache_and_rotated_logs_exclude_apple_database_symlinks_and_recent() {
        let temp = tempfile::tempdir().unwrap();
        let roots = fixture(&temp.path().canonicalize().unwrap());
        let log = roots.logs.join("org.lindo.fixture/worker.log.2.gz");
        let body = roots
            .caches
            .join("org.lindo.fixture/Cache/Cache_Data/0123456789abcdef_0");
        put(&log, true);
        put(&body, false);
        for file in [
            "org.lindo.fixture/current.log",
            "com.apple.private/archived.log.2",
            "org.other.unmatched/archived.log.2",
            "org.lindo.fixture/store.sqlite",
            "org.lindo.fixture/Private/archived.log.2",
        ] {
            put(&roots.logs.join(file), true);
        }
        put(&roots.logs.join("org.lindo.fixture/new.log.1"), false);
        put(
            &roots
                .caches
                .join("org.lindo.fixture/Cache/Cache_Data/index"),
            true,
        );
        std::os::unix::fs::symlink(&log, roots.logs.join("org.lindo.fixture/linked.log.1"))
            .unwrap();
        let inv = scan(&roots, &AtomicBool::new(false)).unwrap();
        assert_eq!(
            inv.items.len(),
            2,
            "{:?}",
            inv.items.iter().map(|i| &i.path).collect::<Vec<_>>()
        );
        assert!(inv.items.iter().all(|i| validate(&roots, i).is_ok()));
        let old = inv.items.iter().find(|i| i.path == log).unwrap();
        put(&log, false);
        assert!(validate(&roots, old).is_err());
    }
    #[test]
    fn an_open_descriptor_blocks_removal_even_if_the_app_bundle_is_closed() {
        let temp = tempfile::tempdir().unwrap();
        let roots = fixture(&temp.path().canonicalize().unwrap());
        let log = roots.logs.join("org.lindo.fixture/pinned.log.1");
        put(&log, true);
        let list = scan(&roots, &AtomicBool::new(false)).unwrap();
        let held = std::fs::File::open(&log).unwrap();
        let attempt =
            crate::system_junk::removal::move_one(&roots, &list.items[0], &AtomicBool::new(false));
        assert!(attempt.unwrap_err().contains("File aperto"));
        assert_eq!(held.metadata().unwrap().len(), 9);
        assert!(log.exists());
    }
    #[test]
    #[ignore = "Moves one disposable synthetic user-owned archive to the native Trash then restores the exact destination"]
    fn native_trash_round_trip_for_reviewed_synthetic_log() {
        let temp = tempfile::tempdir().unwrap();
        let roots = fixture(&temp.path().canonicalize().unwrap());
        let log = roots
            .logs
            .join("org.lindo.fixture/only-synthetic-file.log.2");
        put(&log, true);
        let list = scan(&roots, &AtomicBool::new(false)).unwrap();
        assert_eq!(list.items.len(), 1);
        let receipt =
            crate::system_junk::removal::move_one(&roots, &list.items[0], &AtomicBool::new(false))
                .unwrap();
        assert!(!log.exists());
        assert_eq!(std::fs::read(&receipt).unwrap(), b"synthetic");
        std::fs::rename(receipt, &log).unwrap();
    }
}
