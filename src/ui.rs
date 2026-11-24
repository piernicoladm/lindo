#[path = "ui_artwork.rs"]
mod artwork;
#[path = "ui_design.rs"]
mod design;
#[path = "ui_navigation.rs"]
mod navigation;
use navigation::{extra_destination, mode_destination};
#[path = "ui_overview.rs"]
mod overview;
#[path = "ui_results.rs"]
mod results;
use crate::model::{Mode, Progress, Report, Request, COMPARE, HASH, TREE};
use eframe::egui;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

const MODES: [Mode; 6] = [
    Mode::Space,
    Mode::Cache,
    Mode::Logs,
    Mode::Large,
    Mode::Duplicates,
    Mode::Apps,
];

fn title(mode: Mode) -> &'static str {
    match mode {
        Mode::Space => "Mappa dello spazio",
        Mode::Large => "File grandi",
        Mode::Duplicates => "Duplicati",
        Mode::Cache => "Cache dei browser",
        Mode::Logs => "Log",
        Mode::Junk => "Pulizia avanzata",
        Mode::Apps => "Applicazioni",
    }
}

fn hint(mode: Mode) -> &'static str {
    match mode {
        Mode::Space => "Misura le cartelle e approfondisci la mappa in sola lettura.",
        Mode::Large => "Trova i file che superano la soglia scelta.",
        Mode::Duplicates => "Confronta copie identiche e scegli quale conservare per ogni gruppo.",
        Mode::Cache => {
            "Analizza le cache HTTP riconosciute. Chiudi i browser prima di procedere al Cestino."
        }
        Mode::Logs => "Esamina i log del tuo utente modificati da almeno sette giorni.",
        Mode::Junk => "Esamina cache e file di supporto delle applicazioni.",
        Mode::Apps => "Seleziona singolarmente i bundle delle applicazioni da rimuovere.",
    }
}

fn item_count(n: usize) -> String {
    format!("{n} {}", if n == 1 { "elemento" } else { "elementi" })
}

pub(crate) fn size(bytes: u64) -> String {
    for (unit, divisor) in [
        ("TB", 1_000_000_000_000u64),
        ("GB", 1_000_000_000),
        ("MB", 1_000_000),
        ("KB", 1_000),
    ] {
        if bytes >= divisor {
            let tenths =
                bytes / (divisor / 10) + u64::from((bytes % (divisor / 10)) >= divisor / 20);
            return if tenths.is_multiple_of(10) {
                format!("{} {unit}", tenths / 10)
            } else {
                format!("{},{} {unit}", tenths / 10, tenths % 10)
            };
        }
    }
    format!("{bytes} B")
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
}

fn initial_request(mode: Mode) -> Request {
    let relative = match mode {
        Mode::Space => Some("Downloads"),
        Mode::Cache => Some("Library/Caches"),
        Mode::Logs => Some("Library/Logs"),
        Mode::Apps => Some("Applications"),
        _ => None,
    };
    let mut roots: Vec<_> = relative
        .and_then(|p| home().map(|h| h.join(p)))
        .into_iter()
        .collect();
    if mode == Mode::Apps {
        roots.insert(0, PathBuf::from("/Applications"));
    }
    Request {
        mode,
        roots,
        min_bytes: if mode == Mode::Large { 100_000_000 } else { 0 },
    }
}

struct Results {
    report: Report,
    names: Vec<String>,
    by_path: Vec<usize>,
    groups: HashMap<u64, Vec<usize>>,
    total: u64,
}

impl Results {
    fn prepare(mode: Mode, mut report: Report) -> Self {
        if mode == Mode::Duplicates {
            let mut extra_bytes: HashMap<Option<u64>, u64> = HashMap::new();
            for item in &report.items {
                extra_bytes
                    .entry(item.group)
                    .and_modify(|sum| *sum = sum.saturating_add(item.bytes))
                    .or_insert(0);
            }
            report.items.sort_unstable_by(|a, b| {
                extra_bytes[&b.group]
                    .cmp(&extra_bytes[&a.group])
                    .then(a.group.cmp(&b.group))
                    .then(a.path.cmp(&b.path))
            });
        } else {
            report
                .items
                .sort_unstable_by(|a, b| b.bytes.cmp(&a.bytes).then(a.path.cmp(&b.path)));
        }
        let mut groups: HashMap<u64, Vec<usize>> = HashMap::new();
        let mut total = 0u64;
        let names = report
            .items
            .iter()
            .enumerate()
            .map(|(i, item)| {
                if let Some(g) = item.group {
                    groups.entry(g).or_default().push(i);
                }
                total = total.saturating_add(item.bytes);
                item.path.to_string_lossy().to_lowercase()
            })
            .collect();
        let mut by_path: Vec<_> = groups.values().collect();
        by_path.sort_unstable_by(|a, b| report.items[a[0]].path.cmp(&report.items[b[0]].path));
        let by_path = by_path.into_iter().flatten().copied().collect();
        Self {
            report,
            by_path,
            names,
            groups,
            total,
        }
    }
    fn filtered(&self, mode: Mode, query: &str) -> (Vec<usize>, Vec<usize>) {
        let needle = query.trim().to_lowercase();
        let mut rows = Vec::new();
        let mut by_path = Vec::new();
        if mode == Mode::Duplicates {
            let groups: HashSet<_> = self
                .names
                .iter()
                .enumerate()
                .filter(|(_, name)| name.contains(&needle))
                .filter_map(|(i, _)| self.report.items[i].group)
                .collect();
            rows.extend(
                self.report
                    .items
                    .iter()
                    .enumerate()
                    .filter(|(_, item)| item.group.is_some_and(|g| groups.contains(&g)))
                    .map(|(i, _)| i),
            );
            by_path.extend(self.by_path.iter().copied().filter(|&i| {
                self.report.items[i]
                    .group
                    .is_some_and(|g| groups.contains(&g))
            }));
        } else {
            rows.extend(
                self.names
                    .iter()
                    .enumerate()
                    .filter(|(_, name)| name.contains(&needle))
                    .map(|(i, _)| i),
            );
        }
        (rows, by_path)
    }
}

// Reclaim the many independent paths, snapshots and group vectors on one
// in-process worker, even if the final Arc is released while painting a frame.
type RetiredRows = (Report, Vec<String>, HashMap<u64, Vec<usize>>);

