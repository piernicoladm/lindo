#[path = "clutter_images.rs"]
mod images;

use crate::{
    extras::Proposal,
    model::{Item, Mode, Progress, Request},
    picker::Picker,
    policy, scan,
};
use eframe::egui;
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    time::{Duration, SystemTime},
};

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Tool {
    #[default]
    LargeOld,
    Downloads,
    Similar,
}
impl Tool {
    fn title(self) -> &'static str {
        match self {
            Self::LargeOld => "File grandi e vecchi",
            Self::Downloads => "Download e installatori",
            Self::Similar => "Immagini simili",
        }
    }
}

fn kind(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|v| v.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match ext.as_str() {
        "dmg" | "pkg" | "mpkg" | "iso" => "Installatori",
        "zip" | "tar" | "gz" | "tgz" | "bz2" | "xz" | "rar" | "7z" | "zst" => "Archivi",
        "jpg" | "jpeg" | "png" | "gif" | "heic" | "tif" | "tiff" | "webp" | "svg" => "Immagini",
        "mov" | "mp4" | "mkv" | "avi" | "m4v" => "Video",
        "wav" | "mp3" | "aiff" | "flac" | "m4a" => "Audio",
        "pdf" | "doc" | "docx" | "txt" | "rtf" | "xlsx" | "csv" | "pptx" => "Documenti",
        _ => "Altri file",
    }
}

struct Row {
    item: Item,
    kind: &'static str,
    age: Option<u64>,
    visual: Option<images::Visual>,
    keeper: bool,
}

struct Results {
    request: Request,
    rows: Vec<Row>,
    status: String,
}

fn group_images(mut rows: Vec<Row>, stop: &AtomicBool) -> Vec<Row> {
    rows.sort_by(|a, b| {
        let pixels = |r: &Row| {
            r.visual
                .as_ref()
                .map(|v| u64::from(v.width) * u64::from(v.height))
        };
        pixels(b)
            .cmp(&pixels(a))
            .then_with(|| b.item.bytes.cmp(&a.item.bytes))
            .then_with(|| a.item.path.cmp(&b.item.path))
    });
    let mut groups: Vec<Vec<Row>> = Vec::new();
    for row in rows {
        if stop.load(Ordering::Acquire) {
            return Vec::new();
        }
        let Some(visual) = row.visual.as_ref() else {
            continue;
        };
        if let Some(group) = groups
            .iter_mut()
            .find(|g| g[0].visual.as_ref().is_some_and(|v| v.similar(visual)))
        {
            group.push(row);
        } else {
            groups.push(vec![row]);
        }
    }
    let mut result = Vec::new();
    for (group_index, group) in groups.into_iter().filter(|v| v.len() > 1).enumerate() {
        for (index, mut row) in group.into_iter().enumerate() {
            row.item.group = Some(group_index as u64 + 1);
            row.keeper = index == 0;
            result.push(row);
        }
    }
    result
}

fn analyze(
    request: Request,
    tool: Tool,
    min_mb: u64,
    days: u64,
    stop: &AtomicBool,
    progress: &Progress,
) -> Results {
    let report = scan::run(&request, stop, progress);
    let mut status = format!(
        "{} voci visitate · {} escluse · {} errori",
        report.visited, report.skipped, report.errors
    );
    if let Some(note) = report.notes.first() {
        status.push_str(&format!(" · {note}"));
    }
    let mut rows = Vec::new();
    let Ok(_guard) = policy::LocalOnly::enter() else {
        return Results {
            request,
            rows,
            status: "Protezione dei file cloud non disponibile.".into(),
        };
    };
    let now = SystemTime::now();
    let mut items = report.items;
    let mut rejected = 0;
    let mut attempted = 0;
    let mut capped = 0;
    if tool == Tool::Similar {
        items.retain(|v| images::supported(&v.path));
        items.sort_by(|a, b| a.path.cmp(&b.path));
    }
    for item in items {
        if stop.load(Ordering::Acquire) {
            break;
        }
        let mut age = None;
        let mut visual = None;
        if tool == Tool::Similar {
            if attempted >= images::MAX_IMAGES {
                capped += 1;
                continue;
            }
            attempted += 1;
            match images::load(&item, stop) {
                Ok(value) => visual = Some(value),
                Err(_) => {
                    rejected += 1;
                    continue;
                }
            }
        } else {
            let metadata =
                policy::open_file(&item.path, libc::O_EVTONLY).and_then(|f| f.metadata());
            match metadata {
                Ok(m) if item.identity.matches_metadata(&m) => {
                    age = m
                        .modified()
                        .ok()
                        .and_then(|v| now.duration_since(v).ok())
                        .map(|d| d.as_secs() / 86_400);
                }
                _ => {
                    rejected += 1;
                    continue;
                }
            }
            if tool == Tool::LargeOld
                && item.bytes < min_mb * 1_000_000
                && !age.is_some_and(|v| v >= days)
            {
                continue;
            }
        }
        rows.push(Row {
            kind: kind(&item.path),
            item,
            age,
            visual,
            keeper: false,
        });
    }
    if tool == Tool::Similar {
        rows = group_images(rows, stop);
        status.push_str(&format!(
            " · {attempted} immagini esaminate · {capped} oltre il limite"
        ));
    } else {
        rows.sort_by(|a, b| {
            b.item
                .bytes
                .cmp(&a.item.bytes)
                .then_with(|| a.item.path.cmp(&b.item.path))
        });
    }
    if rejected > 0 {
        status.push_str(&format!(
            " · {rejected} file illeggibili, cambiati o fuori limite"
        ));
    }
    Results {
        request,
        rows,
        status,
    }
}

