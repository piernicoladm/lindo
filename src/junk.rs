#[path = "junk_caches.rs"]
mod caches;
#[path = "junk_formats.rs"]
mod formats;
#[path = "junk_ollama.rs"]
mod ollama;
use crate::trash::TreeKind;
use crate::{
    extras::Proposal,
    model::{Identity, Item, Mode, Request},
    policy,
};
use eframe::egui;
pub(crate) use formats::{closed, model_link, model_locks, model_node};
use std::{
    collections::HashSet,
    io,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
        Arc,
    },
    time::Duration,
};

// Only these names below a known app's data directory describe reproducible
// renderer caches. Other user data in the same container is never offered.
const ELECTRON: &[(&str, &str)] = &[
    ("com.microsoft.VSCode", "Code"),
    ("com.todesktop.230313mzl4w4u92", "Cursor"),
    ("com.anthropic.claudefordesktop", "Claude"),
    ("com.tinyspeck.slackmacgap", "Slack"),
    ("com.hnc.Discord", "discord"),
];
const RENDERER: &[&str] = &["GPUCache", "Code Cache"];
const DERIVED: &str = "Library/Developer/Xcode/DerivedData";

fn app_parent(home: &Path, path: &Path) -> bool {
    path == home.join("Applications") || path == Path::new("/Applications")
}
fn app_location(home: &Path, path: &Path) -> bool {
    path.parent().is_some_and(|p| app_parent(home, p))
        && path
            .extension()
            .is_some_and(|s| s.eq_ignore_ascii_case("app"))
        && !policy::protected(path, Mode::Apps, None)
}
fn third_party(id: &str) -> bool {
    id.contains('.')
        && id.split('.').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        })
        && !id.to_ascii_lowercase().starts_with("com.apple.")
        && !id.eq_ignore_ascii_case("com.piernicolademaria.lindo")
}
pub(crate) fn allowed_root(path: &Path) -> bool {
    let home = policy::home();
    app_parent(&home, path)
        || path == home.join(DERIVED)
        || [formats::HUB, formats::BACKUPS, "Downloads"]
            .iter()
            .any(|r| path == home.join(r))
        || path == home.join("Library/Caches")
        || ELECTRON
            .iter()
            .any(|(_, name)| path == home.join("Library/Application Support").join(name))
}

fn xcode_cache(home: &Path, path: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(home.join(DERIVED)) else {
        return false;
    };
    let Some(parts): Option<Vec<_>> = relative.iter().map(|p| p.to_str()).collect() else {
        return false;
    };
    match parts.as_slice() {
        ["ModuleCache.noindex"] | ["SDKStatCaches.noindex"] => true,
        [project, "Index.noindex"] | [project, "Build", "Intermediates.noindex"] => {
            project.rsplit_once('-').is_some_and(|(name, suffix)| {
                !name.is_empty()
                    && !name.starts_with('.')
                    && suffix.len() == 28
                    && suffix.bytes().all(|b| b.is_ascii_lowercase())
            })
        }
        _ => false,
    }
}

fn cache_locations(home: &Path, id: &str) -> Vec<PathBuf> {
    // Browser profiles and unrecognized stores are inspection-only. Only
    // audited application identifiers have an actionable cache association.
    let Some((_, folder)) = ELECTRON.iter().find(|(known, _)| *known == id) else {
        return Vec::new();
    };
    let parents = [
        home.join("Library/Caches").join(id),
        home.join("Library/Application Support").join(folder),
    ];
    parents
        .iter()
        .flat_map(|p| RENDERER.iter().map(move |name| p.join(name)))
        .collect()
}

