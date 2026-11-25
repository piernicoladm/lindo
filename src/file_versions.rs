//! Explicit inspection of one document's macOS version history.
use crate::{
    model::{Identity, Mode},
    policy,
};
use block2::RcBlock;
use eframe::egui;
use objc2::rc::{autoreleasepool, Retained};
use objc2_foundation::{NSFileCoordinator, NSFileCoordinatorWritingOptions, NSFileVersion, NSURL};
use std::{
    cell::RefCell,
    os::unix::{fs::MetadataExt, io::AsRawFd},
    path::{Path, PathBuf},
    ptr::NonNull,
    sync::mpsc::{self, Receiver, Sender},
    time::Duration,
};

fn validate_doc(path: &Path, home: &Path) -> Result<Identity, String> {
    let denied =
        "Scegli un documento locale di tua proprietà, fuori dalle cartelle cloud e protette.";
    if !home.is_absolute()
        || !path.is_absolute()
        || (!path.starts_with(home) && !path.starts_with("/Volumes"))
        || policy::protected(path, Mode::Large, None)
        || policy::cloud(path)
        || path.components().any(|p| {
            let s = p.as_os_str().to_string_lossy().to_lowercase();
            matches!(
                s.as_str(),
                "_3_vault"
                    | "library"
                    | "applications"
                    | "system"
                    | "dropbox"
                    | "onedrive"
                    | "google drive"
            ) || s.starts_with("dropbox-")
                || s.starts_with("onedrive-")
                || s.starts_with("googledrive-")
        })
        || !path.extension().and_then(|x| x.to_str()).is_some_and(|x| {
            [
                "txt", "text", "md", "markdown", "rtf", "csv", "tsv", "pdf", "pages", "key",
                "numbers", "odt", "ods", "odp", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "svg",
                "html", "xml", "json", "tex", "yaml", "yml", "png", "jpeg", "jpg", "heic", "heif",
                "tif", "tiff",
            ]
            .iter()
            .any(|e| x.eq_ignore_ascii_case(e))
        })
        || path.ancestors().skip(1).any(policy::package)
    {
        return Err(denied.into());
    }
    let fd = policy::open_file(path, libc::O_EVTONLY).map_err(|_| denied)?;
    if policy::local_provider_state(path).map_err(|_| denied)? != (true, false) {
        return Err(denied.into());
    }
    let mut disk = std::mem::MaybeUninit::uninit();
    if unsafe { libc::fstatfs(fd.as_raw_fd(), disk.as_mut_ptr()) } != 0
        || unsafe { disk.assume_init() }.f_flags & libc::MNT_LOCAL as u32 == 0
    {
        return Err(denied.into());
    }
    document_identity(path, &fd).map_err(|_| denied.into())
}