impl Drop for Results {
    fn drop(&mut self) {
        if self.report.items.len() < 1024 && self.report.space.len() < 1024 {
            return;
        }
        static RECLAIM: std::sync::OnceLock<mpsc::Sender<RetiredRows>> = std::sync::OnceLock::new();
        let sender = RECLAIM.get_or_init(|| {
            let (sender, receiver) = mpsc::channel::<RetiredRows>();
            std::thread::spawn(move || {
                for rows in receiver {
                    drop(rows);
                }
            });
            sender
        });
        let retired = (
            std::mem::take(&mut self.report),
            std::mem::take(&mut self.names),
            std::mem::take(&mut self.groups),
        );
        let _ = sender.send(retired);
    }
}

struct Page {
    req: Request,
    result: Option<Arc<Results>>,
    query: String,
    visible: Vec<usize>,
    visible_by_path: Vec<usize>,
    sort_by_path: bool,
    applied: String,
    selected: HashSet<usize>,
    selected_visible: usize,
    selected_bytes: u64,
    keepers: HashMap<u64, usize>,
    history: Vec<PathBuf>,
}

impl Page {
    fn choose_root(&mut self, path: PathBuf, append: bool) {
        if append {
            if self.req.roots.iter().any(|root| path.starts_with(root)) {
                return;
            }
            self.req.roots.retain(|root| !root.starts_with(&path));
            self.req.roots.push(path);
        } else {
            if self.req.roots == [path.clone()] {
                return;
            }
            self.req.roots = vec![path];
        }
        self.history.clear();
        self.invalidate();
    }

    fn new(mode: Mode) -> Self {
        Self {
            req: initial_request(mode),
            result: None,
            query: String::new(),
            visible: vec![],
            visible_by_path: vec![],
            sort_by_path: false,
            applied: String::new(),
            selected: HashSet::new(),
            selected_visible: 0,
            selected_bytes: 0,
            keepers: HashMap::new(),
            history: vec![],
        }
    }
    fn invalidate(&mut self) {
        self.result = None;
        self.keepers.clear();
        self.selected.clear();
        self.visible.clear();
        self.visible_by_path.clear();
        self.applied.clear();
        self.selected_visible = 0;
        self.selected_bytes = 0;
    }
    fn install(&mut self, result: Arc<Results>) {
        self.invalidate();
        self.result = Some(result);
    }
    fn is_visible(&self, i: usize) -> bool {
        self.applied.is_empty() || self.visible.binary_search(&i).is_ok()
    }
    fn count(&self) -> usize {
        if self.applied.is_empty() {
            self.result.as_ref().map_or(0, |r| r.report.items.len())
        } else {
            self.visible.len()
        }
    }
    fn row_id(&self, row: usize) -> usize {
        match (self.applied.is_empty(), self.sort_by_path) {
            (true, true) => self.result.as_ref().unwrap().by_path[row],
            (true, false) => row,
            (false, true) => self.visible_by_path[row],
            (false, false) => self.visible[row],
        }
    }
    fn keeper(&self, group: u64) -> Option<usize> {
        self.keepers
            .get(&group)
            .copied()
            .or_else(|| self.result.as_ref()?.groups.get(&group)?.first().copied())
    }
    fn select(&mut self, id: usize, needed: bool) {
        let Some(item) = self.result.as_ref().and_then(|r| r.report.items.get(id)) else {
            return;
        };
        let bytes = item.bytes;
        if needed && self.selected.insert(id) {
            self.selected_bytes = self.selected_bytes.saturating_add(bytes);
            if self.is_visible(id) {
                self.selected_visible += 1;
            }
        } else if !needed && self.selected.remove(&id) {
            self.selected_bytes = self.selected_bytes.saturating_sub(bytes);
            if self.is_visible(id) {
                self.selected_visible -= 1;
            }
        }
    }
    fn select_visible(&mut self) {
        let Some(result) = self.result.clone() else {
            return;
        };
        self.selected
            .reserve(self.count().saturating_sub(self.selected.len()));
        // Each row here is already visible; no per-row membership lookup is needed.
        for row in 0..self.count() {
            let i = self.row_id(row);
            let protected = result.report.items[i].group.and_then(|g| self.keeper(g)) == Some(i);
            if !protected && self.selected.insert(i) {
                self.selected_bytes = self
                    .selected_bytes
                    .saturating_add(result.report.items[i].bytes);
                self.selected_visible += 1;
            }
        }
    }
}

#[derive(Clone)]
struct Review {
    label: String,
    req: Request,
    data: Arc<Results>,
    moves: Vec<usize>,
    rows: Vec<(usize, bool)>,
    keepers: HashMap<u64, usize>,
    total: u64,
    cloud: bool,
}

impl Review {
    fn from_proposal(proposal: crate::extras::Proposal) -> Option<Self> {
        let crate::extras::Proposal {
            request,
            label,
            mut items,
            retained,
        } = proposal;
        if items.is_empty() {
            return None;
        }
        let mut paths = HashSet::new();
        if !items
            .iter()
            .chain(retained.iter())
            .all(|v| paths.insert(&v.path))
        {
            return None;
        }
        let count = items.len();
        let total = items
            .iter()
            .fold(0_u64, |sum, row| sum.saturating_add(row.bytes));
        let cloud = items.iter().any(|row| row.cloud);
        let mut keepers = HashMap::new();
        for (i, row) in retained.iter().enumerate() {
            if keepers.insert(row.group?, count + i).is_some() {
                return None;
            }
        }
        if items
            .iter()
            .filter_map(|row| row.group)
            .any(|g| !keepers.contains_key(&g))
        {
            return None;
        }
        let moves: Vec<_> = (0..count).collect();
        items.extend(retained);
        let rows = (0..items.len()).map(|i| (i, i >= count)).collect();
        let data = Arc::new(Results {
            report: Report {
                items,
                ..Default::default()
            },
            names: Vec::new(),
            by_path: Vec::new(),
            groups: HashMap::new(),
            total,
        });
        Some(Self {
            req: request,
            label,
            data,
            moves,
            rows,
            keepers,
            total,
            cloud,
        })
    }

