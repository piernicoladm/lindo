use crate::{
    model::{Item, Mode, Request},
    policy, scan,
};
use objc2_app_kit::NSWorkspace;
use objc2_foundation::{NSFileManager, NSURL};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    ffi::{CString, OsStr},
    io,
    mem::{size_of, MaybeUninit},
    os::unix::ffi::OsStrExt,
    path::{Component, Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::{SystemTime, UNIX_EPOCH},
};

fn denied() -> io::Error {
    io::Error::other("Percorso cambiato, protetto o non verificabile.")
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Node {
    device: u64,
    inode: u64,
    mode: u16,
    links: u16,
    owner: u32,
    group: u32,
    flags: u32,
    size: u64,
    modified: (i64, i64),
    changed: (i64, i64),
    birth: (i64, i64),
}
impl Node {
    fn from_stat(s: libc::stat) -> Self {
        Self {
            device: s.st_dev as u64,
            inode: s.st_ino,
            mode: s.st_mode,
            links: s.st_nlink,
            owner: s.st_uid,
            group: s.st_gid,
            flags: s.st_flags,
            size: s.st_size.max(0) as u64,
            modified: (s.st_mtime, s.st_mtime_nsec),
            changed: (s.st_ctime, s.st_ctime_nsec),
            birth: (s.st_birthtime, s.st_birthtime_nsec),
        }
    }
    fn hash_into(&self, hash: &mut Sha256) {
        hash.update(self.device.to_le_bytes());
        hash.update(self.inode.to_le_bytes());
        hash.update(self.mode.to_le_bytes());
        hash.update(self.links.to_le_bytes());
        hash.update(self.owner.to_le_bytes());
        hash.update(self.group.to_le_bytes());
        hash.update(self.flags.to_le_bytes());
        hash.update(self.size.to_le_bytes());
        for (seconds, nanos) in [self.modified, self.changed, self.birth] {
            hash.update(seconds.to_le_bytes());
            hash.update(nanos.to_le_bytes());
        }
    }
    fn read(path: &Path) -> io::Result<Self> {
        let name = CString::new(path.as_os_str().as_bytes())?;
        let mut value = MaybeUninit::uninit();
        if unsafe { libc::lstat(name.as_ptr(), value.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self::from_stat(unsafe { value.assume_init() }))
    }
    fn kind(&self) -> u16 {
        self.mode & libc::S_IFMT
    }
    fn key(&self) -> (u64, u64) {
        (self.device, self.inode)
    }
}

/// Ancestors use stable node identifiers so moving one selected sibling does
/// not invalidate the others. Bundle/cache snapshots also record every child.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TreeKind {
    Application,
    Cache,
    Model,
}
impl TreeKind {
    pub(crate) fn from_app(app: bool) -> Self {
        if app {
            Self::Application
        } else {
            Self::Cache
        }
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Identity {
    entry: Node,
    parents: Arc<[(u64, u64, u16)]>,
    tree: Option<(TreeKind, [u8; 32])>,
}

impl Identity {
    pub fn read(path: &Path) -> io::Result<Self> {
        if !path.is_absolute() || path.components().any(|c| c == Component::ParentDir) {
            return Err(denied());
        }
        let mut parents = Vec::new();
        let ancestors: Vec<_> = path.ancestors().skip(1).collect();
        for parent in ancestors.into_iter().rev() {
            let node = Node::read(parent)?;
            if node.kind() != libc::S_IFDIR || node.flags & 0x40000000 != 0 {
                return Err(denied());
            }
            parents.push((node.device, node.inode, node.mode));
        }
        let entry = Node::read(path)?;
        if !matches!(entry.kind(), libc::S_IFREG | libc::S_IFDIR) || entry.flags & 0x40000000 != 0 {
            return Err(denied());
        }
        Ok(Self {
            entry,
            parents: parents.into(),
            tree: None,
        })
    }
    pub fn is_dir(&self) -> bool {
        self.entry.kind() == libc::S_IFDIR
    }
    pub fn child_parents(&self) -> Arc<[(u64, u64, u16)]> {
        let mut parents = self.parents.to_vec();
        parents.push((self.entry.device, self.entry.inode, self.entry.mode));
        parents.into()
    }
    pub fn from_stat_and_parents(stat: libc::stat, parents: Arc<[(u64, u64, u16)]>) -> Self {
        Self {
            entry: Node::from_stat(stat),
            parents,
            tree: None,
        }
    }
    pub fn matches_metadata(&self, value: &std::fs::Metadata) -> bool {
        use std::os::macos::fs::MetadataExt as DarwinMetadata;
        use std::os::unix::fs::MetadataExt;
        let s = &self.entry;
        s.device == value.dev()
            && s.inode == value.ino()
            && u32::from(s.mode) == value.mode()
            && u64::from(s.links) == value.nlink()
            && s.owner == value.uid()
            && s.group == value.gid()
            && s.flags == value.st_flags()
            && s.birth == (value.st_birthtime(), value.st_birthtime_nsec())
            && s.size == value.size()
            && s.modified == (value.mtime(), value.mtime_nsec())
            && s.changed == (value.ctime(), value.ctime_nsec())
    }
    pub fn node(&self) -> (u64, u64) {
        self.entry.key()
    }
    fn root_matches(&self, path: &Path) -> bool {
        Self::read(path).is_ok_and(|v| v.entry == self.entry && v.parents == self.parents)
    }
    pub fn matches(&self, path: &Path) -> bool {
        match &self.tree {
            None => self.root_matches(path),
            Some((kind, _)) => Self::snapshot(path, *kind, &AtomicBool::new(false), None)
                .is_ok_and(|(v, _, _)| &v == self),
        }
    }
    pub fn tree_cancel(
        path: &Path,
        app: bool,
        cancel: &AtomicBool,
    ) -> io::Result<(Self, u64, u64)> {
        Self::tree_cancel_progress(path, app, cancel, None)
    }

    pub fn tree_cancel_progress(
        path: &Path,
        app: bool,
        cancel: &AtomicBool,
        progress: Option<&AtomicU64>,
    ) -> io::Result<(Self, u64, u64)> {
        Self::snapshot(path, TreeKind::from_app(app), cancel, progress)
    }
    pub(crate) fn snapshot(
        path: &Path,
        profile: TreeKind,
        cancel: &AtomicBool,
        progress: Option<&AtomicU64>,
    ) -> io::Result<(Self, u64, u64)> {
        let app = profile == TreeKind::Application;
        let model = profile == TreeKind::Model;
        let mut identity = Self::read(path)?;
        if !identity.is_dir() {
            return Err(denied());
        }
        let mode = if app { Mode::Apps } else { Mode::Cache };
        let mut pending = vec![(path.to_owned(), identity.entry.clone())];
        let mut digest = Sha256::new();
        let mut count = 0;
        let mut seen = HashSet::new();
        let mut bytes: u64 = 0;
        let uid = unsafe { libc::getuid() };
        while let Some((current, mut node)) = pending.pop() {
            if let Some(progress) = progress {
                progress.fetch_add(1, Ordering::Relaxed);
            }
            if cancel.load(Ordering::Relaxed) {
                return Err(io::ErrorKind::Interrupted.into());
            }
            if policy::protected(&current, mode, app.then_some(path))
                || node.device != identity.entry.device
                || node.flags & 0x40000000 != 0
                || current
                    .file_name()
                    .is_some_and(|s| s.as_bytes().ends_with(b".icloud"))
            {
                return Err(denied());
            }
            let kind = node.kind();
            if model && !crate::junk::model_node(path, &current, kind) {
                return Err(denied());
            }
            if !((app || model) && kind == libc::S_IFLNK) && !policy::local_path(&current)? {
                return Err(denied());
            }
            if !app && (node.owner != uid || (kind == libc::S_IFDIR && policy::package(&current))) {
                return Err(denied());
            }
            // The first Finder metadata lookup can set UF_TRACKED and update ctime.
            // Accept that exact transition; every other attribute must still match.
            let after = Node::read(&current)?;
            if after.flags == (node.flags | libc::UF_TRACKED) && node.flags != after.flags {
                node.flags = after.flags;
                node.changed = after.changed;
            }
            if node != after {
                return Err(denied());
            }
            if current == path {
                identity.entry = node.clone();
            }
            match kind {
                libc::S_IFDIR => {
                    if !seen.insert(node.key()) {
                        return Err(denied());
                    }
                    let start = pending.len();
                    let mut dir = policy::children(&current)?;
                    while let Some(name) = dir.next() {
                        if cancel.load(Ordering::Relaxed) {
                            return Err(io::ErrorKind::Interrupted.into());
                        }
                        let name = name?;
                        let child_path = current.join(&name);
                        if policy::protected(&child_path, mode, app.then_some(path)) {
                            return Err(denied());
                        }
                        let child = Node::from_stat(dir.stat(&name)?);
                        pending.push((child_path, child));
                    }
                    pending[start..].sort_unstable_by(|a, b| b.0.cmp(&a.0));
                    if Node::read(&current)? != node {
                        return Err(denied());
                    }
                }
                libc::S_IFREG => {
                    if node.links == 1 || seen.insert(node.key()) {
                        bytes = bytes.saturating_add(node.size);
                    }
                }
                libc::S_IFLNK if app => {}
                libc::S_IFLNK if model && crate::junk::model_link(path, &current) => {}
                _ => return Err(denied()),
            }
            let relative = current
                .strip_prefix(path)
                .map_err(|_| denied())?
                .as_os_str()
                .as_bytes();
            digest.update((relative.len() as u64).to_le_bytes());
            digest.update(relative);
            node.hash_into(&mut digest);
            count += 1;
        }
        if !identity.root_matches(path) {
            return Err(denied());
        }
        identity.tree = Some((profile, digest.finalize().into()));
        Ok((identity, bytes, count))
    }
}

fn scoped(request: &Request, item: &Item) -> bool {
    request.roots.iter().any(|root| {
        if !item.path.starts_with(root)
            || (root == &item.path && request.mode != Mode::Cache)
            || !policy::allowed_root(request.mode, root)
            || !policy::local_path(root).unwrap_or(false)
            || policy::package(root)
        {
            return false;
        }
        // A package discovered after the scan is a new boundary.
        !item.path.ancestors().skip(1).any(policy::package)
    }) && !policy::protected(&item.path, request.mode, None)
        && policy::local_path(&item.path).unwrap_or(false)
        && item.identity.root_matches(&item.path)
}

fn bsd_info(pid: i32) -> io::Result<libc::proc_bsdinfo> {
    let mut info = MaybeUninit::<libc::proc_bsdinfo>::uninit();
    let size = size_of::<libc::proc_bsdinfo>() as i32;
    if unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            size,
        )
    } != size
    {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { info.assume_init() })
}

// Darwin's proc_regionwithpathinfo from <sys/proc_info.h>. libproc exposes
// a pathname even when a running extension has lost its original vnode.
#[repr(C)]
struct RegionPath {
    protection: u32,
    max_protection: u32,
    inheritance: u32,
    flags: u32,
    offset: u64,
    attributes: [u32; 14],
    address: u64,
    size: u64,
    vnode: libc::vnode_info_path,
}
fn mapped_path(pid: i32, name: &[u8]) -> Option<PathBuf> {
    if name.is_empty() {
        return None;
    }
    let mut address = 0;
    let mut result = None;
    for _ in 0..4096 {
        let mut out = MaybeUninit::<RegionPath>::uninit();
        let size = size_of::<RegionPath>() as i32;
        if unsafe { libc::proc_pidinfo(pid, 8, address, out.as_mut_ptr().cast(), size) } != size {
            return result;
        }
        let region = unsafe { out.assume_init() };
        if region.protection & 4 != 0 {
            let raw: Vec<u8> = region
                .vnode
                .vip_path
                .iter()
                .flatten()
                .take_while(|&&c| c != 0)
                .map(|&c| c as u8)
                .collect();
            let path = Path::new(OsStr::from_bytes(&raw));
            if path.is_absolute()
                && path
                    .file_name()
                    .is_some_and(|s| s.as_bytes().starts_with(name))
            {
                if result.as_deref().is_some_and(|prev| prev != path) {
                    return None;
                }
                result = Some(path.to_owned());
            }
        }
        let next = region.address.checked_add(region.size)?;
        if next <= address {
            return None;
        }
        address = next;
    }
    None
}

fn process_paths() -> io::Result<Vec<PathBuf>> {
    const PROC_UID_ONLY: u32 = 4;
    let uid = unsafe { libc::getuid() };
    let required = unsafe { libc::proc_listpids(PROC_UID_ONLY, uid, std::ptr::null_mut(), 0) };
    if required <= 0 {
        return Err(denied());
    }
    let mut pids = vec![0i32; required as usize / size_of::<i32>() + 512];
    let cap = (pids.len() * size_of::<i32>()) as i32;
    let count = unsafe { libc::proc_listpids(PROC_UID_ONLY, uid, pids.as_mut_ptr().cast(), cap) };
    if count <= 0 || count >= cap || count % 4 != 0 {
        return Err(denied());
    }
    let mut paths = Vec::new();
    for &pid in &pids[..count as usize / 4] {
        if pid <= 0 {
            continue;
        }
        let info = match bsd_info(pid) {
            Err(err) if err.raw_os_error() == Some(libc::ESRCH) => continue,
            result => result?,
        };
        if info.pbi_uid != uid || info.pbi_status == libc::SZOMB {
            continue;
        }
        let mut buf = [0u8; 4096];
        if unsafe { libc::proc_pidpath(pid, buf.as_mut_ptr().cast(), buf.len() as u32) } <= 0 {
            let name: Vec<u8> = info
                .pbi_name
                .iter()
                .take_while(|&&c| c != 0)
                .map(|&c| c as u8)
                .collect();
            if let Some(path) = mapped_path(pid, &name) {
                paths.push(path);
                continue;
            }
            match bsd_info(pid) {
                Err(err) if err.raw_os_error() == Some(libc::ESRCH) => continue,
                Ok(v) if v.pbi_status == libc::SZOMB => continue,
                _ => return Err(denied()),
            }
        }
        let end = buf.iter().position(|b| *b == 0).ok_or_else(denied)?;
        let path = Path::new(OsStr::from_bytes(&buf[..end]));
        if !path.is_absolute() {
            return Err(denied());
        }
        paths.push(path.to_owned());
    }
    if paths.is_empty() {
        return Err(denied());
    }
    Ok(paths)
}

fn browser_identifier(owner: &str, identifier: &str) -> bool {
    let family = |base: &str| {
        identifier == base
            || identifier
                .strip_prefix(base)
                .is_some_and(|suffix| suffix.starts_with('.'))
    };
    if owner != "org.mozilla.firefox" {
        return family(owner);
    }
    [
        "org.mozilla.firefox",
        "org.mozilla.firefoxdeveloperedition",
        "org.mozilla.nightly",
        "org.mozilla.plugincontainer",
    ]
    .into_iter()
    .any(family)
        || [
            "firefox",
            "firefox-developer-edition",
            "firefox-nightly",
            "nightly",
        ]
        .into_iter()
        .any(|channel| {
            [
                "gpu-helper",
                "media-plugin-helper",
                "security-module-helper",
            ]
            .into_iter()
            .any(|helper| family(&format!("org.mozilla.{channel}-{helper}")))
        })
}

fn browser_path(id: &str, path: &Path) -> bool {
    let names: &[&str] = match id {
        "com.google.Chrome" => &["Google Chrome"],
        "org.chromium.Chromium" => &["Chromium"],
        "org.mozilla.firefox" => &[
            "Firefox",
            "Firefox Developer Edition",
            "Firefox Nightly",
            "Nightly",
        ],
        _ => return true,
    };
    if path.components().any(|p| {
        names
            .iter()
            .any(|name| p.as_os_str() == OsStr::new(&format!("{name}.app")))
    }) {
        return true;
    }
    let Some(exec) = path.file_name().and_then(OsStr::to_str) else {
        return true;
    };
    let Some(macos) = path
        .parent()
        .filter(|p| p.file_name() == Some(OsStr::new("MacOS")))
    else {
        return false;
    };
    let Some(contents) = macos
        .parent()
        .filter(|p| p.file_name() == Some(OsStr::new("Contents")))
    else {
        return false;
    };
    if !contents
        .parent()
        .is_some_and(|p| p.extension() == Some(OsStr::new("app")))
    {
        return false;
    }
    if id == "org.mozilla.firefox"
        && path
            .components()
            .any(|c| c.as_os_str() == "Thunderbird.app")
    {
        return false;
    }
    if id == "org.mozilla.firefox" {
        return matches!(
            exec,
            "firefox"
                | "firefox-bin"
                | "plugin-container"
                | "gpu-helper"
                | "media-plugin-helper"
                | "security-module-helper"
        );
    }
    exec == names[0] || exec.starts_with(&format!("{} Helper", names[0]))
}

fn check_running(path: &Path, id: &str, cache: bool) -> Result<(), String> {
    let blocked = |p: &Path| p.starts_with(path) || (cache && browser_path(id, p));
    let mut in_use = objc2::rc::autoreleasepool(|_| {
        let running = NSWorkspace::sharedWorkspace().runningApplications();
        running.iter().any(|app| {
            let identifier = app
                .bundleIdentifier()
                .map(|s| s.to_string())
                .unwrap_or_default();
            identifier == id
                || (cache && browser_identifier(id, &identifier))
                || [app.bundleURL(), app.executableURL()]
                    .iter()
                    .flatten()
                    .filter_map(|u| u.to_file_path())
                    .any(|p| blocked(&p))
        })
    });
    if !in_use {
        in_use = process_paths()
            .map_err(|_| "Verifica dei processi incompleta. Rimozione bloccata.")?
            .iter()
            .any(|p| blocked(p));
    }
    if in_use {
        Err("Chiudi l’applicazione e i suoi processi prima di riprovare.".into())
    } else {
        Ok(())
    }
}

fn validate_junk(request: &Request, item: &Item) -> Result<(), String> {
    let stale = "Applicazione o cache cambiata. Ripeti la scansione.";
    if !scoped(request, item) || policy::cloud(&item.path) {
        return Err(stale.into());
    }
    let app = item
        .path
        .extension()
        .is_some_and(|s| s.eq_ignore_ascii_case("app"));
    if app {
        if !item
            .path
            .parent()
            .is_some_and(|p| policy::allowed_root(Mode::Apps, p))
            || ![0, unsafe { libc::getuid() }].contains(&item.identity.entry.owner)
        {
            return Err(stale.into());
        }
        let root = item.path.parent().ok_or(stale)?;
        let app_request = Request {
            mode: Mode::Apps,
            roots: vec![root.into()],
            min_bytes: 0,
        };
        return validate(&app_request, item, None);
    }
    if item.identity.entry.kind() == libc::S_IFREG {
        if item.identity.entry.owner != unsafe { libc::getuid() }
            || item.identity.entry.size != item.bytes
            || !crate::junk::installer(&item.path)
        {
            return Err(stale.into());
        }
        crate::junk::closed(&item.path, false)?;
        return if scoped(request, item) {
            Ok(())
        } else {
            Err(stale.into())
        };
    }
    let owner = crate::junk::cache_owner(request, &item.path).ok_or(stale)?;
    let expected = if matches!(owner, crate::junk::Owner::Model) {
        TreeKind::Model
    } else {
        TreeKind::Cache
    };
    if item.identity.entry.owner != unsafe { libc::getuid() }
        || !matches!(item.identity.tree, Some((kind, _)) if kind==expected)
        || !item.identity.matches(&item.path)
    {
        return Err(stale.into());
    }
    match owner {
        crate::junk::Owner::Model => {
            crate::junk::closed(&item.path, true)?;
            let locks = crate::junk::model_locks(&item.path);
            match std::fs::symlink_metadata(&locks) {
                Ok(metadata)
                    if metadata.is_dir() && policy::local_path(&locks).unwrap_or(false) =>
                {
                    crate::junk::closed(&locks, true)?
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                _ => return Err(stale.into()),
            }
        }
        crate::junk::Owner::Backup => crate::junk::closed(&item.path, true)?,
        crate::junk::Owner::Leftover(id) => {
            check_running(&item.path, &id, false)?;
            crate::junk::closed(&item.path, true)?;
        }
        crate::junk::Owner::WholeCache(bundle, id) => {
            check_running(&bundle, &id, false)?;
            crate::junk::closed(&item.path, true)?;
        }
        crate::junk::Owner::Application(bundle, id) => check_running(&bundle, &id, false)?,
        crate::junk::Owner::Xcode => {
            check_running(
                Path::new("/Applications/Xcode.app"),
                "com.apple.dt.Xcode",
                false,
            )?;
            let paths = process_paths().map_err(|_| "Verifica dei processi Xcode incompleta.")?;
            if paths.iter().any(|p| {
                p.file_name().and_then(|s| s.to_str()).is_some_and(|name| {
                    [
                        "Xcode",
                        "xcodebuild",
                        "XCBBuildService",
                        "SourceKitService",
                        "clang",
                        "clang++",
                        "swift",
                        "swiftc",
                        "swift-frontend",
                        "swift-build",
                        "sourcekit-lsp",
                    ]
                    .contains(&name)
                }) || p.starts_with(&item.path)
            }) {
                return Err("Chiudi Xcode e attendi la fine delle compilazioni.".into());
            }
        }
    }
    if !scoped(request, item) || !item.identity.matches(&item.path) {
        return Err(stale.into());
    }
    Ok(())
}

fn validate(request: &Request, item: &Item, keeper: Option<&Item>) -> Result<(), String> {
    let stale = "Elemento cambiato, protetto o fuori dall’ambito. Ripeti la scansione.";
    if !scoped(request, item)
        || std::env::current_exe()
            .map_err(|_| stale)?
            .starts_with(&item.path)
    {
        return Err(stale.into());
    }
    let node = &item.identity.entry;
    match request.mode {
        Mode::Junk => return validate_junk(request, item),
        Mode::Space => return Err("La mappa dello spazio è in sola lettura.".into()),
        Mode::Duplicates | Mode::Large | Mode::Logs => {
            if node.kind() != libc::S_IFREG
                || node.size != item.bytes
                || item
                    .path
                    .components()
                    .any(|c| c.as_os_str().as_bytes().starts_with(b"."))
            {
                return Err(stale.into());
            }
            if request.mode == Mode::Large && item.bytes < request.min_bytes {
                return Err(stale.into());
            }
            if request.mode == Mode::Logs {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_err(|_| stale)?
                    .as_secs() as i64;
                if node.owner != unsafe { libc::getuid() } || node.modified.0 >= now - 7 * 86400 {
                    return Err(stale.into());
                }
            }
            if request.mode == Mode::Large && item.group.is_some() {
                let keep = keeper.ok_or("Manca l’immagine da conservare.")?;
                if keep.group != item.group
                    || keep.identity.node() == item.identity.node()
                    || !scoped(request, keep)
                    || !keep.identity.matches(&keep.path)
                {
                    return Err("L’immagine da conservare è cambiata. Ripeti l’analisi.".into());
                }
            }
            if request.mode == Mode::Duplicates {
                let keep = keeper.ok_or("Manca la copia da conservare.")?;
                if item.group.is_none()
                    || item.group != keep.group
                    || item.identity.node() == keep.identity.node()
                    || item.bytes != keep.bytes
                    || !scoped(request, keep)
                    || !scan::same_contents(item, keep, &AtomicBool::new(false)).unwrap_or(false)
                    || !keep.identity.root_matches(&keep.path)
                {
                    return Err(
                        "La copia da conservare non è più verificabile. Ripeti la scansione."
                            .into(),
                    );
                }
            }
        }
        Mode::Apps | Mode::Cache => {
            let app = request.mode == Mode::Apps;
            if !matches!(&item.identity.tree,Some((kind,_)) if *kind == TreeKind::from_app(app)) {
                return Err(stale.into());
            }
            if !item.identity.matches(&item.path) {
                return Err(stale.into());
            }
            let id = if app {
                let id = policy::app_identifier(&item.path).map_err(|_| stale)?;
                if id.is_empty()
                    || id.to_lowercase().starts_with("com.apple.")
                    || id.eq_ignore_ascii_case("com.piernicolademaria.lindo")
                    || !item
                        .path
                        .extension()
                        .is_some_and(|s| s.eq_ignore_ascii_case("app"))
                {
                    return Err(stale.into());
                }
                id
            } else {
                policy::cache_owner(&item.path).ok_or(stale)?.to_owned()
            };
            if !scoped(request, item) {
                return Err(stale.into());
            }
            return check_running(&item.path, &id, !app);
        }
    }
    if !scoped(request, item) {
        return Err(stale.into());
    }
    Ok(())
}

/// Revalidate the reviewed file type, ancestry and descriptor around the
/// feature-specific check. A malware finding additionally requires a fresh scan.
pub(crate) fn move_reviewed_file(
    path: &Path,
    identity: &Identity,
    validate_feature: impl FnOnce(&std::fs::File) -> Result<(), String>,
) -> Result<Option<PathBuf>, String> {
    let _guard = policy::LocalOnly::enter().map_err(|e| e.to_string())?;
    let stale = "Il file è cambiato o non può essere spostato nel Cestino.";
    if policy::protected(path, Mode::Space, None)
        || policy::cloud(path)
        || path.components().any(|c| {
            c.as_os_str()
                .to_string_lossy()
                .eq_ignore_ascii_case("_3_Vault")
        })
        || identity.entry.kind() != libc::S_IFREG
        || identity.entry.links != 1
        || identity.entry.owner != unsafe { libc::geteuid() }
        || !identity.matches(path)
        || !policy::local_path(path).unwrap_or(false)
    {
        return Err(stale.into());
    }
    let handle = policy::open_file(path, 0).map_err(|_| stale)?;
    let matches_handle = || {
        handle
            .metadata()
            .is_ok_and(|m| identity.matches_metadata(&m))
    };
    if !matches_handle() {
        return Err(stale.into());
    }
    validate_feature(&handle)?;
    if !matches_handle() || !identity.matches(path) || !policy::local_path(path).unwrap_or(false) {
        return Err(stale.into());
    }
    native_trash(path, false)
}

pub(crate) fn move_mail_download_copy(item: &Item) -> Result<Option<PathBuf>, String> {
    move_reviewed_file(&item.path, &item.identity, |_| {
        crate::accounts::attachments::validate(item)
    })
}

fn native_trash(path: &Path, directory: bool) -> Result<Option<PathBuf>, String> {
    objc2::rc::autoreleasepool(|_| {
        let url = if directory {
            NSURL::from_directory_path(path)
        } else {
            NSURL::from_file_path(path)
        }
        .ok_or("Percorso non valido per il Cestino.")?;
        let mut destination = None;
        NSFileManager::defaultManager()
            .trashItemAtURL_resultingItemURL_error(&url, Some(&mut destination))
            .map_err(|err| {
                format!(
                    "Impossibile spostare nel Cestino. {}",
                    err.localizedDescription()
                )
            })?;
        Ok(destination.and_then(|u| u.to_file_path()))
    })
}

#[derive(Debug, PartialEq, Eq)]
pub enum MoveOutcome {
    Moved,
    KeeperChanged,
}

fn move_checked(
    request: &Request,
    item: &Item,
    keeper: Option<&Item>,
) -> Result<(MoveOutcome, Option<PathBuf>), String> {
    let _local_only =
        policy::LocalOnly::enter().map_err(|_| "Protezione dei file cloud non disponibile.")?;
    validate(request, item, keeper)?;
    let destination = native_trash(&item.path, item.identity.is_dir())?;
    let outcome = if (request.mode == Mode::Duplicates
        || (request.mode == Mode::Large && item.group.is_some()))
        && !keeper.is_some_and(|k| {
            policy::local_path(&k.path).unwrap_or(false)
                && !policy::protected(&k.path, request.mode, None)
                && Identity::read(&k.path).is_ok_and(|current| {
                    current.node() == k.identity.node()
                        && current.parents == k.identity.parents
                        && current.entry.kind() == libc::S_IFREG
                        && current.entry.size == k.bytes
                })
        }) {
        MoveOutcome::KeeperChanged
    } else {
        MoveOutcome::Moved
    };
    Ok((outcome, destination))
}

pub fn move_one(
    request: &Request,
    item: &Item,
    keeper: Option<&Item>,
) -> Result<MoveOutcome, String> {
    move_checked(request, item, keeper).map(|(outcome, _)| outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::symlink};

    #[test]
    fn junk_validator_pins_installer_identity_and_refuses_active_or_unrecognized_files() {
        if std::env::var_os("LINDO_JUNK_GUARD_WORKER").is_none() {
            let temp = tempfile::tempdir().unwrap();
            let home = temp.path().canonicalize().unwrap();
            let status=std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact","trash::tests::junk_validator_pins_installer_identity_and_refuses_active_or_unrecognized_files"])
                .env("LINDO_FIXTURE_WORKER",&home).env("LINDO_JUNK_GUARD_WORKER","1").status().unwrap();
            assert!(status.success());
            return;
        }
        let downloads = policy::home().join("Downloads");
        fs::create_dir_all(&downloads).unwrap();
        let path = downloads.join("fixture.pkg");
        fs::write(&path, b"xar!fixture").unwrap();
        policy::local_path(&path).unwrap();
        let item = Item {
            path: path.clone(),
            bytes: 11,
            identity: Identity::read(&path).unwrap(),
            group: None,
            cloud: false,
        };
        let request = Request {
            mode: Mode::Junk,
            roots: vec![downloads],
            min_bytes: 0,
        };
        validate(&request, &item, None).unwrap();
        let opened = fs::File::open(&path).unwrap();
        assert!(validate(&request, &item, None).is_err());
        drop(opened);
        fs::write(&path, b"xxxxfixture").unwrap();
        assert!(validate(&request, &item, None).is_err());
        let forged = Item {
            identity: Identity::read(&path).unwrap(),
            ..item
        };
        assert!(validate(&request, &forged, None).is_err());
    }
    #[test]
    fn in_place_changes_and_replaced_parents_invalidate_identity() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().canonicalize().unwrap();
        let parent = root.join("parent");
        fs::create_dir(&parent).unwrap();
        let path = parent.join("file");
        fs::write(&path, b"one").unwrap();
        let before = Identity::read(&path).unwrap();
        fs::write(&path, b"two").unwrap();
        assert!(!before.matches(&path));
        let current = Identity::read(&path).unwrap();
        let renamed = root.join("renamed");
        fs::rename(&parent, &renamed).unwrap();
        symlink(&renamed, &parent).unwrap();
        assert!(!current.matches(&path));
    }

    #[test]
    fn tree_detects_changes_and_never_follows_internal_links() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().canonicalize().unwrap().join("Sample.app");
        fs::create_dir_all(root.join("Contents")).unwrap();
        let file = root.join("Contents/file");
        fs::write(&file, b"one").unwrap();
        symlink("/nonexistent", root.join("Contents/link")).unwrap();
        let (before, size, count) =
            Identity::tree_cancel(&root, true, &AtomicBool::new(false)).unwrap();
        assert_eq!((size, count), (3, 4));
        assert!(before.matches(&root));
        fs::write(file, b"two").unwrap();
        assert!(!before.matches(&root));
        assert!(Identity::tree_cancel(&root, false, &AtomicBool::new(false)).is_err());
    }

    #[test]
    fn renamed_browser_and_helpers_block_cache_removal() {
        for (id, exe) in [
            ("com.google.Chrome", "Google Chrome Helper (GPU)"),
            ("org.chromium.Chromium", "Chromium"),
            ("org.mozilla.firefox", "plugin-container"),
        ] {
            assert!(browser_path(
                id,
                &Path::new("/Applications/Renamed.app/Contents/MacOS").join(exe)
            ));
        }
        assert!(!browser_path(
            "com.google.Chrome",
            Path::new("/usr/bin/true")
        ));
    }

    #[test]
    #[ignore = "inspects current-user process paths without changing any process"]
    fn live_process_inventory_includes_caller() {
        let exe = std::env::current_exe().unwrap().canonicalize().unwrap();
        let paths = process_paths().expect("Current-user process inventory is incomplete");
        assert!(paths.iter().any(|p| p == &exe));
    }

    #[test]
    fn firefox_identifiers_include_helpers_and_exclude_thunderbird() {
        for id in [
            "org.mozilla.firefoxdeveloperedition",
            "org.mozilla.nightly",
            "org.mozilla.firefox-nightly-media-plugin-helper.worker",
            "org.mozilla.plugincontainer",
        ] {
            assert!(browser_identifier("org.mozilla.firefox", id));
        }
        for id in [
            "org.mozilla.thunderbird",
            "org.mozilla.firefoxes",
            "org.mozilla.firefox-thunderbird",
        ] {
            assert!(!browser_identifier("org.mozilla.firefox", id));
        }
        assert!(!browser_path(
            "org.mozilla.firefox",
            Path::new("/Applications/Thunderbird.app/Contents/MacOS/gpu-helper")
        ));
    }

    #[test]
    fn freshly_created_cache_tree_matches_first_recheck() {
        let parent = std::env::var_os("LINDO_TRASH_TEST_PARENT")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let tmp = tempfile::Builder::new()
            .prefix("Lindo-tree-fixture-")
            .tempdir_in(parent)
            .unwrap();
        let root = tmp.path().canonicalize().unwrap().join("Cache");
        fs::create_dir_all(root.join("nested")).unwrap();
        fs::write(root.join("nested/data"), b"fresh").unwrap();
        let (id, bytes, entries) =
            Identity::tree_cancel(&root, false, &AtomicBool::new(false)).unwrap();
        assert_eq!((bytes, entries), (5, 3));
        assert!(id.matches(&root));
    }

    // Opt-in, requires an explicit local fixture parent within a user document
    // scope. Restore only the exact OS-returned destination, without listing Trash.
    #[test]
    #[ignore = "moves and restores a synthetic duplicate through the macOS Trash"]
    fn real_duplicate_trash_round_trip() {
        let parent =
            std::env::var_os("LINDO_TRASH_TEST_PARENT").expect("Choose a local fixture parent");
        let fixture = tempfile::Builder::new()
            .prefix("Lindo-rust-fixture-")
            .tempdir_in(parent)
            .unwrap();
        let root = fixture.path().canonicalize().unwrap();
        let make_item = |name: &str| {
            let path = root.join(name);
            fs::write(&path, b"synthetic").unwrap();
            assert!(policy::local_path(&path).unwrap());
            Item {
                identity: Identity::read(&path).unwrap(),
                bytes: 9,
                path,
                group: Some(7),
                cloud: false,
            }
        };
        let keep = make_item("keeper");
        let extra = make_item("extra");
        let req = Request {
            mode: Mode::Duplicates,
            roots: vec![root],
            min_bytes: 1,
        };
        assert!(validate(&req, &extra, Some(&extra)).is_err());
        let (outcome, destination) = move_checked(&req, &extra, Some(&keep)).unwrap();
        let destination = destination.expect("OS Trash did not report a destination");
        let good = !extra.path.exists()
            && fs::read(&keep.path).is_ok_and(|b| b == b"synthetic")
            && fs::read(&destination).is_ok_and(|b| b == b"synthetic");
        fs::rename(&destination, &extra.path).unwrap_or_else(|err| {
            panic!(
                "Restore fixture {} to {}: {err}",
                destination.display(),
                extra.path.display()
            )
        });
        assert!(good);
        assert_eq!(outcome, MoveOutcome::Moved);
        assert_eq!(fs::read(&extra.path).unwrap(), b"synthetic");
    }
}
