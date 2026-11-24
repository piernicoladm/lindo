//! Read-only orchestration for the overview. Filesystem work uses the main queue;
//! system checks run separately and publish a bounded number of reports.
use crate::{
    model::{Mode, Report, Request},
    policy, system_tools,
};
use eframe::egui;
use std::{
    path::{Component, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
};

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
        Mode::Cache => "Cache dei browser",
        Mode::Logs => "Log",
        Mode::Large => "File grandi",
        Mode::Duplicates => "Duplicati esatti",
        Mode::Apps => "Applicazioni",
        Mode::Junk => "Pulizia avanzata",
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Check {
    Performance,
    Protection,
    Clam(PathBuf),
}
impl Check {
    fn title(&self) -> &'static str {
        match self {
            Self::Performance => "CPU, memoria e alimentazione",
            Self::Protection => "Protezioni di macOS",
            Self::Clam(_) => "Scansione ClamAV",
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct Counts {
    visited: u64,
    results: usize,
    skipped: u64,
    errors: u64,
    cancelled: bool,
    notes: Vec<String>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
enum DiskState {
    Queued,
    Result(Counts),
    Cancelled,
}
struct DiskRow {
    request: Request,
    state: DiskState,
}
enum Status {
    Waiting,
    Running,
    Ok { text: String, findings: usize },
    Error(String),
    Cancelled,
}
struct CheckRow {
    check: Check,
    status: Status,
}
enum Event {
    Started(usize),
    Finished(usize, Result<(String, usize), String>),
}
struct Worker {
    stop: Arc<AtomicBool>,
    rx: mpsc::Receiver<Event>,
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}
#[derive(Default)]
pub(crate) struct Assessment {
    performance: bool,
    protection: bool,
    clam: bool,
    folder: String,
    core_busy: bool,
    cancel_request: bool,
    disks: Vec<DiskRow>,
    checks: Vec<CheckRow>,
    worker: Option<Worker>,
    notice: String,
}
fn validate_requests(requests: &[Request]) -> Result<(), String> {
    if requests.len() != MODES.len() || !MODES.iter().all(|m| requests.iter().any(|r| r.mode == *m))
    {
        return Err("Gli ambiti delle sei analisi non sono disponibili.".into());
    }
    if let Some(r) = requests.iter().find(|r| {
        r.roots.is_empty()
            || r.roots
                .iter()
                .any(|p| !p.is_absolute() || p.components().any(|c| c == Component::ParentDir))
    }) {
        return Err(format!("Scegli una cartella valida per {}.", title(r.mode)));
    }
    Ok(())
}
fn display_request(request: &Request) -> String {
    let scope = request
        .roots
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let size = if request.mode == Mode::Large {
        format!(" · soglia {:.1} MB", request.min_bytes as f64 / 1_000_000.)
    } else {
        String::new()
    };
    format!("{} · {scope}{size}", title(request.mode))
}
impl Assessment {
    pub(crate) fn set_core_busy(&mut self, busy: bool) {
        self.core_busy = busy;
    }
    pub(crate) fn take_cancel_request(&mut self) -> bool {
        std::mem::take(&mut self.cancel_request)
    }
    pub(crate) fn record_core(&mut self, mode: Mode, report: &Report) {
        if let Some(row) = self
            .disks
            .iter_mut()
            .find(|r| r.request.mode == mode && r.state == DiskState::Queued)
        {
            row.state = DiskState::Result(Counts {
                visited: report.visited,
                results: report.items.len(),
                skipped: report.skipped,
                errors: report.errors,
                cancelled: report.cancelled,
                notes: report.notes.clone(),
            });
        }
    }
    fn pending_core(&self) -> bool {
        self.disks.iter().any(|r| r.state == DiskState::Queued)
    }
    pub(crate) fn cancel(&mut self) {
        self.worker.take();
        for row in &mut self.checks {
            if matches!(row.status, Status::Waiting | Status::Running) {
                row.status = Status::Cancelled;
            }
        }
        for row in &mut self.disks {
            if row.state == DiskState::Queued {
                row.state = DiskState::Cancelled;
            }
        }
    }
    fn plan_checks(&self) -> Result<Vec<Check>, String> {
        let mut list = Vec::new();
        if self.performance {
            list.push(Check::Performance);
        }
        if self.protection {
            list.push(Check::Protection);
        }
        if self.clam {
            let p = if let Some(s) = self.folder.trim().strip_prefix("~/") {
                policy::home().join(s)
            } else {
                PathBuf::from(self.folder.trim())
            };
            if !p.is_absolute()
                || p.components()
                    .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
            {
                return Err("Inserisci il percorso assoluto della cartella per ClamAV.".into());
            }
            list.push(Check::Clam(p));
        }
        Ok(list)
    }
    fn begin(
        &mut self,
        requests: &[Request],
        ctx: &egui::Context,
        run: impl Fn(Check, &AtomicBool) -> Result<(String, usize), String> + Send + 'static,
    ) -> bool {
        if self.core_busy || self.pending_core() || self.worker.is_some() {
            return false;
        }
        let planned = match validate_requests(requests).and_then(|_| self.plan_checks()) {
            Ok(v) => v,
            Err(e) => {
                self.notice = e;
                return false;
            }
        };
        self.notice.clear();
        self.cancel_request = false;
        self.disks = requests
            .iter()
            .cloned()
            .map(|request| DiskRow {
                request,
                state: DiskState::Queued,
            })
            .collect();
        self.checks = planned
            .iter()
            .cloned()
            .map(|check| CheckRow {
                check,
                status: Status::Waiting,
            })
            .collect();
        if planned.is_empty() {
            return true;
        }
        let (tx, rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        self.worker = Some(Worker {
            rx,
            stop: stop.clone(),
        });
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            for (i, check) in planned.into_iter().enumerate() {
                if stop.load(Ordering::Acquire) || tx.send(Event::Started(i)).is_err() {
                    break;
                }
                ctx.request_repaint();
                let result = run(check, &stop);
                if stop.load(Ordering::Acquire) || tx.send(Event::Finished(i, result)).is_err() {
                    break;
                }
                ctx.request_repaint();
            }
            drop(tx);
            ctx.request_repaint();
        });
        true
    }
    pub(crate) fn poll(&mut self) {
        let Some(worker) = &self.worker else {
            return;
        };
        loop {
            match worker.rx.try_recv() {
                Ok(Event::Started(i)) => {
                    self.checks[i].status = Status::Running;
                }
                Ok(Event::Finished(i, result)) => {
                    self.checks[i].status = match result {
                        Ok((text, findings)) => Status::Ok { text, findings },
                        Err(e) => Status::Error(e),
                    };
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.worker.take();
                    for check in &mut self.checks {
                        if matches!(check.status, Status::Running | Status::Waiting) {
                            check.status =
                                Status::Error("Controllo interrotto prima del rapporto.".into());
                        }
                    }
                    break;
                }
            }
        }
    }
    fn summary(&self, ui: &mut egui::Ui) {
        if self.disks.is_empty() {
            return;
        }
        ui.separator();
        ui.strong("Riepilogo dell’analisi");
        for row in &self.disks {
            let status = match &row.state {
                DiskState::Queued => "in attesa del risultato".into(),
                DiskState::Cancelled => "interrotta".into(),
                DiskState::Result(c) => format!(
                    "{} risultati · {} voci visitate · {} esclusioni · {} errori{}",
                    c.results,
                    c.visited,
                    c.skipped,
                    c.errors,
                    if c.cancelled { " · interrotta" } else { "" }
                ),
            };
            ui.label(format!("{} · {status}", title(row.request.mode)))
                .on_hover_text(display_request(&row.request));
            if let DiskState::Result(c) = &row.state {
                if c.skipped > 0 || c.errors > 0 {
                    egui::CollapsingHeader::new(format!(
                        "Ambito ed errori · {}",
                        title(row.request.mode)
                    ))
                    .show(ui, |ui| {
                        for path in &row.request.roots {
                            ui.small(path.display().to_string());
                        }
                        for note in &c.notes {
                            ui.small(note);
                        }
                    });
                }
            }
        }
        for (i, row) in self.checks.iter().enumerate() {
            let label = row.check.title();
            match &row.status {
                Status::Waiting => {
                    ui.label(format!("{label} · in attesa"));
                }
                Status::Running => {
                    ui.label(format!("{label} · in corso"));
                }
                Status::Cancelled => {
                    ui.label(format!("{label} · interrotto"));
                }
                Status::Error(s) => {
                    ui.label(format!("{label} · verifica incompleta: {s}"));
                }
                Status::Ok { text, findings } => {
                    let text_label = if matches!(row.check, Check::Clam(_)) {
                        format!("{label} · {findings} segnalazioni · vedi copertura")
                    } else {
                        format!("{label} · rapporto disponibile")
                    };
                    egui::CollapsingHeader::new(text_label)
                        .id_salt(("assessment_report", i))
                        .show(ui, |ui| {
                            ui.label(text);
                        });
                }
            }
        }
    }
    pub(crate) fn show(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        requests: &mut [Request],
        choose_scope: impl FnOnce(&mut egui::Ui, &mut [Request]),
    ) -> bool {
        let mut launch = false;
        let running = self.worker.is_some() || self.pending_core();
        let header =
            egui::CollapsingHeader::new("Analisi completa del Mac").id_salt("overview_assessment");
        header.show(ui, |ui| {
            ui.label("Le sei analisi usano le cartelle indicate. I risultati di ciascuna restano consultabili nella relativa sezione.");
            if running {
                for row in &self.disks { ui.small(display_request(&row.request)); }
            } else {
                choose_scope(ui,requests);
                for req in requests.iter() { ui.small(display_request(req)); }
            }
            ui.add_enabled_ui(!running, |ui| {
                ui.checkbox(&mut self.performance, "Misura CPU, memoria e alimentazione");
                ui.checkbox(&mut self.protection, "Verifica Gatekeeper, FileVault e versione ClamAV");
                let toggle = ui.checkbox(&mut self.clam, "Includi la scansione locale con ClamAV");
                if toggle.changed() && self.clam && self.folder.is_empty() {
                    if let Some(path) = requests.iter().find(|r| r.mode == Mode::Large).and_then(|r| r.roots.first()) {
                        self.folder = path.to_string_lossy().into_owned();
                    }
                }
                if self.clam {
                    ui.label("Cartella per ClamAV");
                    let field = egui::TextEdit::singleline(&mut self.folder)
                        .hint_text("Percorso assoluto oppure ~/Downloads").desired_width(f32::INFINITY);
                    ui.add(field);
                    ui.small("Usa le firme già disponibili sul Mac. Il rapporto indica copertura, limiti ed eventuali segnalazioni.");
                }
            });
            if !self.notice.is_empty() { ui.label(&self.notice); }
            ui.horizontal_wrapped(|ui| {
                let start = ui.add_enabled(!self.core_busy && !running && requests.iter().all(|r| !r.roots.is_empty()), egui::Button::new("Avvia analisi completa"));
                if start.clicked() { launch = self.begin(requests, ctx, system_tools::scan_assessment); }
                if running && ui.button("Interrompi analisi completa").clicked() {
                    self.cancel_request = self.pending_core();
                    self.cancel();
                }
            });
            self.summary(ui);
        });
        launch
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    fn requests() -> Vec<Request> {
        MODES
            .into_iter()
            .map(|mode| Request {
                mode,
                roots: vec![PathBuf::from("/synthetic/Downloads")],
                min_bytes: 1,
            })
            .collect()
    }
    fn deadline_wait(ui: &mut Assessment, condition: impl Fn(&Assessment) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while !condition(ui) {
            ui.poll();
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
    }
    #[test]
    fn six_scopes_and_partial_auxiliary_failures_preserve_disk_results() {
        let mut ui = Assessment {
            performance: true,
            protection: true,
            clam: true,
            folder: "/synthetic/Downloads".into(),
            ..Default::default()
        };
        let req = requests();
        let ctx = egui::Context::default();
        assert!(ui.begin(&req, &ctx, |check, _| match check {
            Check::Clam(_) => Err("ClamAV assente".into()),
            Check::Performance => Err("Misurazione incompleta".into()),
            Check::Protection => Ok(("Protezione fixture".into(), 0)),
        }));
        assert!(!ui.begin(&req, &ctx, |_, _| panic!("duplicate job")));
        let report = Report {
            visited: 23,
            skipped: 1,
            ..Default::default()
        };
        for m in MODES {
            ui.record_core(m, &report);
        }
        deadline_wait(&mut ui, |u| u.worker.is_none());
        assert!(matches!(ui.checks[0].status, Status::Error(_)));
        assert!(matches!(ui.checks[1].status, Status::Ok { .. }));
        assert!(matches!(&ui.checks[2].status,Status::Error(s) if s.contains("assente")));
        assert!(ui
            .disks
            .iter()
            .all(|r| matches!(&r.state,DiskState::Result(v) if v.visited==23 && v.skipped==1)));
    }
    #[test]
    fn cancelled_worker_cannot_replace_a_new_session() {
        let mut ui = Assessment {
            performance: true,
            ..Default::default()
        };
        let req = requests();
        let ctx = egui::Context::default();
        let (started, observed) = mpsc::channel();
        let (done, completed) = mpsc::channel();
        assert!(ui.begin(&req, &ctx, move |_, stop| {
            started.send(()).unwrap();
            while !stop.load(Ordering::Acquire) {
                std::thread::yield_now();
            }
            done.send(()).unwrap();
            Ok(("stale".into(), 0))
        }));
        observed.recv_timeout(Duration::from_secs(2)).unwrap();
        ui.cancel();
        completed.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(ui.disks.iter().all(|r| r.state == DiskState::Cancelled));
        assert!(ui.begin(&req, &ctx, |_, _| Ok(("fresh".into(), 0))));
        deadline_wait(&mut ui, |u| u.worker.is_none());
        assert!(ui
            .checks
            .iter()
            .all(|r| matches!(&r.status,Status::Ok{text,..} if text=="fresh")));
    }
    #[test]
    fn incomplete_or_ambiguous_scopes_never_start() {
        let mut ui = Assessment {
            clam: true,
            folder: "relative/path".into(),
            ..Default::default()
        };
        let mut req = requests();
        let ctx = egui::Context::default();
        assert!(!ui.begin(&req, &ctx, |_, _| panic!()));
        assert!(ui.disks.is_empty());
        ui.clam = false;
        req[2].roots.clear();
        assert!(!ui.begin(&req, &ctx, |_, _| panic!()));
        req[2] = req[1].clone();
        assert!(!ui.begin(&req, &ctx, |_, _| panic!()));
        ui.set_core_busy(true);
        assert!(!ui.begin(&requests(), &ctx, |_, _| panic!()));
    }
}