    fn snapshot(page: &Page) -> Option<Self> {
        let data = page.result.clone()?;
        let mut moves: Vec<_> = page.selected.iter().copied().collect();
        moves.sort_unstable();
        let mut touched = HashSet::new();
        let mut total = 0u64;
        let mut cloud = false;
        for &i in &moves {
            let item = &data.report.items[i];
            if let Some(g) = item.group {
                touched.insert(g);
            }
            total = total.saturating_add(item.bytes);
            cloud |= item.cloud;
        }
        let mut rows = Vec::new();
        if page.req.mode == Mode::Duplicates {
            let mut ordered_groups: Vec<_> = touched.iter().copied().collect();
            ordered_groups.sort_unstable_by_key(|g| data.groups[g][0]);
            for group in ordered_groups {
                let keeper = page.keeper(group)?;
                rows.push((keeper, true));
                let ids = &data.groups[&group];
                rows.extend(
                    ids.iter()
                        .copied()
                        .filter(|i| page.selected.contains(i))
                        .map(|i| (i, false)),
                );
                rows.extend(
                    ids.iter()
                        .copied()
                        .filter(|&i| i != keeper && !page.selected.contains(&i))
                        .map(|i| (i, true)),
                );
            }
        } else {
            rows.extend(moves.iter().copied().map(|i| (i, false)));
        }
        Some(Self {
            label: title(page.req.mode).into(),
            req: page.req.clone(),
            data,
            moves,
            rows,
            keepers: touched
                .into_iter()
                .filter_map(|g| page.keeper(g).map(|i| (g, i)))
                .collect(),
            total,
            cloud,
        })
    }
}

#[derive(Default)]
struct TrashSummary {
    moved: usize,
    failures: Vec<String>,
    stale: HashSet<Mode>,
}

impl TrashSummary {
    fn record_move(&mut self, path: &Path, scopes: &[(Mode, Vec<PathBuf>)]) {
        self.moved += 1;
        for (mode, roots) in scopes {
            if !self.stale.contains(mode)
                && roots
                    .iter()
                    .any(|root| path.starts_with(root) || root.starts_with(path))
            {
                self.stale.insert(*mode);
            }
        }
    }
}

enum Event {
    Scan(u64, Mode, Arc<Results>),
    Filter(Mode, Arc<Results>, String, (Vec<usize>, Vec<usize>)),
    Trash(Mode, TrashSummary),
    Disk(Option<(u64, u64, String)>),
    Reveal(Result<(), String>),
}

