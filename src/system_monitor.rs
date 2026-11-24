use super::{command, installed, scanned_clean, security_scan, signatures, CLAM};
use crate::policy;
use eframe::egui;
use std::collections::{BTreeMap, VecDeque};
use std::fs::File;
use std::os::{fd::AsRawFd, unix::fs::MetadataExt};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const MAX_ENTRIES: usize = 4_000;
const MAX_REPORTS: usize = 32;
type Stamp = (u64, u64, i64, i64, i64);
type Inventory = BTreeMap<PathBuf, Stamp>;

#[derive(Default, Clone)]
struct State {
    status: String,
    scanned: u64,
    alerts: u64,
    latest: VecDeque<security_scan::Finding>,
    excluded: usize,
    pending: usize,
    failures: u64,
    last_error: String,
    running: bool,
}

struct Session {
    root: PathBuf,
    cancel: Arc<AtomicBool>,
    state: Arc<Mutex<State>>,
    worker: Option<JoinHandle<()>>,
}
impl Session {
    fn stop(&self) {
        self.cancel.store(true, Ordering::Release);
        if let Some(h) = &self.worker {
            h.thread().unpark();
        }
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        self.stop();
        if let Some(h) = self.worker.take() {
            let _ = h.join();
        }
    }
}

fn root_node(root: &Path) -> Result<(u64, u64), String> {
    let file = security_scan::local_handle(root, true)?;
    let mut fs = std::mem::MaybeUninit::<libc::statfs>::uninit();
    if unsafe { libc::fstatfs(file.as_raw_fd(), fs.as_mut_ptr()) } != 0
        || unsafe { fs.assume_init() }.f_flags & libc::MNT_LOCAL as u32 == 0
    {
        return Err("Il monitor richiede una cartella su un volume locale.".into());
    }
    node(&file)
}
fn node(file: &File) -> Result<(u64, u64), String> {
    file.metadata()
        .map(|m| (m.dev(), m.ino()))
        .map_err(|e| e.to_string())
}
fn inventory(
    root: &Path,
    original: (u64, u64),
    cancel: &AtomicBool,
) -> Result<(Inventory, usize), String> {
    let mut work = vec![root.to_owned()];
    let (mut data, mut excluded, mut entries) = (Inventory::new(), 0, 0);
    let deadline = Instant::now() + Duration::from_secs(15);
    while let Some(path) = work.pop() {
        let identity = security_scan::local_handle(&path, true).and_then(|f| node(&f));
        if path == root && identity.as_ref().ok() != Some(&original) {
            return Err("Cartella iniziale spostata, cambiata o non accessibile.".into());
        }
        let Ok(id) = identity else {
            excluded += 1;
            continue;
        };
        if id.0 != original.0 {
            excluded += 1;
            continue;
        }
        let mut dir = match policy::children(&path) {
            Ok(d) if d.node().ok() == Some(id) => d,
            _ if path == root => return Err("Cartella iniziale non più verificabile.".into()),
            _ => {
                excluded += 1;
                continue;
            }
        };
        while let Some(entry) = dir.next() {
            if cancel.load(Ordering::Acquire) {
                return Err("Monitor arrestato.".into());
            }
            entries += 1;
            if entries > MAX_ENTRIES || Instant::now() > deadline {
                return Err("Copertura incompleta. Inventario oltre 4.000 voci o 15 secondi. Scegli una cartella più piccola.".into());
            }
            let Ok(name) = entry else {
                excluded += 1;
                break;
            };
            let p = path.join(&name);
            if !security_scan::permitted(&p) || policy::placeholder_name(&p) {
                excluded += 1;
                continue;
            }
            let Ok(s) = dir.stat(&name) else {
                excluded += 1;
                continue;
            };
            if s.st_dev as u64 != original.0 || s.st_flags & 0x40000000 != 0 {
                excluded += 1;
                continue;
            }
            match s.st_mode & libc::S_IFMT {
                libc::S_IFDIR => work.push(p),
                libc::S_IFREG if s.st_size > 0 && s.st_size as u64 <= security_scan::MAX_FILE => {
                    data.insert(
                        p,
                        (
                            s.st_dev as u64,
                            s.st_ino,
                            s.st_size,
                            s.st_ctime,
                            s.st_ctime_nsec,
                        ),
                    );
                }
                _ => excluded += 1,
            }
        }
    }
    Ok((data, excluded))
}

