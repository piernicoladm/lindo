use crate::{
    model::{
        Identity, Item, Mode, Progress, Report, Request, SpaceStats, COMPARE, HASH, TREE, WALK,
    },
    policy,
};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    fs::File,
    io::{self, Read, Seek},
    os::unix::fs::MetadataExt,
    path::Path,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::{SystemTime, UNIX_EPOCH},
};

const DATALESS: u32 = 0x40000000;
const BLOCK: usize = 256 * 1024;
type Node = (u64, u64);
fn node(s: &libc::stat) -> Node {
    (s.st_dev as u64, s.st_ino)
}
fn cancelled(stop: &AtomicBool) -> io::Result<()> {
    if stop.load(Ordering::Relaxed) {
        Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "Analisi interrotta",
        ))
    } else {
        Ok(())
    }
}
fn changed() -> io::Error {
    io::Error::other("Il file è cambiato durante l'analisi")
}
#[derive(Clone, Copy)]
enum ProviderCheck {
    Scan,
    Always,
}
impl ProviderCheck {
    fn local(self, item: &Item) -> io::Result<bool> {
        // Every candidate was classified on discovery. For files outside cloud
        // scopes the kernel ban on materialization and the unchanged fd/path
        // identities (including flags) still apply to every scan read.
        if item.cloud || matches!(self, Self::Always) {
            policy::local_path(&item.path)
        } else {
            Ok(true)
        }
    }
}
fn open_duplicate(item: &Item, provider: ProviderCheck) -> io::Result<File> {
    if !provider.local(item)? {
        return Err(changed());
    }
    let f = policy::open_file(&item.path, 0)?;
    let m = f.metadata()?;
    if !m.is_file()
        || !item.identity.matches_metadata(&m)
        || m.len() != item.bytes
        || !item.identity.matches(&item.path)
    {
        return Err(changed());
    }
    require_no_fork(&f)?;
    Ok(f)
}
fn require_no_fork(f: &File) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    let size = unsafe {
        libc::fgetxattr(
            f.as_raw_fd(),
            c"com.apple.ResourceFork".as_ptr(),
            std::ptr::null_mut(),
            0,
            0,
            0,
        )
    };
    if size == 0 {
        return Ok(());
    }
    if size < 0 {
        let e = io::Error::last_os_error();
        if e.raw_os_error() == Some(libc::ENOATTR) {
            return Ok(());
        }
        return Err(e);
    }
    Err(io::Error::other("Resource fork presente"))
}
fn verify(item: &Item, file: &File, provider: ProviderCheck) -> io::Result<()> {
    let m = file.metadata()?;
    if !item.identity.matches_metadata(&m)
        || m.len() != item.bytes
        || !item.identity.matches(&item.path)
        || !provider.local(item)?
    {
        return Err(changed());
    }
    require_no_fork(file)
}
fn fingerprint(
    item: &Item,
    stop: &AtomicBool,
    scratch: &mut [u8],
    progress: &AtomicU64,
) -> io::Result<[u8; 32]> {
    let mut f = open_duplicate(item, ProviderCheck::Scan)?;
    let mut digest = Sha256::new();
    let mut remaining = item.bytes;
    while remaining > 0 {
        cancelled(stop)?;
        let n = remaining.min(scratch.len() as u64) as usize;
        f.read_exact(&mut scratch[..n])?;
        digest.update(&scratch[..n]);
        progress.fetch_add(n as u64, Ordering::Relaxed);
        remaining -= n as u64;
    }
    verify(item, &f, ProviderCheck::Scan)?;
    Ok(digest.finalize().into())
}
pub fn same_contents(a: &Item, b: &Item, stop: &AtomicBool) -> io::Result<bool> {
    let _local_only = policy::LocalOnly::enter()?;
    if a.bytes != b.bytes || a.identity.node() == b.identity.node() {
        return Ok(false);
    }
    let (mut fa, mut fb) = (
        open_duplicate(a, ProviderCheck::Always)?,
        open_duplicate(b, ProviderCheck::Always)?,
    );
    let equal = CompareBuffers::new(a.bytes).equal(&mut fa, &mut fb, a.bytes, stop, None)?;
    verify(a, &fa, ProviderCheck::Always)?;
    verify(b, &fb, ProviderCheck::Always)?;
    Ok(equal)
}