struct Activity {
    id: u64,
    mode: Mode,
    stop: Arc<AtomicBool>,
    progress: Arc<Progress>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Extra {
    Clutter,
    Junk,
    System,
    Accounts,
    Remote,
    Updates,
    Reset,
    Versions,
    Legacy,
    MailTrash,
    SystemJunk,
    Trim,
}
const EXTRAS: [(Extra, &str); 12] = [
    (Extra::Clutter, "Download e immagini"),
    (Extra::Junk, "Pulizia avanzata"),
    (Extra::System, "Strumenti di sistema"),
    (Extra::Accounts, "Mail e browser"),
    (Extra::Remote, "Archivi cloud"),
    (Extra::Updates, "Aggiornamenti app"),
    (Extra::Reset, "Ripristino app"),
    (Extra::Versions, "Revisioni documento"),
    (Extra::Legacy, "Preferenze e avvii obsoleti"),
    (Extra::MailTrash, "Cestino di Mail"),
    (Extra::SystemJunk, "Cache e log di sistema"),
    (Extra::Trim, "Alleggerisci un’app"),
];

struct Lindo {
    artwork: artwork::Artwork,
    home_panel: overview::HomePanel,
    destination: design::Destination,
    searching: bool,
    search_origin: Option<(design::Destination, Option<Mode>, Option<Extra>)>,
    extra: Option<Extra>,
    assessment: crate::assessment::Assessment,
    reset: crate::app_reset::AppReset,
    versions: crate::file_versions::FileVersions,
    legacy: crate::legacy_junk::LegacyJunk,
    trim: crate::bundle_trim::BundleTrim,
    mail_trash: crate::mail_trash::MailTrash,
    system_junk: crate::system_junk::SystemJunk,
    vendor: crate::vendor_updates::VendorUpdates,
    remote: crate::remote::Remote,
    accounts: crate::accounts::Accounts,
    desktop: crate::desktop::Desktop,
    baselines: HashMap<PathBuf, u64>,
    tool_query: String,
    favorites: HashSet<String>,
    clutter: crate::clutter::Clutter,
    junk: crate::junk::Junk,
    system: crate::system_tools::SystemTools,
    quick: std::collections::VecDeque<Request>,
    pages: HashMap<Mode, Page>,
    mode: Option<Mode>,
    tx: mpsc::Sender<Event>,
    rx: mpsc::Receiver<Event>,
    active: Option<Activity>,
    workers: usize,
    next_id: u64,
    picker: crate::picker::Picker,
    append_root: bool,
    review: Option<Review>,
    confirm: bool,
    cleaning: bool,
    revealing: bool,
    status: String,
    failures: Vec<String>,
    disk: Option<(u64, u64)>,
    volume_label: String,
    disk_pending: bool,
    disk_measured: std::time::Instant,
    filtering: bool,
    fixture: Option<Fixture>,
}

fn disk_usage() -> Option<(u64, u64, String)> {
    use std::os::unix::ffi::OsStrExt;
    let path = home().unwrap_or_else(|| PathBuf::from("/"));
    let cpath = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut out = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // statvfs initializes out only on success; the CString remains live for the call.
    let stat = unsafe {
        if libc::statvfs(cpath.as_ptr(), out.as_mut_ptr()) != 0 {
            return None;
        }
        out.assume_init()
    };
    let unit = stat.f_frsize as u64;
    let label = objc2::rc::autoreleasepool(|_| {
        use objc2_foundation::{NSString, NSURLVolumeLocalizedNameKey, NSURL};
        let url = NSURL::fileURLWithPath(&NSString::from_str(path.to_str()?));
        let mut value = None;
        // Foundation documents the localized volume name as an NSString resource.
        unsafe { url.getResourceValue_forKey_error(&mut value, NSURLVolumeLocalizedNameKey) }
            .ok()?;
        Some(value?.downcast_ref::<NSString>()?.to_string())
    })
    .unwrap_or_else(|| "Disco dell’utente".into());
    Some((
        (stat.f_blocks as u64).saturating_mul(unit),
        (stat.f_bavail as u64).saturating_mul(unit),
        label,
    ))
}

impl Lindo {
    fn favorite_tools() -> HashSet<String> {
        let prefs = objc2_foundation::NSUserDefaults::standardUserDefaults();
        MODES
            .into_iter()
            .map(title)
            .chain(EXTRAS.iter().map(|(_, label)| *label))
            .filter(|name| {
                prefs.boolForKey(&objc2_foundation::NSString::from_str(&format!(
                    "tool.favorite.{name}"
                )))
            })
            .map(str::to_owned)
            .collect()
    }
    fn new(ctx: &egui::Context) -> Self {
        let (tx, rx) = mpsc::channel();
        let sender = tx.clone();
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let _ = sender.send(Event::Disk(disk_usage()));
            repaint.request_repaint();
        });
        Self {
            artwork: artwork::Artwork::new(ctx),
            home_panel: Default::default(),
            destination: design::Destination::Home,
            searching: false,
            search_origin: None,
            pages: MODES.into_iter().map(|m| (m, Page::new(m))).collect(),
            tool_query: String::new(),
            favorites: Self::favorite_tools(),
            reset: Default::default(),
            versions: Default::default(),
            legacy: Default::default(),
            trim: Default::default(),
            mail_trash: Default::default(),
            system_junk: Default::default(),
            vendor: Default::default(),
            remote: Default::default(),
            accounts: Default::default(),
            desktop: Default::default(),
            baselines: Default::default(),
            assessment: Default::default(),
            extra: None,
            clutter: Default::default(),
            junk: Default::default(),
            system: Default::default(),
            quick: Default::default(),
            mode: None,
            tx,
            rx,
            active: None,
            workers: 0,
            next_id: 0,
            picker: crate::picker::Picker::new(),
            append_root: false,
            review: None,
            confirm: false,
            cleaning: false,
            revealing: false,
            status: String::new(),
            failures: vec![],
            disk: None,
            volume_label: "Disco dell’utente".into(),
            disk_pending: true,
            disk_measured: std::time::Instant::now(),
            filtering: false,
            fixture: None,
        }
    }
    fn receive(&mut self) {
        while let Ok(event) = self.rx.try_recv() {
            match event {
                Event::Scan(id, mode, result) => {
                    self.workers = self.workers.saturating_sub(1);
                    if self.active.as_ref().is_some_and(|a| a.id == id) {
                        self.active = None;
                        self.assessment.record_core(mode, &result.report);
                        self.status = overview::scan_status(mode, &result.report);
                        self.pages.get_mut(&mode).unwrap().install(result);
                    }
                }
                Event::Filter(mode, source, query, rows) => {
                    self.filtering = false;
                    let page = self.pages.get_mut(&mode).unwrap();
                    if page
                        .result
                        .as_ref()
                        .is_some_and(|r| Arc::ptr_eq(r, &source))
                        && page.query == query
                    {
                        page.applied = query;
                        (page.visible, page.visible_by_path) = rows;
                        page.selected_visible = page
                            .selected
                            .iter()
                            .filter(|&&i| page.is_visible(i))
                            .count();
                    }
                }
                Event::Trash(origin, mut summary) => {
                    self.cleaning = false;
                    if summary.moved > 0 {
                        self.clutter = Default::default();
                        self.junk = Default::default();
                    }
                    if summary.moved == 0 {
                        summary.stale.insert(origin);
                        self.status = format!(
                            "Nessun elemento spostato. Ripeti l’analisi in {}.",
                            title(origin)
                        );
                    } else {
                        self.status = format!(
                            "Spostati nel Cestino: {}. Ripeti l’analisi degli ambiti interessati.",
                            summary.moved
                        );
                    }
                    self.failures = summary.failures;
                    for mode in summary.stale {
                        if let Some(page) = self.pages.get_mut(&mode) {
                            page.invalidate();
                        }
                    }
                }
                Event::Disk(value) => {
                    self.disk = value.map(|(total, free, label)| {
                        self.volume_label = label;
                        (total, free)
                    });
                    self.disk_pending = false;
                    self.disk_measured = std::time::Instant::now();
                }
                Event::Reveal(result) => {
                    self.revealing = false;
                    if let Err(e) = result {
                        self.status = e;
                    }
                }
            }
        }
    }
    fn available(&self) -> bool {
        self.active.is_none() && !self.cleaning && !self.picker.visible() && self.workers < 2
    }
    fn launch_scan(&mut self, mode: Mode, ctx: &egui::Context) {
        if !self.available() {
            return;
        }
        let page = self.pages.get_mut(&mode).unwrap();
        if page.req.roots.is_empty() {
            return;
        }
        page.invalidate();
        let req = page.req.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(Progress::default());
        let id = self.next_id;
        self.next_id += 1;
        self.active = Some(Activity {
            id,
            mode,
            stop: stop.clone(),
            progress: progress.clone(),
        });
        self.workers += 1;
        self.status.clear();
        let sender = self.tx.clone();
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            crate::scan::set_worker_qos();
            let report = crate::scan::run(&req, &stop, &progress);
            let result = if stop.load(Ordering::Acquire) {
                // Discard the interrupted scan on this worker before indexing any rows.
                drop(report);
                Results::prepare(mode, Report::default())
            } else {
                Results::prepare(mode, report)
            };
            let _ = sender.send(Event::Scan(id, mode, Arc::new(result)));
            repaint.request_repaint();
        });
    }
    fn cancel(&mut self) {
        self.assessment.cancel();
        self.quick.clear();
        if let Some(activity) = self.active.take() {
            activity.stop.store(true, Ordering::Release);
            self.status =
                "Analisi interrotta. I risultati incompleti sono stati scartati.".to_owned();
        }
    }
    fn reveal(&mut self, path: PathBuf, ctx: &egui::Context) {
        if self.revealing {
            return;
        }
        self.revealing = true;
        let sender = self.tx.clone();
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let result = std::process::Command::new("/usr/bin/open")
                .arg("-R")
                .arg(&path)
                .status()
                .map_err(|e| format!("Finder: {e}"))
                .and_then(|s| {
                    if s.success() {
                        Ok(())
                    } else {
                        Err(format!("Finder non ha aperto {}", path.display()))
                    }
                });
            let _ = sender.send(Event::Reveal(result));
            repaint.request_repaint();
        });
    }
    fn trash(&mut self, review: Review, ctx: &egui::Context) {
        self.cleaning = true;
        self.confirm = false;
        // Scope controls are disabled until completion. The worker checks only
        // these few roots, without walking any indexed rows on the UI thread.
        let scopes: Vec<_> = self
            .pages
            .iter()
            .filter(|(_, page)| page.result.is_some())
            .map(|(&mode, page)| (mode, page.req.roots.clone()))
            .collect();
        let sender = self.tx.clone();
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let mut summary = TrashSummary::default();
            let items = &review.data.report.items;
            for &i in &review.moves {
                let item = &items[i];
                let keeper = item
                    .group
                    .and_then(|g| review.keepers.get(&g))
                    .map(|&i| &items[i]);
                match crate::trash::move_one(&review.req, item, keeper) {
                    Ok(outcome) => {
                        summary.record_move(&item.path, &scopes);
                        if matches!(outcome, crate::trash::MoveOutcome::KeeperChanged) {
                            summary.failures.push(format!("{} spostato nel Cestino. La copia protetta non è più verificabile dopo lo spostamento. Operazione interrotta.", item.path.display()));
                            break;
                        }
                    }
                    Err(reason) => {
                        summary
                            .failures
                            .push(format!("{} · {reason}", item.path.display()));
                        if item.group.is_some() {
                            summary.failures.push(
                                "Operazione interrotta. Ripeti l’analisi delle copie.".to_owned(),
                            );
                            break;
                        }
                    }
                }
            }
            let _ = sender.send(Event::Trash(review.req.mode, summary));
            repaint.request_repaint();
        });
    }
}

