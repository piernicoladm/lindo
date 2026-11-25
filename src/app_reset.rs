//! Reset one reviewed, user-wide CFPreferences domain of a signed, closed app.
use crate::{model::Identity, policy, system_tools::command, trash};
use eframe::egui;
use objc2_app_kit::NSWorkspace;
use objc2_foundation::NSString;
use sha2::{Digest, Sha256};
use std::{
    ffi::{c_void, CString},
    fs::{self, File},
    io::{self, Read, Write},
    os::{
        fd::AsRawFd,
        unix::fs::{MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
    ptr,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
type Result<T> = std::result::Result<T, String>;
type Ref = *const c_void;
const MAX_SIZE: u64 = 16 * 1024 * 1024;
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFDataCreate(alloc: Ref, p: *const u8, len: isize) -> Ref;
    fn CFPropertyListCreateWithData(
        alloc: Ref,
        data: Ref,
        opts: usize,
        format: *mut isize,
        error: *mut Ref,
    ) -> Ref;
    fn CFStringCreateWithCString(alloc: Ref, string: *const i8, encoding: u32) -> Ref;
    fn CFGetTypeID(value: Ref) -> usize;
    fn CFDictionaryGetTypeID() -> usize;
    fn CFDictionaryGetCount(dict: Ref) -> isize;
    fn CFEqual(a: Ref, b: Ref) -> u8;
    fn CFRelease(value: Ref);
    fn CFPreferencesCopyMultiple(keys: Ref, app: Ref, user: Ref, host: Ref) -> Ref;
    fn CFDictionaryGetKeysAndValues(dict: Ref, keys: *mut Ref, values: *mut Ref);
    fn CFArrayCreate(allocator: Ref, values: *const Ref, count: isize, callbacks: Ref) -> Ref;
    fn CFPreferencesSetMultiple(set: Ref, remove: Ref, app: Ref, user: Ref, host: Ref);
    fn CFPreferencesSynchronize(app: Ref, user: Ref, host: Ref) -> u8;
    static kCFPreferencesCurrentUser: Ref;
    static kCFPreferencesAnyHost: Ref;
}
struct Owned(Ref);
impl Owned {
    fn new(r: Ref) -> Result<Self> {
        if r.is_null() {
            Err("Preferenze non verificabili.".into())
        } else {
            Ok(Self(r))
        }
    }
    fn dictionary(&self) -> bool {
        unsafe { CFGetTypeID(self.0) == CFDictionaryGetTypeID() }
    }
    fn count(&self) -> isize {
        unsafe { CFDictionaryGetCount(self.0) }
    }
    fn eq(&self, other: &Owned) -> bool {
        unsafe { CFEqual(self.0, other.0) != 0 }
    }
}
impl Drop for Owned {
    fn drop(&mut self) {
        unsafe { CFRelease(self.0) }
    }
}
fn plist(bytes: &[u8]) -> Result<Owned> {
    let data =
        Owned::new(unsafe { CFDataCreate(ptr::null(), bytes.as_ptr(), bytes.len() as isize) })?;
    let v = Owned::new(unsafe {
        CFPropertyListCreateWithData(ptr::null(), data.0, 0, ptr::null_mut(), ptr::null_mut())
    })?;
    if !v.dictionary() {
        return Err("Il plist deve contenere un dizionario.".into());
    }
    Ok(v)
}
struct Domain(Owned);
impl Domain {
    fn new(id: &str) -> Result<Self> {
        let key = CString::new(id).map_err(|e| e.to_string())?;
        Ok(Self(Owned::new(unsafe {
            CFStringCreateWithCString(ptr::null(), key.as_ptr(), 0x08000100)
        })?))
    }
    fn sync(&self) -> Result<()> {
        if unsafe {
            CFPreferencesSynchronize(self.0 .0, kCFPreferencesCurrentUser, kCFPreferencesAnyHost)
        } == 0
        {
            return Err("macOS non ha confermato la sincronizzazione delle preferenze.".into());
        }
        Ok(())
    }
    fn read(&self) -> Result<Owned> {
        self.sync()?;
        Owned::new(unsafe {
            CFPreferencesCopyMultiple(
                ptr::null(),
                self.0 .0,
                kCFPreferencesCurrentUser,
                kCFPreferencesAnyHost,
            )
        })
    }
    fn reset(&self, expected: &Owned) -> Result<()> {
        // Remove precisely the keys in the exported dictionary.
        let mut key_ptrs = vec![ptr::null(); expected.count() as usize];
        unsafe {
            CFDictionaryGetKeysAndValues(expected.0, key_ptrs.as_mut_ptr(), ptr::null_mut());
        }
        let keys = Owned::new(unsafe {
            CFArrayCreate(
                ptr::null(),
                key_ptrs.as_ptr(),
                expected.count(),
                ptr::null(),
            )
        })?;
        verified_update(
            || self.read(),
            Some(expected),
            None,
            || unsafe {
                CFPreferencesSetMultiple(
                    ptr::null(),
                    keys.0,
                    self.0 .0,
                    kCFPreferencesCurrentUser,
                    kCFPreferencesAnyHost,
                )
            },
        )
    }
    fn restore(&self, saved: &Owned) -> Result<()> {
        verified_update(
            || self.read(),
            None,
            Some(saved),
            || unsafe {
                CFPreferencesSetMultiple(
                    saved.0,
                    ptr::null(),
                    self.0 .0,
                    kCFPreferencesCurrentUser,
                    kCFPreferencesAnyHost,
                )
            },
        )
    }
}
fn verified_update(
    read: impl Fn() -> Result<Owned>,
    before: Option<&Owned>,
    after: Option<&Owned>,
    mutate: impl FnOnce(),
) -> Result<()> {
    let matches = |actual: &Owned, expected: Option<&Owned>| match expected {
        Some(dict) => actual.eq(dict),
        None => actual.count() == 0,
    };
    if !matches(&read()?, before) {
        return Err(
            "Il dominio contiene nuove preferenze o è cambiato. Ripeti la verifica.".into(),
        );
    }
    mutate();
    if !matches(&read()?, after) {
        return Err("Il risultato dell’operazione non è stato verificato.".into());
    }
    Ok(())
}
fn valid_id(id: &str) -> bool {
    let lower = id.to_ascii_lowercase();
    id.len() <= 180
        && id.split('.').count() >= 2
        && id
            .split('.')
            .all(|c| !c.is_empty() && c.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
        && !["com.apple.", "org.apple.", "group."]
            .iter()
            .any(|p| lower.starts_with(p))
        && lower != "com.apple"
        && lower != "org.apple"
        && !lower.starts_with("com.piernicolademaria.lindo")
        && !helper(&lower)
}
fn helper(name: &str) -> bool {
    name.to_ascii_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .any(|c| {
            matches!(
                c,
                "helper" | "updater" | "installer" | "daemon" | "agent" | "service" | "loginitem"
            ) || c.ends_with("helper")
        })
}
fn app_path(home: &Path, p: &Path) -> bool {
    p.is_absolute()
        && p.extension().is_some_and(|x| x.eq_ignore_ascii_case("app"))
        && (p.parent() == Some(Path::new("/Applications"))
            || p.parent() == Some(home.join("Applications").as_path()))
        && !helper(&p.file_stem().unwrap_or_default().to_string_lossy())
}
fn tool(exe: &str, args: &[&str], stop: &AtomicBool) -> Result<String> {
    // The shared runner uses its no-materialization child policy for an explicit input file.
    let empty = tempfile::tempfile().map_err(|e| e.to_string())?;
    // codesign emits display metadata on stderr, including signature details.
    let run = if exe == "/usr/bin/codesign" {
        command::run
    } else {
        command::run_stdout
    };
    let out = run(exe, args, Some(empty), stop, 45)?;
    if out.code != Some(0) || out.truncated {
        return Err(format!(
            "Verifica non riuscita con {}.",
            Path::new(exe)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
        ));
    }
    Ok(out.text)
}
#[derive(Clone)]
struct App {
    path: PathBuf,
    id: String,
    team: String,
    hash: String,
    executable: String,
}
pub(crate) fn verified_domain_for_cleanup(
    path: &Path,
    stop: &AtomicBool,
) -> Result<(String, String, String)> {
    signed_app(path, stop).map(|app| (app.id, app.team, app.hash))
}

pub(crate) fn verified_bundle_for_trim(
    path: &Path,
    stop: &AtomicBool,
) -> Result<(String, String, String)> {
    // Bundle thinning only touches the staged copy. Sandboxed preferences are not involved.
    signed_app_for(path, stop, false).map(|app| (app.id, app.team, app.hash))
}

fn signed_app(p: &Path, stop: &AtomicBool) -> Result<App> {
    signed_app_for(p, stop, true)
}

fn signed_app_for(p: &Path, stop: &AtomicBool, preference_cleanup: bool) -> Result<App> {
    let home = policy::home();
    if !app_path(&home, p)
        || fs::canonicalize(p).ok().as_deref() != Some(p)
        || !policy::local_path(p).unwrap_or(false)
    {
        return Err("Scegli un’app locale direttamente in Applicazioni o ~/Applications.".into());
    }
    let id = policy::app_identifier(p).map_err(|e| e.to_string())?;
    if !valid_id(&id) {
        return Err("Dominio protetto oppure identificatore di app non valido.".into());
    }
    let path = p.to_str().ok_or("Percorso non rappresentabile.")?;
    let req=format!("identifier \"{id}\" and anchor apple generic and (certificate leaf[field.1.2.840.113635.100.6.1.13] exists or certificate leaf[field.1.2.840.113635.100.6.1.9] exists)");
    tool(
        "/usr/bin/codesign",
        &["--verify", "--deep", "--strict", "-R", &req, path],
        stop,
    )?;
    let entitlements = tool(
        "/usr/bin/codesign",
        &["--display", "--entitlements", "-", "--xml", path],
        stop,
    )?;
    if preference_cleanup && entitlements.contains("com.apple.security.app-sandbox") {
        return Err("App sandbox escluse dal reset.".into());
    }
    let details = tool(
        "/usr/bin/codesign",
        &["--display", "--verbose=4", path],
        stop,
    )?;
    let field = |prefix: &str| {
        details
            .lines()
            .find_map(|line| line.strip_prefix(prefix))
            .map(str::to_owned)
    };
    let team = field("TeamIdentifier=")
        .filter(|v| {
            v.len() == 10
                && v.bytes()
                    .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
        })
        .ok_or("Team della firma non riconosciuto.")?;
    let hash = field("CDHash=").ok_or("Firma incompleta.")?;
    let info = p.join("Contents/Info.plist");
    let json = tool(
        "/usr/bin/plutil",
        &[
            "-convert",
            "json",
            "-o",
            "-",
            info.to_str().ok_or("Info.plist non disponibile.")?,
        ],
        stop,
    )?;
    let info: serde_json::Value =
        serde_json::from_str(&json).map_err(|_| "Info.plist non verificabile.")?;
    let active = |name: &str| {
        !matches!(
            &info[name],
            serde_json::Value::Null | serde_json::Value::Bool(false)
        ) && info[name].as_i64() != Some(0)
            && info[name]
                .as_str()
                .is_none_or(|v| !["0", "false", "no"].contains(&v.to_ascii_lowercase().as_str()))
    };
    if info["CFBundlePackageType"] != "APPL" || active("LSBackgroundOnly") || active("LSUIElement")
    {
        return Err("Agenti, helper e app in background sono esclusi.".into());
    }
    let executable = info["CFBundleExecutable"]
        .as_str()
        .filter(|v| !v.is_empty() && !v.starts_with('.') && !v.contains('/'))
        .ok_or("Eseguibile non valido.")?
        .to_owned();
    let unique = objc2::rc::autoreleasepool(|_| {
        let urls = NSWorkspace::sharedWorkspace()
            .URLsForApplicationsWithBundleIdentifier(&NSString::from_str(&id));
        let mut matches = std::collections::HashSet::new();
        for u in urls.iter() {
            let Some(known) = u.to_file_path() else {
                return false;
            };
            matches.insert(known);
        }
        matches.len() == 1 && matches.contains(p)
    });
    if !unique {
        return Err("Launch Services non identifica una sola installazione di questa app.".into());
    }
    trash::check_running(p, &id, false)?;
    Ok(App {
        path: p.to_owned(),
        id,
        team,
        hash,
        executable,
    })
}
fn fp(home: &Path, app: &App) -> PathBuf {
    home.join("Library/Preferences")
        .join(format!("{}.plist", app.id))
}
fn archive_dir(home: &Path, app: &App) -> PathBuf {
    home.join("Library/Application Support/Lindo/AppReset")
        .join(&app.team)
        .join(&app.id)
}
fn safe_bytes(path: &Path, private: bool) -> Result<(Vec<u8>, Identity)> {
    let id = Identity::read(path).map_err(|e| e.to_string())?;
    if !policy::local_path(path).unwrap_or(false)
        || policy::provider_state(path, false).ok() != Some((true, false))
    {
        return Err("File non locale.".into());
    }
    let f = policy::open_file(path, 0).map_err(|e| e.to_string())?;
    let m = f.metadata().map_err(|e| e.to_string())?;
    if !m.is_file()
        || m.uid() != unsafe { libc::geteuid() }
        || m.nlink() != 1
        || m.len() > MAX_SIZE
        || !id.matches_metadata(&m)
        || (private && m.mode() & 0o077 != 0)
    {
        return Err("File non idoneo al reset.".into());
    }
    let mut b = Vec::new();
    f.take(MAX_SIZE + 1)
        .read_to_end(&mut b)
        .map_err(|e| e.to_string())?;
    if b.len() as u64 != m.len() || !id.matches(path) {
        return Err("File modificato durante la verifica.".into());
    }
    Ok((b, id))
}
fn absent(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(false),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(true),
        Err(e) => Err(e.to_string()),
    }
}
fn save_backup(home: &Path, app: &App, bytes: &[u8]) -> Result<PathBuf> {
    let base = home.join("Library/Application Support");
    if !Identity::read(&base).is_ok_and(|v| v.is_dir()) {
        return Err("Application Support non disponibile.".into());
    }
    let mut path = base;
    let mut parent = policy::open_file(&path, libc::O_DIRECTORY).map_err(|e| e.to_string())?;
    for part in ["Lindo", "AppReset", &app.team, &app.id] {
        path.push(part);
        let name = CString::new(part).map_err(|e| e.to_string())?;
        if unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o700) } != 0 {
            let e = io::Error::last_os_error();
            if e.kind() != io::ErrorKind::AlreadyExists {
                return Err(e.to_string());
            }
        }
        parent = policy::open_file(&path, libc::O_DIRECTORY).map_err(|e| e.to_string())?;
        let d = parent.metadata().map_err(|e| e.to_string())?;
        if d.uid() != unsafe { libc::geteuid() }
            || d.mode() & 0o022 != 0
            || (part != "Lindo" && d.mode() & 0o077 != 0)
        {
            return Err("Cartella delle copie non privata.".into());
        }
    }
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    let final_path = path.join(format!("reset-{stamp}-{:x}.plist", Sha256::digest(bytes)));
    let mut f = File::options()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(0x20000000)
        .open(&final_path)
        .map_err(|e| e.to_string())?;
    f.write_all(bytes)
        .and_then(|_| f.sync_all())
        .map_err(|e| e.to_string())?;
    policy::open_file(&path, libc::O_DIRECTORY)
        .and_then(|f| f.sync_all())
        .map_err(|e| e.to_string())?;
    if safe_bytes(&final_path, true)?.0 != bytes {
        return Err("Verifica della copia privata non riuscita.".into());
    }
    Ok(final_path)
}
fn archive_name(p: &Path) -> bool {
    archive_digest(p).is_some()
}
fn archive_digest(p: &Path) -> Option<&str> {
    let name = p
        .file_name()?
        .to_str()?
        .strip_prefix("reset-")?
        .strip_suffix(".plist")?;
    let (stamp, hash) = name.split_once('-')?;
    (!stamp.is_empty()
        && stamp.bytes().all(|b| b.is_ascii_digit())
        && hash.len() == 64
        && hash.bytes().all(|b| b.is_ascii_hexdigit()))
    .then_some(hash)
}
fn backup_label(path: &Path) -> String {
    let stamp = path
        .file_name()
        .and_then(|n| n.to_str())
        .and_then(|n| n.strip_prefix("reset-"))
        .and_then(|n| n.split_once('-'))
        .and_then(|(n, _)| n.parse::<u128>().ok())
        .and_then(|v| libc::time_t::try_from(v / 1_000_000_000).ok());
    if let Some(moment) = stamp.and_then(crate::model::local_timestamp) {
        return format!("Rivedi copia del {moment}");
    }
    "Rivedi copia salvata".into()
}
fn backup_bytes(path: &Path) -> Result<(Vec<u8>, Identity)> {
    let (bytes, id) = safe_bytes(path, true)?;
    if archive_digest(path) != Some(format!("{:x}", Sha256::digest(&bytes)).as_str()) {
        return Err("Contenuto della copia diverso dall’esportazione.".into());
    }
    Ok((bytes, id))
}
#[derive(Clone)]
struct Preview {
    app: App,
    current: Option<Identity>,
    count: isize,
    cache: Option<PathBuf>,
    backups: Vec<PathBuf>,
    audit: String,
}
fn inspect(path: &Path, audit: bool, stop: &AtomicBool) -> Result<Preview> {
    let _guard = policy::LocalOnly::enter().map_err(|e| e.to_string())?;
    let app = signed_app(path, stop)?;
    let home = policy::home();
    if !absent(&home.join("Library/Containers").join(&app.id))? {
        return Err("Preferenze in container escluse dal reset.".into());
    }
    let physical = fp(&home, &app);
    let current = Domain::new(&app.id)?.read()?;
    let count = current.count();
    let identity = if count == 0 {
        None
    } else {
        let (bytes, id) = safe_bytes(&physical, false)?;
        if !current.eq(&plist(&bytes)?) {
            return Err(
                "Il dominio attivo non coincide con il plist locale. Reset bloccato.".into(),
            );
        }
        Some(id)
    };
    let cache_path = home.join("Library/Caches").join(&app.id);
    let cache = (Identity::read(&cache_path).is_ok_and(|v| v.is_dir())
        && policy::local_path(&cache_path).unwrap_or(false))
    .then_some(cache_path);
    let dir = archive_dir(&home, &app);
    let mut backups = Vec::new();
    if !absent(&dir)? {
        let listing = policy::children(&dir).map_err(|e| e.to_string())?;
        for name in listing {
            let p = dir.join(name.map_err(|e| e.to_string())?);
            if archive_name(&p) && Identity::read(&p).is_ok_and(|v| !v.is_dir()) {
                backups.push(p);
            }
        }
        backups.sort_by(|a, b| b.cmp(a));
    }
    let audit = if audit {
        audit_bundle(&app, stop)?
    } else {
        String::new()
    };
    Ok(Preview {
        app,
        current: identity,
        count,
        cache,
        backups,
        audit,
    })
}
fn audit_bundle(app: &App, stop: &AtomicBool) -> Result<String> {
    let bin = app.path.join("Contents/MacOS").join(&app.executable);
    if !policy::local_path(&bin).unwrap_or(false) {
        return Err("Eseguibile non locale.".into());
    }
    let arch = tool(
        "/usr/bin/lipo",
        &[
            "-archs",
            bin.to_str().ok_or("Percorso non rappresentabile.")?,
        ],
        stop,
    )?;
    let root = app.path.join("Contents/Resources");
    let mut locales = Vec::new();
    if !absent(&root)? {
        for n in policy::children(&root).map_err(|e| e.to_string())? {
            if stop.load(Ordering::Acquire) {
                return Err("Operazione annullata.".into());
            }
            let p = root.join(n.map_err(|e| e.to_string())?);
            if p.extension().is_some_and(|v| v == "lproj")
                && Identity::read(&p).is_ok_and(|i| i.is_dir())
            {
                locales.push(
                    p.file_stem()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned(),
                );
            }
        }
    }
    locales.sort();
    Ok(format!(
        "Architetture: {}. Localizzazioni in Resources: {}.",
        arch.trim(),
        if locales.is_empty() {
            "nessuna".into()
        } else {
            locales.join(", ")
        }
    ))
}
#[derive(Clone)]
enum Action {
    Reset,
    Restore(PathBuf, Box<Identity>, [u8; 32]),
}
fn apply(review: &Preview, action: &Action, stop: &AtomicBool) -> Result<String> {
    let _guard = policy::LocalOnly::enter().map_err(|e| e.to_string())?;
    let home = policy::home();
    let same = signed_app(&review.app.path, stop)?;
    if same.id != review.app.id || same.team != review.app.team || same.hash != review.app.hash {
        return Err("Firma o app cambiata. Ripeti la verifica.".into());
    }
    if !absent(&home.join("Library/Containers").join(&same.id))? {
        return Err("App in container esclusa.".into());
    }
    let domain = Domain::new(&same.id)?;
    let physical = fp(&home, &same);
    match action {
        Action::Reset => {
            let (bytes, identity) = safe_bytes(&physical, false)?;
            if review.current.as_ref() != Some(&identity) {
                return Err("Preferenze cambiate. Ripeti l’analisi.".into());
            }
            let original = plist(&bytes)?;
            if !domain.read()?.eq(&original) {
                return Err("Preferenze attive cambiate.".into());
            }
            let saved = save_backup(&home, &same, &bytes)?;
            let result = (|| {
                trash::check_running(&same.path, &same.id, false)?;
                if !identity.matches(&physical) {
                    return Err("Il plist è cambiato.".into());
                }
                domain.reset(&original)
            })();
            result.map_err(|e| format!("{e} Copia recuperabile: {}", saved.display()))?;
            Ok(format!("Preferenze reimpostate. Copia recuperabile: {}. Analizza di nuovo per ripristinarla.",saved.display()))
        }
        Action::Restore(path, identity, digest) => {
            if path.parent() != Some(archive_dir(&home, &same).as_path()) || !archive_name(path) {
                return Err("Copia fuori dall’archivio dell’app.".into());
            }
            let (bytes, id) = backup_bytes(path)?;
            if &id != identity.as_ref() || <[u8; 32]>::from(Sha256::digest(&bytes)) != *digest {
                return Err("La copia selezionata è cambiata.".into());
            }
            if !absent(&physical)? && plist(&safe_bytes(&physical, false)?.0)?.count() != 0 {
                return Err("Il plist contiene nuove preferenze.".into());
            }
            trash::check_running(&same.path, &same.id, false)?;
            domain.restore(&plist(&bytes)?)?;
            Ok(format!("Ripristino verificato per il dominio {}.", same.id))
        }
    }
}
enum Reply {
    Inspected(Box<Preview>),
    Reviewed(Action),
    Done(String),
}
struct Work {
    stop: Arc<AtomicBool>,
    rx: mpsc::Receiver<Result<Reply>>,
    mutation: bool,
}
impl Drop for Work {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}
#[derive(Default)]
pub(crate) struct AppReset {
    input: String,
    audit: bool,
    preview: Option<Preview>,
    action: Option<Action>,
    work: Option<Work>,
    notice: String,
}
impl AppReset {
    fn submit(
        &mut self,
        ctx: &egui::Context,
        mutation: bool,
        op: impl FnOnce(&AtomicBool) -> Result<Reply> + Send + 'static,
    ) {
        self.notice.clear();
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        self.work = Some(Work {
            stop: stop.clone(),
            rx,
            mutation,
        });
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = op(&stop);
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    }
    pub(crate) fn show(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        if let Some(work) = &self.work {
            match work.rx.try_recv() {
                Ok(result) => {
                    self.work.take();
                    match result {
                        Ok(Reply::Inspected(v)) => self.preview = Some(*v),
                        Ok(Reply::Reviewed(a)) => self.action = Some(a),
                        Ok(Reply::Done(s)) => {
                            self.preview = None;
                            self.action = None;
                            self.notice = s;
                        }
                        Err(e) => {
                            self.action = None;
                            self.notice = e;
                        }
                    }
                }
                Err(mpsc::TryRecvError::Empty) => {
                    ui.spinner();
                    ui.label(if work.mutation {
                        "Aggiornamento delle preferenze in corso…"
                    } else {
                        "Verifica in corso…"
                    });
                    if !work.mutation && ui.button("Interrompi verifica").clicked() {
                        work.stop.store(true, Ordering::Release);
                        self.work.take();
                        self.notice = "Verifica interrotta.".into();
                    }
                    ctx.request_repaint_after(Duration::from_millis(160));
                    return;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.work = None;
                    self.action = None;
                    self.notice = "Verifica interrotta. Controlla le copie salvate.".into();
                }
            }
        }
        if !self.notice.is_empty() {
            ui.label(&self.notice);
        }
        if let (Some(a), Some(p)) = (&self.action, &self.preview) {
            ui.heading("Conferma preferenze");
            ui.label(format!("{} · {}", p.app.path.display(), p.app.id));
            match a {
                Action::Reset => {
                    ui.label(format!("{} preferenze saranno archiviate prima di rimuoverle dal dominio dell’app. Al prossimo avvio l’app ripartirà dalle impostazioni iniziali.",p.count));
                }
                Action::Restore(path, _, _) => {
                    ui.label(format!(
                        "Ripristina le impostazioni dalla copia {} nel dominio vuoto.",
                        path.display()
                    ));
                }
            }
            ui.label("Documenti, cache e bundle dell’app restano nelle rispettive cartelle.");
            let mut go = false;
            let mut back = false;
            ui.horizontal_wrapped(|ui| {
                go = ui
                    .button(match a {
                        Action::Reset => "Conferma reset delle preferenze",
                        Action::Restore(..) => "Conferma ripristino delle preferenze",
                    })
                    .clicked();
                back = ui.button("Indietro").clicked();
            });
            if go {
                let review = p.clone();
                let action = a.clone();
                self.preview = None;
                self.action = None;
                self.submit(ctx, true, move |stop| {
                    apply(&review, &action, stop).map(Reply::Done)
                });
            }
            if back {
                self.action = None;
            }
            return;
        }
        ui.label("Scegli il percorso o trascina un’app di terzi installata direttamente in Applicazioni. Chiudi l’app prima della verifica.");
        if let Some(drop) = ctx.input(|i| {
            crate::extras::single_drop(&i.raw.dropped_files).map(|r| r.map(Path::to_path_buf))
        }) {
            self.preview = None;
            self.notice.clear();
            match drop {
                Ok(path) => self.input = path.to_string_lossy().into_owned(),
                Err(reason) => {
                    self.input.clear();
                    self.notice = reason.into();
                }
            }
        }
        if ui.text_edit_singleline(&mut self.input).changed() {
            self.preview = None;
            self.notice.clear();
        }
        if ui
            .checkbox(
                &mut self.audit,
                "Esamina anche architetture e lingue del bundle",
            )
            .changed()
        {
            self.preview = None;
        }
        if ui
            .add_enabled(
                !self.input.trim().is_empty(),
                egui::Button::new("Verifica app e preferenze"),
            )
            .clicked()
        {
            self.preview = None;
            let path = if let Some(rel) = self.input.trim().strip_prefix("~/") {
                policy::home().join(rel)
            } else {
                PathBuf::from(self.input.trim())
            };
            let audit = self.audit;
            self.submit(ctx, false, move |stop| {
                inspect(&path, audit, stop)
                    .map(Box::new)
                    .map(Reply::Inspected)
            });
            return;
        }
        if let Some(p) = &self.preview {
            ui.label(format!(
                "Dominio {} · Team {} · {} impostazioni",
                p.app.id, p.app.team, p.count
            ));
            if !p.audit.is_empty() {
                ui.label(&p.audit);
            }
            if let Some(c) = &p.cache {
                ui.label(format!(
                    "Cache associata: {}. Puoi rivederla in Pulizia avanzata > App e residui.",
                    c.display()
                ));
            }
            if ui
                .add_enabled(
                    p.current.is_some(),
                    egui::Button::new("Rivedi reset delle preferenze"),
                )
                .clicked()
            {
                self.action = Some(Action::Reset);
            }
            ui.label("Il reset riguarda il dominio locale dell’utente. Le impostazioni per host, i gruppi condivisi e i container sandbox sono esclusi.");
            if !p.backups.is_empty() {
                ui.label("Copie recuperabili. Il ripristino richiede un dominio vuoto.");
            }
            let mut selected = None;
            egui::ScrollArea::vertical()
                .id_salt("app_preference_backups")
                .show_rows(ui, 24.0, p.backups.len(), |ui, range| {
                    for i in range {
                        let b = &p.backups[i];
                        if ui
                            .add_enabled(p.count == 0, egui::Button::new(backup_label(b)))
                            .on_hover_text(b.display().to_string())
                            .clicked()
                        {
                            selected = Some(b.clone());
                        }
                    }
                });
            if let Some(path) = selected {
                self.submit(ctx, false, move |_| {
                    let _guard = policy::LocalOnly::enter().map_err(|e| e.to_string())?;
                    let (bytes, id) = backup_bytes(&path)?;
                    plist(&bytes)?;
                    Ok(Reply::Reviewed(Action::Restore(
                        path,
                        Box::new(id),
                        Sha256::digest(bytes).into(),
                    )))
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::os::unix::fs::symlink;

    fn data(value: &str) -> Vec<u8> {
        format!(r#"<?xml version="1.0"?><plist version="1.0"><dict>{value}</dict></plist>"#)
            .into_bytes()
    }
    fn fixture() -> App {
        App {
            path: "/Applications/Fixture.app".into(),
            id: "org.example.fixture".into(),
            team: "FIXTURE123".into(),
            hash: "fixture".into(),
            executable: "Fixture".into(),
        }
    }
    #[test]
    fn only_direct_third_party_app_identifiers_are_allowed() {
        for id in [
            "com.apple.Finder",
            "com.piernicolademaria.lindo",
            "group.org.example",
            "org.example.helper",
            "x/../../outside",
            "a..b",
            "-globalDomain",
        ] {
            assert!(!valid_id(id), "{id}");
        }
        assert!(valid_id("org.example.My-App"));
        let home = Path::new("/fixture");
        assert!(app_path(home, Path::new("/Applications/Widget.app")));
        assert!(app_path(
            home,
            Path::new("/fixture/Applications/Widget.app")
        ));
        for p in [
            "/Applications/Widget.app/Contents/Helper.app",
            "/Applications/Widget Helper.app",
            "/fixture/Downloads/Widget.app",
            "/Applications/Widget.plist",
        ] {
            assert!(!app_path(home, Path::new(p)));
        }
    }
    #[test]
    fn mock_domain_reset_restore_and_stale_guards() {
        let old = data("<key>size</key><integer>42</integer>");
        let empty = data("");
        let changed = data("<key>size</key><integer>43</integer>");
        let state = RefCell::new(old.clone());
        let read = || plist(&state.borrow());
        let initial = plist(&old).unwrap();
        verified_update(read, Some(&initial), None, || {
            state.replace(empty.clone()).clear()
        })
        .unwrap();
        verified_update(read, None, Some(&initial), || {
            state.replace(old.clone()).clear()
        })
        .unwrap();
        state.replace(changed);
        assert!(verified_update(read, Some(&initial), None, || panic!(
            "changed domain must stay untouched"
        ))
        .is_err());
        assert!(verified_update(read, None, Some(&initial), || panic!(
            "new preferences must stay untouched"
        ))
        .is_err());
        assert!(plist(b"invalid").is_err());
    }
    #[test]
    fn backup_roundtrip_permissions_tampering_and_symlinks() {
        let temp = tempfile::Builder::new()
            .prefix("lindo-reset-")
            .tempdir()
            .unwrap();
        let home = temp.path().canonicalize().unwrap();
        let app = fixture();
        fs::create_dir_all(home.join("Library/Application Support")).unwrap();
        let bytes = data("<key>color</key><string>blue</string>");
        let backup = save_backup(&home, &app, &bytes).unwrap();
        assert_eq!(backup.parent().unwrap(), archive_dir(&home, &app));
        assert_eq!(fs::metadata(&backup).unwrap().mode() & 0o777, 0o600);
        assert_eq!(backup_bytes(&backup).unwrap().0, bytes);
        assert!(backup_label(&backup).contains(" alle "));
        assert!(save_backup(&home, &app, &bytes).unwrap() != backup);
        fs::write(&backup, data("<key>color</key><string>red</string>")).unwrap();
        assert!(backup_bytes(&backup).is_err());
        let foreign = app.path.join("foreign.plist");
        assert!(!archive_name(&foreign));
        let other = home.join("other");
        fs::create_dir_all(other.join("Library/Application Support")).unwrap();
        symlink(&home, other.join("Library/Application Support/Lindo")).unwrap();
        assert!(save_backup(&other, &app, &bytes).is_err());
    }
}