pub(crate) enum Owner {
    Application(PathBuf, String),
    WholeCache(PathBuf, String),
    Xcode,
    Model,
    Backup,
    Leftover(String),
}
pub(crate) fn cache_owner(request: &Request, path: &Path) -> Option<Owner> {
    let home = policy::home();
    if xcode_cache(&home, path) {
        return Some(Owner::Xcode);
    }
    if formats::model(&home, path) {
        return Some(Owner::Model);
    }
    if formats::backup(&home, path).is_some() {
        return Some(Owner::Backup);
    }
    let owner = request
        .roots
        .iter()
        .filter(|p| app_location(&home, p))
        .find_map(|bundle| {
            let id = policy::app_identifier(bundle).ok()?;
            if caches::permitted(&home, &id, path, &AtomicBool::new(false)) {
                return Some(Owner::WholeCache(bundle.clone(), id));
            }
            cache_locations(&home, &id)
                .iter()
                .any(|p| p == path)
                .then(|| Owner::Application(bundle.clone(), id))
        });
    owner.or_else(|| {
        ELECTRON.iter().find_map(|(id, _)| {
            let base = home.join("Library/Caches").join(id);
            RENDERER
                .iter()
                .any(|n| path == base.join(n))
                .then(|| Owner::Leftover(id.to_string()))
        })
    })
}
pub(crate) fn installer(path: &Path) -> bool {
    formats::installer(&policy::home(), path)
}

#[derive(Clone, Copy, Default, PartialEq)]
enum Scope {
    #[default]
    Apps,
    Caches,
    Ollama,
    Development,
    Installers,
    Backups,
}
#[derive(Default)]
struct Inventory {
    rows: Vec<Row>,
    paths: HashSet<PathBuf>,
    omitted: usize,
}
struct Row {
    path: PathBuf,
    reason: String,
    bytes: Option<u64>,
    item: Option<Item>,
    owner: Option<PathBuf>,
    selected: bool,
}
impl Inventory {
    fn inspect(&mut self, path: &Path, reason: impl Into<String>, bytes: Option<u64>) {
        if !self.paths.insert(path.to_owned()) {
            return;
        }
        self.rows.push(Row {
            path: path.into(),
            reason: reason.into(),
            bytes,
            item: None,
            owner: None,
            selected: false,
        });
    }
    fn tree(
        &mut self,
        path: &Path,
        profile: TreeKind,
        reason: &str,
        owner: Option<&Path>,
        stop: &AtomicBool,
    ) {
        if self.paths.contains(path) {
            return;
        }
        let snapshot = if profile == TreeKind::Model {
            Identity::snapshot(path, profile, stop, None)
        } else {
            Identity::tree_cancel(path, profile == TreeKind::Application, stop)
        };
        match snapshot {
            Ok((identity, bytes, _)) => {
                self.paths.insert(path.to_owned());
                let item = Item {
                    path: path.into(),
                    bytes,
                    identity,
                    group: None,
                    cloud: false,
                };
                self.rows.push(Row {
                    path: path.into(),
                    reason: reason.into(),
                    bytes: Some(bytes),
                    item: Some(item),
                    owner: owner.map(Path::to_owned),
                    selected: false,
                });
            }
            Err(_) => {
                self.omitted += 1;
                self.inspect(path, format!("{reason} · Verifica incompleta"), None);
            }
        }
    }
}

fn accessible(path: &Path) -> bool {
    !policy::protected(path, Mode::Junk, None)
        && !policy::cloud(path)
        && policy::local_path(path).unwrap_or(false)
        && !path.ancestors().skip(1).any(policy::package)
}
fn entries(
    base: &Path,
    stop: &AtomicBool,
    result: &mut Inventory,
    mut each: impl FnMut(&Path, &libc::stat, &mut Inventory),
) {
    if std::fs::symlink_metadata(base).is_err_and(|e| e.kind() == io::ErrorKind::NotFound) {
        return;
    }
    if !accessible(base) || policy::package(base) {
        result.omitted += 1;
        return;
    }
    let Ok(mut dir) = policy::children(base) else {
        result.omitted += 1;
        return;
    };
    while let Some(entry) = dir.next() {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        let Ok(name) = entry else {
            result.omitted += 1;
            continue;
        };
        if name.to_string_lossy().starts_with('.') {
            continue;
        }
        let p = base.join(&name);
        if policy::protected(&p, Mode::Junk, None) {
            continue;
        }
        let Ok(stat) = dir.stat(&name) else {
            result.omitted += 1;
            continue;
        };
        if matches!(stat.st_mode & libc::S_IFMT, libc::S_IFDIR | libc::S_IFREG)
            && stat.st_flags & 0x40000000 == 0
            && !policy::placeholder_name(&p)
        {
            each(&p, &stat, result)
        }
    }
}
fn existing_dir(p: &Path) -> bool {
    accessible(p) && policy::open_file(p, libc::O_DIRECTORY).is_ok() && !policy::package(p)
}