fn document_identity(path: &Path, fd: &std::fs::File) -> std::io::Result<Identity> {
    let identity = Identity::read(path)?;
    let metadata = fd.metadata()?;
    if !metadata.is_file()
        || metadata.uid() != unsafe { libc::getuid() }
        || metadata.nlink() != 1
        || !identity.matches_metadata(&metadata)
    {
        return Err(std::io::Error::other(
            "Identità del documento non verificabile.",
        ));
    }
    Ok(identity)
}
#[derive(Clone)]
struct Row {
    name: String,
    date: String,
    computer: String,
    local: bool,
    conflict: bool,
}
fn row(version: &NSFileVersion) -> Row {
    Row {
        name: version
            .localizedName()
            .map(|s| s.to_string())
            .unwrap_or_else(|| "Documento".into()),
        date: version
            .modificationDate()
            .map(|d| d.description().to_string())
            .unwrap_or_else(|| "Data non disponibile".into()),
        computer: version
            .localizedNameOfSavingComputer()
            .map(|s| s.to_string())
            .unwrap_or_default(),
        local: version.hasLocalContents(),
        conflict: version.isConflict(),
    }
}
struct Catalog {
    identity: Identity,
    url: Retained<NSURL>,
    current: Retained<NSFileVersion>,
    history: Vec<Retained<NSFileVersion>>,
}
impl Catalog {
    fn read(path: &Path, home: &Path) -> Result<Self, String> {
        let identity = validate_doc(path, home)?;
        let url = NSURL::from_file_path(path).ok_or("Percorso del documento non valido.")?;
        let current = NSFileVersion::currentVersionOfItemAtURL(&url)
            .ok_or("La versione corrente non è disponibile.")?;
        let versions = NSFileVersion::otherVersionsOfItemAtURL(&url)
            .ok_or("macOS non ha restituito l’elenco delle revisioni.")?;
        if !identity.matches(path) {
            return Err("Documento cambiato durante la consultazione.".into());
        }
        let history = versions.iter().collect();
        Ok(Self {
            identity,
            url,
            current,
            history,
        })
    }
    fn remove(&self, path: &Path, home: &Path, index: usize) -> Result<(), String> {
        let selected = self
            .history
            .get(index)
            .ok_or("Revisione non presente nell’elenco.")?;
        if !selected.hasLocalContents() || selected.isConflict() || selected == &self.current {
            return Err("Revisione protetta.".into());
        }
        let id = selected.persistentIdentifier();
        coordinated(&self.url, |url| {
            if url.to_file_path().as_deref() != Some(path)
                || validate_doc(path, home)? != self.identity
            {
                return Err("Il documento è cambiato. Consulta di nuovo le revisioni.".into());
            }
            // This is Apple's own identifier from the selected NSFileVersion,
            // retained on this worker for the lifetime of the explicit review.
            let resolve = || unsafe {
                NSFileVersion::versionOfItemAtURL_forPersistentIdentifier(url, id.as_ref())
            };
            let actual = resolve().ok_or("La revisione selezionata non è più disponibile.")?;
            let current = NSFileVersion::currentVersionOfItemAtURL(url)
                .ok_or("Versione corrente non identificabile.")?;
            let others =
                NSFileVersion::otherVersionsOfItemAtURL(url).ok_or("Elenco non verificabile.")?;
            let members = others.iter().filter(|v| v == &actual).count();
            if !eligible(
                &actual,
                selected,
                &current,
                members,
                actual.hasLocalContents(),
                actual.isConflict(),
            ) || current != self.current
                || !self.identity.matches(path)
            {
                return Err("La revisione o il documento sono cambiati.".into());
            }
            actual
                .removeAndReturnError()
                .map_err(|e| format!("Rimozione non riuscita. {}", e.localizedDescription()))?;
            if resolve().is_some()
                || NSFileVersion::currentVersionOfItemAtURL(url)
                    .is_none_or(|after| after != current)
            {
                return Err("macOS ha accettato la rimozione. Consulta di nuovo la cronologia per verificarne l’esito.".into());
            }
            Ok(())
        })
    }
}
fn eligible<T: PartialEq>(
    actual: &T,
    selected: &T,
    current: &T,
    members: usize,
    local: bool,
    conflict: bool,
) -> bool {
    actual == selected && actual != current && members == 1 && local && !conflict
}
fn coordinated<T>(url: &NSURL, access: impl Fn(&NSURL) -> Result<T, String>) -> Result<T, String> {
    let result = RefCell::new(None);
    let writer = RcBlock::new(|location: NonNull<NSURL>| {
        // Foundation supplies a valid coordinated file URL for this synchronous call.
        result.replace(Some(access(unsafe { location.as_ref() })));
    });
    let mut error = None;
    NSFileCoordinator::new().coordinateWritingItemAtURL_options_error_byAccessor(
        url,
        NSFileCoordinatorWritingOptions::empty(),
        Some(&mut error),
        &writer,
    );
    drop(writer);
    if let Some(error) = error {
        return Err(format!(
            "Coordinamento del documento non riuscito. {}",
            error.localizedDescription()
        ));
    }
    result
        .into_inner()
        .ok_or("macOS non ha concesso l’accesso al documento.")?
}

