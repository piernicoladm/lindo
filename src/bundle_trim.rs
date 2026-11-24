//! Stage, verify, and exchange a publisher-signed bundle without re-signing it.
#[path = "bundle_trim_files.rs"]
mod files;
#[path = "bundle_trim_locale.rs"]
mod locales;
use crate::{app_reset, model::Identity, policy, system_tools::command, trash};
use eframe::egui;
use objc2_foundation::{NSFileManager, NSURL};
use std::{
    collections::BTreeSet,
    ffi::CString,
    fs::{self, File},
    io::{self, Write},
    os::{
        fd::AsRawFd,
        unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    },
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    time::Duration,
};
type Result<T> = std::result::Result<T, String>;
fn tool(path: &str, args: &[&str], stop: &AtomicBool, timeout: u64) -> Result<String> {
    let marker = tempfile::tempfile().map_err(|e| e.to_string())?;
    let r = command::run(path, args, Some(marker), stop, timeout)?;
    if r.code != Some(0) || r.truncated {
        return Err(format!(
            "{}: {}",
            Path::new(path)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy(),
            r.text.chars().take(450).collect::<String>()
        ));
    }
    Ok(r.text)
}
fn as_str(p: &Path) -> Result<&str> {
    p.to_str()
        .ok_or_else(|| "Percorso non rappresentabile.".into())
}
fn verified(path: &Path, stop: &AtomicBool) -> Result<()> {
    tool(
        "/usr/bin/codesign",
        &[
            "--verify",
            "--all-architectures",
            "--deep",
            "--strict",
            as_str(path)?,
        ],
        stop,
        120,
    )
    .map(|_| ())
}
fn gate(path: &Path, stop: &AtomicBool) -> Result<()> {
    tool(
        "/usr/sbin/spctl",
        &[
            "--assess",
            "--type",
            "execute",
            "--verbose=2",
            as_str(path)?,
        ],
        stop,
        90,
    )
    .map(|_| ())
}
fn quarantine(p: &Path) -> Result<Option<Vec<u8>>> {
    let file = policy::open_file(p, libc::O_EVTONLY).map_err(|e| e.to_string())?;
    let name = c"com.apple.quarantine";
    let fd = file.as_raw_fd();
    let size = unsafe { libc::fgetxattr(fd, name.as_ptr(), std::ptr::null_mut(), 0, 0, 0) };
    if size < 0 {
        let e = io::Error::last_os_error();
        return if e.raw_os_error() == Some(libc::ENOATTR) {
            Ok(None)
        } else {
            Err(e.to_string())
        };
    }
    if size > 65536 {
        return Err("Quarantena non verificabile.".into());
    }
    let mut value = vec![0u8; size as usize];
    if unsafe {
        libc::fgetxattr(
            fd,
            name.as_ptr(),
            value.as_mut_ptr().cast(),
            value.len(),
            0,
            0,
        )
    } != size
    {
        return Err("Quarantena cambiata.".into());
    }
    Ok(Some(value))
}
fn capacity(dir: &File) -> Result<u64> {
    let mut st = std::mem::MaybeUninit::<libc::statfs>::uninit();
    if unsafe { libc::fstatfs(dir.as_raw_fd(), st.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error().to_string());
    }
    let st = unsafe { st.assume_init() };
    Ok(st.f_bavail.saturating_mul(st.f_bsize as u64))
}
fn private_base(home: &Path) -> Result<(PathBuf, File)> {
    let mut path = home.join("Library/Application Support");
    let mut dir = policy::open_file(&path, libc::O_DIRECTORY).map_err(|e| e.to_string())?;
    for component in ["Lindo", "BundleTrim"] {
        let name = CString::new(component).unwrap();
        if unsafe { libc::mkdirat(dir.as_raw_fd(), name.as_ptr(), 0o700) } != 0 {
            let e = io::Error::last_os_error();
            if e.kind() != io::ErrorKind::AlreadyExists {
                return Err(e.to_string());
            }
        }
        path.push(component);
        dir = policy::open_file(&path, libc::O_DIRECTORY).map_err(|e| e.to_string())?;
        let m = dir.metadata().map_err(|e| e.to_string())?;
        if m.uid() != unsafe { libc::geteuid() }
            || m.mode() & (if component == "Lindo" { 0o022 } else { 0o077 }) != 0
        {
            return Err("Cartella di staging non privata.".into());
        }
    }
    Ok((path, dir))
}
fn lock(base: &Path) -> Result<File> {
    let p = base.join(".exchange.lock");
    let f = File::options()
        .write(true)
        .read(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(0x20000000)
        .open(p)
        .map_err(|e| e.to_string())?;
    let m = f.metadata().map_err(|e| e.to_string())?;
    if !m.is_file()
        || m.nlink() != 1
        || m.uid() != unsafe { libc::geteuid() }
        || m.mode() & 0o077 != 0
    {
        return Err("Lock non valido.".into());
    }
    if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err("Un altro scambio è in corso.".into());
    }
    Ok(f)
}
fn writable(p: &Path) -> Result<()> {
    let parent = p.parent().ok_or("Cartella assente.")?;
    let m = policy::open_file(p, libc::O_DIRECTORY)
        .and_then(|f| f.metadata())
        .map_err(|e| e.to_string())?;
    if m.uid() != unsafe { libc::geteuid() }
        || [p, parent].iter().any(|v| {
            !policy::name_c(v)
                .is_ok_and(|s| unsafe { libc::access(s.as_ptr(), libc::W_OK | libc::X_OK) } == 0)
        })
    {
        return Err(
            "Servono un bundle di tua proprietà e una cartella Applicazioni scrivibile.".into(),
        );
    }
    Ok(())
}
fn dev_region(p: &Path, stop: &AtomicBool) -> Result<String> {
    let plist = p.join("Contents/Info.plist");
    tool(
        "/usr/bin/plutil",
        &[
            "-extract",
            "CFBundleDevelopmentRegion",
            "raw",
            "-o",
            "-",
            as_str(&plist)?,
        ],
        stop,
        10,
    )
}
struct Plan {
    source: PathBuf,
    id: String,
    team: String,
    cdhash: String,
    tree: Identity,
    manifest: files::Manifest,
    locales: Vec<locales::Locale>,
    dev: String,
    quarantine: Option<Vec<u8>>,
    developer_id: bool,
}
fn plan(source: &Path, stop: &AtomicBool) -> Result<Plan> {
    let _guard = policy::LocalOnly::enter().map_err(|e| e.to_string())?;
    if !source.is_absolute()
        || !(source.parent() == Some(Path::new("/Applications"))
            || source.parent() == Some(policy::home().join("Applications").as_path()))
        || !source
            .extension()
            .is_some_and(|v| v.eq_ignore_ascii_case("app"))
        || source.canonicalize().ok().as_deref() != Some(source)
    {
        return Err("Scegli un bundle direttamente in Applicazioni o ~/Applications.".into());
    }
    writable(source)?;
    let (tree, _, _) = Identity::tree_cancel(source, true, stop).map_err(|e| e.to_string())?;
    let manifest = files::manifest(source, stop)?;
    let (id, team, cdhash) = app_reset::verified_bundle_for_trim(source, stop)?;
    verified(source, stop)?;
    let info = tool(
        "/usr/bin/codesign",
        &["--display", "--verbose=4", as_str(source)?],
        stop,
        20,
    )?;
    let developer_id = info
        .lines()
        .any(|s| s.starts_with("Authority=Developer ID Application:"));
    if developer_id {
        gate(source, stop)?;
    }
    let dev = dev_region(source, stop).unwrap_or_default();
    let locales = locales::list(source, &id, &dev)?;
    if !matches_tree(source, &tree, stop)? {
        return Err("Bundle cambiato durante l’analisi.".into());
    }
    Ok(Plan {
        source: source.into(),
        id,
        team,
        cdhash,
        tree,
        manifest,
        locales,
        dev,
        quarantine: quarantine(source)?,
        developer_id,
    })
}
fn matches_tree(p: &Path, id: &Identity, stop: &AtomicBool) -> Result<bool> {
    Ok(Identity::tree_cancel(p, true, stop)
        .map_err(|e| e.to_string())?
        .0
        == *id)
}
#[derive(Clone, Default)]
struct Selection {
    intel: bool,
    locales: BTreeSet<String>,
}
fn approved(plan: &Plan, selection: &Selection) -> Result<BTreeSet<PathBuf>> {
    if !selection.intel && selection.locales.is_empty() {
        return Err("Seleziona almeno una voce.".into());
    }
    if selection.intel && (!cfg!(target_arch = "aarch64") || !plan.manifest.removable_intel) {
        return Err("Nessuna slice Intel rimovibile su questo Mac.".into());
    }
    let active = locales::list(&plan.source, &plan.id, &plan.dev)?;
    let mut removed = BTreeSet::new();
    for n in &selection.locales {
        if !plan.locales.iter().any(|l| &l.name == n && !l.protected)
            || !active.iter().any(|l| &l.name == n && !l.protected)
        {
            return Err("Lingua protetta, cambiata o non presente.".into());
        }
        removed.insert(Path::new("Contents/Resources").join(format!("{n}.lproj")));
    }
    Ok(removed)
}
fn copy_variant(
    src: &Path,
    dst: &Path,
    thin: bool,
    removed: &BTreeSet<PathBuf>,
    stop: &AtomicBool,
) -> Result<()> {
    let mut args = vec!["--rsrc", "--extattr", "--acl"];
    if thin {
        args.extend(["--arch", "arm64", "--arch", "arm64e"]);
    }
    args.extend([as_str(src)?, as_str(dst)?]);
    tool("/usr/bin/ditto", &args, stop, 900)?;
    for relative in removed {
        if relative.parent() != Some(Path::new("Contents/Resources"))
            || relative.extension().is_none_or(|s| s != "lproj")
        {
            return Err("Localizzazione non valida.".into());
        }
        let dir = dst.join(relative);
        let pinned = policy::open_file(&dir, libc::O_DIRECTORY).map_err(|e| e.to_string())?;
        let m = pinned.metadata().map_err(|e| e.to_string())?;
        if !m.is_dir() || m.uid() != unsafe { libc::geteuid() } {
            return Err("Copia locale non verificata.".into());
        }
        // Only remove a selected top-level localization from the disposable new copy.
        fs::remove_dir_all(&dir).map_err(|e| e.to_string())?;
    }
    Ok(())
}
fn publisher(plan: &Plan, path: &Path, stop: &AtomicBool) -> Result<()> {
    verified(path, stop)?;
    let req=format!("identifier \"{}\" and anchor apple generic and certificate leaf[subject.OU] = \"{}\" and (certificate leaf[field.1.2.840.113635.100.6.1.13] exists or certificate leaf[field.1.2.840.113635.100.6.1.9] exists)",plan.id,plan.team);
    tool(
        "/usr/bin/codesign",
        &[
            "--verify",
            "--all-architectures",
            "--strict",
            "-R",
            &req,
            as_str(path)?,
        ],
        stop,
        90,
    )?;
    let details = tool(
        "/usr/bin/codesign",
        &["--display", "--verbose=4", as_str(path)?],
        stop,
        20,
    )?;
    if policy::app_identifier(path).map_err(|e| e.to_string())? != plan.id
        || !details
            .lines()
            .any(|s| s.strip_prefix("CDHash=") == Some(&plan.cdhash))
        || quarantine(path)? != plan.quarantine
    {
        return Err("Identità, firma dell’architettura nativa o quarantena cambiate.".into());
    }
    if plan.developer_id {
        gate(path, stop)?;
    }
    Ok(())
}
struct Prepared {
    plan: Arc<Plan>,
    selection: Selection,
    path: PathBuf,
    tree: Identity,
    logical: u64,
    allocated: u64,
}
fn prepare(plan: Arc<Plan>, selection: Selection, stop: &AtomicBool) -> Result<Prepared> {
    let _guard = policy::LocalOnly::enter().map_err(|e| e.to_string())?;
    let removed = approved(&plan, &selection)?;
    if !matches_tree(&plan.source, &plan.tree, stop)? {
        return Err("L’app originale è cambiata.".into());
    }
    trash::check_running(&plan.source, &plan.id, false)?;
    let (base, dir) = private_base(&policy::home())?;
    if capacity(&dir)? < plan.manifest.logical.saturating_add(128 * 1024 * 1024) {
        return Err(
            "Spazio insufficiente per preparare una copia e conservare l’originale.".into(),
        );
    }
    if dir.metadata().map_err(|e| e.to_string())?.dev()
        != fs::symlink_metadata(&plan.source)
            .map_err(|e| e.to_string())?
            .dev()
    {
        return Err("Lo staging e l’app devono trovarsi sullo stesso volume.".into());
    }
    let tmp = tempfile::Builder::new()
        .prefix("riduzione-")
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir_in(base)
        .map_err(|e| e.to_string())?;
    let dst = tmp
        .path()
        .join(plan.source.file_name().ok_or("Nome app assente.")?);
    copy_variant(&plan.source, &dst, selection.intel, &removed, stop)?;
    let candidate = files::manifest(&dst, stop)?;
    files::compare(&plan.manifest, &candidate, selection.intel, &removed)?;
    publisher(&plan, &dst, stop).map_err(|e| {
        format!("La copia non supera firma e verifica dell’editore. Originale intatto. {e}")
    })?;
    if candidate.logical >= plan.manifest.logical {
        return Err("Nessuna riduzione logica verificabile.".into());
    }
    if !matches_tree(&plan.source, &plan.tree, stop)? {
        return Err("L’app originale è cambiata durante la copia.".into());
    }
    let tree = Identity::tree_cancel(&dst, true, stop)
        .map_err(|e| e.to_string())?
        .0;
    let instructions=format!("Recupero del bundle {}\n\nApp installata: {}\nCopia conservata: {}\nEditore: {}\n\nPrima dello scambio questa cartella contiene la copia ridotta. Dopo lo scambio contiene l’originale recuperabile. Lindo verifica le identità dei bundle durante ogni scambio.\nIdentità iniziale originale (volume, inode): {:?}\nIdentità iniziale copia ridotta (volume, inode): {:?}\n\nNella sessione di installazione puoi rivedere il ripristino o scegliere di spostare il backup nel Cestino. Dal Finder, Rimetti a posto riporta un backup cestinato in questa cartella.\n",plan.id,plan.source.display(),dst.display(),plan.team,plan.tree.node(),tree.node());
    let mut receipt = File::options()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(tmp.path().join("RECUPERO.txt"))
        .map_err(|e| e.to_string())?;
    receipt
        .write_all(instructions.as_bytes())
        .and_then(|_| receipt.sync_all())
        .map_err(|e| e.to_string())?;
    if stop.load(Ordering::Acquire) {
        return Err("Preparazione interrotta.".into());
    }
    let _retained = tmp.keep();
    Ok(Prepared {
        plan,
        selection,
        path: dst,
        tree,
        logical: candidate.logical,
        allocated: candidate.allocated,
    })
}
type Key = (u64, u64);
fn at_key(dir: &File, name: &CString) -> Result<Key> {
    let mut s = std::mem::MaybeUninit::<libc::stat>::uninit();
    if unsafe {
        libc::fstatat(
            dir.as_raw_fd(),
            name.as_ptr(),
            s.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } != 0
    {
        return Err(io::Error::last_os_error().to_string());
    }
    let s = unsafe { s.assume_init() };
    if s.st_mode & libc::S_IFMT != libc::S_IFDIR {
        return Err("Bundle sostituito.".into());
    }
    Ok((s.st_dev as u64, s.st_ino))
}
fn exchange(left: &Path, right: &Path, left_key: Key, right_key: Key) -> Result<()> {
    unsafe extern "C" {
        fn renameatx_np(a: i32, b: *const i8, c: i32, d: *const i8, flags: u32) -> i32;
    }
    let open = |p: &Path| -> Result<(File, CString)> {
        let dir = policy::open_file(p.parent().ok_or("Cartella assente.")?, libc::O_DIRECTORY)
            .map_err(|e| e.to_string())?;
        let name = policy::name_c(Path::new(p.file_name().ok_or("Nome assente.")?))
            .map_err(|e| e.to_string())?;
        Ok((dir, name))
    };
    let (a, n) = open(left)?;
    let (b, m) = open(right)?;
    if left_key.0 != right_key.0 || at_key(&a, &n)? != left_key || at_key(&b, &m)? != right_key {
        return Err("Percorso o volume cambiato prima dello scambio.".into());
    }
    if unsafe { renameatx_np(a.as_raw_fd(), n.as_ptr(), b.as_raw_fd(), m.as_ptr(), 2) } != 0 {
        return Err(io::Error::last_os_error().to_string());
    }
    if at_key(&a, &n)? != right_key || at_key(&b, &m)? != left_key {
        return Err(format!(
            "Scambio non verificabile. Controlla {} e {}.",
            left.display(),
            right.display()
        ));
    }
    Ok(())
}
struct Receipt {
    prepared: Arc<Prepared>,
    live: Identity,
    backup: Identity,
    free: u64,
}
fn checked_exchange(prepared: &Prepared, stop: &AtomicBool) -> Result<(Identity, Identity, u64)> {
    let _guard = policy::LocalOnly::enter().map_err(|e| e.to_string())?;
    let p = &prepared.plan;
    let (base, dir) = private_base(&policy::home())?;
    let _lock = lock(&base)?;
    valid_stage(&base, &p.source, &prepared.path)?;
    writable(&p.source)?;
    approved(p, &prepared.selection)?;
    let (id, team, hash) = app_reset::verified_bundle_for_trim(&p.source, stop)?;
    if (id, team, hash) != (p.id.clone(), p.team.clone(), p.cdhash.clone()) {
        return Err("Editore o app cambiati.".into());
    }
    publisher(p, &prepared.path, stop)?;
    if !matches_tree(&prepared.path, &prepared.tree, stop)?
        || !matches_tree(&p.source, &p.tree, stop)?
    {
        return Err("Una delle copie è cambiata. Ripeti l’analisi.".into());
    }
    trash::check_running(&p.source, &p.id, false)?;
    let free = capacity(&dir)?;
    verify_exchange(
        &p.source,
        &prepared.path,
        p.tree.node(),
        prepared.tree.node(),
        || trash::check_running(&p.source, &p.id, false),
        || {
            let live = Identity::tree_cancel(&p.source, true, stop)
                .map_err(|e| e.to_string())?
                .0;
            let backup = Identity::tree_cancel(&prepared.path, true, stop)
                .map_err(|e| e.to_string())?
                .0;
            publisher(p, &p.source, stop)?;
            if live.node() != prepared.tree.node() || backup.node() != p.tree.node() {
                return Err("Identità dopo lo scambio non verificata.".into());
            }
            Ok((live, backup, capacity(&dir).unwrap_or(free)))
        },
    )
}
fn verify_exchange<T>(
    left: &Path,
    right: &Path,
    left_key: Key,
    right_key: Key,
    closed: impl Fn() -> Result<()>,
    verify: impl FnOnce() -> Result<T>,
) -> Result<T> {
    closed()?;
    exchange(left, right, left_key, right_key)?;
    match verify() {
        Ok(out) => Ok(out),
        Err(error) => {
            if closed().is_ok() && exchange(left, right, right_key, left_key).is_ok() {
                Err(format!(
                    "Verifica successiva fallita. Originale rimesso al suo posto. {error}"
                ))
            } else {
                Err(format!(
                    "Controlla il bundle installato e la copia recuperabile in {}. {error}",
                    right.display()
                ))
            }
        }
    }
}

fn restore(r: &Receipt, stop: &AtomicBool) -> Result<String> {
    let _guard = policy::LocalOnly::enter().map_err(|e| e.to_string())?;
    let p = &r.prepared;
    let (base, _) = private_base(&policy::home())?;
    let _lock = lock(&base)?;
    valid_stage(&base, &p.plan.source, &p.path)?;
    if !matches_tree(&p.plan.source, &r.live, stop)? || !matches_tree(&p.path, &r.backup, stop)? {
        return Err("App installata o backup cambiati. Ripristino automatico bloccato.".into());
    }
    publisher(&p.plan, &p.path, stop)?;
    trash::check_running(&p.plan.source, &p.plan.id, false)?;
    exchange(&p.plan.source, &p.path, r.live.node(), r.backup.node())?;
    Ok(format!(
        "Originale ripristinato. La copia ridotta è conservata in {}.",
        p.path.display()
    ))
}
fn valid_stage(base: &Path, live: &Path, backup: &Path) -> Result<()> {
    let directory = backup.parent().ok_or("Cartella di backup assente.")?;
    let suffix = directory
        .file_name()
        .and_then(|s| s.to_str())
        .and_then(|s| s.strip_prefix("riduzione-"));
    if directory.parent() != Some(base)
        || backup.file_name() != live.file_name()
        || !suffix.is_some_and(|s| !s.is_empty() && s.bytes().all(|c| c.is_ascii_alphanumeric()))
    {
        return Err("Il backup deve trovarsi nella cartella privata preparata da Lindo.".into());
    }
    let m = policy::open_file(directory, libc::O_DIRECTORY)
        .and_then(|f| f.metadata())
        .map_err(|e| e.to_string())?;
    if m.uid() != unsafe { libc::geteuid() } || m.mode() & 0o077 != 0 {
        return Err("Permessi della cartella di backup cambiati.".into());
    }
    Ok(())
}
struct TrashOutcome {
    destination: Option<PathBuf>,
    verified: bool,
}
fn trash_verified_backup(
    base: &Path,
    (live, active): (&Path, &Identity),
    (backup, original): (&Path, &Identity),
    stop: &AtomicBool,
    validate: impl FnOnce() -> Result<()>,
    closed: impl FnOnce() -> Result<()>,
) -> Result<TrashOutcome> {
    valid_stage(base, live, backup)?;
    if !matches_tree(live, active, stop)? || !matches_tree(backup, original, stop)? {
        return Err("Bundle installato o backup cambiato. Spostamento bloccato.".into());
    }
    validate()?;
    if !matches_tree(backup, original, stop)? || !matches_tree(live, active, stop)? {
        return Err("Bundle cambiato durante la verifica.".into());
    }
    closed()?;
    let destination = objc2::rc::autoreleasepool(|_| {
        let url = NSURL::from_directory_path(backup).ok_or("Percorso di backup non valido.")?;
        let mut destination = None;
        NSFileManager::defaultManager()
            .trashItemAtURL_resultingItemURL_error(&url, Some(&mut destination))
            .map_err(|err| {
                format!(
                    "Spostamento non confermato da macOS. {} · Controlla {}",
                    err.localizedDescription(),
                    backup.display()
                )
            })?;
        Ok::<_, String>(destination.and_then(|u| u.to_file_path()))
    })?;
    let verified = destination.as_ref().is_some_and(|p| {
        p.components()
            .any(|component| matches!(component.as_os_str().to_str(), Some(".Trash" | ".Trashes")))
            && fs::symlink_metadata(p)
                .is_ok_and(|m| m.is_dir() && (m.dev(), m.ino()) == original.node())
    }) && fs::symlink_metadata(backup)
        .is_err_and(|e| e.kind() == io::ErrorKind::NotFound)
        && matches_tree(live, active, stop).unwrap_or(false);
    Ok(TrashOutcome {
        destination,
        verified,
    })
}
fn retire(r: &Receipt, stop: &AtomicBool) -> Result<String> {
    let _guard = policy::LocalOnly::enter().map_err(|e| e.to_string())?;
    let p = &r.prepared;
    let (base, _) = private_base(&policy::home())?;
    let _lock = lock(&base)?;
    let out = trash_verified_backup(
        &base,
        (&p.plan.source, &r.live),
        (&p.path, &r.backup),
        stop,
        || {
            publisher(&p.plan, &p.plan.source, stop)?;
            publisher(&p.plan, &p.path, stop)
        },
        || trash::check_running(&p.plan.source, &p.plan.id, false),
    )?;
    let destination = out
        .destination
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "non comunicata da macOS".into());
    if !out.verified {
        return Ok(format!("macOS ha accettato lo spostamento; verifica posizione e bundle nel Finder. Destinazione: {destination}."));
    }
    Ok(format!("Backup originale nel Cestino: {destination}. Il Finder può rimetterlo nella cartella di staging. Lo spazio diventa disponibile dopo lo svuotamento del Cestino. Riduzione logica rispetto all’app iniziale: {}.",mb(p.plan.manifest.logical-p.logical)))
}
fn open_archive(stop: &AtomicBool) -> Result<String> {
    let base = policy::home().join("Library/Application Support/Lindo/BundleTrim");
    let dir = policy::open_file(&base, libc::O_DIRECTORY).map_err(|e| {
        if e.kind() == io::ErrorKind::NotFound {
            "Nessuna copia preparata.".into()
        } else {
            e.to_string()
        }
    })?;
    let m = dir.metadata().map_err(|e| e.to_string())?;
    if m.uid() != unsafe { libc::geteuid() } || m.mode() & 0o077 != 0 {
        return Err("Permessi di staging non validi.".into());
    }
    tool("/usr/bin/open", &[as_str(&base)?], stop, 10)?;
    Ok(format!(
        "Archivio aperto nel Finder. Ogni cartella di staging contiene RECUPERO.txt: {}.",
        base.display()
    ))
}
#[derive(Default, PartialEq, Eq)]
enum Confirmation {
    #[default]
    None,
    Install,
    Restore,
    Trash,
}
enum Update {
    Plan(Arc<Plan>),
    Staged(Arc<Prepared>),
    Installed(Arc<Receipt>),
    Closed(String),
    Message(String),
}
struct Job {
    stop: Arc<AtomicBool>,
    rx: mpsc::Receiver<Result<Update>>,
    mutation: bool,
}
impl Drop for Job {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}
#[derive(Default)]
pub(crate) struct BundleTrim {
    input: String,
    plan: Option<Arc<Plan>>,
    pick: Selection,
    prepared: Option<Arc<Prepared>>,
    installed: Option<Arc<Receipt>>,
    confirm: Confirmation,
    job: Option<Job>,
    notice: String,
}
fn mb(n: u64) -> String {
    format!("{:.2} MB", n as f64 / 1e6)
}
impl BundleTrim {
    fn work(
        &mut self,
        ctx: &egui::Context,
        mutation: bool,
        f: impl FnOnce(&AtomicBool) -> Result<Update> + Send + 'static,
    ) {
        self.notice.clear();
        let (tx, rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        self.job = Some(Job {
            rx,
            stop: stop.clone(),
            mutation,
        });
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(f(&stop));
            ctx.request_repaint();
        });
    }
    fn clear(&mut self) {
        self.plan = None;
        self.pick = Selection::default();
        self.prepared = None;
        self.installed = None;
        self.confirm = Confirmation::None;
        self.notice.clear();
    }
    pub(crate) fn show(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        if let Some(job) = &self.job {
            match job.rx.try_recv() {
                Ok(result) => {
                    self.job = None;
                    self.confirm = Confirmation::None;
                    match result {
                        Err(e) => self.notice = e,
                        Ok(Update::Plan(p)) => {
                            self.pick = Selection::default();
                            self.plan = Some(p);
                        }
                        Ok(Update::Staged(p)) => self.prepared = Some(p),
                        Ok(Update::Installed(r)) => self.installed = Some(r),
                        Ok(Update::Message(s)) => self.notice = s,
                        Ok(Update::Closed(s)) => {
                            self.clear();
                            self.notice = s;
                        }
                    }
                }
                Err(mpsc::TryRecvError::Empty) => {
                    ui.spinner();
                    if !job.mutation && ui.button("Interrompi verifica").clicked() {
                        job.stop.store(true, Ordering::Release);
                    }
                    ctx.request_repaint_after(Duration::from_millis(180));
                    return;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.job = None;
                    self.confirm = Confirmation::None;
                    self.notice = "Verifica interrotta. Controlla lo staging.".into();
                }
            }
        }
        if !self.notice.is_empty() {
            ui.label(&self.notice);
        }
        if let Some(r) = self.installed.clone() {
            ui.heading("Copia verificata installata");
            ui.label(format!(
                "Originale recuperabile: {}",
                r.prepared.path.display()
            ));
            ui.label(format!(
                "Spazio libero misurato sul volume: {}. Lo scambio conserva entrambi i bundle.",
                mb(r.free)
            ));
            match self.confirm {
                Confirmation::Restore => {
                    ui.label(
                        "Ripristinare l’originale e conservare la copia ridotta nello staging?",
                    );
                    if ui.button("Conferma ripristino originale").clicked() {
                        self.work(ctx, true, move |stop| restore(&r, stop).map(Update::Closed));
                    }
                    if ui.button("Indietro").clicked() {
                        self.confirm = Confirmation::None;
                    }
                }
                Confirmation::Trash => {
                    ui.label(format!("Spostare nel Cestino il solo originale di {}? Percorso: {}. Il bundle ridotto resta installato. Per recuperare l’originale usa Rimetti a posto nel Finder.",mb(r.prepared.plan.manifest.logical),r.prepared.path.display()));
                    if ui.button("Conferma spostamento del backup").clicked() {
                        self.work(ctx, true, move |stop| retire(&r, stop).map(Update::Closed));
                    }
                    if ui.button("Indietro").clicked() {
                        self.confirm = Confirmation::None;
                    }
                }
                _ => {
                    if ui.button("Rivedi ripristino originale").clicked() {
                        self.confirm = Confirmation::Restore;
                    }
                    if ui.button("Sposta backup originale nel Cestino").clicked() {
                        self.confirm = Confirmation::Trash;
                    }
                    if ui.button("Torna alla scelta dell’app").clicked() {
                        self.clear();
                    }
                }
            }
            return;
        }
        if let Some(p) = self.prepared.clone() {
            ui.heading("Copia ridotta verificata");
            ui.label(format!(
                "Installata: {} · Staging: {}",
                p.plan.source.display(),
                p.path.display()
            ));
            ui.label(format!(
                "Dimensioni logiche: {} → {}. Riduzione: {}.",
                mb(p.plan.manifest.logical),
                mb(p.logical),
                mb(p.plan.manifest.logical - p.logical)
            ));
            ui.label(format!(
                "Blocchi allocati dei file: {} → {}. Il backup completo rimarrà nello staging.",
                mb(p.plan.manifest.allocated),
                mb(p.allocated)
            ));
            if self.confirm == Confirmation::Install {
                ui.label(
                    "Scambiare la copia verificata con l’app originale? Mantieni l’app chiusa.",
                );
                if ui.button("Conferma installazione della copia").clicked() {
                    self.work(ctx, true, move |stop| {
                        let (live, backup, free) = checked_exchange(&p, stop)?;
                        Ok(Update::Installed(Arc::new(Receipt {
                            prepared: p,
                            live,
                            backup,
                            free,
                        })))
                    });
                }
                if ui.button("Indietro").clicked() {
                    self.confirm = Confirmation::None;
                }
            } else {
                if ui.button("Rivedi installazione").clicked() {
                    self.confirm = Confirmation::Install;
                }
                if ui.button("Conserva la copia senza installare").clicked() {
                    let notice = format!("Copia verificata conservata in {}.", p.path.display());
                    self.clear();
                    self.notice = notice;
                }
            }
            return;
        }
        ui.label("Scegli un’app firmata di terzi, chiusa e di tua proprietà. Lindo prepara una copia e verifica la firma originale prima di proporre l’installazione.");
        if let Some(path) = ctx.input(|i| i.raw.dropped_files.iter().find_map(|f| f.path.clone())) {
            self.clear();
            self.input = path.to_string_lossy().into_owned();
        }
        if ui
            .add(egui::TextEdit::singleline(&mut self.input).hint_text("/Applications/Nome.app"))
            .changed()
        {
            self.clear();
        }
        if ui
            .button("Mostra archivio delle copie nel Finder")
            .clicked()
        {
            self.work(ctx, false, |stop| open_archive(stop).map(Update::Message));
            return;
        }
        if ui.button("Analizza bundle scelto").clicked() {
            self.clear();
            let path = if let Some(v) = self.input.trim().strip_prefix("~/") {
                policy::home().join(v)
            } else {
                PathBuf::from(self.input.trim())
            };
            self.work(ctx, false, move |stop| {
                plan(&path, stop).map(Arc::new).map(Update::Plan)
            });
            return;
        }
        let Some(p) = self.plan.clone() else {
            return;
        };
        ui.label(format!(
            "{} · Team {} · {} logici",
            p.id,
            p.team,
            mb(p.manifest.logical)
        ));
        ui.add_enabled(
            p.manifest.removable_intel && cfg!(target_arch = "aarch64"),
            egui::Checkbox::new(
                &mut self.pick.intel,
                "Rimuovi le slice Intel conservando tutte le slice ARM64",
            ),
        );
        ui.label("Scegli le localizzazioni. Lingue attive, Base e lingua di sviluppo sono protette. Le risorse obbligatorie per la firma saranno rifiutate.");
        ui.add_enabled_ui(self.pick.intel || !self.pick.locales.is_empty(), |ui| {
            if ui.button("Prepara e verifica la copia").clicked() {
                let sel = self.pick.clone();
                let p = p.clone();
                self.work(ctx, false, move |stop| {
                    prepare(p, sel, stop).map(Arc::new).map(Update::Staged)
                });
            }
        });
        egui::ScrollArea::vertical()
            .id_salt("bundle_trim_languages")
            .show_rows(ui, 22., p.locales.len(), |ui, range| {
                for i in range {
                    let lang = &p.locales[i];
                    let mut checked = self.pick.locales.contains(&lang.name);
                    let name = if lang.protected {
                        format!("{} · protetta", lang.name)
                    } else {
                        lang.name.clone()
                    };
                    if ui
                        .add_enabled(!lang.protected, egui::Checkbox::new(&mut checked, name))
                        .changed()
                    {
                        if checked {
                            self.pick.locales.insert(lang.name.clone());
                        } else {
                            self.pick.locales.remove(&lang.name);
                        }
                    }
                }
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{os::unix::fs::symlink, process::Command};
    fn run(tool: &str, args: &[&str]) -> String {
        let r = Command::new(tool).args(args).output().unwrap();
        assert!(
            r.status.success(),
            "{tool}: {}",
            String::from_utf8_lossy(&r.stderr)
        );
        String::from_utf8_lossy(&r.stdout).trim().to_owned()
    }
    pub(super) fn synthetic(base: &Path) -> PathBuf {
        let app = base.join("SyntheticFixture.app");
        let contents = app.join("Contents");
        let mac = contents.join("MacOS");
        fs::create_dir_all(&mac).unwrap();
        for name in ["Base", "en", "ja"] {
            let folder = contents.join("Resources").join(format!("{name}.lproj"));
            fs::create_dir_all(&folder).unwrap();
            fs::write(folder.join("sample.strings"), b"\"sample\" = \"fixture\";").unwrap();
        }
        fs::write(
            contents.join("Resources/required.bin"),
            b"sealed fixture resource",
        )
        .unwrap();
        fs::write(
            contents.join("Info.plist"),
            br#"<?xml version="1.0"?><plist version="1.0"><dict>
    <key>CFBundleIdentifier</key><string>org.example.synthetic-trim-fixture</string>
    <key>CFBundleName</key><string>SyntheticFixture</string>
    <key>CFBundleExecutable</key><string>Fixture</string>
    <key>CFBundleDevelopmentRegion</key><string>en</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleVersion</key><string>1</string>
    </dict></plist>"#,
        )
        .unwrap();
        let src = base.join("fixture.c");
        fs::write(&src, b"int main(void){ return 0; }").unwrap();
        for arch in ["arm64", "x86_64"] {
            run(
                "/usr/bin/clang",
                &[
                    "-arch",
                    arch,
                    as_str(&src).unwrap(),
                    "-o",
                    as_str(&base.join(arch)).unwrap(),
                ],
            );
        }
        let main = mac.join("Fixture");
        run(
            "/usr/bin/lipo",
            &[
                "-create",
                as_str(&base.join("arm64")).unwrap(),
                as_str(&base.join("x86_64")).unwrap(),
                "-output",
                as_str(&main).unwrap(),
            ],
        );
        run(
            "/usr/bin/codesign",
            &["--sign", "-", "--timestamp=none", as_str(&app).unwrap()],
        );
        app
    }
    #[test]
    fn universal_fixture_preserves_signed_arm_code_and_exchanges_atomically() {
        let tmp = tempfile::Builder::new()
            .prefix("lindo-bundle-trim-")
            .tempdir()
            .unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let _guard = policy::LocalOnly::enter().unwrap();
        let stop = AtomicBool::new(false);
        let src = synthetic(&base);
        let dst = base.join("StagedFixture.app");
        verified(&src, &stop).unwrap();
        let old = files::manifest(&src, &stop).unwrap();
        assert!(old.removable_intel);
        copy_variant(&src, &dst, true, &BTreeSet::new(), &stop).unwrap();
        let new = files::manifest(&dst, &stop).unwrap();
        files::compare(&old, &new, true, &BTreeSet::new()).unwrap();
        verified(&dst, &stop).unwrap();
        assert!(new.logical < old.logical);
        let bin = |r: &Path| r.join("Contents/MacOS/Fixture");
        assert_eq!(
            run("/usr/bin/lipo", &["-archs", as_str(&bin(&dst)).unwrap()]),
            "arm64"
        );
        let source_key = Identity::read(&src).unwrap().node();
        let stage_key = Identity::read(&dst).unwrap().node();
        assert!(verify_exchange(
            &src,
            &dst,
            source_key,
            stage_key,
            || Ok(()),
            || Err::<(), _>("synthetic check failed".into())
        )
        .unwrap_err()
        .contains("rimesso"));
        assert_eq!(Identity::read(&src).unwrap().node(), source_key);
        verify_exchange(
            &src,
            &dst,
            source_key,
            stage_key,
            || Ok(()),
            || verified(&src, &stop),
        )
        .unwrap();
        assert_eq!(Identity::read(&dst).unwrap().node(), source_key);
        assert!(exchange(&src, &dst, source_key, stage_key).is_err());
        exchange(&src, &dst, stage_key, source_key).unwrap();
        assert!(run("/usr/bin/lipo", &["-archs", as_str(&bin(&src)).unwrap()]).contains("x86_64"));
        verified(&src, &stop).unwrap();
        let cdhash = |p: &Path| {
            tool(
                "/usr/bin/codesign",
                &[
                    "--display",
                    "--arch",
                    "arm64",
                    "--verbose=4",
                    as_str(p).unwrap(),
                ],
                &stop,
                10,
            )
            .unwrap()
            .lines()
            .find_map(|s| s.strip_prefix("CDHash="))
            .unwrap()
            .to_owned()
        };
        assert_eq!(cdhash(&src), cdhash(&dst));
        fs::write(dst.join("Contents/Resources/required.bin"), b"changed").unwrap();
        assert!(verified(&dst, &stop).is_err());
        assert!(files::compare(
            &old,
            &files::manifest(&dst, &stop).unwrap(),
            true,
            &BTreeSet::new()
        )
        .is_err());
    }
    #[test]
    fn locale_changes_stay_in_copy_and_dangerous_links_are_rejected() {
        let tmp = tempfile::Builder::new()
            .prefix("lindo-locale-fixture-")
            .tempdir()
            .unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let _guard = policy::LocalOnly::enter().unwrap();
        let stop = AtomicBool::new(false);
        let src = synthetic(&base);
        let locales = locales::list(&src, "org.example.synthetic-trim-fixture", "en").unwrap();
        assert!(locales
            .iter()
            .filter(|v| v.name == "Base" || v.name == "en")
            .all(|v| v.protected));
        let selected = BTreeSet::from([PathBuf::from("Contents/Resources/ja.lproj")]);
        let dst = base.join("LocaleFixture.app");
        copy_variant(&src, &dst, false, &selected, &stop).unwrap();
        files::compare(
            &files::manifest(&src, &stop).unwrap(),
            &files::manifest(&dst, &stop).unwrap(),
            false,
            &selected,
        )
        .unwrap();
        verified(&src, &stop).unwrap();
        assert!(src
            .join("Contents/Resources/ja.lproj/sample.strings")
            .exists());
        assert!(!dst.join("Contents/Resources/ja.lproj").exists());
        // Modern signing envelopes can mark localization resources optional. Strict
        // codesign remains the authority. Mandatory resources were tested above.
        let outcome = verified(&dst, &stop);
        println!("Localized fixture after removal: {outcome:?}");
        symlink("required.bin", src.join("Contents/Resources/inside")).unwrap();
        assert!(files::manifest(&src, &stop).is_ok());
        symlink("/tmp", src.join("Contents/Resources/outside")).unwrap();
        assert!(files::manifest(&src, &stop).is_err());
    }
}

#[cfg(test)]
mod trash_tests {
    use super::*;
    #[test]
    fn confirmed_backup_uses_native_trash_and_is_restorable() {
        if std::env::var("LINDO_REAL_TRASH_TEST").as_deref() != Ok("1") {
            return;
        }
        let temp = tempfile::Builder::new()
            .prefix("lindo-trim-trash-fixture-")
            .tempdir()
            .unwrap();
        let root = temp.path().canonicalize().unwrap();
        let live = super::tests::synthetic(&root);
        let base = root.join("staging");
        fs::create_dir(&base).unwrap();
        let stage = tempfile::Builder::new()
            .prefix("riduzione-")
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir_in(&base)
            .unwrap();
        let backup = stage.path().join(live.file_name().unwrap());
        let _guard = policy::LocalOnly::enter().unwrap();
        let stop = AtomicBool::new(false);
        copy_variant(&live, &backup, true, &BTreeSet::new(), &stop).unwrap();
        verified(&live, &stop).unwrap();
        verified(&backup, &stop).unwrap();
        let a = Identity::read(&live).unwrap().node();
        let b = Identity::read(&backup).unwrap().node();
        exchange(&live, &backup, a, b).unwrap();
        let live_id = Identity::tree_cancel(&live, true, &stop).unwrap().0;
        let orig_id = Identity::tree_cancel(&backup, true, &stop).unwrap().0;
        assert!(trash_verified_backup(
            &base,
            (&live, &orig_id),
            (&backup, &live_id),
            &stop,
            || Ok(()),
            || Ok(())
        )
        .is_err());
        assert!(trash_verified_backup(
            &base,
            (&live, &live_id),
            (&backup, &orig_id),
            &stop,
            || Ok(()),
            || Err("synthetic active".into())
        )
        .is_err());
        let outcome = trash_verified_backup(
            &base,
            (&live, &live_id),
            (&backup, &orig_id),
            &stop,
            || {
                verified(&live, &stop)?;
                verified(&backup, &stop)
            },
            || Ok(()),
        )
        .unwrap();
        let destination = outcome
            .destination
            .expect("macOS should report the fixture's Trash URL");
        assert_eq!(fs::symlink_metadata(&destination).unwrap().ino(), a.1);
        assert!(!backup.exists());
        // Restore only the identified synthetic bundle and only to its empty private path.
        fs::rename(&destination, &backup).unwrap();
        assert!(outcome.verified);
        verified(&backup, &stop).unwrap();
        verified(&live, &stop).unwrap();
        assert_eq!(Identity::read(&backup).unwrap().node(), a);
        assert_eq!(Identity::read(&live).unwrap().node(), b);
    }
}