fn app_caches(result: &mut Inventory, home: &Path, app: &Path, id: &str, stop: &AtomicBool) {
    for cache in cache_locations(home, id) {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        if existing_dir(&cache) {
            result.tree(
                &cache,
                TreeKind::Cache,
                &format!("Cache grafica o di codice · {id}"),
                Some(app),
                stop,
            );
        }
    }
}

fn inventory(
    scope: Scope,
    home: &Path,
    app_roots: &[PathBuf],
    stop: &AtomicBool,
) -> Result<Inventory, String> {
    let _local = policy::LocalOnly::enter().map_err(|e| e.to_string())?;
    let mut result = Inventory::default();
    match scope {
        Scope::Caches => {
            caches::inventory(home, app_roots, Path::new("/Library"), stop, &mut result)
        }
        Scope::Ollama => return Err("Usa la gestione del servizio Ollama locale.".into()),
        Scope::Apps => {
            for root in app_roots {
                entries(root, stop, &mut result, |p, stat, result| {
                    if !app_location(home, p) || !accessible(p) {
                        return;
                    }
                    let Ok(id) = policy::app_identifier(p) else {
                        result.omitted += 1;
                        return;
                    };
                    if !third_party(&id) {
                        return;
                    }
                    let reason = format!("Applicazione · {id}");
                    if stat.st_uid == unsafe { libc::getuid() } || stat.st_uid == 0 {
                        result.tree(p, TreeKind::Application, &reason, None, stop);
                    } else {
                        result.inspect(p, &reason, None)
                    }
                    app_caches(result, home, p, &id, stop);
                    let cache = home.join("Library/Caches").join(&id);
                    if existing_dir(&cache) {
                        result.inspect(
                            &cache,
                            format!("Cartella cache con identificatore {id}"),
                            None,
                        )
                    }
                });
            }
            entries(
                &home.join("Library/Caches"),
                stop,
                &mut result,
                |p, stat, result| {
                    if stat.st_mode & libc::S_IFMT == libc::S_IFDIR
                        && p.file_name()
                            .and_then(|s| s.to_str())
                            .is_some_and(third_party)
                        && existing_dir(p)
                    {
                        if let Some((id, _)) = ELECTRON.iter().find(|(id, _)| {
                            p.file_name()
                                .is_some_and(|name| name == std::ffi::OsStr::new(id))
                        }) {
                            for name in RENDERER {
                                let cache = p.join(name);
                                if existing_dir(&cache) {
                                    result.tree(
                                        &cache,
                                        TreeKind::Cache,
                                        &format!(
                                            "Cache rigenerabile identificata dal percorso · {id}"
                                        ),
                                        None,
                                        stop,
                                    );
                                }
                            }
                        }
                        result.inspect(
                            p,
                            "Cache con identificatore · associazione da verificare",
                            None,
                        );
                    }
                },
            );
        }
        Scope::Development => {
            for root in app_roots {
                entries(root, stop, &mut result, |p, _, result| {
                    if app_location(home, p) && accessible(p) {
                        if let Ok(id) = policy::app_identifier(p) {
                            if matches!(
                                id.as_str(),
                                "com.anthropic.claudefordesktop" | "com.todesktop.230313mzl4w4u92"
                            ) {
                                app_caches(result, home, p, &id, stop);
                            }
                        }
                    }
                });
            }
            entries(&home.join(DERIVED), stop, &mut result, |p, _, result| {
                if !existing_dir(p) {
                    return;
                }
                for candidate in [
                    p.to_owned(),
                    p.join("Index.noindex"),
                    p.join("Build/Intermediates.noindex"),
                ] {
                    if stop.load(Ordering::Relaxed) {
                        return;
                    }
                    if xcode_cache(home, &candidate) && existing_dir(&candidate) {
                        result.tree(
                            &candidate,
                            TreeKind::Cache,
                            "Cache di compilazione o indicizzazione Xcode",
                            None,
                            stop,
                        );
                    }
                }
            });
            for (relative, desc) in [
                (".cache/huggingface/hub", "Modelli Hugging Face"),
                (".ollama/models/blobs", "Dati dei modelli Ollama"),
            ] {
                entries(
                    &home.join(relative),
                    stop,
                    &mut result,
                    |p, stat, result| {
                        let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
                        if (name.starts_with("models--") || name.starts_with("sha256-"))
                            && accessible(p)
                        {
                            if formats::model(home, p) {
                                result.tree(p,TreeKind::Model,"Modello Hugging Face · intero repository locale, download necessario per riutilizzarlo",None,stop);
                                return;
                            }
                            result.inspect(
                                p,
                                desc,
                                (stat.st_mode & libc::S_IFMT == libc::S_IFREG)
                                    .then_some(stat.st_size as u64),
                            );
                        }
                    },
                );
            }
        }
        Scope::Installers => entries(
            &home.join("Downloads"),
            stop,
            &mut result,
            |p, stat, result| {
                if p.extension().and_then(|s| s.to_str()).is_some_and(|ext| {
                    ["pkg", "mpkg", "dmg"]
                        .iter()
                        .any(|s| ext.eq_ignore_ascii_case(s))
                }) && accessible(p)
                {
                    if stat.st_uid == unsafe { libc::getuid() } && formats::installer(home, p) {
                        if let Ok(identity) = Identity::read(p) {
                            let bytes = stat.st_size as u64;
                            result.rows.push(Row {
                                path: p.into(),
                                reason: "Installatore PKG o immagine DMG · formato verificato"
                                    .into(),
                                bytes: Some(bytes),
                                item: Some(Item {
                                    path: p.into(),
                                    bytes,
                                    identity,
                                    group: None,
                                    cloud: false,
                                }),
                                owner: None,
                                selected: false,
                            });
                            return;
                        }
                    }
                    result.inspect(
                        p,
                        "Installatore o immagine disco in Download",
                        (stat.st_mode & libc::S_IFMT == libc::S_IFREG)
                            .then_some(stat.st_size as u64),
                    );
                }
            },
        ),
        Scope::Backups => {
            entries(
                &home.join("Library/Application Support/MobileSync/Backup"),
                stop,
                &mut result,
                |p, stat, result| {
                    if stat.st_mode & libc::S_IFMT == libc::S_IFDIR && existing_dir(p) {
                        if let Some(archived) = formats::backup(home, p) {
                            let reason = if archived {
                                "Backup iOS archiviato · il ripristino richiede questa copia"
                            } else {
                                "Backup iOS completato · potrebbe essere l’unica copia di ripristino"
                            };
                            result.tree(p, TreeKind::Cache, reason, None, stop);
                        } else {
                            result.inspect(p, "Cartella di backup locale di iPhone o iPad · formato non verificato", None);
                        }
                    }
                },
            )
        }
    }
    if stop.load(Ordering::Relaxed) {
        return Err("Scansione annullata.".into());
    }
    result.rows.sort_by(|a, b| {
        a.owner
            .as_ref()
            .unwrap_or(&a.path)
            .cmp(b.owner.as_ref().unwrap_or(&b.path))
            .then_with(|| a.path.cmp(&b.path))
    });
    Ok(result)
}