struct CompareBuffers {
    left: Vec<u8>,
    right: Vec<u8>,
}
impl CompareBuffers {
    fn new(bytes: u64) -> Self {
        let block = bytes.min(BLOCK as u64) as usize;
        Self {
            left: vec![0; block],
            right: vec![0; block],
        }
    }
    fn equal(
        &mut self,
        left: &mut File,
        right: &mut File,
        bytes: u64,
        stop: &AtomicBool,
        progress: Option<&AtomicU64>,
    ) -> io::Result<bool> {
        let mut remaining = bytes;
        while remaining > 0 {
            cancelled(stop)?;
            let n = remaining.min(self.left.len() as u64) as usize;
            left.read_exact(&mut self.left[..n])?;
            right.read_exact(&mut self.right[..n])?;
            if let Some(counter) = progress {
                counter.fetch_add(n as u64, Ordering::Relaxed);
            }
            if self.left[..n] != self.right[..n] {
                return Ok(false);
            }
            remaining -= n as u64;
        }
        cancelled(stop)?;
        Ok(true)
    }
}

struct Scan<'a> {
    req: &'a Request,
    stop: &'a AtomicBool,
    progress: &'a Progress,
    report: Report,
    directories: HashSet<Node>,
    hardlinks: HashSet<Node>,
    cutoff: i64,
    space_totals: SpaceStats,
}
impl Scan<'_> {
    fn visit(&mut self, n: u64) {
        self.report.visited += n;
        self.progress
            .visited
            .store(self.report.visited, Ordering::Relaxed)
    }
    fn error(&mut self, p: &Path, e: io::Error) {
        if self.stop.load(Ordering::Relaxed) {
            return;
        }
        self.report.errors += 1;
        if self.report.notes.len() < 12 {
            self.report.notes.push(format!("{}: {e}", p.display()))
        }
    }
    fn emit(
        &mut self,
        path: &Path,
        bytes: u64,
        stat: libc::stat,
        parents: &Arc<[(u64, u64, u16)]>,
        ubiquitous: bool,
    ) {
        self.report.items.push(Item {
            path: path.into(),
            bytes,
            identity: Identity::from_stat_and_parents(stat, parents.clone()),
            group: None,
            cloud: ubiquitous || policy::cloud(path),
        });
    }
    fn tree(&mut self, path: &Path, apps: bool, node_before: Node) {
        // A unit may appear both beneath a requested root and as an explicit root.
        if !self.directories.insert(node_before) {
            return;
        }
        let bundle = if apps {
            match policy::app_identifier(path) {
                Ok(id)
                    if id.to_ascii_lowercase().starts_with("com.apple.")
                        || id.eq_ignore_ascii_case("com.piernicolademaria.lindo") =>
                {
                    self.report.skipped += 1;
                    return;
                }
                Ok(id) => Some(id),
                Err(e) => {
                    self.error(path, e);
                    return;
                }
            }
        } else {
            None
        };
        self.progress.phase.store(TREE, Ordering::Relaxed);
        self.progress.tree_entries.store(0, Ordering::Relaxed);
        let snapshot = Identity::tree_cancel_progress(
            path,
            apps,
            self.stop,
            Some(&self.progress.tree_entries),
        );
        self.visit(self.progress.tree_entries.swap(0, Ordering::Relaxed));
        self.progress.phase.store(WALK, Ordering::Relaxed);
        match snapshot {
            Ok((id, bytes, _)) => {
                if id.node() != node_before
                    || bundle
                        .as_ref()
                        .is_some_and(|b| policy::app_identifier(path).ok().as_ref() != Some(b))
                {
                    self.error(path, changed());
                    return;
                }
                self.report.items.push(Item {
                    path: path.into(),
                    bytes,
                    identity: id,
                    group: None,
                    cloud: policy::cloud(path),
                });
            }
            Err(e) => self.error(path, e),
        }
    }
    fn walk(&mut self, path: &Path, depth: usize, expected: Option<Node>) -> u64 {
        let mut dir = match policy::children(path) {
            Ok(d) => d,
            Err(e) => {
                self.error(path, e);
                return 0;
            }
        };
        let current = match dir.node() {
            Ok(n) => n,
            Err(e) => {
                self.error(path, e);
                return 0;
            }
        };
        if expected.is_some_and(|n| n != current) {
            self.error(path, changed());
            return 0;
        }
        if !self.directories.insert(current) {
            return 0;
        }
        let parents = match Identity::read(path) {
            Ok(dir_id) if dir_id.node() == current => dir_id.child_parents(),
            Ok(_) => {
                self.error(path, changed());
                return 0;
            }
            Err(e) => {
                self.error(path, e);
                return 0;
            }
        };
        let mut sum = 0u64;
        while !self.stop.load(Ordering::Relaxed) {
            let name = match dir.next() {
                None => break,
                Some(Ok(n)) => n,
                Some(Err(e)) => {
                    self.error(path, e);
                    break;
                }
            };
            let p = path.join(&name);
            self.visit(1);
            if policy::placeholder_name(&p) || policy::protected(&p, self.req.mode, None) {
                self.report.skipped += 1;
                continue;
            }
            let s = match dir.stat(&name) {
                Ok(s) => s,
                Err(e) => {
                    self.error(&p, e);
                    continue;
                }
            };
            let kind = s.st_mode & libc::S_IFMT;
            if s.st_flags & DATALESS != 0 || !matches!(kind, libc::S_IFREG | libc::S_IFDIR) {
                self.report.skipped += 1;
                continue;
            }
            let directory = kind == libc::S_IFDIR;
            let size = s.st_size.max(0) as u64;
            if self.req.mode == Mode::Space {
                let before = self.space_totals;
                let errors = self.report.errors;
                let skipped = self.report.skipped;
                let mut bytes = (s.st_blocks.max(0) as u64).saturating_mul(512);
                let repeated = !directory && s.st_nlink > 1 && !self.hardlinks.insert(node(&s));
                if repeated {
                    bytes = 0;
                    self.space_totals.hardlinks += 1;
                } else {
                    self.space_totals.logical_bytes =
                        self.space_totals.logical_bytes.saturating_add(size);
                    self.space_totals.files += u64::from(!directory);
                }
                if directory {
                    if depth >= 64 {
                        self.error(&p, io::Error::other("Profondità massima raggiunta"))
                    } else {
                        bytes = bytes.saturating_add(self.walk(&p, depth + 1, Some(node(&s))))
                    }
                }
                sum = sum.saturating_add(bytes);
                if depth == 0 {
                    self.report.space.insert(
                        p.clone(),
                        SpaceStats {
                            logical_bytes: self.space_totals.logical_bytes - before.logical_bytes,
                            files: self.space_totals.files - before.files,
                            hardlinks: self.space_totals.hardlinks - before.hardlinks,
                            skipped: self.report.skipped - skipped,
                            errors: self.report.errors - errors,
                        },
                    );
                    self.emit(&p, bytes, s, &parents, false)
                }
                continue;
            }
            if !directory
                && (matches!(self.req.mode, Mode::Cache | Mode::Apps)
                    || (self.req.mode == Mode::Logs
                        && (s.st_mtime >= self.cutoff || s.st_uid != unsafe { libc::getuid() }))
                    || (matches!(self.req.mode, Mode::Large | Mode::Duplicates)
                        && size < self.req.min_bytes))
            {
                continue;
            }
            let ubiquitous = match policy::provider_state(&p, directory) {
                Ok((true, ubiquitous)) => ubiquitous,
                Ok((false, _)) => {
                    self.report.skipped += 1;
                    continue;
                }
                Err(e) => {
                    self.error(&p, e);
                    continue;
                }
            };
            if directory {
                if self.req.mode == Mode::Apps
                    && p.extension().is_some_and(|s| s.eq_ignore_ascii_case("app"))
                {
                    self.tree(&p, true, node(&s));
                    continue;
                }
                if self.req.mode == Mode::Cache {
                    if s.st_uid != unsafe { libc::getuid() } {
                        self.report.skipped += 1;
                        continue;
                    }
                    if policy::cache_owner(&p).is_some() {
                        self.tree(&p, false, node(&s));
                        continue;
                    }
                    if !policy::cache_branch(&p) {
                        self.report.skipped += 1;
                        continue;
                    }
                }
                if policy::package(&p) {
                    self.report.skipped += 1;
                    continue;
                }
                if depth >= 64 || (self.req.mode == Mode::Apps && depth >= 3) {
                    self.error(&p, io::Error::other("Profondità massima raggiunta"));
                    continue;
                }
                self.walk(&p, depth + 1, Some(node(&s)));
            } else if s.st_nlink <= 1 || self.hardlinks.insert(node(&s)) {
                // Foundation may set UF_TRACKED on the first resource lookup.
                // Capture the resulting identity without accepting file replacement.
                match dir.stat(&name) {
                    Ok(after)
                        if node(&after) == node(&s)
                            && after.st_size == s.st_size
                            && after.st_mtime == s.st_mtime
                            && after.st_mtime_nsec == s.st_mtime_nsec
                            && after.st_flags & DATALESS == 0 =>
                    {
                        self.emit(&p, size, after, &parents, ubiquitous)
                    }
                    Ok(_) => self.error(&p, changed()),
                    Err(e) => self.error(&p, e),
                }
            }
        }
        sum
    }
    fn compare_group(&mut self, mut keeper: Item, copies: impl Iterator<Item = Item>, group: u64) {
        if cancelled(self.stop).is_err() {
            return;
        }
        let mut anchor = match open_duplicate(&keeper, ProviderCheck::Always) {
            Ok(file) => file,
            Err(e) => {
                self.error(&keeper.path, e);
                return;
            }
        };
        let mut buffers = CompareBuffers::new(keeper.bytes);
        let start = self.report.items.len();
        for mut item in copies {
            if cancelled(self.stop).is_err() {
                self.report.items.truncate(start);
                return;
            }
            // Keep the opened node pinned. A cheap fd stat catches mutations
            // immediately; the pathname, ancestors, fork and provider are checked
            // again before publishing any member of this group.
            if !anchor
                .metadata()
                .is_ok_and(|m| keeper.identity.matches_metadata(&m))
            {
                self.report.items.truncate(start);
                self.error(&keeper.path, changed());
                return;
            }
            if let Err(e) = anchor.rewind() {
                self.report.items.truncate(start);
                self.error(&keeper.path, e);
                return;
            }
            let compared = (|| {
                if item.bytes != keeper.bytes || item.identity.node() == keeper.identity.node() {
                    return Ok(false);
                }
                let mut file = open_duplicate(&item, ProviderCheck::Scan)?;
                let equal = buffers.equal(
                    &mut anchor,
                    &mut file,
                    item.bytes,
                    self.stop,
                    Some(&self.progress.compared_bytes),
                )?;
                verify(&item, &file, ProviderCheck::Scan)?;
                Ok(equal)
            })();
            match compared {
                Ok(true) => {
                    item.group = Some(group);
                    self.report.items.push(item)
                }
                Ok(false) => self.error(
                    &item.path,
                    io::Error::other("Confronto dei contenuti differente"),
                ),
                Err(e) => self.error(&item.path, e),
            }
        }
        if let Err(e) =
            cancelled(self.stop).and_then(|()| verify(&keeper, &anchor, ProviderCheck::Always))
        {
            self.report.items.truncate(start);
            self.error(&keeper.path, e);
            return;
        }
        if self.report.items.len() > start {
            keeper.group = Some(group);
            self.report.items.push(keeper)
        }
    }
    fn duplicates(&mut self) {
        let mut sizes: HashMap<u64, Vec<Item>> = HashMap::new();
        for item in self.report.items.drain(..) {
            sizes.entry(item.bytes).or_default().push(item)
        }
        let mut scratch = vec![0u8; BLOCK];
        let mut group = 0u64;
        for candidates in sizes.into_values().filter(|v| v.len() > 1) {
            let mut hashes: HashMap<[u8; 32], Vec<Item>> = HashMap::new();
            self.progress.phase.store(HASH, Ordering::Relaxed);
            for item in candidates {
                if cancelled(self.stop).is_err() {
                    return;
                }
                match fingerprint(&item, self.stop, &mut scratch, &self.progress.hashed_bytes) {
                    Ok(h) => hashes.entry(h).or_default().push(item),
                    Err(e) => self.error(&item.path, e),
                }
            }
            self.progress.phase.store(COMPARE, Ordering::Relaxed);
            for mut copies in hashes.into_values().filter(|v| v.len() > 1) {
                copies.sort_unstable_by(|a, b| a.path.cmp(&b.path));
                let mut iter = copies.into_iter();
                let keeper = iter.next().unwrap();
                group += 1;
                self.compare_group(keeper, iter, group);
            }
        }
    }
}

