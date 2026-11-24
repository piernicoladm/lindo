use super::design::{self, CREAM, INK, LIME, LINE, MUTED};
use super::*;
use std::sync::Weak;

const REVIEW: [Mode; 4] = [Mode::Large, Mode::Duplicates, Mode::Cache, Mode::Logs];
const QUICK: [Mode; 3] = [Mode::Space, Mode::Cache, Mode::Logs];

#[derive(Default)]
pub(super) struct HomePanel {
    sources: Vec<Option<Weak<Results>>>,
    totals: Option<[u64; 4]>,
    pending: Option<mpsc::Receiver<[u64; 4]>>,
    scopes: bool,
    quick_scope: bool,
    inspect: Option<bool>,
    batch: Vec<Mode>,
    batch_start: u64,
}

// Reported bytes are logical sizes. Count a cache tree once and reserve one
// uncovered member per duplicate group before counting independent large files.
fn review_totals(reports: &[Option<Arc<Results>>]) -> [u64; 4] {
    let mut totals = [0u64; 4];
    let mut seen = HashSet::new();
    let mut trees = HashSet::new();
    let mut paths = HashSet::new();
    if let Some(cache) = &reports[2] {
        let mut entries: Vec<_> = cache.report.items.iter().collect();
        entries.sort_unstable_by(|a, b| a.path.cmp(&b.path));
        for item in entries {
            if !item.path.ancestors().any(|path| trees.contains(path))
                && seen.insert(item.identity.node())
            {
                trees.insert(item.path.as_path());
                totals[2] = totals[2].saturating_add(item.bytes);
            }
        }
    }
    if let Some(duplicates) = &reports[1] {
        for ids in duplicates.groups.values() {
            let mut keeper = false;
            for &id in ids {
                let item = &duplicates.report.items[id];
                if !paths.insert(item.path.as_path())
                    || !seen.insert(item.identity.node())
                    || item.path.ancestors().any(|p| trees.contains(p))
                {
                    continue;
                }
                if keeper {
                    totals[1] = totals[1].saturating_add(item.bytes);
                }
                keeper = true;
            }
        }
    }
    for index in [3, 0] {
        if let Some(result) = &reports[index] {
            for item in &result.report.items {
                if !item.path.ancestors().any(|p| trees.contains(p))
                    && paths.insert(item.path.as_path())
                    && seen.insert(item.identity.node())
                {
                    totals[index] = totals[index].saturating_add(item.bytes);
                }
            }
        }
    }
    totals
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Coverage {
    Unmeasured,
    Partial,
    Complete,
}

fn coverage<'a>(results: impl Iterator<Item = Option<&'a Arc<Results>>>) -> Coverage {
    let mut measured = false;
    let mut complete = true;
    for result in results {
        if let Some(result) = result {
            measured = true;
            let report = &result.report;
            complete &= report.errors == 0
                && !report.cancelled
                && !(report.skipped > 0 && report.visited == 0 && report.items.is_empty());
        } else {
            complete = false;
        }
    }
    if !measured {
        Coverage::Unmeasured
    } else if complete {
        Coverage::Complete
    } else {
        Coverage::Partial
    }
}

fn category_amount(coverage: Coverage, amount: Option<u64>) -> String {
    match (coverage, amount) {
        (Coverage::Unmeasured, _) => "Da analizzare".into(),
        (_, None) => "Calcolo…".into(),
        (Coverage::Partial, Some(0)) => "Da completare".into(),
        (Coverage::Partial, Some(value)) => format!("{} · parziale", size(value)),
        (Coverage::Complete, Some(value)) => size(value),
    }
}