type InventoryReceiver = Receiver<Result<Inventory, String>>;

#[derive(Default)]
pub(crate) struct Junk {
    ollama: ollama::Ollama,
    scope: Scope,
    result: Inventory,
    status: String,
    scanning: Option<(Arc<AtomicBool>, InventoryReceiver)>,
}
impl Drop for Junk {
    fn drop(&mut self) {
        if let Some((stop, _)) = &self.scanning {
            stop.store(true, Ordering::Relaxed);
        }
    }
}
impl Junk {
    pub(crate) fn show(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) -> Option<Proposal> {
        ui.heading("Applicazioni e cache di sviluppo");
        let old = self.scope;
        ui.add_enabled_ui(self.scanning.is_none(), |ui| {
            ui.horizontal_wrapped(|ui| {
                for (scope, label) in [
                    (Scope::Apps, "App e residui"),
                    (Scope::Caches, "Cache utente e sistema"),
                    (Scope::Ollama, "Modelli Ollama"),
                    (Scope::Development, "Xcode e modelli AI"),
                    (Scope::Installers, "Installatori"),
                    (Scope::Backups, "Backup iOS"),
                ] {
                    ui.selectable_value(&mut self.scope, scope, label);
                }
            })
        });
        if old != self.scope {
            self.result = Inventory::default();
            self.status.clear();
        }
        if self.scope == Scope::Ollama {
            self.ollama.show(ui, ctx);
            return None;
        }
        ui.label(match self.scope {
            Scope::Ollama=>"",
            Scope::Caches=>"Cache delle app in ~/Library/Caches e inventario di /Library/Caches e /Library/Logs. Per i log archiviati usa la scansione Log. Cartelle Apple, collegamenti e percorsi privati sono esclusi.",
            Scope::Apps=>"Applicazioni installate in Applicazioni e nelle Applicazioni personali. Le cache associate si selezionano singolarmente.",
            Scope::Development=>"Cache rigenerabili di Xcode, Claude e Cursor e inventario dei modelli Hugging Face e Ollama. Puoi selezionare un intero repository Hugging Face verificato. I blob condivisi di Ollama si consultano nel Finder.",
            Scope::Installers=>"File PKG, MPKG e DMG direttamente nella cartella Download. Puoi selezionare i file con formato verificato.",
            Scope::Backups=>"Backup locali in MobileSync. Seleziona un backup completato solo se puoi rinunciare a quel punto di ripristino. Le cartelle non verificate si consultano nel Finder.",
        });
        if let Some((stop, receiver)) = &self.scanning {
            ctx.request_repaint_after(Duration::from_millis(150));
            if ui.button("Annulla scansione").clicked() {
                stop.store(true, Ordering::Relaxed);
            }
            match receiver.try_recv() {
                Ok(Ok(value)) => {
                    self.status = format!(
                        "{} elementi · {} verifiche incomplete",
                        value.rows.len(),
                        value.omitted
                    );
                    self.result = value;
                    self.scanning = None;
                }
                Ok(Err(e)) => {
                    self.status = e;
                    self.scanning = None;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.status = "Scansione interrotta.".into();
                    self.scanning = None;
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        } else if ui.button("Analizza questa categoria").clicked() {
            let (send, rx) = mpsc::channel();
            let stop = Arc::new(AtomicBool::new(false));
            self.scanning = Some((stop.clone(), rx));
            self.result = Inventory::default();
            self.status = "Scansione in corso…".into();
            let scope = self.scope;
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                let home = policy::home();
                let roots = [PathBuf::from("/Applications"), home.join("Applications")];
                let _ = send.send(inventory(scope, &home, &roots, &stop));
                ctx.request_repaint();
            });
        }
        ui.label(&self.status);
        let count = self.result.rows.iter().filter(|r| r.selected).count();
        let mut proposal = None;
        if ui
            .add_enabled(
                count > 0,
                egui::Button::new(format!("Rivedi selezione ({count})")),
            )
            .clicked()
        {
            let mut roots = Vec::new();
            let mut items = Vec::new();
            let home = policy::home();
            for row in self.result.rows.iter_mut().filter(|r| r.selected) {
                if let Some(item) = &row.item {
                    if let Some(base) = item.path.ancestors().skip(1).find(|p| allowed_root(p)) {
                        roots.push(base.into())
                    }
                    if let Some(bundle) = &row.owner {
                        roots.push(bundle.clone())
                    }
                    items.push(item.clone());
                }
                row.selected = false;
            }
            items.sort_by_key(|i| app_location(&home, &i.path));
            roots.sort();
            roots.dedup();
            proposal = Some(Proposal {
                request: Request {
                    mode: Mode::Junk,
                    roots,
                    min_bytes: 0,
                },
                items,
                retained: vec![],
                label: "Applicazioni e cache selezionate".into(),
            });
        }
        egui::ScrollArea::vertical().show_rows(ui, 66., self.result.rows.len(), |ui, range| {
            for row in &mut self.result.rows[range] {
                ui.horizontal(|ui| {
                    if row.item.is_some() {
                        crate::extras::file_checkbox(ui, &mut row.selected, true, &row.path);
                    }
                    ui.strong(row.path.file_name().unwrap_or_default().to_string_lossy());
                    if let Some(size) = row.bytes {
                        ui.label(if size < 1024 * 1024 {
                            format!("{} B", size)
                        } else {
                            format!("{:.1} MiB", size as f64 / 1048576.)
                        });
                    }
                    if ui.small_button("Finder").clicked() {
                        if let Err(e) = Command::new("/usr/bin/open")
                            .arg("-R")
                            .arg(&row.path)
                            .spawn()
                        {
                            self.status = e.to_string();
                        }
                    }
                });
                ui.add(egui::Label::new(&row.reason).truncate());
                ui.add(egui::Label::new(row.path.to_string_lossy()).truncate())
                    .on_hover_text(row.path.display().to_string());
            }
        });
        proposal
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::symlink};