fn watch(
    root: &Path,
    cancel: &AtomicBool,
    state: &Mutex<State>,
    ctx: &egui::Context,
    interval: Duration,
) -> Result<(), String> {
    let scanner = installed(CLAM).ok_or("ClamAV non installato.")?;
    let _local = policy::LocalOnly::enter().map_err(|e| e.to_string())?;
    let private_db = signatures::local_database().map(|p| format!("--database={}", p.display()));
    let mut args = vec!["--version"];
    if let Some(value) = private_db.as_deref() {
        args.push(value);
    }
    let version = command::run(scanner, &args, None, cancel, 10)?;
    if version.code != Some(0)
        || !version.text.lines().any(|line| {
            line.starts_with("ClamAV ")
                && line
                    .split('/')
                    .nth(1)
                    .is_some_and(|n| n.parse::<u64>().is_ok())
        })
    {
        return Err(
            "Database di firme ClamAV non verificabile. Aggiorna le firme e riattiva il monitor."
                .into(),
        );
    }
    let original = root_node(root)?;
    let (mut previous, excluded) = inventory(root, original, cancel)?;
    let mut processed = previous.clone();
    {
        let mut s = state.lock().unwrap();
        s.excluded = excluded;
        s.status = "In osservazione".into();
    }
    ctx.request_repaint();
    while !cancel.load(Ordering::Acquire) {
        thread::park_timeout(interval);
        if cancel.load(Ordering::Acquire) {
            break;
        }
        let (current, excluded) = inventory(root, original, cancel)?;
        processed.retain(|path, _| current.contains_key(path));
        let pending = current
            .iter()
            .filter(|(p, stamp)| processed.get(*p) != Some(*stamp))
            .count();
        state.lock().unwrap().excluded = excluded;
        state.lock().unwrap().pending = pending;
        for (path, stamp) in current
            .iter()
            .filter(|(p, stamp)| {
                previous.get(*p) == Some(*stamp) && processed.get(*p) != Some(*stamp)
            })
            .take(3)
            .map(|(p, s)| (p.clone(), *s))
            .collect::<Vec<_>>()
        {
            if cancel.load(Ordering::Acquire) {
                return Ok(());
            }
            let result = security_scan::inspect_file(scanner, &path, cancel);
            if cancel.load(Ordering::Acquire) {
                return Ok(());
            }
            let mut s = state.lock().unwrap();
            match result {
                Ok((_, Some(f))) => {
                    s.scanned += 1;
                    s.alerts += 1;
                    s.latest.push_front(f);
                    s.latest.truncate(MAX_REPORTS);
                }
                Ok((r, None)) if scanned_clean(&r) => s.scanned += 1,
                Ok((r, None)) => {
                    return Err(format!(
                        "Monitor fermato: ClamAV non ha completato la scansione. {}",
                        r.text.chars().take(600).collect::<String>()
                    ))
                }
                Err(e) => {
                    s.failures += 1;
                    s.last_error = e;
                }
            }
            processed.insert(path, stamp);
            s.pending = s.pending.saturating_sub(1);
            drop(s);
            ctx.request_repaint();
        }
        previous = current;
    }
    Ok(())
}