impl HomePanel {
    fn refresh(&mut self, reports: Vec<Option<Arc<Results>>>, ctx: &egui::Context) {
        if let Some(rx) = &self.pending {
            match rx.try_recv() {
                Ok(totals) => self.totals = Some(totals),
                Err(mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(Duration::from_millis(150));
                    return;
                }
                Err(mpsc::TryRecvError::Disconnected) => self.sources.clear(),
            }
            self.pending = None;
        }
        if self.sources.len() == reports.len()
            && self
                .sources
                .iter()
                .zip(&reports)
                .all(|(a, b)| match (a, b) {
                    (Some(a), Some(b)) => a.ptr_eq(&Arc::downgrade(b)),
                    (None, None) => true,
                    _ => false,
                })
        {
            return;
        }
        self.sources = reports
            .iter()
            .map(|r| r.as_ref().map(Arc::downgrade))
            .collect();
        self.totals = None;
        if reports.iter().any(Option::is_some) {
            let (tx, rx) = mpsc::channel();
            self.pending = Some(rx);
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                let totals = review_totals(&reports);
                drop(reports);
                let _ = tx.send(totals);
                ctx.request_repaint();
            });
        }
    }
}

fn text(value: impl Into<String>, size: f32) -> egui::RichText {
    egui::RichText::new(value).size(size).color(CREAM)
}

fn heading(ui: &mut egui::Ui, value: &str) {
    ui.label(design::display(value, 32.));
}

fn icon(ui: &egui::Ui, rect: egui::Rect, mode: Mode) {
    let p = ui.painter();
    let center = rect.center();
    let pt = |x, y| center + egui::vec2(x, y);
    let stroke = egui::Stroke::new(1.8_f32, CREAM);
    match mode {
        Mode::Duplicates => {
            p.rect_stroke(
                egui::Rect::from_min_max(pt(-8., -8.), pt(4., 4.)),
                2.,
                stroke,
                egui::StrokeKind::Inside,
            );
            p.rect(
                egui::Rect::from_min_max(pt(-3., -3.), pt(9., 9.)),
                2.,
                design::CANVAS,
                stroke,
                egui::StrokeKind::Inside,
            );
        }
        Mode::Cache | Mode::Logs => {
            p.add(egui::Shape::closed_line(
                vec![pt(-10., -3.), pt(0., -8.), pt(10., -3.), pt(0., 2.)],
                stroke,
            ));
            for y in [2., 7.] {
                p.add(egui::Shape::line(
                    vec![pt(-10., y), pt(0., y + 5.), pt(10., y)],
                    stroke,
                ));
            }
        }
        _ => {
            p.add(egui::Shape::closed_line(
                vec![
                    pt(-7., -9.),
                    pt(2., -9.),
                    pt(8., -3.),
                    pt(8., 10.),
                    pt(-7., 10.),
                ],
                stroke,
            ));
            p.add(egui::Shape::line(
                vec![pt(2., -9.), pt(2., -3.), pt(8., -3.)],
                stroke,
            ));
        }
    }
}

fn row(
    ui: &mut egui::Ui,
    mode: Mode,
    name: &str,
    detail: &str,
    interactive: bool,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), 48.),
        if interactive {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        },
    );
    let p = ui.painter();
    if interactive && response.hovered() {
        p.rect_filled(rect, 4., design::RAIL);
    }
    icon(
        ui,
        egui::Rect::from_center_size(
            egui::pos2(rect.left() + 20., rect.center().y),
            egui::vec2(22., 22.),
        ),
        mode,
    );
    let available = (rect.width() - if interactive { 195. } else { 205. }).max(40.);
    let label = egui::WidgetText::from(design::display(name, 18.)).into_galley(
        ui,
        Some(egui::TextWrapMode::Truncate),
        available,
        egui::TextStyle::Body,
    );
    p.galley(
        egui::pos2(rect.left() + 48., rect.center().y - label.size().y * 0.5),
        label,
        CREAM,
    );
    if !interactive {
        let center = egui::pos2(rect.right() - 130., rect.center().y);
        if detail == "Completata" {
            p.circle_filled(center, 9., LIME);
            p.add(egui::Shape::line(
                vec![
                    center + egui::vec2(-4., 0.),
                    center + egui::vec2(-1., 3.),
                    center + egui::vec2(5., -4.),
                ],
                egui::Stroke::new(1.7_f32, INK),
            ));
        } else {
            p.circle_stroke(
                center,
                8.,
                egui::Stroke::new(1.5_f32, if detail == "In corso" { LIME } else { MUTED }),
            );
            if detail == "In corso" {
                let angle = ui.input(|i| i.time) as f32 * 3.;
                p.circle_filled(
                    center + egui::vec2(angle.cos(), angle.sin()) * 8.,
                    2.5,
                    LIME,
                );
            }
        }
    }
    p.text(
        egui::pos2(
            rect.right() - if interactive { 32. } else { 8. },
            rect.center().y,
        ),
        egui::Align2::RIGHT_CENTER,
        detail,
        egui::FontId::proportional(16.),
        if detail == "In corso" { LIME } else { CREAM },
    );
    if interactive {
        let mid = egui::pos2(rect.right() - 12., rect.center().y);
        p.add(egui::Shape::line(
            vec![
                mid + egui::vec2(-3., -5.),
                mid + egui::vec2(2., 0.),
                mid + egui::vec2(-3., 5.),
            ],
            egui::Stroke::new(1.5_f32, MUTED),
        ));
    }
    p.line_segment(
        [rect.left_bottom(), rect.right_bottom()],
        egui::Stroke::new(1.0_f32, LINE),
    );
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            if interactive {
                egui::WidgetType::Button
            } else {
                egui::WidgetType::Label
            },
            true,
            format!("{name} {detail}"),
        )
    });
    response
}