    #[test]
    fn inventories_installers_and_backups_without_removal_candidates() {
        let fixture = tempfile::tempdir().unwrap();
        let home = fixture.path().canonicalize().unwrap();
        let downloads = home.join("Downloads");
        fs::create_dir(&downloads).unwrap();
        fs::write(downloads.join("tool.pkg"), b"installer").unwrap();
        fs::write(downloads.join("draft.txt"), b"draft").unwrap();
        symlink(downloads.join("tool.pkg"), downloads.join("alias.dmg")).unwrap();
        let stop = AtomicBool::new(false);
        let scanned = inventory(Scope::Installers, &home, &[], &stop).unwrap();
        assert_eq!(scanned.rows.len(), 1);
        assert_eq!(scanned.rows[0].bytes, Some(9));
        assert!(scanned.rows[0].item.is_none());
        fs::create_dir_all(home.join("Library/Application Support/MobileSync/Backup/device"))
            .unwrap();
        let backups = inventory(Scope::Backups, &home, &[], &stop).unwrap();
        assert_eq!(backups.rows.len(), 1);
        assert!(backups.rows[0].item.is_none());
    }

    #[test]
    fn scans_only_regenerable_development_directories() {
        let fixture = tempfile::tempdir().unwrap();
        let home = fixture.path().canonicalize().unwrap();
        let project = home
            .join(DERIVED)
            .join("Project-abcdefghijklmnopqrstuvwxyzab");
        for relative in [
            "Index.noindex",
            "Build/Intermediates.noindex",
            "SourcePackages/checkouts",
        ] {
            fs::create_dir_all(project.join(relative)).unwrap();
            fs::write(project.join(relative).join("entry"), b"test").unwrap();
        }
        let ai = home.join(".cache/huggingface/hub/models--example--model");
        fs::create_dir_all(&ai).unwrap();
        fs::write(home.join(".cache/huggingface/token"), b"fixture").unwrap();
        let stop = AtomicBool::new(false);
        let report = inventory(Scope::Development, &home, &[], &stop).unwrap();
        assert_eq!(report.rows.iter().filter(|r| r.item.is_some()).count(), 2);
        assert!(report.rows.iter().any(|r| r.path == ai && r.item.is_none()));
        assert_eq!(report.rows.len(), 3);
        stop.store(true, Ordering::Relaxed);
        assert!(inventory(Scope::Development, &home, &[], &stop).is_err());
    }