struct Job {
    stop: Arc<AtomicBool>,
    progress: Arc<Progress>,
    rx: mpsc::Receiver<Results>,
}
impl Drop for Job {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

pub(crate) struct Clutter {
    tool: Tool,
    root: PathBuf,
    picker: Picker,
    mb: u64,
    days: u64,
    filter: &'static str,
    job: Option<Job>,
    results: Option<Results>,
    visible: Vec<usize>,
    selected: HashSet<usize>,
    textures: HashMap<usize, egui::TextureHandle>,
    message: String,
}
impl Default for Clutter {
    fn default() -> Self {
        Self {
            tool: Tool::LargeOld,
            root: policy::home().join("Downloads"),
            picker: Picker::new(),
            mb: 100,
            days: 180,
            filter: "Tutti",
            job: None,
            results: None,
            visible: Vec::new(),
            selected: HashSet::new(),
            textures: HashMap::new(),
            message: String::new(),
        }
    }
}
impl Clutter {
    fn clear(&mut self) {
        self.job.take();
        self.results.take();
        self.selected.clear();
        self.visible.clear();
        self.textures.clear();
        self.message.clear();
    }
    fn refilter(&mut self) {
        self.visible.clear();
        if let Some(r) = &self.results {
            self.visible.extend(
                r.rows
                    .iter()
                    .enumerate()
                    .filter(|(_, row)| self.filter == "Tutti" || row.kind == self.filter)
                    .map(|(i, _)| i),
            );
        }
    }
    fn start(&mut self, ctx: &egui::Context) {
        self.clear();
        let request = Request {
            mode: Mode::Large,
            roots: vec![self.root.clone()],
            min_bytes: 0,
        };
        let ctx = ctx.clone();
        let (tx, rx) = mpsc::channel();
        let (stop, progress) = (
            Arc::new(AtomicBool::new(false)),
            Arc::new(Progress::default()),
        );
        self.job = Some(Job {
            rx,
            stop: stop.clone(),
            progress: progress.clone(),
        });
        let (tool, mb, days) = (self.tool, self.mb, self.days);
        std::thread::spawn(move || {
            let result = analyze(request, tool, mb, days, &stop, &progress);
            if !stop.load(Ordering::Acquire) {
                let _ = tx.send(result);
            }
            ctx.request_repaint();
        });
    }
    fn proposal(&self) -> Option<Proposal> {
        let r = self.results.as_ref()?;
        let mut indices: Vec<_> = self.selected.iter().copied().collect();
        indices.sort_unstable();
        let items: Vec<_> = indices
            .into_iter()
            .filter_map(|i| r.rows.get(i))
            .filter(|v| !v.keeper)
            .map(|v| v.item.clone())
            .collect();
        if items.is_empty() {
            return None;
        }
        let groups: HashSet<_> = items.iter().filter_map(|v| v.group).collect();
        let retained: Vec<_> = r
            .rows
            .iter()
            .filter(|v| v.keeper && v.item.group.is_some_and(|g| groups.contains(&g)))
            .map(|v| v.item.clone())
            .collect();
        if groups.len() != retained.len() {
            return None;
        }
        Some(Proposal {
            request: r.request.clone(),
            items,
            retained,
            label: self.tool.title().into(),
        })
    }
    pub(crate) fn show(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) -> Option<Proposal> {
        self.picker.poll();
        if self.picker.visible() {
            if let Some((_, root)) = self.picker.draw(ui, ctx) {
                self.clear();
                self.root = root;
            }
            return None;
        }
        let old = self.tool;
        ui.horizontal_wrapped(|ui| {
            for tool in [Tool::LargeOld, Tool::Downloads, Tool::Similar] {
                ui.selectable_value(&mut self.tool, tool, tool.title());
            }
        });
        if self.tool != old {
            self.clear();
            self.filter = "Tutti";
        }
        ui.horizontal_wrapped(|ui| {
            ui.label(self.root.display().to_string());
            if ui.button("Scegli cartella").clicked() {
                self.clear();
                self.picker.open(Mode::Large, ctx);
            }
        });
        match self.tool {
            Tool::LargeOld => {
                ui.horizontal_wrapped(|ui| {
                    ui.label("Almeno");
                    let a = ui
                        .add(
                            egui::DragValue::new(&mut self.mb)
                                .range(1..=100_000)
                                .suffix(" MB"),
                        )
                        .changed();
                    ui.label("oppure ultima modifica da almeno");
                    let b = ui
                        .add(
                            egui::DragValue::new(&mut self.days)
                                .range(1..=36_500)
                                .suffix(" giorni"),
                        )
                        .changed();
                    if a || b {
                        self.clear();
                    }
                });
            }
            Tool::Downloads => {
                ui.label("Tipi di file riconosciuti dall’estensione, ordinati per dimensione.");
            }
            Tool::Similar => {
                ui.label("JPEG e PNG locali · fino a 2.000 file in ordine di percorso · 32 MiB, 16 megapixel e 8.192 px per lato. Confronto di forme e colori dopo la correzione dell’orientamento. Ogni gruppo conserva l’immagine di riferimento con più pixel.");
            }
        }
        let job_result = self.job.as_ref().map(|job| job.rx.try_recv());
        match job_result {
            Some(Ok(value)) => {
                self.job.take();
                self.results = Some(value);
                self.refilter();
            }
            Some(Err(mpsc::TryRecvError::Disconnected)) => {
                self.clear();
                self.message = "Analisi interrotta.".into();
            }
            _ => {}
        }
        ui.horizontal_wrapped(|ui| {
            if let Some(job) = &self.job {
                ui.spinner();
                ui.label(format!(
                    "{} voci visitate",
                    job.progress.visited.load(Ordering::Relaxed)
                ));
                ctx.request_repaint_after(Duration::from_millis(140));
                if ui.button("Interrompi").clicked() {
                    self.clear();
                    self.message = "Analisi interrotta.".into();
                }
            } else if ui.button("Analizza cartella").clicked() {
                self.start(ctx);
            }
        });
        if !self.message.is_empty() {
            ui.label(&self.message);
        }
        self.results.as_ref()?;
        if self.tool != Tool::Similar {
            let before = self.filter;
            egui::ComboBox::from_id_salt("clutter_kind")
                .selected_text(self.filter)
                .show_ui(ui, |ui| {
                    for kind in [
                        "Tutti",
                        "Installatori",
                        "Archivi",
                        "Immagini",
                        "Video",
                        "Audio",
                        "Documenti",
                        "Altri file",
                    ] {
                        ui.selectable_value(&mut self.filter, kind, kind);
                    }
                });
            if self.filter != before {
                self.refilter();
            }
        }
        let r = self.results.as_ref()?;
        ui.label(&r.status);
        let mut review = false;
        ui.horizontal_wrapped(|ui| {
            ui.label(format!(
                "{} risultati visibili · {} selezionati in tutti i filtri",
                self.visible.len(),
                self.selected.len()
            ));
            review = ui
                .add_enabled(
                    !self.selected.is_empty(),
                    egui::Button::new("Rivedi selezione"),
                )
                .clicked();
        });
        let height = if self.tool == Tool::Similar {
            54.0
        } else {
            27.0
        };
        let mut rendered = HashSet::new();
        egui::ScrollArea::vertical()
            .id_salt("clutter_results")
            .show_rows(ui, height, self.visible.len(), |ui, range| {
                for i in range {
                    let index = self.visible[i];
                    let row = &r.rows[index];
                    ui.push_id(index, |ui| {
                        ui.horizontal(|ui| {
                            ui.set_min_height(height);
                            if row.keeper {
                                ui.label("Conservata");
                            } else {
                                let mut checked = self.selected.contains(&index);
                                if ui.checkbox(&mut checked, "").changed() {
                                    if checked {
                                        self.selected.insert(index);
                                    } else {
                                        self.selected.remove(&index);
                                    }
                                }
                            }
                            if let Some(v) = &row.visual {
                                rendered.insert(index);
                                let texture = self.textures.entry(index).or_insert_with(|| {
                                    ctx.load_texture(
                                        format!("clutter-{index}"),
                                        egui::ColorImage::from_rgba_unmultiplied(
                                            [48, 48],
                                            &v.preview,
                                        ),
                                        Default::default(),
                                    )
                                });
                                ui.image((texture.id(), egui::vec2(48.0, 48.0)));
                                ui.label(format!(
                                    "Gruppo {} · {}×{}",
                                    row.item.group.unwrap_or(0),
                                    v.width,
                                    v.height
                                ));
                            }
                            ui.label(format!("{:.2} MB", row.item.bytes as f64 / 1e6));
                            if self.tool != Tool::Similar {
                                ui.label(
                                    row.age
                                        .map(|v| format!("{v} g"))
                                        .unwrap_or_else(|| "Data incerta".into()),
                                );
                            }
                            ui.add(
                                egui::Label::new(
                                    row.item
                                        .path
                                        .file_name()
                                        .unwrap_or_default()
                                        .to_string_lossy(),
                                )
                                .truncate(),
                            )
                            .on_hover_text(row.item.path.display().to_string());
                        })
                    });
                }
            });
        self.textures.retain(|i, _| rendered.contains(i));
        if review {
            self.proposal()
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};
    use std::{fs, os::unix::fs::symlink, process::Command};
    #[test]
    fn storage_tools_on_isolated_fixture() {
        let tmp = tempfile::Builder::new()
            .prefix("lindo-clutter-test-")
            .tempdir()
            .unwrap();
        let home = tmp.path().canonicalize().unwrap();
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "clutter::tests::storage_worker",
                "--ignored",
                "--nocapture",
            ])
            .env("LINDO_FIXTURE_WORKER", home)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    #[test]
    #[ignore = "invoked in an isolated process with a synthetic home"]
    fn storage_worker() {
        let root =
            PathBuf::from(std::env::var_os("LINDO_FIXTURE_WORKER").unwrap()).join("Downloads");
        fs::create_dir(&root).unwrap();
        let path = |name| root.join(name);
        fs::File::create(path("old.ZIP"))
            .unwrap()
            .set_modified(SystemTime::now() - Duration::from_secs(200 * 86_400))
            .unwrap();
        fs::File::create(path("large.dmg"))
            .unwrap()
            .set_len(2_000_000)
            .unwrap();
        fs::write(path("recent.pdf"), b"new").unwrap();
        fs::write(path("broken.png"), b"invalid").unwrap();
        for (name, size, shift) in [("original.png", 96, 0), ("smaller.jpg", 70, 1)] {
            RgbImage::from_fn(size, size, |x, y| {
                Rgb([(200 * x / size) as u8 + shift, (180 * y / size) as u8, 80])
            })
            .save(path(name))
            .unwrap();
        }
        symlink(path("original.png"), path("link.png")).unwrap();
        let req = Request {
            roots: vec![root.clone()],
            mode: Mode::Large,
            min_bytes: 0,
        };
        let run = |tool| {
            analyze(
                req.clone(),
                tool,
                1,
                180,
                &AtomicBool::new(false),
                &Progress::default(),
            )
        };
        let big = run(Tool::LargeOld);
        assert_eq!(big.rows.len(), 2);
        assert_eq!(big.rows[0].kind, "Installatori");
        assert_eq!(big.rows[1].kind, "Archivi");
        assert!(big.rows[1].age.unwrap() >= 200);
        assert_eq!(run(Tool::Downloads).rows.len(), 6);
        let similar = run(Tool::Similar);
        assert_eq!(similar.rows.len(), 2, "{}", similar.status);
        assert_eq!(similar.rows[0].item.path, path("original.png"));
        assert_eq!(similar.rows[1].item.path, path("smaller.jpg"));
        assert!(similar.rows[0].keeper);
        assert!(!similar.rows[1].keeper);
        let mut ui = Clutter {
            tool: Tool::Similar,
            results: Some(similar),
            ..Default::default()
        };
        assert!(ui.proposal().is_none());
        ui.selected.extend([0, 1]);
        let proposal = ui.proposal().unwrap();
        assert_eq!(proposal.items.len(), 1);
        assert_eq!(proposal.retained.len(), 1);
        assert_eq!(proposal.retained[0].path, path("original.png"));
        assert_eq!(proposal.items[0].group, proposal.retained[0].group);
        ui.clear();
        assert!(ui.proposal().is_none());
        assert!(analyze(
            req,
            Tool::Similar,
            1,
            180,
            &AtomicBool::new(true),
            &Progress::default()
        )
        .rows
        .is_empty());
    }
}