enum Update {
    Listed(Row, Vec<Row>),
    Finished(Result<(), String>),
}
struct Session {
    tx: Sender<usize>,
    rx: Receiver<Update>,
}
#[derive(Default)]
pub(crate) struct FileVersions {
    entry: String,
    path: Option<PathBuf>,
    current: Option<Row>,
    versions: Vec<Row>,
    review: Option<usize>,
    session: Option<Session>,
    busy: bool,
    status: String,
}
impl FileVersions {
    fn reset(&mut self) {
        self.session = None;
        self.review = None;
        self.current = None;
        self.versions.clear();
        self.path = None;
        self.busy = false;
        self.status.clear();
    }
    fn inspect(&mut self, ctx: &egui::Context, path: PathBuf) {
        self.reset();
        self.path = Some(path.clone());
        self.busy = true;
        self.status = "Consultazione della cronologia…".into();
        let (tx, request) = mpsc::channel();
        let (send, rx) = mpsc::channel();
        let ctx = ctx.clone();
        self.session = Some(Session { tx, rx });
        std::thread::spawn(move || {
            let result = autoreleasepool(|_| {
                let _guard = policy::LocalOnly::enter().map_err(|e| e.to_string())?;
                let home = policy::home();
                let catalog = Catalog::read(&path, &home)?;
                let summary = catalog.history.iter().map(|v| row(v)).collect();
                if send
                    .send(Update::Listed(row(&catalog.current), summary))
                    .is_err()
                {
                    return Ok(());
                }
                ctx.request_repaint();
                // Dropping the panel's sender releases this document's identifiers
                // without invoking any mutation.
                if let Ok(index) = request.recv() {
                    catalog.remove(&path, &home, index)?;
                }
                Ok(())
            });
            let _ = send.send(Update::Finished(result));
            ctx.request_repaint();
        });
    }
    pub(crate) fn show(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.heading("Revisioni di un documento");
        ui.label("Trascina un documento locale o inserisci il percorso completo. La cronologia dipende dall’app che ha salvato il file.");
        if let Some(session) = &self.session {
            match session.rx.try_recv() {
                Ok(Update::Listed(now, rows)) => {
                    self.busy = false;
                    self.current = Some(now);
                    self.versions = rows;
                    self.status = format!("{} revisioni storiche disponibili", self.versions.len());
                }
                Ok(Update::Finished(result)) => {
                    self.reset();
                    self.status = match result {
                        Ok(()) => "Revisione storica rimossa. Versione corrente verificata.".into(),
                        Err(e) => e,
                    };
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.reset();
                    self.status = "Consultazione interrotta.".into();
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if self.busy {
            ui.spinner();
            ui.label(&self.status);
            if self.current.is_none() && ui.button("Interrompi consultazione").clicked() {
                self.reset();
                self.status = "Consultazione interrotta.".into();
            }
            ctx.request_repaint_after(Duration::from_millis(180));
            return;
        }
        if let Some(drop) = ctx.input(|i| {
            crate::extras::single_drop(&i.raw.dropped_files).map(|r| r.map(Path::to_path_buf))
        }) {
            self.reset();
            match drop {
                Ok(path) => self.entry = path.to_string_lossy().into_owned(),
                Err(reason) => {
                    self.entry.clear();
                    self.status = reason.into();
                }
            }
        }
        if ui.text_edit_singleline(&mut self.entry).changed() {
            self.reset();
        }
        if ui
            .add_enabled(
                !self.entry.is_empty(),
                egui::Button::new("Consulta revisioni"),
            )
            .clicked()
        {
            self.inspect(ctx, PathBuf::from(&self.entry));
            return;
        }
        ui.label(&self.status);
        if let Some(current) = &self.current {
            ui.label(format!(
                "Versione corrente · {} · {}",
                current.name, current.date
            ));
        }
        if let Some(index) = self.review {
            let Some(r) = self.versions.get(index) else {
                return;
            };
            ui.heading("Rimozione permanente della revisione");
            if let Some(p) = &self.path {
                ui.label(p.display().to_string());
            }
            ui.label(format!("Revisione {} · {} · {}", index + 1, r.name, r.date));
            ui.label("macOS elimina definitivamente questa copia storica. Il Cestino non consente di recuperarla.");
            if ui
                .button("Elimina definitivamente questa revisione")
                .clicked()
            {
                self.review = None;
                if self
                    .session
                    .as_ref()
                    .is_some_and(|s| s.tx.send(index).is_ok())
                {
                    self.busy = true;
                    self.status = "Verifica e rimozione della revisione…".into();
                } else {
                    self.reset();
                    self.status = "Consultazione scaduta.".into();
                }
            } else if ui.button("Annulla").clicked() {
                self.review = None;
            }
            return;
        }
        egui::ScrollArea::vertical().show_rows(ui, 38., self.versions.len(), |ui, range| {
            for i in range {
                let r = &self.versions[i];
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(
                            r.local && !r.conflict,
                            egui::Button::new("Rivedi rimozione"),
                        )
                        .clicked()
                    {
                        self.review = Some(i);
                    }
                    let label = format!("{} · {}", r.name, r.date);
                    ui.add(egui::Label::new(&label).truncate())
                        .on_hover_text(label);
                });
                if r.conflict {
                    ui.small("In conflitto");
                } else if !r.local {
                    ui.small("Contenuto non locale");
                } else if !r.computer.is_empty() {
                    ui.small(&r.computer);
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use objc2_foundation::NSFileVersionAddingOptions;
    use std::fs;
    #[test]
    fn cancelling_inspection_closes_the_mutation_channel() {
        let (tx, requests) = mpsc::channel();
        let (_updates, rx) = mpsc::channel();
        let mut panel = FileVersions {
            busy: true,
            session: Some(Session { tx, rx }),
            entry: "/synthetic/document".into(),
            ..Default::default()
        };
        panel.reset();
        assert!(requests.recv().is_err());
        assert!(!panel.busy);
        assert_eq!(panel.entry, "/synthetic/document");
    }

    #[test]
    fn version_history_rejects_document_replaced_after_open() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().canonicalize().unwrap();
        let doc = home.join("draft.txt");
        fs::write(&doc, b"original").unwrap();
        let fd = policy::open_file(&doc, libc::O_EVTONLY).unwrap();
        assert!(document_identity(&doc, &fd).is_ok());
        fs::rename(&doc, home.join("previous.txt")).unwrap();
        fs::write(&doc, b"replaced").unwrap();
        assert!(document_identity(&doc, &fd).is_err());
    }

    #[test]
    fn version_history_requires_absolute_home() {
        let temp = tempfile::Builder::new()
            .prefix("Lindo-doc-")
            .tempdir()
            .unwrap();
        let home = temp.path().canonicalize().unwrap();
        let doc = home.join("draft.txt");
        fs::write(&doc, b"local document").unwrap();
        assert!(validate_doc(&doc, Path::new("")).is_err());
        assert!(validate_doc(&doc, Path::new("relative")).is_err());
        assert!(validate_doc(&doc, &home).is_ok());
    }

    #[test]
    fn requires_exact_unique_historical_local_identifier() {
        assert!(eligible(&5, &5, &9, 1, true, false));
        for (actual, selected, current, members, local, conflict) in [
            (9, 9, 9, 1, true, false),
            (5, 6, 9, 1, true, false),
            (5, 5, 9, 0, true, false),
            (5, 5, 9, 2, true, false),
            (5, 5, 9, 1, false, false),
            (5, 5, 9, 1, true, true),
        ] {
            assert!(!eligible(
                &actual, &selected, &current, members, local, conflict
            ));
        }
    }
    #[test]
    fn native_history_preserves_current_file_on_synthetic_volume() {
        let temp = tempfile::Builder::new()
            .prefix("Lindo-versions-")
            .tempdir()
            .unwrap();
        let root = temp.path().canonicalize().unwrap();
        let doc = root.join("draft.txt");
        let old = root.join("previous.txt");
        fs::write(&doc, b"Current fixture contents").unwrap();
        fs::write(&old, b"Historical fixture contents").unwrap();
        let _local = policy::LocalOnly::enter().unwrap();
        let before = Catalog::read(&doc, &root).unwrap();
        assert!(before.history.is_empty());
        let url = NSURL::from_file_path(&doc).unwrap();
        let add = |p: &Path| {
            let prior = NSURL::from_file_path(p).unwrap();
            coordinated(&url, |url| {
                NSFileVersion::addVersionOfItemAtURL_withContentsOfURL_options_error(
                    url,
                    &prior,
                    NSFileVersionAddingOptions::empty(),
                )
                .map_err(|e| e.localizedDescription().to_string())
            })
            .unwrap()
        };
        let first = add(&old);
        let older = root.join("older.txt");
        fs::write(&older, b"Another historical fixture").unwrap();
        let second = add(&older);
        let history = Catalog::read(&doc, &root).unwrap();
        assert_eq!(history.history.len(), 2);
        let index = history.history.iter().position(|v| v == &first).unwrap();
        assert!(history.remove(&doc, &root, 2).is_err());
        history.remove(&doc, &root, index).unwrap();
        let remaining = Catalog::read(&doc, &root).unwrap();
        assert_eq!(remaining.history.len(), 1);
        assert_eq!(remaining.history[0], second);
        remaining.remove(&doc, &root, 0).unwrap();
        assert!(Catalog::read(&doc, &root).unwrap().history.is_empty());
        assert_eq!(fs::read(&doc).unwrap(), b"Current fixture contents");
    }
}
