//! Narrow, on-demand inventory of old preferences, launch jobs and archived homes.
use crate::{
    model::{Identity, Mode},
    policy,
    system_tools::command,
    trash,
};
use eframe::egui;
use objc2_app_kit::NSWorkspace;
use objc2_foundation::NSString;
use std::{
    collections::HashSet,
    ffi::{c_void, CString},
    fs::File,
    io::{self, Read},
    os::unix::fs::MetadataExt,
    path::{Component, Path, PathBuf},
    ptr,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    time::{Duration, SystemTime},
};
type Result<T> = std::result::Result<T, String>;
type Ref = *const c_void;
const MAX_PLIST: u64 = 512 * 1024;
const MAX_VISITED: usize = 20_000;
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
    fn CFStringCreateWithCString(alloc: Ref, s: *const i8, encoding: u32) -> Ref;
    fn CFStringGetCString(s: Ref, buf: *mut i8, size: isize, encoding: u32) -> u8;
    fn CFGetTypeID(o: Ref) -> usize;
    fn CFDictionaryGetTypeID() -> usize;
    fn CFStringGetTypeID() -> usize;
    fn CFArrayGetTypeID() -> usize;
    fn CFDictionaryGetValue(dict: Ref, key: Ref) -> Ref;
    fn CFArrayGetCount(array: Ref) -> isize;
    fn CFArrayGetValueAtIndex(array: Ref, i: isize) -> Ref;
    fn CFRelease(o: Ref);
}
struct Owned(Ref);
impl Drop for Owned {
    fn drop(&mut self) {
        unsafe { CFRelease(self.0) }
    }
}
enum Parsed {
    Corrupt,
    Other,
    Dictionary(Owned),
}
fn parse(bytes: &[u8]) -> Result<Parsed> {
    let data = unsafe { CFDataCreate(ptr::null(), bytes.as_ptr(), bytes.len() as isize) };
    if data.is_null() {
        return Err("Memoria insufficiente.".into());
    }
    let data = Owned(data);
    let mut error = ptr::null();
    let r = unsafe {
        CFPropertyListCreateWithData(ptr::null(), data.0, 0, ptr::null_mut(), &mut error)
    };
    if !error.is_null() {
        drop(Owned(error));
    }
    if r.is_null() {
        return if error.is_null() {
            Err("Lettura plist incompleta.".into())
        } else {
            Ok(Parsed::Corrupt)
        };
    }
    let v = Owned(r);
    if unsafe { CFGetTypeID(v.0) == CFDictionaryGetTypeID() } {
        Ok(Parsed::Dictionary(v))
    } else {
        Ok(Parsed::Other)
    }
}
fn field(dict: &Owned, key: &str) -> Ref {
    let Ok(name) = CString::new(key) else {
        return ptr::null();
    };
    let s = unsafe { CFStringCreateWithCString(ptr::null(), name.as_ptr(), 0x08000100) };
    if s.is_null() {
        return ptr::null();
    }
    let s = Owned(s);
    unsafe { CFDictionaryGetValue(dict.0, s.0) }
}
fn string(v: Ref) -> Option<String> {
    if v.is_null() || unsafe { CFGetTypeID(v) != CFStringGetTypeID() } {
        return None;
    }
    let mut buf = [0i8; 4096];
    if unsafe { CFStringGetCString(v, buf.as_mut_ptr(), buf.len() as isize, 0x08000100) } == 0 {
        return None;
    }
    unsafe { std::ffi::CStr::from_ptr(buf.as_ptr()) }
        .to_str()
        .ok()
        .map(str::to_owned)
}
fn launch_fields(dict: &Owned) -> Option<(String, PathBuf)> {
    // These keys can change how launchd resolves an executable.
    if !field(dict, "BundleProgram").is_null() || !field(dict, "RootDirectory").is_null() {
        return None;
    }
    let label = string(field(dict, "Label"))?;
    let primary = field(dict, "Program");
    let value = if primary.is_null() {
        let args = field(dict, "ProgramArguments");
        if args.is_null()
            || unsafe { CFGetTypeID(args) != CFArrayGetTypeID() || CFArrayGetCount(args) == 0 }
        {
            return None;
        }
        unsafe { CFArrayGetValueAtIndex(args, 0) }
    } else {
        primary
    };
    let path = PathBuf::from(string(value)?);
    if !path.is_absolute() || path.components().any(|p| p == Component::ParentDir) {
        return None;
    }
    Some((label, path))
}
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
enum Zone {
    #[default]
    Preferences,
    Agents,
    Homes,
}
impl Zone {
    fn title(self) -> &'static str {
        match self {
            Self::Preferences => "Preferenze locali",
            Self::Agents => "LaunchAgent utente",
            Self::Homes => "Utenti rimossi",
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
enum Proof {
    Preference {
        app: PathBuf,
        domain: String,
        team: String,
        hash: String,
    },
    Agent {
        label: String,
        program: PathBuf,
    },
}
struct Entry {
    path: PathBuf,
    identity: Identity,
    reason: String,
    proof: Option<Proof>,
    bytes: Option<u64>,
}
struct Report {
    rows: Vec<Entry>,
    status: String,
}
struct Scope {
    home: PathBuf,
    deleted: PathBuf,
}
impl Scope {
    fn native() -> Self {
        Self {
            home: policy::home(),
            deleted: Path::new("/Users").join("Deleted Users"),
        }
    }
    fn root(&self, zone: Zone) -> PathBuf {
        match zone {
            Zone::Preferences => self.home.join("Library/Preferences"),
            Zone::Agents => self.home.join("Library/LaunchAgents"),
            Zone::Homes => self.deleted.clone(),
        }
    }
}
enum Registration {
    Missing,
    Unique(PathBuf),
    Uncertain,
}
trait Platform {
    fn registration(&self, id: &str) -> Registration;
    fn signature(&self, p: &Path, stop: &AtomicBool) -> Result<(String, String, String)>;
    fn inactive(&self, label: &str, stop: &AtomicBool) -> bool;
}
struct Native;
impl Platform for Native {
    fn registration(&self, id: &str) -> Registration {
        objc2::rc::autoreleasepool(|_| {
            let urls = NSWorkspace::sharedWorkspace()
                .URLsForApplicationsWithBundleIdentifier(&NSString::from_str(id));
            let mut paths = HashSet::new();
            for u in urls.iter() {
                let Some(p) = u.to_file_path() else {
                    return Registration::Uncertain;
                };
                paths.insert(p);
            }
            match paths.len() {
                0 => Registration::Missing,
                1 => Registration::Unique(paths.into_iter().next().unwrap()),
                _ => Registration::Uncertain,
            }
        })
    }
    fn signature(&self, p: &Path, stop: &AtomicBool) -> Result<(String, String, String)> {
        crate::app_reset::verified_domain_for_cleanup(p, stop)
    }
    fn inactive(&self, label: &str, stop: &AtomicBool) -> bool {
        let uid = unsafe { libc::geteuid() };
        for (domain, description) in [
            (format!("gui/{uid}"), format!("user gui: {uid}")),
            (format!("user/{uid}"), format!("uid: {uid}")),
            ("system".into(), "system".into()),
        ] {
            let name = format!("{domain}/{label}");
            // Discard stdout to avoid collecting job arguments or environments.
            let Ok(out) = command::run(
                "/bin/sh",
                &[
                    "-c",
                    r#"exec /bin/launchctl print "$1" >/dev/null"#,
                    "lindo",
                    &name,
                ],
                None,
                stop,
                5,
            ) else {
                return false;
            };
            let expected =
                format!("Could not find service \"{label}\" in domain for {description}");
            if out.code != Some(113)
                || out.truncated
                || !out.text.lines().any(|line| line == expected)
            {
                return false;
            }
        }
        true
    }
}
fn domain_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    name.len() <= 180
        && name.split('.').count() >= 2
        && name
            .split('.')
            .all(|c| !c.is_empty() && c.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
        && ![
            "com.apple.",
            "org.apple.",
            "group.",
            "com.piernicolademaria.lindo",
        ]
        .iter()
        .any(|p| lower.starts_with(p))
        && !matches!(lower.as_str(), "com.apple" | "org.apple")
}
fn safe_program(scope: &Scope, path: &Path) -> bool {
    if !path.is_absolute()
        || path.components().any(|c| {
            matches!(c, Component::ParentDir | Component::CurDir)
                || c.as_os_str().to_string_lossy().starts_with('.')
                || c.as_os_str().eq_ignore_ascii_case("_3_Vault")
        })
        || policy::protected(path, Mode::Space, None)
        || policy::cloud(path)
    {
        return false;
    }
    [
        PathBuf::from("/Applications"),
        scope.home.join("Applications"),
        PathBuf::from("/usr/local"),
        PathBuf::from("/opt/homebrew"),
        scope.home.join("Library/Application Support"),
    ]
    .iter()
    .any(|p| path.starts_with(p) && path != p)
}
fn definitely_missing(scope: &Scope, path: &Path) -> bool {
    if !safe_program(scope, path) {
        return false;
    }
    for (i, ancestor) in path.ancestors().enumerate() {
        match policy::open_file(ancestor, libc::O_EVTONLY) {
            Ok(f) => {
                return i != 0
                    && f.metadata().is_ok_and(|m| m.is_dir())
                    && policy::local_path(ancestor).unwrap_or(false)
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(_) => return false,
        }
    }
    false
}
fn read_plist(file: &File, id: &Identity, path: &Path) -> Result<Vec<u8>> {
    let m = file.metadata().map_err(|e| e.to_string())?;
    if !m.is_file()
        || m.len() > MAX_PLIST
        || m.nlink() != 1
        || m.uid() != unsafe { libc::geteuid() }
        || !id.matches_metadata(&m)
        || policy::provider_state(path, false).ok() != Some((true, false))
    {
        return Err("Plist non locale o cambiato.".into());
    }
    let mut data = Vec::new();
    file.take(MAX_PLIST + 1)
        .read_to_end(&mut data)
        .map_err(|e| e.to_string())?;
    if data.len() as u64 != m.len() || !id.matches(path) {
        return Err("Plist cambiato durante la verifica.".into());
    }
    Ok(data)
}
fn classify(
    scope: &Scope,
    zone: Zone,
    path: &Path,
    id: &Identity,
    file: &File,
    platform: &impl Platform,
    stop: &AtomicBool,
) -> Result<Option<(String, Option<Proof>)>> {
    if path.parent() != Some(scope.root(zone).as_path())
        || path.extension().is_none_or(|s| s != "plist")
    {
        return Ok(None);
    }
    let Some(name) = path
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|n| domain_name(n))
    else {
        return Ok(None);
    };
    let data = read_plist(file, id, path)?;
    let parsed = parse(&data)?;
    if zone == Zone::Agents {
        let Parsed::Dictionary(dict) = parsed else {
            return Ok(None);
        };
        let Some((label, program)) = launch_fields(&dict) else {
            return Ok(None);
        };
        if !domain_name(&label) || !definitely_missing(scope, &program) {
            return Ok(None);
        }
        if label != name {
            return Ok(Some((
                format!(
                    "Eseguibile mancante: {}. Label diversa dal nome del plist.",
                    program.display()
                ),
                None,
            )));
        }
        let inactive = platform.inactive(&label, stop);
        let reason = format!(
            "Eseguibile mancante: {}. {}",
            program.display(),
            if inactive {
                "Servizio assente dai domini launchd verificati."
            } else {
                "Servizio attivo o stato non verificabile."
            }
        );
        return Ok(Some((
            reason,
            inactive.then_some(Proof::Agent { label, program }),
        )));
    }
    let corrupt = matches!(parsed, Parsed::Corrupt);
    if matches!(parsed, Parsed::Other) {
        return Ok(Some(("Plist leggibile con struttura diversa da un dominio di preferenze. Verifica nel Finder.".into(),None)));
    }
    let old = file
        .metadata()
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| SystemTime::now().duration_since(t).ok())
        .is_some_and(|age| age >= Duration::from_secs(180 * 86400));
    if !corrupt && !old {
        return Ok(None);
    }
    match platform.registration(name){
        Registration::Unique(app) if corrupt=>{
            let proof=platform.signature(&app,stop).ok().and_then(|(domain,team,hash)|
                (domain==name).then_some(Proof::Preference{app:app.clone(),domain,team,hash}));
            let reason=if proof.is_some(){format!("Plist illeggibile; dominio dell’app firmata e chiusa {}.",app.display())}
                else{"Plist illeggibile; firma, attribuzione o stato dell’app non confermati.".into()};
            Ok(Some((reason,proof)))
        }
        Registration::Missing=>Ok(Some((if corrupt{"Plist illeggibile. Nessuna app registrata per il dominio."}else{"Plist invariato da almeno 180 giorni. Nessuna app registrata per il dominio; anche strumenti da riga di comando possono usarlo."}.into(),None))),
        Registration::Uncertain if corrupt=>Ok(Some(("Plist illeggibile. Attribuzione ambigua.".into(),None))),
        _=>Ok(None)
    }
}
fn scan(scope: &Scope, zone: Zone, platform: &impl Platform, stop: &AtomicBool) -> Result<Report> {
    let _guard = policy::LocalOnly::enter().map_err(|e| e.to_string())?;
    let root = scope.root(zone);
    let mut listing = match policy::children(&root) {
        Ok(dir) => dir,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Ok(Report {
                rows: Vec::new(),
                status: "Cartella assente.".into(),
            })
        }
        Err(e) => return Err(format!("Cartella non accessibile: {e}.")),
    };
    let mut rows = Vec::new();
    let mut count = 0;
    let mut errors = 0;
    let mut excluded = 0;
    let mut capped = false;
    while let Some(name) = listing.next() {
        if stop.load(Ordering::Acquire) {
            return Err("Analisi annullata.".into());
        }
        if count == MAX_VISITED {
            capped = true;
            break;
        }
        count += 1;
        let Ok(name) = name else {
            errors += 1;
            break;
        };
        let path = root.join(&name);
        if name.to_string_lossy().starts_with('.')
            || policy::protected(&path, Mode::Space, None)
            || policy::cloud(&path)
        {
            excluded += 1;
            continue;
        }
        let Ok(stat) = listing.stat(&name) else {
            errors += 1;
            continue;
        };
        let kind = stat.st_mode & libc::S_IFMT;
        if stat.st_flags & 0x40000000 != 0 || (kind != libc::S_IFREG && kind != libc::S_IFDIR) {
            excluded += 1;
            continue;
        }
        if zone == Zone::Homes {
            let ext = path
                .extension()
                .and_then(|v| v.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if !((kind == libc::S_IFREG && matches!(ext.as_str(), "dmg" | "sparseimage"))
                || (kind == libc::S_IFDIR && ext == "sparsebundle"))
            {
                continue;
            }
        } else if kind != libc::S_IFREG
            || stat.st_uid != unsafe { libc::geteuid() }
            || stat.st_size as u64 > MAX_PLIST
            || stat.st_nlink != 1
            || path.extension().is_none_or(|s| s != "plist")
            || !path
                .file_stem()
                .and_then(|v| v.to_str())
                .is_some_and(domain_name)
        {
            excluded += 1;
            continue;
        }
        let Ok(id) = Identity::read(&path) else {
            errors += 1;
            continue;
        };
        if id.node() != (stat.st_dev as u64, stat.st_ino)
            || !policy::local_path(&path).unwrap_or(false)
        {
            excluded += 1;
            continue;
        }
        let info = if zone == Zone::Homes {
            Some(("Immagine nella cartella degli utenti rimossi. Può contenere l’unica copia della home. Verifica proprietario e conservazione nel Finder.".into(),None))
        } else {
            let result = policy::open_file(&path, 0)
                .map_err(|e| e.to_string())
                .and_then(|f| classify(scope, zone, &path, &id, &f, platform, stop));
            match result {
                Ok(info) => info,
                Err(_) => {
                    errors += 1;
                    continue;
                }
            }
        };
        if let Some((reason, proof)) = info {
            rows.push(Entry {
                path,
                identity: id,
                reason,
                proof,
                bytes: (kind == libc::S_IFREG).then_some(stat.st_size as u64),
            });
        }
    }
    rows.sort_by(|a, b| {
        b.proof
            .is_some()
            .cmp(&a.proof.is_some())
            .then_with(|| a.path.cmp(&b.path))
    });
    let status = format!(
        "{count} voci visitate · {} risultati · {excluded} escluse · {errors} errori{}",
        rows.len(),
        if capped {
            " · limite di 20.000 raggiunto"
        } else {
            ""
        }
    );
    Ok(Report { rows, status })
}
fn reviewed(
    scope: &Scope,
    zone: Zone,
    entry: &Entry,
    file: &File,
    platform: &impl Platform,
    stop: &AtomicBool,
) -> Result<()> {
    let expected = entry
        .proof
        .as_ref()
        .ok_or("Elemento consultabile soltanto.")?;
    let result = classify(
        scope,
        zone,
        &entry.path,
        &entry.identity,
        file,
        platform,
        stop,
    )?;
    if result.as_ref().and_then(|(_, p)| p.as_ref()) != Some(expected) {
        return Err("Attribuzione, file o stato del servizio cambiati.".into());
    }
    Ok(())
}
enum Reply {
    Scanned(Report),
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
pub(crate) struct LegacyJunk {
    zone: Zone,
    report: Option<Report>,
    selected: HashSet<usize>,
    review: Option<Vec<usize>>,
    job: Option<Work>,
    notice: String,
}
impl LegacyJunk {
    fn reset(&mut self) {
        self.job = None;
        self.report = None;
        self.review = None;
        self.selected.clear();
        self.notice.clear();
    }
    fn work(
        &mut self,
        ctx: &egui::Context,
        mutation: bool,
        f: impl FnOnce(&AtomicBool) -> Result<Reply> + Send + 'static,
    ) {
        let (tx, rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let ctx = ctx.clone();
        self.job = Some(Work {
            stop: stop.clone(),
            rx,
            mutation,
        });
        std::thread::spawn(move || {
            let _ = tx.send(f(&stop));
            ctx.request_repaint();
        });
    }
    pub(crate) fn show(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        if let Some(job) = &self.job {
            match job.rx.try_recv() {
                Ok(value) => {
                    self.job = None;
                    match value {
                        Ok(Reply::Scanned(r)) => self.report = Some(r),
                        Ok(Reply::Done(s)) => self.notice = s,
                        Err(e) => self.notice = e,
                    }
                }
                Err(mpsc::TryRecvError::Empty) => {
                    ui.spinner();
                    ui.label(if job.mutation {
                        "Spostamento dei file confermati nel Cestino…"
                    } else {
                        "Analisi della cartella in corso…"
                    });
                    if !job.mutation && ui.button("Interrompi").clicked() {
                        self.reset();
                        self.notice = "Analisi interrotta.".into();
                    }
                    ctx.request_repaint_after(Duration::from_millis(140));
                    return;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.job = None;
                    self.notice = "Verifica interrotta. Controlla l’esito nel Finder.".into();
                }
            }
        }
        if !self.notice.is_empty() {
            ui.label(&self.notice);
        }
        if let (Some(indices), Some(report)) = (&self.review, &self.report) {
            ui.heading("Rivedi gli elementi");
            ui.label("Sposta i plist selezionati nel Cestino. Le impostazioni o il job launchd possono servire dopo una reinstallazione.");
            let mut go = false;
            let mut back = false;
            ui.horizontal_wrapped(|ui| {
                go = ui.button("Conferma spostamento nel Cestino").clicked();
                back = ui.button("Indietro").clicked();
            });
            egui::ScrollArea::vertical()
                .id_salt("legacy_review")
                .show_rows(ui, 45.0, indices.len(), |ui, range| {
                    for n in range {
                        let e = &report.rows[indices[n]];
                        ui.add(egui::Label::new(e.path.to_string_lossy()).truncate())
                            .on_hover_text(e.path.display().to_string());
                        ui.add(egui::Label::new(&e.reason).truncate())
                            .on_hover_text(&e.reason);
                    }
                });
            if back {
                self.review = None;
            }
            if go {
                let ids = self
                    .review
                    .take()
                    .unwrap()
                    .into_iter()
                    .collect::<HashSet<_>>();
                let rows: Vec<_> = self
                    .report
                    .take()
                    .unwrap()
                    .rows
                    .into_iter()
                    .enumerate()
                    .filter(|(i, e)| ids.contains(i) && e.proof.is_some())
                    .map(|(_, e)| e)
                    .collect();
                self.selected.clear();
                let zone = self.zone;
                self.work(ctx, true, move |stop| {
                    let scope = Scope::native();
                    let mut moved = 0;
                    for e in &rows {
                        if stop.load(Ordering::Acquire) {
                            break;
                        }
                        let result = trash::move_reviewed_file(&e.path, &e.identity, |file| {
                            reviewed(&scope, zone, e, file, &Native, stop)?;
                            if let Some(Proof::Agent { label, program }) = &e.proof {
                                trash::check_running(program, label, false)?;
                            }
                            Ok(())
                        });
                        if let Err(err) = result {
                            return Ok(Reply::Done(format!(
                                "{moved} di {} spostati. {}: {err}",
                                rows.len(),
                                e.path.display()
                            )));
                        }
                        moved += 1;
                    }
                    Ok(Reply::Done(format!(
                        "{moved} di {} spostati nel Cestino.",
                        rows.len()
                    )))
                });
            }
            return;
        }
        let old = self.zone;
        ui.horizontal_wrapped(|ui| {
            for z in [Zone::Preferences, Zone::Agents, Zone::Homes] {
                ui.selectable_value(&mut self.zone, z, z.title());
            }
        });
        if old != self.zone {
            self.reset();
        }
        ui.label(match self.zone{Zone::Preferences=>"Plist locali fino a 512 KiB. I domini senza app registrata sono indizi da verificare. Solo un plist corrotto attribuito a un’app firmata e chiusa può essere selezionato.",
            Zone::Agents=>"Job utente che indicano un eseguibile assoluto mancante in una posizione locale riconosciuta. Lo stato è verificato nei domini launchd GUI, utente e sistema.",
            Zone::Homes=>"Immagini .dmg, .sparseimage e .sparsebundle nella cartella di sistema Deleted Users. Lindo mostra i metadati e apre la posizione nel Finder."});
        if ui.button("Analizza questa cartella").clicked() {
            self.reset();
            let zone = self.zone;
            self.work(ctx, false, move |stop| {
                scan(&Scope::native(), zone, &Native, stop).map(Reply::Scanned)
            });
            return;
        }
        if let Some(r) = &self.report {
            ui.label(&r.status);
            if ui
                .add_enabled(
                    !self.selected.is_empty(),
                    egui::Button::new(format!("Rivedi {} selezionati", self.selected.len())),
                )
                .clicked()
            {
                let mut v: Vec<_> = self.selected.iter().copied().collect();
                v.sort_unstable();
                self.review = Some(v);
            }
            let mut reveal = None;
            egui::ScrollArea::vertical()
                .id_salt("legacy_list")
                .show_rows(ui, 48.0, r.rows.len(), |ui, range| {
                    for i in range {
                        let e = &r.rows[i];
                        ui.push_id(i, |ui| {
                            ui.horizontal(|ui| {
                                if e.proof.is_some() {
                                    let mut checked = self.selected.contains(&i);
                                    if crate::extras::file_checkbox(ui, &mut checked, true, &e.path)
                                        .changed()
                                    {
                                        if checked {
                                            self.selected.insert(i);
                                        } else {
                                            self.selected.remove(&i);
                                        }
                                    }
                                }
                                if ui.small_button("Finder").clicked() {
                                    reveal = Some((e.path.clone(), e.identity.clone()));
                                }
                                if let Some(n) = e.bytes {
                                    ui.label(format!("{:.1} KiB", n as f64 / 1024.));
                                }
                                ui.add(
                                    egui::Label::new(
                                        e.path.file_name().unwrap_or_default().to_string_lossy(),
                                    )
                                    .truncate(),
                                )
                                .on_hover_text(e.path.display().to_string());
                            });
                            ui.add(egui::Label::new(&e.reason).truncate())
                                .on_hover_text(&e.reason);
                        });
                    }
                });
            if let Some((path, id)) = reveal {
                self.work(ctx, false, move |stop| {
                    if !id.matches(&path) {
                        return Err("Elemento cambiato.".into());
                    }
                    let r = command::run(
                        "/usr/bin/open",
                        &["-R", path.to_str().ok_or("Percorso non rappresentabile.")?],
                        None,
                        stop,
                        8,
                    )?;
                    if r.code != Some(0) {
                        return Err("Finder non disponibile.".into());
                    }
                    Ok(Reply::Done("Richiesta inviata al Finder.".into()))
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::symlink};
    struct Mock {
        active: bool,
        team: &'static str,
    }
    impl Platform for Mock {
        fn registration(&self, id: &str) -> Registration {
            if id == "org.example.app" {
                Registration::Unique("/Applications/Fixture.app".into())
            } else {
                Registration::Missing
            }
        }
        fn signature(&self, _: &Path, _: &AtomicBool) -> Result<(String, String, String)> {
            Ok((
                "org.example.app".into(),
                self.team.into(),
                "fixture-hash".into(),
            ))
        }
        fn inactive(&self, _: &str, _: &AtomicBool) -> bool {
            !self.active
        }
    }
    fn area() -> (tempfile::TempDir, Scope) {
        let tmp = tempfile::Builder::new()
            .prefix("lindo-legacy-")
            .tempdir()
            .unwrap();
        let home = tmp.path().canonicalize().unwrap();
        let scope = Scope {
            deleted: home.join("archived"),
            home,
        };
        for z in [Zone::Preferences, Zone::Agents, Zone::Homes] {
            fs::create_dir_all(scope.root(z)).unwrap();
        }
        (tmp, scope)
    }
    fn xml(fields: &str) -> String {
        format!(r#"<?xml version="1.0"?><plist version="1.0"><dict>{fields}</dict></plist>"#)
    }
    fn job(label: &str, program: &Path, extras: &str) -> String {
        xml(&format!("<key>Label</key><string>{label}</string><key>ProgramArguments</key><array><string>{}</string></array>{extras}",program.display()))
    }
    #[test]
    fn preferences_require_corrupt_data_signed_app_and_unchanged_identity() {
        let (_tmp, scope) = area();
        let root = scope.root(Zone::Preferences);
        let p = root.join("org.example.app.plist");
        fs::write(&p, b"bplist00\x00\xffbroken").unwrap();
        fs::write(root.join("com.apple.test.plist"), b"broken").unwrap();
        let unknown = root.join("org.example.old.plist");
        fs::write(&unknown, xml("")).unwrap();
        File::options()
            .write(true)
            .open(&unknown)
            .unwrap()
            .set_modified(SystemTime::now() - Duration::from_secs(190 * 86400))
            .unwrap();
        symlink(&p, root.join("org.example.link.plist")).unwrap();
        let stop = AtomicBool::new(false);
        let mock = Mock {
            active: false,
            team: "FIXTURE123",
        };
        let report = scan(&scope, Zone::Preferences, &mock, &stop).unwrap();
        assert_eq!(report.rows.len(), 2, "{}", report.status);
        let e = &report.rows[0];
        assert!(e.proof.is_some());
        assert_eq!(e.path, p);
        assert!(report.rows[1].proof.is_none());
        let check = |mock: &Mock| {
            reviewed(
                &scope,
                Zone::Preferences,
                e,
                &File::open(&p).unwrap(),
                mock,
                &stop,
            )
        };
        assert!(check(&mock).is_ok());
        assert!(check(&Mock {
            active: false,
            team: "NEWTEAM123"
        })
        .is_err());
        fs::write(&p, xml("")).unwrap();
        assert!(check(&mock).is_err());
    }
    #[test]
    fn launch_agents_recheck_label_job_and_missing_executable() {
        let (_tmp, scope) = area();
        let folder = scope.home.join("Applications");
        fs::create_dir(&folder).unwrap();
        let program = folder.join("missing");
        let root = scope.root(Zone::Agents);
        let label = "org.example.fixture";
        let path = root.join(format!("{label}.plist"));
        fs::write(&path, job(label, &program, "")).unwrap();
        fs::write(
            root.join("org.example.other.plist"),
            job(label, &program, ""),
        )
        .unwrap();
        fs::write(
            root.join("org.example.chroot.plist"),
            job(
                "org.example.chroot",
                &program,
                "<key>RootDirectory</key><string>/somewhere</string>",
            ),
        )
        .unwrap();
        fs::write(
            root.join("org.example.relative.plist"),
            job("org.example.relative", Path::new("relative"), ""),
        )
        .unwrap();
        let stop = AtomicBool::new(false);
        let mock = Mock {
            active: false,
            team: "MOCK",
        };
        let report = scan(&scope, Zone::Agents, &mock, &stop).unwrap();
        assert_eq!(report.rows.len(), 2);
        let e = &report.rows[0];
        assert_eq!(e.path, path);
        assert!(e.proof.is_some());
        assert!(report.rows[1].proof.is_none());
        let check = |m: &Mock| {
            reviewed(
                &scope,
                Zone::Agents,
                e,
                &File::open(&path).unwrap(),
                m,
                &stop,
            )
        };
        assert!(check(&mock).is_ok());
        assert!(check(&Mock {
            active: true,
            team: "MOCK"
        })
        .is_err());
        fs::write(&program, b"reappeared").unwrap();
        assert!(check(&mock).is_err());
    }
    #[test]
    fn deleted_home_images_are_metadata_only_and_never_removable() {
        let (_tmp, scope) = area();
        let dir = scope.root(Zone::Homes);
        let sparse = dir.join("fixture.sparsebundle");
        fs::create_dir(&sparse).unwrap();
        fs::write(sparse.join("inside.dmg"), b"private fixture").unwrap();
        fs::write(dir.join("synthetic.dmg"), b"sample").unwrap();
        fs::write(dir.join("not-an-image.txt"), b"ignored").unwrap();
        symlink(&sparse, dir.join("linked.sparsebundle")).unwrap();
        let report = scan(
            &scope,
            Zone::Homes,
            &Mock {
                active: false,
                team: "MOCK",
            },
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(report.rows.len(), 2);
        assert!(report
            .rows
            .iter()
            .all(|e| e.proof.is_none() && e.path.parent() == Some(&dir)));
        assert!(report.rows.iter().any(|e| e.bytes.is_none()));
        assert!(scan(
            &scope,
            Zone::Homes,
            &Mock {
                active: false,
                team: "MOCK"
            },
            &AtomicBool::new(true)
        )
        .is_err());
    }
}