pub fn run(req: &Request, stop: &AtomicBool, progress: &Progress) -> Report {
    if req.mode == Mode::Junk {
        return Report {
            errors: 1,
            notes: vec!["Scegli una categoria nella sezione Pulizia avanzata.".into()],
            ..Report::default()
        };
    }
    progress.visited.store(0, Ordering::Relaxed);
    progress.tree_entries.store(0, Ordering::Relaxed);
    progress.hashed_bytes.store(0, Ordering::Relaxed);
    progress.compared_bytes.store(0, Ordering::Relaxed);
    progress.phase.store(WALK, Ordering::Relaxed);
    let _local_only = match policy::LocalOnly::enter() {
        Ok(guard) => guard,
        Err(e) => {
            return Report {
                errors: 1,
                notes: vec![format!("Protezione dei file cloud non disponibile: {e}")],
                ..Report::default()
            }
        }
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let mut scan = Scan {
        req,
        stop,
        progress,
        report: Report::default(),
        directories: HashSet::new(),
        hardlinks: HashSet::new(),
        cutoff: now - 7 * 24 * 3600,
        space_totals: SpaceStats::default(),
    };
    for root in &req.roots {
        if cancelled(stop).is_err() {
            break;
        }
        if !policy::allowed_root(req.mode, root) {
            scan.error(
                root,
                io::Error::other("Cartella fuori dall'ambito consentito"),
            );
            continue;
        }
        let metadata = match policy::open_file(root, libc::O_EVTONLY | libc::O_DIRECTORY)
            .and_then(|f| f.metadata())
        {
            Ok(m) => m,
            Err(e) => {
                scan.error(root, e);
                continue;
            }
        };
        let mut safe = true;
        for ancestor in root.ancestors() {
            match policy::local_path(ancestor) {
                Ok(true) => {}
                Ok(false) => {
                    safe = false;
                    scan.report.skipped += 1;
                    break;
                }
                Err(e) => {
                    safe = false;
                    scan.error(ancestor, e);
                    break;
                }
            }
            if req.mode != Mode::Space && policy::package(ancestor) {
                safe = false;
                scan.report.skipped += 1;
                break;
            }
        }
        if !safe {
            continue;
        }
        let root_node = (metadata.dev(), metadata.ino());
        if req.mode == Mode::Cache && policy::cache_owner(root).is_some() {
            scan.tree(root, false, root_node);
        } else {
            scan.walk(root, 0, Some(root_node));
        }
    }
    if req.mode == Mode::Duplicates && cancelled(stop).is_ok() {
        scan.duplicates()
    }
    scan.report.cancelled = stop.load(Ordering::Relaxed);
    if scan.report.cancelled {
        scan.report.items.clear();
        scan.report.space.clear()
    }
    scan.report
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::symlink, process::Command};

    #[test]
    fn pinned_keeper_group_is_atomic_on_change_or_cancellation() {
        let _local_only = policy::LocalOnly::enter().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let req = Request {
            mode: Mode::Duplicates,
            roots: vec![base.clone()],
            min_bytes: 0,
        };
        for outcome in ["identical", "cancelled", "rewritten", "replaced"] {
            let make = |name: &str| {
                let path = base.join(name);
                fs::write(&path, b"same bytes").unwrap();
                assert!(policy::local_path(&path).unwrap());
                Item {
                    identity: Identity::read(&path).unwrap(),
                    path,
                    bytes: 10,
                    group: None,
                    cloud: false,
                }
            };
            let keeper = make("keeper");
            let path = keeper.path.clone();
            let copies = [make("a"), make("b")];
            let stop = AtomicBool::new(false);
            let progress = Progress::default();
            let mut worker = Scan {
                req: &req,
                stop: &stop,
                progress: &progress,
                report: Report::default(),
                directories: HashSet::new(),
                hardlinks: HashSet::new(),
                cutoff: 0,
                space_totals: SpaceStats::default(),
            };
            // The terminal iterator callback runs after both members compare
            // successfully and immediately before the final keeper validation.
            let iter = copies.into_iter().chain(std::iter::from_fn(|| {
                match outcome {
                    "cancelled" => stop.store(true, Ordering::Relaxed),
                    "rewritten" => fs::write(&path, b"new! bytes").unwrap(),
                    "replaced" => {
                        let replacement = base.join("replacement");
                        fs::write(&replacement, b"same bytes").unwrap();
                        fs::rename(replacement, &path).unwrap()
                    }
                    _ => {}
                }
                None
            }));
            worker.compare_group(keeper, iter, 7);
            assert_eq!(progress.compared_bytes.load(Ordering::Relaxed), 20);
            if outcome == "identical" {
                assert_eq!(worker.report.items.len(), 3);
                assert!(worker.report.items.iter().all(|i| i.group == Some(7)));
            } else {
                assert!(worker.report.items.is_empty());
                assert_eq!(worker.report.errors, u64::from(outcome != "cancelled"));
            }
        }
    }

    #[test]
    #[ignore = "set LINDO_DUPLICATE_BENCH_FIXTURE to an explicit synthetic dataset"]
    fn duplicate_benchmark() {
        let root = std::path::PathBuf::from(
            std::env::var_os("LINDO_DUPLICATE_BENCH_FIXTURE")
                .expect("synthetic fixture path required"),
        );
        let progress = Progress::default();
        let start = std::time::Instant::now();
        let report = run(
            &Request {
                mode: Mode::Duplicates,
                roots: vec![root],
                min_bytes: 0,
            },
            &AtomicBool::new(false),
            &progress,
        );
        assert_eq!(report.errors, 0, "{:?}", report.notes);
        println!(
            "elapsed={:.3}s visited={} grouped={} hashed={} compared={}",
            start.elapsed().as_secs_f64(),
            report.visited,
            report.items.len(),
            progress.hashed_bytes.load(Ordering::Relaxed),
            progress.compared_bytes.load(Ordering::Relaxed)
        );
    }
    // Exercise the actual root policy in an isolated process with a synthetic home.
    #[test]
    fn six_modes_on_synthetic_home() {
        let tmp = tempfile::Builder::new()
            .prefix("lindo-scan-test-")
            .tempdir()
            .unwrap();
        let home = tmp.path().canonicalize().unwrap();
        let result = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "scan::tests::fixture_worker",
                "--ignored",
                "--nocapture",
            ])
            .env("LINDO_FIXTURE_WORKER", &home)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{} {}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }
    #[test]
    #[ignore = "invoked by six_modes_on_synthetic_home with an isolated HOME"]
    fn fixture_worker() {
        let home = std::path::PathBuf::from(
            std::env::var_os("LINDO_FIXTURE_WORKER").expect("fixture home missing"),
        );
        assert_eq!(home, policy::home());
        let base = home.join("Dataset");
        fs::create_dir(&base).unwrap();
        let original = base.join("a.bin");
        fs::write(&original, b"same").unwrap();
        fs::write(base.join("b.bin"), b"same").unwrap();
        fs::write(base.join("c.bin"), b"diff").unwrap();
        fs::hard_link(&original, base.join("link.bin")).unwrap();
        symlink(&original, base.join("symlink.bin")).unwrap();
        for p in ["_6_Archivio_Private", "notes.pages", ".hidden"] {
            fs::create_dir(base.join(p)).unwrap();
            fs::write(base.join(p).join("data"), b"same").unwrap();
        }
        fs::write(base.join("remote.icloud"), b"same").unwrap();
        let scan = |mode, root: &Path| {
            let progress = Progress::default();
            let r = run(
                &Request {
                    mode,
                    roots: vec![root.to_owned()],
                    min_bytes: 1,
                },
                &AtomicBool::new(false),
                &progress,
            );
            assert_eq!(progress.visited.load(Ordering::Relaxed), r.visited);
            assert_eq!(r.errors, 0, "mode {mode:?} {:?}", r.notes);
            if mode == Mode::Duplicates {
                assert_eq!(progress.hashed_bytes.load(Ordering::Relaxed), 12);
                assert_eq!(progress.compared_bytes.load(Ordering::Relaxed), 4);
            }
            r
        };
        let large = scan(Mode::Large, &base);
        assert_eq!(large.items.len(), 3, "{large:?}");
        let duplicates = scan(Mode::Duplicates, &base);
        assert_eq!(duplicates.items.len(), 2);
        assert_eq!(duplicates.items[0].group, duplicates.items[1].group);
        assert!(same_contents(
            &duplicates.items[0],
            &duplicates.items[1],
            &AtomicBool::new(false)
        )
        .unwrap());
        let map = scan(Mode::Space, &base);
        assert!(map.items.iter().any(|i| i.path.ends_with(".hidden")));
        assert_eq!(
            map.space[&base.join(".hidden")].logical_bytes,
            fs::metadata(base.join(".hidden")).unwrap().len() + 4
        );
        assert_eq!(map.space[&base.join(".hidden")].files, 1);
        assert_eq!(map.space.values().map(|s| s.hardlinks).sum::<u64>(), 1);
        assert!(map
            .items
            .iter()
            .all(|i| !i.path.ends_with("_6_Archivio_Private")));
        assert_eq!(
            map.items
                .iter()
                .filter(|i| i.path.ends_with("a.bin") || i.path.ends_with("link.bin"))
                .filter(|i| i.bytes == 0)
                .count(),
            1
        );

        // Equal data forks are insufficient when the candidate has a resource fork.
        let fork_file = base.join("b.bin");
        let name = policy::name_c(&fork_file).unwrap();
        let fork = b"fixture";
        let set = unsafe {
            libc::setxattr(
                name.as_ptr(),
                c"com.apple.ResourceFork".as_ptr(),
                fork.as_ptr().cast(),
                fork.len(),
                0,
                0,
            )
        };
        assert_eq!(set, 0, "{}", io::Error::last_os_error());
        let fork_report = run(
            &Request {
                mode: Mode::Duplicates,
                roots: vec![base.clone()],
                min_bytes: 1,
            },
            &AtomicBool::new(false),
            &Progress::default(),
        );
        assert_eq!(fork_report.errors, 1);
        assert!(fork_report.items.is_empty());
        let alias = home.join("alias");
        symlink(&base, &alias).unwrap();
        let alias_report = run(
            &Request {
                mode: Mode::Large,
                roots: vec![alias],
                min_bytes: 1,
            },
            &AtomicBool::new(false),
            &Progress::default(),
        );
        assert!(alias_report.items.is_empty() && alias_report.errors == 1);

        let logs = home.join("Library/Logs");
        fs::create_dir_all(&logs).unwrap();
        let old = File::create(logs.join("old.log")).unwrap();
        old.set_modified(UNIX_EPOCH + std::time::Duration::from_secs(1))
            .unwrap();
        fs::write(logs.join("new.log"), b"recent").unwrap();
        assert_eq!(scan(Mode::Logs, &logs).items[0].path, logs.join("old.log"));

        let caches = home.join("Library/Caches");
        let unit = caches.join("Google/Chrome/Profile 2/Cache");
        fs::create_dir_all(&unit).unwrap();
        fs::write(unit.join("data"), b"1234567").unwrap();
        fs::create_dir_all(caches.join("Chromium/Default/Code Cache")).unwrap();
        let cache_report = scan(Mode::Cache, &caches);
        assert_eq!(cache_report.items.len(), 1);
        assert_eq!(cache_report.items[0].bytes, 7);

        for roots in [
            vec![caches.clone(), unit.clone()],
            vec![unit.clone(), caches.clone()],
        ] {
            let progress = Progress::default();
            let report = run(
                &Request {
                    mode: Mode::Cache,
                    roots,
                    min_bytes: 0,
                },
                &AtomicBool::new(false),
                &progress,
            );
            assert_eq!(report.items.len(), 1, "{:?}", report.notes);
            assert_eq!(report.items[0].path, unit);
            assert_eq!(report.visited, cache_report.visited);
            assert_eq!(progress.visited.load(Ordering::Relaxed), report.visited);
            assert_eq!(progress.tree_entries.load(Ordering::Relaxed), 0);
        }
        // Work inspected before rejecting an incomplete unit is still visible.
        symlink(unit.join("data"), unit.join("invalid-link")).unwrap();
        let progress = Progress::default();
        let rejected = run(
            &Request {
                mode: Mode::Cache,
                roots: vec![unit],
                min_bytes: 0,
            },
            &AtomicBool::new(false),
            &progress,
        );
        assert!(rejected.items.is_empty() && rejected.errors == 1);
        assert!(rejected.visited >= 2);
        assert_eq!(progress.visited.load(Ordering::Relaxed), rejected.visited);
        assert_eq!(progress.tree_entries.load(Ordering::Relaxed), 0);
        assert_eq!(progress.phase.load(Ordering::Relaxed), WALK);

        let apps = home.join("Applications");
        for (name, id) in [
            ("Fixture.app", "org.fixture.demo"),
            ("Apple.app", "CoM.AppLe.fixture"),
            ("Renamed.app", "COM.PIERNICOLADEMARIA.LINDO"),
        ] {
            let contents = apps.join(name).join("Contents");
            fs::create_dir_all(&contents).unwrap();
            fs::write(contents.join("Info.plist"),format!("<plist version=\"1.0\"><dict><key>CFBundleIdentifier</key><string>{id}</string></dict></plist>")).unwrap();
        }
        assert_eq!(scan(Mode::Apps, &apps).items.len(), 1);
        let stopped = run(
            &Request {
                mode: Mode::Large,
                roots: vec![base],
                min_bytes: 1,
            },
            &AtomicBool::new(true),
            &Progress::default(),
        );
        assert!(stopped.cancelled && stopped.items.is_empty());
    }
}