impl Lindo {
    fn refresh_filter(&mut self, mode: Mode, ctx: &egui::Context) {
        let page = self.pages.get_mut(&mode).unwrap();
        if page.applied == page.query {
            return;
        }
        if page.query.is_empty() {
            page.applied.clear();
            page.visible.clear();
            page.visible_by_path.clear();
            page.selected_visible = page.selected.len();
            return;
        }
        if self.filtering {
            return;
        }
        let Some(source) = page.result.clone() else {
            return;
        };
        let query = page.query.clone();
        let sender = self.tx.clone();
        let repaint = ctx.clone();
        self.filtering = true;
        std::thread::spawn(move || {
            let rows = source.filtered(mode, &query);
            let _ = sender.send(Event::Filter(mode, source, query, rows));
            repaint.request_repaint();
        });
    }

    fn request_disk_usage(&mut self, ctx: &egui::Context) {
        if self.disk_pending {
            return;
        }
        self.disk_pending = true;
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Event::Disk(disk_usage()));
            ctx.request_repaint();
        });
    }
}

#[derive(Default)]
struct Fixture {
    launched: Option<std::time::Instant>,
    window_start: Option<std::time::Instant>,
    frames: usize,
    warmup_frames: usize,
    last_frame: Option<std::time::Instant>,
    previous_phase: usize,
    gap: [Duration; 3],
    work: [Duration; 3],
    first_frame: Option<std::time::Instant>,
    last_sample: Option<std::time::Instant>,
    result_frames: usize,
}