fn hero(
    ui: &mut egui::Ui,
    art: &egui::TextureHandle,
    content: impl FnOnce(&mut egui::Ui),
) -> egui::Rect {
    let width = ui.available_width();
    let art_width = if width < 610. {
        width * 0.33
    } else {
        (width * 0.48).min(460.)
    };
    let art_height = (art_width / art.aspect_ratio()).min(265.);
    let origin = ui.next_widget_position();
    let mut rect = egui::Rect::from_min_size(origin, egui::vec2(width, art_height));
    let mut left = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(egui::Rect::from_min_max(
                rect.min,
                egui::pos2(rect.right() - art_width - 8., rect.bottom()),
            ))
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    content(&mut left);
    rect.max.y = rect.bottom().max(left.min_rect().bottom());
    ui.advance_cursor_after_rect(rect);
    let artwork = egui::Rect::from_center_size(
        egui::pos2(rect.right() - art_width * 0.5, rect.center().y),
        egui::vec2(art_height * art.aspect_ratio(), art_height),
    );
    egui::Image::new(art)
        .maintain_aspect_ratio(true)
        .paint_at(ui, artwork);
    artwork
}

fn stop_scan_button(ui: &mut egui::Ui) -> bool {
    ui.add(
        egui::Button::new(text("Interrompi", 16.))
            .min_size(egui::vec2(138., 36.))
            .stroke(egui::Stroke::new(1.0_f32, MUTED))
            .corner_radius(7.),
    )
    .clicked()
}

fn number(value: u64) -> String {
    let digits = value.to_string();
    let mut result = String::new();
    for (index, byte) in digits.bytes().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            result.push('.');
        }
        result.push(byte as char);
    }
    result
}

impl Lindo {
    fn home_requests(&self, modes: &[Mode]) -> Vec<Request> {
        modes
            .iter()
            .map(|m| {
                let mut req = self.pages[m].req.clone();
                if req.roots.is_empty() && matches!(m, Mode::Duplicates | Mode::Large) {
                    req.roots.extend(home().map(|p| p.join("Downloads")));
                }
                req
            })
            .collect()
    }

    fn queue_home(&mut self, requests: Vec<Request>, ctx: &egui::Context) {
        self.home_panel.batch = requests.iter().map(|r| r.mode).collect();
        self.home_panel.batch_start = self.next_id;
        self.quick.extend(requests);
        self.home_panel.scopes = false;
        ctx.request_repaint();
    }