#[derive(Default)]
pub(super) struct Monitor {
    session: Option<Session>,
    acknowledged: u64,
}
impl Monitor {
    fn start(
        &mut self,
        root: PathBuf,
        ctx: &egui::Context,
        interval: Duration,
    ) -> Result<(), String> {
        if self
            .session
            .as_ref()
            .is_some_and(|s| !s.worker.as_ref().is_some_and(|h| h.is_finished()))
        {
            return Err("Monitor già attivo.".into());
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let state = Arc::new(Mutex::new(State {
            status: "Acquisizione inventario iniziale…".into(),
            running: true,
            ..Default::default()
        }));
        let path = root.clone();
        let shared = state.clone();
        let stop = cancel.clone();
        let repaint = ctx.clone();
        let worker = thread::Builder::new()
            .name("lindo-folder-monitor".into())
            .spawn(move || {
                let result = watch(&path, &stop, &shared, &repaint, interval);
                let mut s = shared.lock().unwrap();
                s.running = false;
                s.status = if stop.load(Ordering::Acquire) {
                    "Monitor arrestato.".into()
                } else {
                    result.err().unwrap_or_else(|| "Monitor arrestato.".into())
                };
                drop(s);
                repaint.request_repaint();
            })
            .map_err(|e| e.to_string())?;
        self.acknowledged = 0;
        self.session = Some(Session {
            root,
            cancel,
            state,
            worker: Some(worker),
        });
        Ok(())
    }
    pub(super) fn show(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        path: &str,
    ) -> Vec<security_scan::Finding> {
        ui.separator();
        ui.strong("Monitor della cartella");
        ui.label("Con Lindo aperta, confronta i metadati della cartella e delle sottocartelle a intervalli di almeno 3 secondi. Analizza fino a 3 file nuovi o modificati per ciclo dopo due osservazioni stabili. Il monitor si ferma se l’inventario supera 4.000 voci o 15 secondi. I file già presenti all’avvio richiedono una scansione manuale. Un file creato e subito rimosso può sfuggire al controllo. La scansione avviene dopo la scrittura e non blocca l’esecuzione.");
        let running = self
            .session
            .as_ref()
            .is_some_and(|s| s.state.lock().unwrap().running);
        if !running
            && ui
                .add_enabled(
                    !path.is_empty(),
                    egui::Button::new("Monitora la cartella indicata"),
                )
                .clicked()
        {
            if let Err(e) = self.start(path.into(), ctx, Duration::from_secs(3)) {
                ui.label(e);
            }
        }
        let Some(session) = &self.session else {
            return Vec::new();
        };
        let s = session.state.lock().unwrap().clone();
        ui.label(format!("{} · {}", session.root.display(), s.status));
        if s.running && ui.button("Ferma monitor").clicked() {
            session.stop();
        }
        ui.label(format!("File analizzati: {} · Segnalazioni: {} · In attesa: {} · Esclusi nell’ultimo inventario: {} · Controlli incompleti: {}",s.scanned,s.alerts,s.pending,s.excluded,s.failures));
        if !s.last_error.is_empty() {
            ui.label(s.last_error);
        }
        if s.alerts > MAX_REPORTS as u64 {
            ui.label("Sono disponibili le ultime 32 segnalazioni.");
        }
        if s.running {
            ctx.request_repaint_after(Duration::from_secs(1));
        }
        s.latest.into_iter().collect()
    }
    pub(super) fn alert(&mut self, ctx: &egui::Context) {
        let Some(session) = &self.session else {
            return;
        };
        let (count, path) = {
            let s = session.state.lock().unwrap();
            (s.alerts, s.latest.front().map(|f| f.path.clone()))
        };
        if count <= self.acknowledged {
            return;
        }
        egui::Window::new("Segnalazione ClamAV")
            .id(egui::Id::new("folder_monitor_alert"))
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(format!(
                    "Nuove segnalazioni: {}. Il file più recente è riportato sotto.",
                    count - self.acknowledged
                ));
                if let Some(p) = path {
                    ui.label(p.display().to_string());
                }
                ui.label("Per rivedere i file, apri Strumenti di sistema > Sicurezza e privacy.");
                if ui.button("Ho letto").clicked() {
                    self.acknowledged = count;
                }
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn wait_for(session: &Session, predicate: impl Fn(&State) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(90);
        loop {
            let s = session.state.lock().unwrap().clone();
            if predicate(&s) {
                return;
            }
            assert!(
                s.running && Instant::now() < deadline,
                "{}; scanned={} alerts={} failures={} {}",
                s.status,
                s.scanned,
                s.alerts,
                s.failures,
                s.last_error
            );
            thread::sleep(Duration::from_millis(50));
        }
    }
    #[test]
    #[ignore = "Requires the synthetic QA home, real ClamAV and the reviewed HDB signature"]
    fn real_monitor_detects_creation_and_modification_and_stops() {
        let base = PathBuf::from(std::env::var_os("LINDO_FIXTURE_WORKER").unwrap());
        assert!(base.join(".lindo-synthetic-qa").is_file());
        let dir = tempfile::Builder::new()
            .prefix("monitor-")
            .tempdir_in(&base)
            .unwrap();
        let child = dir.path().join("sample");
        std::fs::create_dir(&child).unwrap();
        let existing = child.join("existing.txt");
        std::fs::write(&existing, b"original benign text").unwrap();
        let ctx = egui::Context::default();
        let mut monitor = Monitor::default();
        monitor
            .start(dir.path().to_owned(), &ctx, Duration::from_millis(50))
            .unwrap();
        let session = monitor.session.as_ref().unwrap();
        wait_for(session, |s| s.status == "In osservazione");
        let signature = b"Lindo benign synthetic test signature sample. Nothing executable.";
        let vault = child.join("_3_Vault");
        std::fs::create_dir(&vault).unwrap();
        std::fs::write(vault.join("excluded.txt"), signature).unwrap();
        std::os::unix::fs::symlink(vault.join("excluded.txt"), child.join("excluded-link"))
            .unwrap();
        let added = child.join("added.txt");
        std::fs::write(&added, signature).unwrap();
        wait_for(session, |s| s.alerts == 1);
        assert_eq!(session.state.lock().unwrap().latest[0].path, added);
        std::fs::write(&existing, signature).unwrap();
        wait_for(session, |s| s.alerts == 2);
        assert_eq!(session.state.lock().unwrap().latest[0].path, existing);
        session.stop();
        wait_for(session, |s| !s.running);
        std::fs::write(child.join("after-stop.txt"), signature).unwrap();
        thread::sleep(Duration::from_millis(180));
        let final_state = session.state.lock().unwrap();
        assert!(final_state.excluded >= 2);
        assert_eq!(final_state.scanned, 2);
        assert_eq!(final_state.alerts, 2);
        assert_eq!(std::fs::read(existing).unwrap(), signature);
        assert_eq!(std::fs::read(added).unwrap(), signature);
    }
}
