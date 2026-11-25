#[path = "system_command.rs"]
pub(crate) mod command;
#[path = "system_maintenance.rs"]
mod maintenance;
use maintenance::Mutation;

use crate::{model::Mode, policy};
#[path = "system_monitor.rs"]
mod folder_monitor;
#[path = "system_scan.rs"]
mod security_scan;
#[path = "system_signatures.rs"]
mod signatures;
use eframe::egui;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc,
};
use std::time::Duration;

const CLAM: &[&str] = &[
    "/opt/homebrew/bin/clamscan",
    "/usr/local/bin/clamscan",
    "/usr/local/clamav/bin/clamscan",
];
const BREW: &[&str] = &["/opt/homebrew/bin/brew", "/usr/local/bin/brew"];

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Section {
    #[default]
    Storage,
    Performance,
    Updates,
    Protection,
    Cloud,
}
#[derive(Clone)]
enum Action {
    Storage,
    Snapshots,
    VerifyVolume,
    PeriodicStatus,
    Performance,
    Login,
    Updates,
    Brew,
    Protection,
    Scan(PathBuf),
    Recheck(security_scan::Finding),
    UpdateSignatures,
    Cloud,
    Open(String),
    Maintenance(Mutation),
}

fn installed(paths: &'static [&'static str]) -> Option<&'static str> {
    paths.iter().copied().find(|path| Path::new(path).is_file())
}

fn scanned_clean(output: &command::Output) -> bool {
    output.code == Some(0)
        && !output.truncated
        && output.text.lines().any(|line| {
            line.trim()
                .strip_prefix("Scanned files:")
                .and_then(|n| n.trim().parse::<usize>().ok())
                .is_some_and(|n| n > 0)
        })
}
fn verdict(output: &command::Output) -> &'static str {
    match output.code {
        Some(0) if scanned_clean(output) => "Nessuna firma rilevata nel file analizzato.",
        Some(1) => "ClamAV ha segnalato una firma o un limite di analisi. Consulta il dettaglio.",
        _ => "Scansione incompleta. Controlla le firme installate e il dettaglio.",
    }
}

struct Report {
    opened: bool,
    text: String,
    providers: Vec<PathBuf>,
    snapshots: Vec<String>,
    packages: Vec<Mutation>,
    findings: Vec<security_scan::Finding>,
}
pub(crate) fn scan_assessment(
    check: crate::assessment::Check,
    cancel: &AtomicBool,
) -> Result<(String, usize), String> {
    use crate::assessment::Check;
    let action = match check {
        Check::Performance => Action::Performance,
        Check::Protection => Action::Protection,
        Check::Clam(path) => {
            let _guard = crate::policy::LocalOnly::enter().map_err(|e| e.to_string())?;
            if !security_scan::permitted(&path) {
                return Err("Scegli una cartella locale consentita.".into());
            }
            let _directory = security_scan::local_handle(&path, true)?;
            return execute(Action::Scan(path), cancel)
                .map(|report| (report.text, report.findings.len()));
        }
    };
    let report = execute(action, cancel)?;
    Ok((report.text, report.findings.len()))
}