impl Lindo {
    fn fixture_frame(&mut self, ctx: &egui::Context, entered: std::time::Instant) {
        let fixture = self.fixture.as_mut().expect("active UI probe");
        let mode = self.mode.unwrap();
        let result = self.pages[&mode].result.as_ref();
        let now = std::time::Instant::now();
        let first = *fixture.first_frame.get_or_insert(entered);
        let phase = if fixture.launched.is_none() {
            0
        } else if result.is_none() {
            1
        } else {
            2
        };
        fixture.work[phase] = fixture.work[phase].max(now.duration_since(entered));
        if ctx.will_discard() {
            return;
        }

        if let Some(prev) = fixture.last_frame.replace(entered) {
            let previous = fixture.previous_phase;
            fixture.gap[previous] = fixture.gap[previous].max(entered.duration_since(prev));
        }
        fixture.previous_phase = phase;
        fixture.frames += 1;
        let last_sample = fixture.last_sample.get_or_insert(now);
        if now.duration_since(*last_sample) >= Duration::from_secs(10) {
            if let Some(active) = &self.active {
                println!(
                    "LINDO_UI_PROGRESS seconds={} visited={} hashed={} compared={}",
                    first.elapsed().as_secs(),
                    active.progress.visited.load(Ordering::Relaxed),
                    active.progress.hashed_bytes.load(Ordering::Relaxed),
                    active.progress.compared_bytes.load(Ordering::Relaxed)
                );
            }
            *last_sample = now;
        }
        if let Some(start) = fixture.launched {
            if let Some(r) = result {
                fixture.result_frames += 1;
                if fixture.result_frames >= 4 {
                    let ok =
                        !r.report.cancelled && r.report.errors == 0 && !r.report.items.is_empty();
                    let g = fixture.gap.map(|v| v.as_millis());
                    let w = fixture.work.map(|v| v.as_millis());
                    println!(
                        concat!(
                            "LINDO_UI_QA version={} mode={:?} ok={} frames={} elapsed_ms={} longest_gap_ms={} ",
                            "opening_ms={} first_frame_ms={} warmup_frames={} opening_gap_ms={} scanning_gap_ms={} results_gap_ms={} ",
                            "opening_work_ms={} scanning_work_ms={} results_work_ms={} result_frames={} items={} groups={} visited={} errors={} cancelled={}"
                        ),
                        env!("CARGO_PKG_VERSION"),
                        mode,
                        ok,
                        fixture.frames,
                        start.elapsed().as_millis(),
                        g[1].max(g[2]),
                        start.duration_since(first).as_millis(),
                        fixture
                            .window_start
                            .map_or(0, |opening| first.duration_since(opening).as_millis()),
                        fixture.warmup_frames,
                        g[0],
                        g[1],
                        g[2],
                        w[0],
                        w[1],
                        w[2],
                        fixture.result_frames,
                        r.report.items.len(),
                        r.groups.len(),
                        r.report.visited,
                        r.report.errors,
                        r.report.cancelled
                    );
                    self.fixture = None;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        } else {
            fixture.warmup_frames += 1;
            // Exercise the same scan action a user invokes from a painted window.
            // Keep the cold-window timings visible separately from scan timings.
            if fixture.warmup_frames >= 8 && now.duration_since(first) >= Duration::from_millis(300)
            {
                fixture.launched = Some(now);
                fixture.last_frame = Some(now);
                fixture.previous_phase = 1;
                self.launch_scan(mode, ctx);
                let fixture = self.fixture.as_mut().expect("active UI probe");
                fixture.work[1] = fixture.work[1].max(now.elapsed());
            }
        }
        ctx.request_repaint_after(Duration::from_millis(40));
    }
}

impl eframe::App for Lindo {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let probe_frame = self.fixture.as_ref().map(|_| std::time::Instant::now());
        if self.desktop.cover(ctx) {
            return;
        }
        let overview_visible = self.mode.is_none() && self.extra.is_none() && self.review.is_none();
        if self.desktop.refresh_due()
            || (overview_visible && self.disk_measured.elapsed() >= Duration::from_secs(30))
        {
            self.request_disk_usage(ctx);
        }
        self.receive();
        self.assessment.poll();
        self.system.monitor_alert(ctx);
        self.desktop.refresh(ctx, self.disk);
        self.picker.poll();
        if self.review.is_none() && self.available() {
            if let Some(req) = self.quick.pop_front() {
                let mode = req.mode;
                self.pages.get_mut(&mode).unwrap().req = req;
                self.launch_scan(mode, ctx);
            }
        }
        let browsing = self.review.is_none() && !self.picker.visible() && !self.cleaning;
        self.sidebar(ctx, browsing);
        let scan_page = !self.searching
            && self.review.is_none()
            && self.extra.is_none()
            && !self.picker.visible()
            && self.active.as_ref().is_some_and(|a| {
                self.mode == Some(a.mode)
                    || (self.mode.is_none() && self.destination == design::Destination::Home)
            });
        if (self.active.is_some() && !scan_page)
            || self.cleaning
            || !self.status.is_empty()
            || !self.failures.is_empty()
        {
            egui::TopBottomPanel::bottom("attivita").show(ctx, |ui| {
                let mut stop = false;
                if let Some(a) = self.active.as_ref().filter(|_| !scan_page) {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(format!(
                            "{} · {} voci visitate",
                            title(a.mode),
                            a.progress.visited.load(Ordering::Relaxed)
                        ));
                        match a.progress.phase.load(Ordering::Relaxed) {
                            HASH => {
                                ui.label(format!(
                                    "Impronte SHA-256 · {} letti",
                                    size(a.progress.hashed_bytes.load(Ordering::Relaxed))
                                ));
                            }
                            COMPARE => {
                                ui.label(format!(
                                    "Confronto esatto · {} confrontati",
                                    size(a.progress.compared_bytes.load(Ordering::Relaxed))
                                ));
                            }
                            TREE => {
                                ui.label(format!(
                                    "Verifica contenuto · {} voci",
                                    a.progress.tree_entries.load(Ordering::Relaxed)
                                ));
                            }
                            _ => {
                                ui.label("Lettura delle cartelle");
                            }
                        }
                        if self.mode != Some(a.mode) && ui.button("Mostra").clicked() {
                            self.extra = None;
                            self.mode = Some(a.mode);
                        }
                        stop = ui.button("Interrompi").clicked();
                    });
                    ctx.request_repaint_after(Duration::from_millis(150));
                }
                if stop {
                    self.cancel();
                }
                if self.cleaning {
                    ui.horizontal(|ui| {
                        ui.label("Verifica e spostamento nel Cestino…");
                    });
                }
                if !self.status.is_empty() {
                    ui.label(&self.status);
                }
                if !self.failures.is_empty() {
                    ui.collapsing("Dettagli dello spostamento", |ui| {
                        egui::ScrollArea::vertical().max_height(110.0).show_rows(
                            ui,
                            18.0,
                            self.failures.len(),
                            |ui, rows| {
                                for i in rows {
                                    ui.add(egui::Label::new(&self.failures[i]).truncate())
                                        .on_hover_text(&self.failures[i]);
                                }
                            },
                        );
                    });
                }
            });
        }
        let gutter = if ctx.screen_rect().width() < 830. {
            16
        } else {
            32
        };
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(design::CANVAS)
                    .inner_margin(egui::Margin {
                        left: gutter,
                        right: gutter,
                        top: 0,
                        bottom: 12,
                    }),
            )
            .show(ctx, |ui| {
                self.toolbar(ui, browsing);
                ui.add_space(3.);
                let reviewing = self.review.is_some();
                ui.scope(|ui| {
                    if reviewing {
                        ui.disable();
                    }
                    if self.picker.visible() {
                        if let Some((mode, path)) = self.picker.draw(ui, ctx) {
                            let page = self.pages.get_mut(&mode).unwrap();
                            page.choose_root(path, self.append_root);
                            self.home_panel.remember_picker_choice(mode);
                            self.append_root = false;
                        }
                    } else if self.searching {
                        ui.heading("Cerca strumenti");
                        egui::ScrollArea::vertical()
                            .id_salt("tool-search")
                            .show(ui, |ui| self.catalog(ui, None));
                        if self.mode.is_some() || self.extra.is_some() {
                            self.searching = false;
                            self.search_origin = None;
                            self.tool_query.clear();
                        }
                    } else if scan_page {
                        self.scan_view(ui, ctx);
                        ctx.request_repaint_after(Duration::from_millis(120));
                    } else if let Some(extra) = self.extra {
                        let proposal = match extra {
                            Extra::Reset => {
                                if ui.small_button("‹ Applicazioni").clicked() {
                                    self.extra = None;
                                    self.mode = Some(Mode::Apps);
                                }
                                if ui.small_button("Alleggerisci un’app…").clicked() {
                                    self.extra = Some(Extra::Trim);
                                }
                                self.reset.show(ui, ctx);
                                None
                            }
                            Extra::Versions => {
                                if ui.small_button("‹ Mappa dello spazio").clicked() {
                                    self.extra = None;
                                    self.mode = Some(Mode::Space);
                                }
                                self.versions.show(ui, ctx);
                                None
                            }
                            Extra::Updates => {
                                self.vendor.show(ui, ctx);
                                None
                            }
                            Extra::Remote => {
                                self.remote.show(ui, ctx);
                                None
                            }
                            Extra::Trim => {
                                if ui.small_button("‹ Ripristino app").clicked() {
                                    self.extra = Some(Extra::Reset);
                                }
                                self.trim.show(ui, ctx);
                                None
                            }
                            Extra::MailTrash => {
                                if ui.small_button("‹ Mail e browser").clicked() {
                                    self.extra = Some(Extra::Accounts);
                                }
                                self.mail_trash.show(ui, ctx);
                                None
                            }
                            Extra::SystemJunk => {
                                if ui.small_button("‹ Pulizia avanzata").clicked() {
                                    self.extra = Some(Extra::Junk);
                                }
                                self.system_junk.show(ui, ctx);
                                None
                            }
                            Extra::Accounts => {
                                if ui.small_button("Cestino di Mail…").clicked() {
                                    self.extra = Some(Extra::MailTrash);
                                }
                                self.accounts.show(ui, ctx);
                                None
                            }
                            Extra::Clutter => self.clutter.show(ui, ctx),
                            Extra::Junk => {
                                ui.horizontal_wrapped(|ui| {
                                    if ui.small_button("Preferenze e avvii obsoleti…").clicked() {
                                        self.extra = Some(Extra::Legacy);
                                    }
                                    if ui.small_button("Cache e log di sistema…").clicked() {
                                        self.extra = Some(Extra::SystemJunk);
                                    }
                                });
                                self.junk.show(ui, ctx)
                            }
                            Extra::Legacy => {
                                if ui.small_button("‹ Pulizia avanzata").clicked() {
                                    self.extra = Some(Extra::Junk);
                                }
                                self.legacy.show(ui, ctx);
                                None
                            }
                            Extra::System => {
                                self.system.show(ui, ctx);
                                None
                            }
                        };
                        if let Some(request) = proposal {
                            if self.available() {
                                self.confirm = false;
                                self.review = Review::from_proposal(request);
                                if self.review.is_none() {
                                    self.status = "Selezione cambiata. Ripeti l’analisi.".into();
                                }
                            } else {
                                self.status = "Attendi la fine dell’operazione in corso.".into();
                            }
                        }
                    } else if let Some(m) = self.mode {
                        self.section(ui, m, ctx);
                    } else {
                        let dest = self.destination;
                        egui::ScrollArea::vertical()
                            .id_salt(("destination", dest.label()))
                            .show(ui, |ui| match dest {
                                design::Destination::Home => self.overview(ui, ctx),
                                design::Destination::Settings => {
                                    ui.heading("Impostazioni");
                                    ui.add_space(16.);
                                    self.desktop.controls(ui, ctx, self.disk);
                                }
                                _ => {
                                    ui.heading(dest.label());
                                    ui.add_space(12.);
                                    self.catalog(ui, Some(dest.label()));
                                }
                            });
                    }
                });
                if self.review.is_some() {
                    self.review_page(ui, ctx);
                }
            });
        if let Some(entered) = probe_frame {
            self.fixture_frame(ctx, entered);
        }
    }
}