    #[test]
    fn app_components_require_known_identifier_and_exact_cache_path() {
        let fixture = tempfile::tempdir().unwrap();
        let home = fixture.path().canonicalize().unwrap();
        let apps = home.join("Applications");
        let bundle = apps.join("Renamed fixture.app");
        fs::create_dir_all(bundle.join("Contents")).unwrap();
        fs::write(bundle.join("Contents/Info.plist"),
            br#"<?xml version="1.0"?><plist version="1.0"><dict><key>CFBundleIdentifier</key><string>com.anthropic.claudefordesktop</string></dict></plist>"#).unwrap();
        let support = home.join("Library/Application Support/Claude");
        for name in ["Code Cache", "GPUCache", "IndexedDB", "Local Storage"] {
            fs::create_dir_all(support.join(name)).unwrap();
        }
        fs::create_dir_all(home.join("Library/Caches/com.example.absent")).unwrap();
        let report = inventory(Scope::Apps, &home, &[apps], &AtomicBool::new(false)).unwrap();
        assert_eq!(report.rows.iter().filter(|r| r.item.is_some()).count(), 3);
        assert_eq!(
            report
                .rows
                .iter()
                .filter(|r| r.owner.as_ref() == Some(&bundle))
                .count(),
            2
        );
        assert!(report.rows.iter().all(|r| !r.selected));
        assert!(report
            .rows
            .iter()
            .any(|r| r.path.ends_with("com.example.absent") && r.item.is_none()));
        assert!(cache_locations(&home, "com.google.Chrome").is_empty());
    }
}