fn execute(action: Action, cancel: &AtomicBool) -> Result<Report, String> {
    let mut report = Report {
        opened: false,
        text: String::new(),
        providers: Vec::new(),
        snapshots: Vec::new(),
        packages: Vec::new(),
        findings: Vec::new(),
    };
    let mut inspect = |title: &str, exe: &str, args: &[&str], timeout| -> Result<(), String> {
        let out = command::run(exe, args, None, cancel, timeout)?;
        if exe == "/usr/bin/tmutil" && out.code == Some(0) && !out.truncated {
            report.snapshots = maintenance::snapshots(&out.text);
        }
        if args.first() == Some(&"outdated") && out.code == Some(0) && !out.truncated {
            report.packages.extend(
                out.text
                    .lines()
                    .map(str::trim)
                    .filter(|n| maintenance::valid_package(n))
                    .take(256)
                    .map(|name| Mutation::Brew {
                        cask: args.contains(&"--cask"),
                        name: name.into(),
                    }),
            );
        }
        report.text.push_str(title);
        report.text.push('\n');
        if out.code != Some(0) {
            report.text.push_str(&format!(
                "Comando non riuscito ({}).\n",
                out.code
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "interrotto".into())
            ));
        }
        report
            .text
            .push_str(if out.text.is_empty() && out.code == Some(0) {
                "Nessuna voce restituita."
            } else {
                &out.text
            });
        if out.truncated {
            report
                .text
                .push_str("\nElenco parziale: raggiunto il limite di visualizzazione.");
        }
        report.text.push_str("\n\n");
        Ok(())
    };
    match action {
        Action::Storage => {
            inspect(
                "Spazio sul volume dati",
                "/bin/df",
                &["-h", "/System/Volumes/Data"],
                10,
            )?;
            inspect(
                "Contenitori e volumi APFS",
                "/usr/sbin/diskutil",
                &["apfs", "list"],
                20,
            )?;
            inspect(
                "Dischi fisici",
                "/usr/sbin/diskutil",
                &["list", "physical"],
                20,
            )?;
        }
        Action::VerifyVolume => {
            report.text = maintenance::verify_volume("/", |exe, args, seconds| {
                command::run(exe, args, None, cancel, seconds)
            })?;
        }
        Action::PeriodicStatus => {
            report.text = maintenance::periodic_status(
                Path::new("/usr/sbin/periodic").is_file(),
                Path::new("/etc/periodic").is_dir(),
            )
            .into();
        }
        Action::Snapshots => {
            inspect(
                "Snapshot locali Time Machine",
                "/usr/bin/tmutil",
                &["listlocalsnapshots", "/"],
                15,
            )?;
            inspect(
                "Snapshot APFS del volume dati",
                "/usr/sbin/diskutil",
                &["apfs", "listSnapshots", "/System/Volumes/Data"],
                15,
            )?;
        }
        Action::Performance => {
            inspect(
                "CPU, memoria e carico del sistema",
                "/usr/bin/top",
                &["-l", "1", "-n", "0"],
                12,
            )?;
            inspect(
                "Memoria disponibile secondo macOS",
                "/usr/bin/memory_pressure",
                &["-Q"],
                10,
            )?;
            inspect(
                "Batteria e alimentazione",
                "/usr/bin/pmset",
                &["-g", "batt"],
                10,
            )?;
        }
        Action::Login => inspect(
            "Registrazioni di login e attività in background",
            "/usr/bin/sfltool",
            &["dumpbtm"],
            20,
        )?,
        Action::Updates => inspect(
            "Aggiornamenti disponibili per macOS",
            "/usr/sbin/softwareupdate",
            &["--list"],
            120,
        )?,
        Action::Brew => {
            let brew = installed(BREW).ok_or("Homebrew non trovato nei percorsi standard.")?;
            inspect(
                "Formule aggiornabili",
                brew,
                &["outdated", "--formula", "--quiet"],
                60,
            )?;
            inspect(
                "Cask aggiornabili",
                brew,
                &["outdated", "--cask", "--quiet"],
                60,
            )?;
        }
        Action::Protection => {
            inspect("Gatekeeper", "/usr/sbin/spctl", &["--status"], 10)?;
            inspect("FileVault", "/usr/bin/fdesetup", &["status"], 10)?;
            if let Some(scanner) = installed(CLAM) {
                let db =
                    signatures::local_database().map(|p| format!("--database={}", p.display()));
                let mut args = vec!["--version"];
                if let Some(p) = db.as_deref() {
                    args.push(p);
                }
                inspect(
                    "ClamAV e versione delle firme disponibili",
                    scanner,
                    &args,
                    15,
                )?;
            } else {
                report.text.push_str("ClamAV assente. La scansione richiede il motore locale e il database delle firme.");
            }
        }
        Action::Scan(path) => (report.text, report.findings) = security_scan::scan(&path, cancel)?,
        Action::Recheck(finding) => report.text = security_scan::recheck(&finding, cancel)?,
        Action::UpdateSignatures => report.text = signatures::update(cancel)?,

        Action::Cloud => {
            let home = policy::home();
            if !home.is_absolute() {
                return Err("Cartella utente non disponibile.".into());
            }
            let base = home.join("Library/CloudStorage");
            let out = command::run(
                "/bin/ls",
                &["-1", base.to_str().ok_or("Percorso non leggibile.")?],
                None,
                cancel,
                12,
            )?;
            if out.code != Some(0) {
                report.text = format!("Cartella dei provider non leggibile.\n{}\n", out.text);
            } else {
                for name in out.text.lines().take(128) {
                    let path = base.join(name);
                    if Path::new(name).components().count() == 1
                        && path.parent() == Some(base.as_path())
                        && !policy::protected(&path, Mode::Space, None)
                        && path.symlink_metadata().is_ok_and(|m| m.is_dir())
                    {
                        report.providers.push(path);
                    }
                }
                if out.truncated || out.text.lines().count() > 128 {
                    report.text.push_str("Elenco parziale.\n");
                }
            }
            let icloud = home.join("Library/Mobile Documents/com~apple~CloudDocs");
            if icloud.symlink_metadata().is_ok_and(|m| m.is_dir()) {
                report.providers.push(icloud);
            }
            report.text.push_str(&format!(
                "Cartelle di provider disponibili in Finder: {}",
                report.providers.len()
            ));
        }
        Action::Maintenance(action) => report.text = maintenance::execute(action, cancel)?,
        Action::Open(target) => {
            let out = command::run("/usr/bin/open", &[&target], None, cancel, 12)?;
            if out.code != Some(0) {
                return Err(format!("Apertura non riuscita. {}", out.text));
            }
            report.opened = true;
            report.text = "Richiesta di apertura inviata a macOS.".into();
        }
    }
    Ok(report)
}