impl Drop for Lindo {
    fn drop(&mut self) {
        if let Some(active) = &self.active {
            active.stop.store(true, Ordering::Release);
        }
    }
}

fn run_window(
    fixture: Option<(PathBuf, Mode)>,
    appearance: Option<egui::Theme>,
    setup: Option<fn(&mut Lindo, &egui::Context)>,
) -> eframe::Result {
    let window_start = fixture.as_ref().map(|_| std::time::Instant::now());
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1080.0, 720.0])
            .with_fullsize_content_view(true)
            .with_title_shown(false)
            .with_titlebar_shown(false)
            .with_min_inner_size([660.0, 460.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Lindo",
        options,
        Box::new(move |cc| {
            design::configure(&cc.egui_ctx);
            {
                let theme = appearance.unwrap_or(egui::Theme::Dark);
                cc.egui_ctx
                    .send_viewport_cmd(egui::ViewportCommand::SetTheme(match theme {
                        egui::Theme::Dark => egui::SystemTheme::Dark,
                        egui::Theme::Light => egui::SystemTheme::Light,
                    }));
            }
            let mut app = Lindo::new(&cc.egui_ctx);
            if let Some((path, mode)) = fixture {
                let req = &mut app.pages.get_mut(&mode).unwrap().req;
                req.roots = vec![path];
                if mode == Mode::Duplicates {
                    req.min_bytes = 1;
                }
                app.mode = Some(mode);
                app.fixture = Some(Fixture {
                    window_start,
                    ..Default::default()
                });
            }
            if let Some(start) = setup {
                start(&mut app, &cc.egui_ctx);
            }
            Ok(Box::new(app))
        }),
    )
}

pub fn launch(appearance: Option<egui::Theme>) -> eframe::Result {
    run_window(None, appearance, None)
}
#[cfg(feature = "ui-probe")]
pub fn launch_menu_fixture() -> eframe::Result {
    run_window(
        None,
        None,
        Some(|app, ctx| app.desktop.start_menu_probe(ctx)),
    )
}