    pub(super) fn overview(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let reports: Vec<_> = REVIEW
            .iter()
            .map(|m| self.pages[m].result.clone())
            .collect();
        let coverage_all = coverage(reports.iter().map(Option::as_ref));
        let measured = coverage_all != Coverage::Unmeasured;
        self.home_panel.refresh(reports, ctx);
        let amounts = self.home_panel.totals;
        let total = amounts.map(|a| a.into_iter().fold(0u64, u64::saturating_add));
        let review = total.is_some_and(|t| t > 0);
        let art = self.artwork.home.clone();
        hero(ui, &art, |ui| {
            heading(ui, "Panoramica");
            ui.add_space(7.);
            let value = match (coverage_all, total) {
                (Coverage::Partial, Some(0)) => "Da completare".into(),
                (Coverage::Unmeasured, _) => "Da analizzare".into(),
                (_, Some(bytes)) => size(bytes),
                (_, None) => "Calcolo…".into(),
            };
            ui.label(design::display(
                value,
                if review || (total.is_some() && coverage_all == Coverage::Complete) {
                    64.
                } else {
                    36.
                },
            ));
            let subtitle = match coverage_all {
                Coverage::Unmeasured => "Spazio sul tuo Mac",
                Coverage::Partial if review => "da rivedere · stima parziale",
                Coverage::Partial => "Misurazione parziale",
                Coverage::Complete => "da rivedere",
            };
            ui.label(text(subtitle, 23.));
            ui.add_space(10.);
            let label = if review {
                format!("Rivedi {}", size(total.unwrap()))
            } else {
                "Analizza il Mac".to_owned()
            };
            if design::primary(ui, &label, self.available()).clicked() {
                if review {
                    self.home_panel.inspect = Some(false);
                } else {
                    self.home_panel.scopes = true;
                }
            }
        });
        ui.add_space(10.);
        if let Some((total, free)) = self.disk {
            let used = total.saturating_sub(free);
            ui.horizontal_wrapped(|ui| {
                ui.label(text(self.volume_label.as_str(), 16.).strong());
                ui.separator();
                ui.label(format!("{} totali", size(total)));
                ui.separator();
                ui.label(format!("{} occupati", size(used)));
                ui.separator();
                ui.label(format!("{} disponibili", size(free)));
            });
            ui.add_space(4.);
            let (bar, _) =
                ui.allocate_exact_size(egui::vec2(ui.available_width(), 10.), egui::Sense::hover());
            ui.painter().rect_filled(bar, 5., MUTED);
            let mut used_bar = bar;
            used_bar.set_width(bar.width() * (used as f32 / total.max(1) as f32).clamp(0., 1.));
            ui.painter()
                .rect_filled(used_bar, 5., egui::Color32::from_rgb(112, 163, 74));
        } else {
            ui.label(if self.disk_pending {
                "Misurazione del volume…"
            } else {
                "Capacità del volume non disponibile."
            });
        }
        ui.add_space(18.);
        ui.separator();
        for (mode, name, indices) in [
            (Mode::Large, "File grandi", &[0][..]),
            (Mode::Duplicates, "Duplicati", &[1][..]),
            (Mode::Cache, "Cache e log", &[2, 3][..]),
        ] {
            let coverage = coverage(
                indices
                    .iter()
                    .map(|&i| self.pages[&REVIEW[i]].result.as_ref()),
            );
            let amount = amounts.map(|a| indices.iter().fold(0u64, |s, &i| s.saturating_add(a[i])));
            let detail = category_amount(coverage, amount);
            if row(ui, mode, name, &detail, true).clicked() {
                if mode == Mode::Cache {
                    self.home_panel.inspect = Some(true);
                } else {
                    self.mode = Some(mode);
                }
            }
        }
        if REVIEW.iter().any(|m| {
            self.pages[m]
                .result
                .as_ref()
                .is_some_and(|r| r.report.errors > 0)
        }) {
            ui.small("Analisi parziale. Apri le categorie per consultare gli errori di accesso.");
        }
        ui.add_space(16.);
        if measured && ui.button("Analizza di nuovo").clicked() {
            self.home_panel.scopes = true;
        }
        let full = self.home_requests(&MODES);
        let busy = !self.available() || !self.quick.is_empty();
        self.assessment.set_core_busy(busy);
        if self.assessment.show(ui, ctx, &full) {
            self.queue_home(full, ctx);
        }
        if self.assessment.take_cancel_request() {
            self.cancel();
        }

        if self.home_panel.scopes {
            let mut open = true;
            egui::Window::new("Ambito della scansione")
                .collapsible(false)
                .resizable(false)
                .open(&mut open)
                .show(ctx, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.selectable_value(
                            &mut self.home_panel.quick_scope,
                            false,
                            "File da rivedere",
                        );
                        ui.selectable_value(
                            &mut self.home_panel.quick_scope,
                            true,
                            "Spazio, cache e log",
                        );
                    });
                    let reqs = self.home_requests(if self.home_panel.quick_scope {
                        &QUICK
                    } else {
                        &REVIEW
                    });
                    for req in &reqs {
                        ui.add_space(5.);
                        ui.strong(title(req.mode));
                        for root in &req.roots {
                            ui.label(root.display().to_string());
                        }
                        if req.mode == Mode::Large {
                            ui.small(format!("Soglia: {}", size(req.min_bytes)));
                        }
                    }
                    ui.add_space(10.);
                    ui.label("I risultati saranno disponibili per la revisione.");
                    if design::primary(
                        ui,
                        "Avvia scansione",
                        !busy && reqs.iter().all(|r| !r.roots.is_empty()),
                    )
                    .clicked()
                    {
                        self.queue_home(reqs, ctx);
                    }
                });
            self.home_panel.scopes &= open;
        }
        if let Some(cache_only) = self.home_panel.inspect {
            let mut open = true;
            egui::Window::new(if cache_only {"Cache e log"} else {"Elementi da rivedere"}).collapsible(false).resizable(false).open(&mut open).show(ctx,|ui|{
                ui.set_min_width(300.);
                for (i,&m) in REVIEW.iter().enumerate().filter(|(i,_)| !cache_only || *i>=2) {
                    let report = self.pages[&m].result.as_ref();
                    let detail = category_amount(coverage(std::iter::once(report)),amounts.map(|a|a[i]));
                    if row(ui,m,title(m),&detail,true).clicked() {self.mode=Some(m); self.home_panel.inspect=None;}
                }
                ui.small("I totali contano una sola volta i file presenti in più categorie e riservano una copia per ogni gruppo di duplicati.");
            });
            if !open {
                self.home_panel.inspect = None;
            }
        }
    }

    pub(super) fn scanning(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let Some(active) = &self.active else {
            return;
        };
        let mode = active.mode;
        let batch = &self.home_panel.batch;
        let position = batch.len().checked_sub(self.quick.len() + 1);
        let is_batch = position.is_some_and(|i| {
            batch.get(i) == Some(&mode) && active.id == self.home_panel.batch_start + i as u64
        });
        let previous = if is_batch {
            &batch[..position.unwrap()]
        } else {
            &[]
        };
        let visited = previous
            .iter()
            .filter_map(|m| self.pages[m].result.as_ref())
            .fold(
                active
                    .progress
                    .visited
                    .load(Ordering::Relaxed)
                    .saturating_add(active.progress.tree_entries.load(Ordering::Relaxed)),
                |n, r| n.saturating_add(r.report.visited),
            );
        let compact = ui.available_width() < 610. || ctx.screen_rect().height() < 560.;
        let mut cancel = false;
        let art = self.artwork.scan.clone();
        let art_rect = hero(ui, &art, |ui| {
            heading(ui, "Scansione in corso");
            ui.add_space(7.);
            ui.label(design::display(
                number(visited),
                if visited >= 1_000_000 { 50. } else { 64. },
            ));
            ui.label(text("elementi esaminati", 23.));
            if compact {
                ui.add_space(6.);
                cancel = stop_scan_button(ui);
            }
        });
        if cancel {
            self.cancel();
            return;
        }
        let left = art_rect.left() + 8.;
        let right = art_rect.right() - 8.;
        let y = art_rect.bottom() - 1.;
        let wave = ui.input(|i| (i.time * 1.8).sin() as f32) * 0.5 + 0.5;
        let light = egui::lerp(left..=right, wave);
        ui.painter().line_segment(
            [egui::pos2(left, y), egui::pos2(right, y)],
            egui::Stroke::new(1.0_f32, LINE),
        );
        ui.painter().line_segment(
            [
                egui::pos2((light - 14.).max(left), y),
                egui::pos2((light + 14.).min(right), y),
            ],
            egui::Stroke::new(2.0_f32, LIME),
        );
        let phase = active.progress.phase.load(Ordering::Relaxed);
        let scope = active
            .progress
            .current_directory
            .try_lock()
            .ok()
            .map(|p| p.clone())
            .filter(|p| !p.as_os_str().is_empty());
        let path = scope
            .as_ref()
            .or_else(|| self.pages[&mode].req.roots.first());
        egui::Frame::new()
            .fill(INK)
            .stroke(egui::Stroke::new(1.0_f32, LINE))
            .corner_radius(8.)
            .inner_margin(10.)
            .show(ui, |ui| {
                ui.small(if phase == HASH {
                    "Calcolo delle impronte"
                } else if phase == COMPARE {
                    "Confronto delle copie"
                } else {
                    "Cartella in esame"
                });
                if matches!(phase, HASH | COMPARE) {
                    let bytes = if phase == HASH {
                        &active.progress.hashed_bytes
                    } else {
                        &active.progress.compared_bytes
                    };
                    ui.strong(format!("{} letti", size(bytes.load(Ordering::Relaxed))));
                } else if let Some(path) = path {
                    let name = home()
                        .and_then(|h| path.strip_prefix(h).ok())
                        .filter(|p| !p.as_os_str().is_empty())
                        .unwrap_or(path)
                        .display()
                        .to_string();
                    ui.add(egui::Label::new(text(name, 18.).strong()).truncate())
                        .on_hover_text(path.display().to_string());
                }
            });
        ui.add_space(20.);
        ui.label(design::display("Stato delle analisi", 22.));
        let single = [mode];
        for &m in if is_batch { batch.as_slice() } else { &single } {
            let state = if m == mode {
                "In corso"
            } else if previous.contains(&m) {
                if self.pages[&m]
                    .result
                    .as_ref()
                    .is_some_and(|r| r.report.errors > 0)
                {
                    "Con errori"
                } else {
                    "Completata"
                }
            } else {
                "In attesa"
            };
            row(ui, m, title(m), state, false);
        }
        if !compact {
            ui.add_space(18.);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if stop_scan_button(ui) {
                    self.cancel();
                }
            });
        }
        ctx.request_repaint_after(Duration::from_millis(150));
    }

    pub(super) fn catalog(&mut self, ui: &mut egui::Ui, category: Option<&str>) {
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
                if category.is_some_and(|c| {
                    Some(c)
                        != mode
                            .map(mode_destination)
                            .or_else(|| extra.map(extra_destination))
                            .map(|d| d.label())
                }) || self.favorites.contains(name) != favorite
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Identity, Item};

    #[test]
    fn zero_bytes_require_complete_coverage() {
        let empty = Arc::new(Results::prepare(Mode::Large, Report::default()));
        let denied = Arc::new(Results::prepare(
            Mode::Large,
            Report {
                errors: 1,
                ..Default::default()
            },
        ));
        for (results, expected) in [
            ([None, None], "Da analizzare"),
            ([Some(&empty), None], "Da completare"),
            ([Some(&empty), Some(&denied)], "Da completare"),
            ([Some(&empty), Some(&empty)], "0 B"),
        ] {
            assert_eq!(
                category_amount(coverage(results.into_iter()), Some(0)),
                expected
            );
        }
    }
    #[test]
    fn totals_reserve_a_duplicate_and_count_each_tree_path_and_inode_once() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        std::fs::create_dir(root.join("cache")).unwrap();
        for name in ["cache/nested", "first", "second", "log"] {
            std::fs::write(root.join(name), b"fixture").unwrap();
        }
        std::fs::hard_link(root.join("log"), root.join("log-alias")).unwrap();
        let item = |name: &str, group| {
            let path = root.join(name);
            Item {
                identity: Identity::read(&path).unwrap(),
                path,
                bytes: 7,
                group,
                cloud: false,
            }
        };
        let report = |mode, items| {
            Some(Arc::new(Results::prepare(
                mode,
                Report {
                    items,
                    ..Default::default()
                },
            )))
        };
        let reports = [
            report(
                Mode::Large,
                vec![
                    item("cache/nested", None),
                    item("first", None),
                    item("second", None),
                    item("log-alias", None),
                ],
            ),
            report(
                Mode::Duplicates,
                vec![item("first", Some(1)), item("second", Some(1))],
            ),
            report(Mode::Cache, vec![item("cache", None)]),
            report(Mode::Logs, vec![item("log", None)]),
        ];
        assert_eq!(review_totals(&reports), [0, 7, 7, 7]);
    }
}