type JobResult = Result<Report, String>;
struct Job {
    action: Action,
    cancel: Arc<AtomicBool>,
    rx: mpsc::Receiver<JobResult>,
}
impl Drop for Job {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}

#[derive(Default)]
pub(crate) struct SystemTools {
    section: Section,
    file: String,
    pending: Option<Mutation>,
    monitor: folder_monitor::Monitor,
    job: Option<Job>,
    report: Option<Report>,
    scan_findings: Vec<security_scan::Finding>,
    message: String,
}
impl SystemTools {
    pub(crate) fn monitor_alert(&mut self, ctx: &egui::Context) {
        self.monitor.alert(ctx);
    }
    fn button(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, label: &str, action: Action) {
        if !ui
            .add_enabled(self.job.is_none(), egui::Button::new(label))
            .clicked()
        {
            return;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&cancel);
        let (tx, rx) = mpsc::sync_channel(1);
        let repaint = ctx.clone();
        self.message = format!("{label}…");
        if !matches!(action, Action::Open(_)) {
            self.report = None;
        }
        let job_action = action.clone();
        let spawn = std::thread::Builder::new()
            .name("lindo-system".into())
            .spawn(move || {
                let _ = tx.send(execute(action, &flag));
                repaint.request_repaint();
            });
        match spawn {
            Ok(_) => {
                self.job = Some(Job {
                    action: job_action,
                    cancel,
                    rx,
                })
            }
            Err(e) => self.message = format!("Impossibile avviare il controllo: {e}"),
        }
    }

    fn propose(&mut self, ui: &mut egui::Ui, action: Mutation) {
        if ui
            .add_enabled(self.job.is_none(), egui::Button::new(action.label()))
            .clicked()
        {
            self.pending = Some(action);
        }
    }
    fn open(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, label: &str, target: &str) {
        self.button(ui, ctx, label, Action::Open(target.to_owned()));
    }

