use super::{accessible, app_location, entries, existing_dir, third_party, Inventory};
use crate::{model::Mode, policy, trash::TreeKind};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

fn regenerable_branch(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "gpucache"
            | "code cache"
            | "shadercache"
            | "grshadercache"
            | "dawncache"
            | "dawngraphitecache"
            | "dawnwebgpucache"
            | "thumbnails"
            | "thumbnailcache"
            | "imagecache"
            | "image-cache"
            | "compiledshaders"
    )
}
fn guarded_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name.starts_with('.')
        || [
            "keychain",
            "password",
            "credential",
            "session",
            "cookie",
            "wallet",
            "profile",
            "bookmark",
            "history",
            "indexeddb",
            "leveldb",
            "login data",
            "local storage",
            "databases",
            "backup",
            "documents",
            "sourcepackages",
            "private",
            "secret",
        ]
        .iter()
        .any(|p| name.contains(p))
        || [
            ".db",
            ".db-wal",
            ".db-shm",
            ".sqlite",
            ".sqlite3",
            ".sqlite-wal",
            ".sqlite-shm",
            ".plist",
            ".key",
            ".pem",
            ".p12",
            ".doc",
            ".docx",
            ".xlsx",
            ".pages",
            ".pdf",
            ".zip",
            ".json",
            ".txt",
            ".md",
        ]
        .iter()
        .any(|s| name.ends_with(s))
}
pub(super) fn permitted(home: &Path, id: &str, path: &Path, stop: &AtomicBool) -> bool {
    if !third_party(id) || path != home.join("Library/Caches").join(id) || !existing_dir(path) {
        return false;
    }
    let sensitive = id.to_ascii_lowercase();
    if [
        "chrome",
        "firefox",
        "chromium",
        "brave",
        "vivaldi",
        "opera",
        "browser",
        "password",
        "wallet",
        "keychain",
        "bitwarden",
        "keepass",
        "protonpass",
        "lastpass",
    ]
    .iter()
    .any(|s| sensitive.contains(s))
    {
        return false;
    }
    let mut pending = vec![(path.to_owned(), 0)];
    let mut branches = 0;
    while let Some((parent, depth)) = pending.pop() {
        if depth > 32 || stop.load(Ordering::Relaxed) {
            return false;
        }
        let Ok(mut directory) = policy::children(&parent) else {
            return false;
        };
        while let Some(name) = directory.next() {
            if stop.load(Ordering::Relaxed) {
                return false;
            }
            let Ok(name) = name else { return false };
            let Some(n) = name.to_str() else { return false };
            let child = parent.join(&name);
            if policy::protected(&child, Mode::Junk, None) || guarded_name(n) {
                return false;
            }
            let Ok(stat) = directory.stat(&name) else {
                return false;
            };
            let kind = stat.st_mode & libc::S_IFMT;
            if stat.st_uid != unsafe { libc::getuid() }
                || stat.st_flags & 0x40000000 != 0
                || policy::placeholder_name(&child)
                || !matches!(kind, libc::S_IFDIR | libc::S_IFREG)
                || !policy::local_path(&child).unwrap_or(false)
            {
                return false;
            }
            if depth == 0 {
                if kind != libc::S_IFDIR || !regenerable_branch(n) {
                    return false;
                }
                branches += 1;
            }
            if kind == libc::S_IFDIR {
                if policy::package(&child) {
                    return false;
                }
                pending.push((child, depth + 1));
            }
        }
    }
    branches > 0
}
pub(super) fn inventory(
    home: &Path,
    app_roots: &[PathBuf],
    system_library: &Path,
    stop: &AtomicBool,
    result: &mut Inventory,
) {
    // An identifier installed twice cannot attribute a shared cache to one bundle.
    let mut apps: HashMap<String, Option<PathBuf>> = HashMap::new();
    for root in app_roots {
        entries(root, stop, result, |path, _, _| {
            if app_location(home, path) && accessible(path) {
                if let Ok(id) = policy::app_identifier(path) {
                    if third_party(&id) {
                        apps.entry(id)
                            .and_modify(|owner| *owner = None)
                            .or_insert_with(|| Some(path.into()));
                    }
                }
            }
        });
    }
    entries(
        &home.join("Library/Caches"),
        stop,
        result,
        |path, stat, result| {
            if stat.st_mode & libc::S_IFMT != libc::S_IFDIR || !existing_dir(path) {
                return;
            }
            let Some(id) = path.file_name().and_then(|n| n.to_str()) else {
                return;
            };
            if let Some(Some(bundle)) = apps.get(id) {
                if permitted(home, id, path, stop) {
                    result.tree(
                        path,
                        TreeKind::Cache,
                        &format!("Cache rigenerabile · identificatore verificato {id}"),
                        Some(bundle),
                        stop,
                    );
                } else {
                    result.inspect(
                        path,
                        format!("Cache associata a {id} · struttura da esaminare nel Finder"),
                        None,
                    );
                }
            } else {
                result.inspect(
                    path,
                    "Cache utente · proprietario non identificato univocamente",
                    None,
                );
            }
        },
    );
    for (name, label) in [
        ("Caches", "Cache di sistema · consultazione"),
        ("Logs", "Log di sistema · consultazione"),
    ] {
        entries(
            &system_library.join(name),
            stop,
            result,
            |path, stat, result| {
                if accessible(path) {
                    result.inspect(
                        path,
                        label,
                        (stat.st_mode & libc::S_IFMT == libc::S_IFREG)
                            .then_some(stat.st_size as u64),
                    );
                } else {
                    result.omitted += 1;
                }
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::symlink};
    #[test]
    fn selects_only_verified_cache_roots_and_keeps_system_inventory_read_only() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().canonicalize().unwrap();
        let id = "org.example.fixture";
        let apps = home.join("Applications");
        let app = apps.join("Renamed.app/Contents");
        fs::create_dir_all(&app).unwrap();
        fs::write(app.join("Info.plist"),format!("<plist version=\"1.0\"><dict><key>CFBundleIdentifier</key><string>{id}</string></dict></plist>")).unwrap();
        let cache = home.join("Library/Caches").join(id);
        let branch = cache.join("thumbnails");
        fs::create_dir_all(&branch).unwrap();
        fs::write(branch.join("image-001.png"), b"png").unwrap();
        let system = home.join("GlobalLibrary");
        fs::create_dir_all(system.join("Caches/org.example.system")).unwrap();
        fs::create_dir_all(system.join("Logs")).unwrap();
        fs::write(system.join("Logs/old.log"), b"log").unwrap();
        let flag = AtomicBool::new(false);
        let mut result = Inventory::default();
        inventory(
            &home,
            std::slice::from_ref(&apps),
            &system,
            &flag,
            &mut result,
        );
        assert_eq!(result.rows.iter().filter(|r| r.item.is_some()).count(), 1);
        assert!(result
            .rows
            .iter()
            .any(|r| r.path == cache && r.owner.as_ref() == Some(&apps.join("Renamed.app"))));
        assert_eq!(
            result
                .rows
                .iter()
                .filter(|r| r.path.starts_with(&system) && r.item.is_none())
                .count(),
            2
        );
        fs::write(branch.join("user.sqlite"), b"data").unwrap();
        assert!(!permitted(&home, id, &cache, &flag));
        fs::remove_file(branch.join("user.sqlite")).unwrap();
        symlink("/etc/passwd", branch.join("alias")).unwrap();
        assert!(!permitted(&home, id, &cache, &flag));
    }
}
