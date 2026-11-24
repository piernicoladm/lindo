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
            return format!("{:.1} {unit}", bytes as f64 / divisor as f64);
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
        if needed && self.selected.insert(id) && self.is_visible(id) {
            self.selected_visible += 1;
        }
        if !needed && self.selected.remove(&id) && self.is_visible(id) {
            self.selected_visible -= 1;
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
    Disk(Option<(u64, u64)>),
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
    disk_pending: bool,
    disk_measured: std::time::Instant,
    filtering: bool,
    fixture: Option<Fixture>,
}

fn disk_usage() -> Option<(u64, u64)> {
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
    Some((
        (stat.f_blocks as u64).saturating_mul(unit),
        (stat.f_bavail as u64).saturating_mul(unit),
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
                        self.status = format!("Analisi completata · {}", title(mode));
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
                    self.disk = value;
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

    fn overview(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.heading("Panoramica");
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            ui.strong("Disco dell’utente");
            if ui
                .add_enabled(
                    !self.disk_pending,
                    egui::Button::new("Aggiorna spazio").small(),
                )
                .clicked()
            {
                self.request_disk_usage(ctx);
            }
        });
        if let Some((total, free)) = self.disk {
            let used = total.saturating_sub(free);
            ui.label(format!("{} disponibili su {}", size(free), size(total)));
            ui.add(egui::ProgressBar::new(used as f32 / total.max(1) as f32).desired_width(340.0));
        } else {
            ui.label(if self.disk_pending {
                "Misurazione…"
            } else {
                "Capacità del volume non disponibile."
            });
        }
        self.desktop.controls(ui, ctx, self.disk);
        ui.add_space(18.0);
        ui.separator();
        ui.strong("Analisi rapida");
        let quick_modes = [Mode::Space, Mode::Cache, Mode::Logs];
        for mode in quick_modes {
            let roots = &self.pages[&mode].req.roots;
            ui.small(format!(
                "{} · {}",
                title(mode),
                roots
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if ui
            .add_enabled(
                self.available() && self.quick.is_empty(),
                egui::Button::new("Analizza queste tre aree"),
            )
            .clicked()
        {
            self.quick
                .extend(quick_modes.map(|mode| self.pages[&mode].req.clone()));
            ctx.request_repaint();
        }
        self.assessment
            .set_core_busy(!self.available() || !self.quick.is_empty());
        let requests: Vec<_> = MODES
            .iter()
            .map(|mode| {
                let mut req = self.pages[mode].req.clone();
                if req.roots.is_empty() && matches!(mode, Mode::Large | Mode::Duplicates) {
                    req.roots.extend(home().map(|h| h.join("Downloads")));
                }
                req
            })
            .collect();
        if self.assessment.show(ui, ctx, &requests) {
            self.quick.extend(requests);
            ctx.request_repaint();
        }
        if self.assessment.take_cancel_request() {
            self.cancel();
        }
        ui.separator();
        ui.horizontal_wrapped(|ui| {
            let search = ui.add(
                egui::TextEdit::singleline(&mut self.tool_query).hint_text("Trova uno strumento…"),
            );
            if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::F)) {
                search.request_focus();
            }
        });
        let query = self.tool_query.to_lowercase();
        let tools = MODES
            .into_iter()
            .map(|m| (title(m), hint(m), Some(m), None))
            .chain(EXTRAS.iter().map(|&(extra, label)| {
                let detail = match extra {
                    Extra::Clutter => "File vecchi, archivi, installatori e immagini simili",
                    Extra::Legacy => {
                        "Preferenze danneggiate, agenti di avvio non più presenti e account rimossi"
                    }
                    Extra::Trim => "Alleggerimento controllato di app universali, architettura Intel e lingue",
                    Extra::MailTrash => "Messaggi nel cestino di un singolo account Apple Mail",
                    Extra::SystemJunk => "Pulizia selettiva di log archiviati e cache rigenerabili in Library del Mac",
                    Extra::Junk => "Cache di sviluppo e AI, disinstallazione, backup iOS e residui",
                    Extra::System => {
                        "Volumi, snapshot, aggiornamenti, malware, manutenzione, login e Cestino"
                    }
                    Extra::Accounts => {
                        "Allegati Mail, newsletter, schede Chrome, privacy e copie locali iCloud"
                    }
                    Extra::Reset => {
                        "Preferenze delle app, backup recuperabili, lingue e architetture"
                    }
                    Extra::Versions => "Revisioni storiche dei documenti salvate da macOS",
                    Extra::Updates => {
                        "Versioni delle app e aggiornamenti degli editori con Sparkle"
                    }
                    Extra::Remote => "Quota e file online di Dropbox, Google Drive e OneDrive",
                };
                (label, detail, None, Some(extra))
            }));
        let mut found = false;
        for favorite in [true, false] {
            for (name, detail, mode, extra) in tools.clone() {
                if self.favorites.contains(name) != favorite
                    || !(name.to_lowercase().contains(&query)
                        || detail.to_lowercase().contains(&query))
                {
                    continue;
                }
                found = true;
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .small_button(if favorite { "★" } else { "☆" })
                        .on_hover_text(if favorite {
                            "Rimuovi dai preferiti"
                        } else {
                            "Aggiungi ai preferiti"
                        })
                        .clicked()
                    {
                        if favorite {
                            self.favorites.remove(name);
                        } else {
                            self.favorites.insert(name.to_owned());
                        }
                        objc2_foundation::NSUserDefaults::standardUserDefaults().setBool_forKey(
                            !favorite,
                            &objc2_foundation::NSString::from_str(&format!("tool.favorite.{name}")),
                        );
                    }
                    if ui.button(name).clicked() {
                        self.mode = mode;
                        self.extra = extra;
                    }
                    if let Some(result) = mode.and_then(|m| self.pages[&m].result.as_ref()) {
                        ui.small(format!(
                            "{} risultati · {} errori",
                            result.report.items.len(),
                            result.report.errors
                        ));
                    }
                });
                ui.small(detail);
                ui.separator();
            }
        }
        if !found {
            ui.label("Nessuno strumento corrisponde alla ricerca.");
        }
    }

    fn section(&mut self, ui: &mut egui::Ui, mode: Mode, ctx: &egui::Context) {
        ui.horizontal_wrapped(|ui| {
            ui.heading(title(mode));
            if mode == Mode::Apps && ui.button("Ripristino app…").clicked() {
                self.extra = Some(Extra::Reset);
                self.mode = None;
            }
            if mode == Mode::Space && ui.button("Revisioni documento…").clicked() {
                self.extra = Some(Extra::Versions);
                self.mode = None;
            }
        });
        ui.label(hint(mode));
        ui.add_space(8.0);
        let available = self.available();
        let page = self.pages.get_mut(&mode).unwrap();
        let mut scan = false;
        let mut choose = false;
        let mut append = false;
        ui.add_enabled_ui(available, |ui| {
            ui.horizontal_wrapped(|ui| {
                if mode == Mode::Space {
                    if let Some(h) = home() {
                        egui::ComboBox::from_id_salt("scope")
                            .selected_text("Ambito")
                            .show_ui(ui, |ui| {
                                for (label, p) in [
                                    ("Download", h.join("Downloads")),
                                    ("Utente", h.clone()),
                                    ("Library", h.join("Library")),
                                    ("Applicazioni", PathBuf::from("/Applications")),
                                ] {
                                    if ui
                                        .selectable_label(page.req.roots.first() == Some(&p), label)
                                        .clicked()
                                    {
                                        page.invalidate();
                                        page.history.clear();
                                        page.req.roots = vec![p];
                                    }
                                }
                            });
                    }
                    if ui
                        .add_enabled(!page.history.is_empty(), egui::Button::new("Indietro"))
                        .clicked()
                    {
                        let parent = page.history.pop().unwrap();
                        page.req.roots = vec![parent];
                        scan = true;
                    }
                }
                if matches!(mode, Mode::Space | Mode::Large | Mode::Duplicates) {
                    choose = ui.button("Scegli cartella…").clicked();
                    if matches!(mode, Mode::Large | Mode::Duplicates) && !page.req.roots.is_empty()
                    {
                        append = ui.button("Aggiungi cartella…").clicked();
                    }
                }
                if matches!(mode, Mode::Large | Mode::Duplicates) {
                    let options: &[u64] = if mode == Mode::Large {
                        &[100_000_000, 500_000_000, 1_000_000_000]
                    } else {
                        &[0, 1_000, 1_000_000, 100_000_000]
                    };
                    let label = |n| if n == 0 { "Tutti".to_owned() } else { size(n) };
                    egui::ComboBox::from_id_salt("threshold")
                        .selected_text(label(page.req.min_bytes))
                        .show_ui(ui, |ui| {
                            for &n in options {
                                if ui
                                    .selectable_value(&mut page.req.min_bytes, n, label(n))
                                    .changed()
                                {
                                    page.invalidate();
                                }
                            }
                        });
                }
                if ui
                    .add_enabled(!page.req.roots.is_empty(), egui::Button::new("Analizza"))
                    .clicked()
                {
                    scan = true;
                }
            });
        });
        if page.req.roots.is_empty() {
            ui.small("Scegli una cartella per iniziare.");
        }
        let mut remove = None;
        egui::ScrollArea::vertical()
            .id_salt(("radici", mode))
            .max_height(84.)
            .show(ui, |ui| {
                for (index, p) in page.req.roots.iter().enumerate() {
                    ui.horizontal_wrapped(|ui| {
                        if matches!(mode, Mode::Duplicates | Mode::Large)
                            && page.req.roots.len() > 1
                            && ui
                                .add_enabled(available, egui::Button::new("Rimuovi").small())
                                .on_hover_text("Rimuovi questa cartella dall’analisi")
                                .clicked()
                        {
                            remove = Some(index);
                        }
                        ui.small(p.to_string_lossy());
                    });
                }
            });
        if let Some(index) = remove {
            page.req.roots.remove(index);
            page.invalidate();
        }
        ui.separator();
        if choose || append {
            self.append_root = append;
            self.picker.open(mode, ctx);
            return;
        }
        if scan {
            self.launch_scan(mode, ctx);
            return;
        }
        let Some(results) = page.result.clone() else {
            ui.add_space(18.0);
            ui.label(if self.active.as_ref().is_some_and(|a| a.mode == mode) {
                "Analisi in corso…"
            } else {
                "Nessun risultato. Avvia un’analisi per esaminare questa sezione."
            });
            return;
        };
        let r = &results.report;
        ui.horizontal_wrapped(|ui| {
            ui.strong(item_count(r.items.len()));
            ui.label(format!("{} visitati · {} esclusi", r.visited, r.skipped));
            if mode == Mode::Duplicates {
                ui.label(format!("{} gruppi", results.groups.len()));
            }
            if mode == Mode::Space {
                ui.label(format!("{} allocati", size(results.total)));
                if let Some(root) = page.req.roots.first() {
                    if let Some(previous) = self.baselines.get(root) {
                        let difference = results.total.abs_diff(*previous);
                        let direction = if results.total >= *previous {
                            "+"
                        } else {
                            "−"
                        };
                        ui.label(format!(
                            "{direction}{} dalla misura salvata",
                            size(difference)
                        ));
                    }
                    if ui
                        .small_button("Salva misura")
                        .on_hover_text(
                            "Usa il totale di questa cartella come riferimento per la sessione.",
                        )
                        .clicked()
                    {
                        self.baselines.insert(root.clone(), results.total);
                    }
                }
            }
        });
        if r.errors > 0 || r.cancelled {
            ui.colored_label(
                ui.visuals().warn_fg_color,
                format!("Copertura parziale · {} errori", r.errors),
            );
        }
        if !r.notes.is_empty() {
            ui.collapsing("Dettagli dell’analisi", |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("notes")
                    .max_height(90.0)
                    .show_rows(ui, 18.0, r.notes.len(), |ui, rows| {
                        for i in rows {
                            ui.add(egui::Label::new(&r.notes[i]).truncate())
                                .on_hover_text(&r.notes[i]);
                        }
                    });
            });
        }
        let search_id = egui::Id::new(("ricerca", mode));
        ui.horizontal_wrapped(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut page.query)
                    .id(search_id)
                    .hint_text("Cerca nel percorso…"),
            );
            if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::F)) {
                ui.memory_mut(|m| m.request_focus(search_id));
            }
            if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
                page.query.clear();
            }
            ui.small(if page.query != page.applied {
                "Ricerca…".to_owned()
            } else {
                format!("{} visibili", page.count())
            });
        });
        if mode == Mode::Duplicates {
            ui.horizontal_wrapped(|ui| {
                ui.small("Ordina gruppi");
                egui::ComboBox::from_id_salt("ordine-duplicati")
                    .selected_text(if page.sort_by_path {
                        "Primo percorso"
                    } else {
                        "Spazio extra"
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut page.sort_by_path, false, "Spazio extra");
                        ui.selectable_value(&mut page.sort_by_path, true, "Primo percorso");
                    });
            });
        }
        if mode != Mode::Space {
            ui.horizontal_wrapped(|ui| {
                if mode != Mode::Apps
                    && ui
                        .add_enabled(
                            page.query == page.applied && page.count() > 0,
                            egui::Button::new(if mode == Mode::Duplicates {
                                "Seleziona extra visibili"
                            } else {
                                "Seleziona visibili"
                            }),
                        )
                        .clicked()
                {
                    page.select_visible();
                }
                if ui
                    .add_enabled(
                        !page.selected.is_empty(),
                        egui::Button::new("Deseleziona tutto"),
                    )
                    .clicked()
                {
                    page.selected.clear();
                    page.selected_visible = 0;
                }
                if ui
                    .add_enabled(
                        !page.selected.is_empty() && available,
                        egui::Button::new("Rivedi la selezione"),
                    )
                    .clicked()
                {
                    self.review = Review::snapshot(page);
                }
            });
            ui.small(format!(
                "{} selezionati · {} nascosti dal filtro",
                page.selected.len(),
                page.selected.len().saturating_sub(page.selected_visible)
            ));
        }
        ui.separator();
        let mut reveal = None;
        let mut drill = None;
        if mode == Mode::Space && page.query.is_empty() && results.total > 0 {
            ui.collapsing("Mappa proporzionale", |ui| {
                if let Some(index) = crate::treemap::show(ui, &r.items, results.total) {
                    let item = &r.items[index];
                    if available && item.identity.is_dir() {
                        drill = Some(item.path.clone());
                    } else if !item.identity.is_dir() {
                        reveal = Some(item.path.clone());
                    }
                }
            });
        }
        let rows = page.count();
        egui::ScrollArea::vertical()
            .id_salt(("risultati", mode))
            .auto_shrink([false, false])
            .show_rows(ui, 48.0, rows, |ui, range| {
                for row in range {
                    let i = page.row_id(row);
                    let item = &r.items[i];
                    ui.push_id(i, |ui| {
                        ui.set_min_height(48.0);
                        ui.horizontal(|ui| {
                            if mode != Mode::Space {
                                let protected = item.group.and_then(|g| page.keeper(g)) == Some(i);
                                let mut chosen = page.selected.contains(&i);
                                if crate::extras::file_checkbox(ui, &mut chosen, !protected, &item.path)
                                    .on_hover_text("Seleziona per il Cestino")
                                    .changed()
                                {
                                    page.select(i, chosen);
                                }
                            }
                            let name = item
                                .path
                                .file_name()
                                .unwrap_or(item.path.as_os_str())
                                .to_string_lossy();
                            let reserve = if mode == Mode::Duplicates {
                                265.0
                            } else {
                                170.0
                            };
                            let width = (ui.available_width() - reserve).max(60.0);
                            if mode == Mode::Space && item.identity.is_dir() {
                                if ui
                                    .add_enabled(
                                        available,
                                        egui::Button::new(
                                            egui::RichText::new(name.as_ref()).strong(),
                                        )
                                        .truncate(),
                                    )
                                    .on_hover_text(item.path.to_string_lossy())
                                    .clicked()
                                {
                                    drill = Some(item.path.clone());
                                }
                            } else {
                                ui.add_sized(
                                    [width, 18.0],
                                    egui::Label::new(egui::RichText::new(name.as_ref()).strong())
                                        .truncate(),
                                );
                            }
                            ui.label(size(item.bytes));
                            if let Some(g) = item.group {
                                if page.keeper(g) == Some(i) {
                                    ui.small("Copia protetta");
                                } else if ui.small_button("Conserva questa").clicked() {
                                    page.keepers.insert(g, i);
                                    page.select(i, false);
                                }
                            }
                            if ui.small_button("Finder").clicked() {
                                reveal = Some(item.path.clone());
                            }
                        });
                        ui.horizontal(|ui| {
                            if mode == Mode::Space {
                                let max = results.total.max(1);
                                ui.add(
                                    egui::ProgressBar::new(item.bytes as f32 / max as f32)
                                        .desired_width(65.0)
                                        .desired_height(5.0),
                                );
                                if let Some(stats) = r.space.get(&item.path) {
                                    let details = format!("{} file · {} logici · {} hard link già conteggiati · {} esclusi · {} errori", stats.files,size(stats.logical_bytes),stats.hardlinks,stats.skipped,stats.errors);
                                    let summary = if stats.errors > 0 { format!("Parziale · {} errori",stats.errors) }
                                        else { format!("{} file · {} logici",stats.files,size(stats.logical_bytes)) };
                                    ui.small(summary).on_hover_text(details);
                                }
                            }
                            if let Some(g) = item.group {
                                ui.small(format!("Gruppo {g}"));
                            }
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(item.path.to_string_lossy())
                                        .small()
                                        .weak(),
                                )
                                .truncate(),
                            )
                            .on_hover_text(item.path.to_string_lossy());
                        });
                    });
                }
            });
        if let Some(p) = drill {
            if let Some(parent) = page.req.roots.first() {
                page.history.push(parent.clone());
            }
            page.req.roots = vec![p];
            self.launch_scan(mode, ctx);
        }
        if let Some(p) = reveal {
            self.reveal(p, ctx);
        }
        self.refresh_filter(mode, ctx);
    }

    fn review_page(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let review = self.review.as_ref().unwrap();
        ui.heading("Rivedi la selezione");
        ui.label(format!(
            "{} · {} · {}",
            review.label,
            item_count(review.moves.len()),
            size(review.total)
        ));
        if review.cloud {
            ui.colored_label(ui.visuals().warn_fg_color,"Alcuni percorsi appartengono a cartelle sincronizzate, come Dropbox o iCloud. Lo spostamento può rimuoverli anche dagli altri dispositivi e per chi condivide la cartella.");
        }
        ui.label("Lo spazio si libera dopo lo svuotamento del Cestino dal Finder.");
        let mut back = false;
        let mut proceed = false;
        ui.horizontal_wrapped(|ui| {
            if self.confirm {
                ui.strong(format!(
                    "Confermi lo spostamento di {}?",
                    item_count(review.moves.len())
                ));
                if ui.button("Annulla conferma").clicked() {
                    self.confirm = false;
                }
                proceed = ui.button("Conferma e sposta nel Cestino").clicked();
            } else {
                back = ui.button("Torna alla selezione").clicked();
                if ui.button("Sposta nel Cestino…").clicked() {
                    self.confirm = true;
                }
            }
        });
        ui.separator();
        let end = review.rows.len();
        let mut reveal = None;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show_rows(ui, 48.0, end, |ui, rows| {
                for row in rows {
                    let (i, keep) = review.rows[row];
                    let item = &review.data.report.items[i];
                    ui.push_id(i, |ui| {
                        ui.set_min_height(48.0);
                        ui.horizontal(|ui| {
                            ui.strong(if keep { "Resta sul Mac" } else { "Al Cestino" });
                            if let Some(g) = item.group {
                                ui.small(format!("Gruppo {g}"));
                            }
                            if keep
                                && item.group.and_then(|g| review.keepers.get(&g)).copied()
                                    == Some(i)
                            {
                                ui.small("Copia protetta");
                            }
                            ui.label(size(item.bytes));
                            if ui.small_button("Finder").clicked() {
                                reveal = Some(item.path.clone());
                            }
                        });
                        ui.add(egui::Label::new(item.path.to_string_lossy()).truncate())
                            .on_hover_text(item.path.to_string_lossy());
                    });
                }
            });
        if back {
            self.review = None;
            self.confirm = false;
        }
        if proceed {
            let snapshot = self.review.take().unwrap();
            self.trash(snapshot, ctx);
        }
        if let Some(path) = reveal {
            self.reveal(path, ctx);
        }
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
        let sections = [
            ("Panoramica", None),
            ("Mappa dello spazio", Some(Mode::Space)),
            ("Cache dei browser", Some(Mode::Cache)),
            ("Log", Some(Mode::Logs)),
            ("File grandi", Some(Mode::Large)),
            ("Duplicati", Some(Mode::Duplicates)),
            ("Applicazioni", Some(Mode::Apps)),
        ];
        if browsing {
            let keys = [
                egui::Key::Num1,
                egui::Key::Num2,
                egui::Key::Num3,
                egui::Key::Num4,
                egui::Key::Num5,
                egui::Key::Num6,
                egui::Key::Num7,
            ];
            for ((_, m), key) in sections.iter().zip(keys) {
                if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, key)) {
                    self.extra = None;
                    self.mode = *m;
                }
            }
        }
        egui::SidePanel::left("navigazione")
            .resizable(false)
            .exact_width(155.0)
            .show(ctx, |ui| {
                ui.add_space(10.0);
                ui.heading("Lindo");
                ui.add_space(14.0);
                ui.add_enabled_ui(browsing, |ui| {
                    for (label, m) in sections {
                        if m == Some(Mode::Cache) {
                            ui.add_space(8.0);
                            ui.separator();
                            ui.small("PULIZIA");
                        }
                        if ui
                            .selectable_label(self.extra.is_none() && self.mode == m, label)
                            .clicked()
                        {
                            self.extra = None;
                            self.mode = m;
                        }
                    }
                    ui.separator();
                    for (extra, label) in EXTRAS {
                        if matches!(
                            extra,
                            Extra::Reset
                                | Extra::Versions
                                | Extra::Legacy
                                | Extra::MailTrash
                                | Extra::SystemJunk
                                | Extra::Trim
                        ) {
                            continue;
                        }
                        if ui
                            .selectable_label(self.extra == Some(extra), label)
                            .clicked()
                        {
                            self.extra = Some(extra);
                            self.mode = None;
                        }
                    }
                });
                ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                    ui.small("Scorciatoie").on_hover_text(
                        "⌘1 fino a ⌘7: cambia sezione. ⌘F: cerca. Esc: cancella ricerca.",
                    );
                });
            });
        if self.active.is_some()
            || self.cleaning
            || !self.status.is_empty()
            || !self.failures.is_empty()
        {
            egui::TopBottomPanel::bottom("attivita").show(ctx, |ui| {
                let mut stop = false;
                if let Some(a) = &self.active {
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
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(10.0);
            if self.picker.visible() {
                if let Some((mode, path)) = self.picker.draw(ui, ctx) {
                    let page = self.pages.get_mut(&mode).unwrap();
                    page.choose_root(path, self.append_root);
                    self.append_root = false;
                }
            } else if self.review.is_some() {
                self.review_page(ui, ctx);
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
                egui::ScrollArea::vertical().show(ui, |ui| self.overview(ui, ctx));
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
            .with_inner_size([920.0, 640.0])
            .with_min_inner_size([660.0, 460.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Lindo",
        options,
        Box::new(move |cc| {
            if let Some(theme) = appearance {
                cc.egui_ctx.set_theme(theme);
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