    pub(crate) fn show(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        if let Some(job) = &self.job {
            let received = match job.rx.try_recv() {
                Ok(r) => Some(r),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => Some(Err("Operazione interrotta.".into())),
            };
            if let Some(result) = received {
                let job = self.job.take().unwrap();
                match result {
                    Ok(r) if r.opened => self.message = r.text,
                    Ok(mut r) => {
                        match &job.action {
                            Action::Scan(_) => self.scan_findings = std::mem::take(&mut r.findings),
                            Action::Maintenance(Mutation::Detected(done)) => {
                                self.scan_findings.retain(|finding| finding != done);
                                self.monitor.resolve(done);
                            }
                            _ => {}
                        }
                        self.report = Some(r);
                        self.message.clear();
                    }
                    Err(e) => self.message = e,
                }
            }
        }
        ui.heading("Strumenti di sistema");
        if let Some(pending) = self.pending.clone() {
            ui.group(|ui| {
                ui.heading(pending.label());
                ui.label(pending.explanation());
                ui.horizontal_wrapped(|ui| {
                    if ui.button("Annulla").clicked() {
                        self.pending = None;
                    }
                    self.button(ui, ctx, "Conferma ed esegui", Action::Maintenance(pending));
                });
            });
            if self.job.is_some() {
                self.pending = None;
            }
            return;
        }
        let previous = self.section;
        ui.add_enabled_ui(self.job.is_none(), |ui| {
            ui.horizontal_wrapped(|ui| {
                for (s, title) in [
                    (Section::Storage, "Dischi"),
                    (Section::Performance, "Prestazioni"),
                    (Section::Updates, "Aggiornamenti"),
                    (Section::Protection, "Sicurezza e privacy"),
                    (Section::Cloud, "Cloud"),
                ] {
                    ui.selectable_value(&mut self.section, s, title);
                }
            });
        });
        if previous != self.section {
            self.message.clear();
            self.report = None;
        }
        ui.separator();
        if let Some(job) = &self.job {
            ui.horizontal(|ui| {
                ui.spinner();
                if ui.button("Annulla").clicked() {
                    job.cancel.store(true, Ordering::Release);
                }
            });
            ctx.request_repaint_after(Duration::from_millis(180));
        }
        if !self.message.is_empty() {
            ui.label(&self.message);
        }
        egui::ScrollArea::vertical().id_salt("system_scroll").show(ui, |ui| {
            match self.section {
                Section::Storage => {
                    ui.label("Esamina lo spazio effettivamente disponibile, i dischi fisici e gli snapshot del volume dati.");
                    ui.horizontal_wrapped(|ui| {
                        self.button(ui,ctx,"Analizza i volumi",Action::Storage);
                        self.button(ui,ctx,"Elenca snapshot",Action::Snapshots);
                        self.open(ui,ctx,"Utility Disco e S.O.S.","/System/Applications/Utilities/Disk Utility.app");
                        self.open(ui,ctx,"Gestisci spazio macOS","x-apple.systempreferences:com.apple.settings.Storage");
                        self.open(ui,ctx,"Configura Time Machine","x-apple.systempreferences:com.apple.Time-Machine-Settings.extension");
                        self.propose(ui,Mutation::EmptyTrash);
                        self.propose(ui,Mutation::ThinSnapshots);
                    });
                    ui.strong("Verifica del filesystem");
                    ui.label("Controlla il volume di avvio con diskutil. Il controllo può rallentare il Mac per alcuni minuti. Per riparare un volume, apri Utility Disco e scegli S.O.S.");
                    self.button(ui,ctx,"Verifica volume di avvio",Action::VerifyVolume);
                    let snapshots = self.report.as_ref().map(|r|r.snapshots.clone()).unwrap_or_default();
                    for date in snapshots { self.propose(ui, Mutation::Snapshot(date)); }
                }
                Section::Performance => {
                    ui.label("Misura CPU, RAM e alimentazione. Consulta le registrazioni degli elementi di login e modifica le abilitazioni in Impostazioni di Sistema.");
                    ui.horizontal_wrapped(|ui| {
                        self.button(ui,ctx,"Misura risorse",Action::Performance);
                        self.button(ui,ctx,"Elenca attività in background",Action::Login);
                        self.open(ui,ctx,"Gestisci elementi di login","x-apple.systempreferences:com.apple.LoginItems-Settings.extension");
                        self.button(ui,ctx,"Controlla strumenti periodici",Action::PeriodicStatus);
                        self.propose(ui,Mutation::FlushDns);
                        self.propose(ui,Mutation::Spotlight);
                        self.propose(ui,Mutation::PurgeBuffers);
                        self.propose(ui,Mutation::ResetUserPermissions);
                    });
                    ui.add_space(8.0);
                    ui.strong("Memoria delle app");
                    ui.label("In Monitoraggio Attività apri Memoria e ordina per memoria utilizzata. Salva il lavoro, seleziona un’app inutilizzata, premi Stop e scegli Esci. macOS gestisce la memoria liberabile.");
                    self.open(ui,ctx,"Apri Monitoraggio Attività","/System/Applications/Utilities/Activity Monitor.app");
                    ui.add_space(8.0);
                    ui.strong("Ricostruzione di una casella Mail");
                    ui.label("Apri Mail, seleziona una casella nella barra laterale e scegli Casella > Ricostruisci. Con un account IMAP, Mail scarica nuovamente i messaggi dal server.");
                    self.open(ui,ctx,"Apri Mail per scegliere la casella","/System/Applications/Mail.app");
                }
                Section::Updates => {
                    ui.label("Verifica la disponibilità di aggiornamenti. Homebrew usa il catalogo locale, che può essere meno recente.");
                    ui.horizontal_wrapped(|ui| {
                        self.button(ui,ctx,"Cerca aggiornamenti macOS",Action::Updates);
                        self.button(ui,ctx,"Controlla Homebrew",Action::Brew);
                        self.open(ui,ctx,"Aggiorna macOS","x-apple.systempreferences:com.apple.Software-Update-Settings.extension");
                        self.open(ui,ctx,"Aggiornamenti App Store","macappstore://showUpdatesPage");
                    });
                    let packages = self.report.as_ref().map(|r| r.packages.clone()).unwrap_or_default();
                    for item in packages {self.propose(ui,item);}
                }
                Section::Protection => {
                    let mut target_changed = false;
                    if self.job.is_none() {
                        if let Some(path) = ctx.input(|i| i.raw.dropped_files.first().and_then(|f| f.path.clone())) {
                            match path.to_str() {
                                Some(p) => {
                                    target_changed = p != self.file;
                                    self.file = p.to_owned();
                                }
                                None => self.message = "Il nome del file contiene caratteri non supportati.".into(),
                            }
                        }
                    }
                    ui.horizontal_wrapped(|ui| {
                        self.button(ui,ctx,"Controlla protezioni",Action::Protection);
                        self.open(ui,ctx,"Gestisci permessi","x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension");
                    });
                    self.button(ui,ctx,"Scarica o aggiorna firme ufficiali",Action::UpdateSignatures);
                    ui.label("Le firme scaricate da ClamAV vengono salvate nella cartella di supporto di Lindo.");
                    ui.add_space(8.0);
                    ui.strong("Scansione locale con ClamAV");
                    ui.label("Trascina un file o una cartella dal Finder oppure inserisci il percorso. Limiti per cartella: 2.000 file, 20.000 voci e 15 minuti. Sono esclusi i file oltre 64 MiB, i pacchetti, i percorsi cloud e le aree protette. Le rilevazioni rimangono disponibili nel rapporto.");
                    let edit = ui.add_enabled(self.job.is_none(), egui::TextEdit::singleline(&mut self.file).hint_text("Percorso del file o della cartella").desired_width(f32::INFINITY));
                    if target_changed || edit.changed() {
                        self.report = None;
                        self.message.clear();
                    }
                    ui.add_enabled_ui(!self.file.is_empty(), |ui| {
                        self.button(ui,ctx,"Avvia scansione",Action::Scan(self.file.as_str().into()));
                    });
                    let mut findings=self.scan_findings.clone();
                    for finding in self.monitor.show(ui,ctx,&self.file) {
                        if !findings.contains(&finding) { findings.push(finding); }
                    }
                    if !findings.is_empty() { ui.strong("Segnalazioni da rivedere"); }
                    for finding in findings {
                        ui.label(finding.path.display().to_string());
                        ui.horizontal_wrapped(|ui| {
                            self.button(ui,ctx,"Verifica e mostra nel Finder",Action::Recheck(finding.clone()));
                            if ui.add_enabled(self.job.is_none(),egui::Button::new("Sposta nel Cestino")).clicked() {
                                self.pending=Some(Mutation::Detected(finding));
                            }
                        });
                    }
                }
                Section::Cloud => {
                    ui.label("Apri un provider nel Finder e usa “Rimuovi download” o “Rendi disponibile solo online” per liberare la copia locale. Lo spostamento nel Cestino può propagare l’eliminazione agli altri dispositivi.");
                    self.button(ui,ctx,"Rileva cartelle cloud",Action::Cloud);
                    let paths = self.report.as_ref().map(|r| r.providers.clone()).unwrap_or_default();
                    for path in paths {
                        if let Some(value) = path.to_str() {
                            let name = if value.ends_with("/com~apple~CloudDocs") { "iCloud Drive" } else { path.file_name().and_then(|s|s.to_str()).unwrap_or(value) };
                            self.open(ui,ctx,&format!("Apri {name} in Finder"),value);
                        }
                    }
                }
            }
            if let Some(report) = &self.report {
                ui.separator();
                ui.add(egui::Label::new(egui::RichText::new(&report.text).monospace()).wrap().selectable(true));
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dropping_a_new_target_clears_the_previous_scan_result() {
        let mut panel = SystemTools {
            section: Section::Protection,
            file: "/fixture/old".into(),
            report: Some(Report {
                opened: false,
                text: "Nessuna firma rilevata.".into(),
                providers: vec![],
                snapshots: vec![],
                packages: vec![],
                findings: vec![],
            }),
            ..Default::default()
        };
        let input = egui::RawInput {
            dropped_files: vec![egui::DroppedFile {
                path: Some("/fixture/new".into()),
                ..Default::default()
            }],
            ..Default::default()
        };
        let ctx = egui::Context::default();
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| panel.show(ui, ctx));
        });
        assert_eq!(panel.file, "/fixture/new");
        assert!(panel.report.is_none());
    }

    #[test]
    fn scanner_requires_a_completed_scan_before_reporting_a_clean_result() {
        for (code, text, truncated, expected) in [
            (0, "Scanned files: 1", false, "Nessuna firma"),
            (0, "Scanned files: 0", false, "Scansione incompleta"),
            (0, "Scanned files: 2", false, "Nessuna firma"),
            (0, "Scanned files: 1", true, "Scansione incompleta"),
            (
                1,
                "stdin: Eicar-Test-Signature FOUND",
                false,
                "ClamAV ha segnalato",
            ),
            (2, "No databases found", false, "Scansione incompleta"),
        ] {
            assert!(verdict(&command::Output {
                code: Some(code),
                text: text.into(),
                truncated
            })
            .starts_with(expected));
        }
    }
    #[test]
    fn command_worker_bounds_output_and_enforces_deadlines() {
        let flag = AtomicBool::new(false);
        let output = command::run(
            "/usr/bin/head",
            &["-c", "300000", "/dev/zero"],
            None,
            &flag,
            5,
        )
        .unwrap();
        assert!(output.truncated);
        assert!(output.text.len() < 100000);
        assert!(command::run("/bin/sleep", &["10"], None, &flag, 0)
            .unwrap_err()
            .contains("massimo"));
        flag.store(true, Ordering::Release);
        assert!(command::run("/bin/sleep", &["10"], None, &flag, 5)
            .unwrap_err()
            .contains("annullata"));
    }
}