#[cfg(feature = "ui-probe")]
pub fn launch_fixture(path: PathBuf, mode: Mode) -> eframe::Result {
    run_window(Some((path, mode)), None, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Identity, Item};

    fn rows() -> Arc<Results> {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().canonicalize().unwrap();
        let mut report = Report::default();
        for (name, group) in [("a", 1), ("b", 1), ("c", 1), ("d", 2), ("e", 2)] {
            let path = root.join(name);
            std::fs::write(&path, b"fixture").unwrap();
            report.items.push(Item {
                identity: Identity::read(&path).unwrap(),
                path,
                bytes: 7,
                group: Some(group),
                cloud: false,
            });
        }
        Arc::new(Results::prepare(Mode::Duplicates, report))
    }

    #[test]
    fn choosing_multiple_roots_eliminates_overlaps_and_keeps_unrelated_scopes() {
        let mut page = Page::new(Mode::Duplicates);
        for path in ["/a/one", "/b", "/a", "/a/deeper"] {
            page.choose_root(path.into(), true);
        }
        assert_eq!(page.req.roots, [PathBuf::from("/b"), PathBuf::from("/a")]);
        page.choose_root("/c".into(), false);
        assert_eq!(page.req.roots, [PathBuf::from("/c")]);
    }

    #[test]
    fn proposed_groups_require_keepers_and_preserve_cleanup_order() {
        let items = rows().report.items.clone();
        let proposal = |retained: Vec<Item>| crate::extras::Proposal {
            request: initial_request(Mode::Large),
            items: vec![items[2].clone(), items[1].clone()],
            label: "Immagini simili".into(),
            retained,
        };
        assert!(Review::from_proposal(proposal(vec![])).is_none());
        assert!(Review::from_proposal(proposal(vec![items[2].clone()])).is_none());
        let r = Review::from_proposal(proposal(vec![items[0].clone()])).unwrap();
        assert_eq!(r.moves, vec![0, 1]);
        assert_eq!(r.data.report.items[0].path, items[2].path);
        assert_eq!(r.rows, vec![(0, false), (1, false), (2, true)]);
        assert_eq!(r.keepers[&1], 2);
    }

    #[test]
    fn review_includes_hidden_selections_and_every_retained_copy() {
        let mut page = Page::new(Mode::Duplicates);
        page.install(rows());
        page.select_visible();
        assert_eq!(page.selected.len(), 3);
        assert_eq!(page.keeper(1), Some(0));
        page.keepers.insert(1, 1);
        page.select(1, false);
        page.applied = "e".to_owned();
        page.visible = vec![3, 4];
        page.selected_visible = 1;
        page.select_visible();
        let review = Review::snapshot(&page).unwrap();
        assert_eq!(review.moves, vec![2, 4]);
        assert_eq!(
            review.rows,
            vec![(1, true), (2, false), (0, true), (3, true), (4, false)]
        );
        assert_eq!(review.keepers[&1], 1);
        page.selected.clear();
        page.keepers.insert(1, 0);
        assert_eq!(review.moves.len(), 2);
        assert_eq!(review.keepers[&1], 1);
    }

    #[test]
    fn cancel_discards_a_late_scan_even_after_a_new_one_started() {
        let mut app = Lindo::new(&egui::Context::default());
        let activity = |id| Activity {
            id,
            mode: Mode::Duplicates,
            stop: Arc::new(AtomicBool::new(false)),
            progress: Arc::new(Progress::default()),
        };
        let first = activity(1);
        let stop = first.stop.clone();
        app.active = Some(first);
        app.workers = 2;
        app.cancel();
        assert!(stop.load(Ordering::Acquire));
        app.active = Some(activity(2));
        let late = rows();
        app.tx.send(Event::Scan(1, Mode::Duplicates, late)).unwrap();
        app.receive();
        assert!(app.pages[&Mode::Duplicates].result.is_none());
        assert_eq!(app.active.as_ref().unwrap().id, 2);
        let fresh = rows();
        app.tx
            .send(Event::Scan(2, Mode::Duplicates, fresh.clone()))
            .unwrap();
        app.receive();
        assert!(Arc::ptr_eq(
            app.pages[&Mode::Duplicates].result.as_ref().unwrap(),
            &fresh
        ));
        assert_eq!(app.workers, 0);
    }

    #[test]
    fn stale_filter_cannot_overwrite_rescanned_results() {
        let mut app = Lindo::new(&egui::Context::default());
        let old = rows();
        let page = app.pages.get_mut(&Mode::Duplicates).unwrap();
        page.query = "a".to_owned();
        page.install(rows());
        app.tx
            .send(Event::Filter(
                Mode::Duplicates,
                old,
                "a".to_owned(),
                (vec![0], vec![0]),
            ))
            .unwrap();
        app.receive();
        assert!(app.pages[&Mode::Duplicates].applied.is_empty());
        assert_eq!(app.pages[&Mode::Duplicates].count(), 5);
    }
    #[test]
    fn matching_one_copy_shows_its_whole_group() {
        let data = rows();
        let path = data.report.items[2].path.to_string_lossy();
        assert_eq!(
            data.filtered(Mode::Duplicates, &path),
            (vec![0, 1, 2], vec![0, 1, 2])
        );
    }

    #[test]
    fn trash_invalidates_only_overlapping_scopes_and_preserves_other_selections() {
        let scopes = vec![
            (Mode::Space, vec![PathBuf::from("/demo")]),
            (Mode::Large, vec![PathBuf::from("/demo/one")]),
            (
                Mode::Duplicates,
                vec![PathBuf::from("/demo/one/package/subdir")],
            ),
            (Mode::Logs, vec![PathBuf::from("/demo/one-more")]),
            (Mode::Apps, vec![PathBuf::from("/demo/two")]),
        ];
        let mut app = Lindo::new(&egui::Context::default());
        let result = rows();
        for (mode, roots) in &scopes {
            let page = app.pages.get_mut(mode).unwrap();
            page.req.roots = roots.clone();
            page.install(result.clone());
            page.select(1, true);
        }
        let mut summary = TrashSummary::default();
        summary.record_move(Path::new("/demo/one/package"), &scopes);
        app.tx.send(Event::Trash(Mode::Large, summary)).unwrap();
        app.receive();
        for mode in [Mode::Space, Mode::Large, Mode::Duplicates] {
            assert!(app.pages[&mode].result.is_none());
        }
        for mode in [Mode::Logs, Mode::Apps] {
            assert!(app.pages[&mode].result.is_some());
            assert!(app.pages[&mode].selected.contains(&1));
        }
        app.tx
            .send(Event::Trash(Mode::Apps, TrashSummary::default()))
            .unwrap();
        app.receive();
        assert!(app.pages[&Mode::Apps].result.is_none());
        assert!(app.pages[&Mode::Logs].result.is_some());
        assert!(app.pages[&Mode::Logs].selected.contains(&1));
    }

    #[test]
    fn switching_group_sort_preserves_selection_ids() {
        let source = rows();
        let mut report = Report::default();
        for original in &source.report.items {
            let mut item = original.clone();
            if item.group == Some(1) {
                item.path = PathBuf::from("/z").join(item.path.file_name().unwrap());
            } else {
                item.path = PathBuf::from("/a").join(item.path.file_name().unwrap());
            }
            report.items.push(item);
        }
        let data = Arc::new(Results::prepare(Mode::Duplicates, report));
        let mut page = Page::new(Mode::Duplicates);
        page.install(data.clone());
        page.select(2, true);
        assert_eq!(page.row_id(0), 0);
        page.sort_by_path = true;
        assert_eq!(page.row_id(0), 3);
        assert!(page.selected.contains(&2));
        page.applied = "z".to_owned();
        (page.visible, page.visible_by_path) = data.filtered(Mode::Duplicates, "/z/");
        assert_eq!(page.row_id(0), 0);
        assert_eq!(page.count(), 3);
    }

    #[test]
    #[ignore = "explicit interaction timing on a synthetic 3600-row proposal"]
    fn interaction_timing() {
        let template = rows().report.items[0].clone();
        let mut report = Report::default();
        for n in 0..3600 {
            let mut item = template.clone();
            item.path = PathBuf::from(format!("/synthetic/folder/copy-{n:06}"));
            report.items.push(item);
        }
        let data = Arc::new(Results::prepare(Mode::Duplicates, report));
        let mut page = Page::new(Mode::Duplicates);
        let start = std::time::Instant::now();
        page.install(data.clone());
        let publication = start.elapsed();
        let start = std::time::Instant::now();
        let visible = data.filtered(Mode::Duplicates, "copy-000010");
        let filter = start.elapsed();
        assert_eq!(visible.0.len(), 3600);
        let start = std::time::Instant::now();
        page.select_visible();
        let selection = start.elapsed();
        let start = std::time::Instant::now();
        let review = Review::snapshot(&page).unwrap();
        let snapshot = start.elapsed();
        assert_eq!(review.moves.len(), 3599);
        assert_eq!(review.rows.iter().filter(|(_, keep)| *keep).count(), 1);
        println!("LINDO_UI_ACTIONS rows=3600 publication_us={} filter_us={} select_all_us={} snapshot_us={}", publication.as_micros(),filter.as_micros(),selection.as_micros(),snapshot.as_micros());
    }
}
