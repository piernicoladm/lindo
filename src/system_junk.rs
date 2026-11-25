#[path = "system_junk_move.rs"]
mod removal;
#[path = "system_junk_scope.rs"]
mod scope;
use eframe::egui;
use scope::{Candidate, Inventory, Roots};
use std::collections::BTreeSet;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc,
};
use std::time::Duration;

enum Outcome {
    Scanned(Inventory),
    Moved(Vec<String>),
}
struct Job {
    cancel: Arc<AtomicBool>,
    rx: mpsc::Receiver<Result<Outcome, String>>,
}
impl Drop for Job {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}
#[derive(Default)]
pub(crate) struct SystemJunk {
    inventory: Inventory,
    chosen: BTreeSet<usize>,
    review: Option<Vec<Candidate>>,
    job: Option<Job>,
    status: String,
    receipt: Vec<String>,
}
impl SystemJunk {
    fn start(
        &mut self,
        ctx: &egui::Context,
        task: impl FnOnce(&AtomicBool) -> Result<Outcome, String> + Send + 'static,
    ) {
        let cancel = Arc::new(AtomicBool::new(false));
        let stop = cancel.clone();
        let (tx, rx) = mpsc::sync_channel(1);
        let repaint = ctx.clone();
        match std::thread::Builder::new()
            .name("lindo-system-junk".into())
            .spawn(move || {
                let _ = tx.send(task(&stop));
                repaint.request_repaint();
            }) {
            Ok(_) => self.job = Some(Job { cancel, rx }),
            Err(e) => self.status = e.to_string(),
        }
    }
    pub(crate) fn show(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.heading("Cache e log condivisi delle app");
        ui.label("Esamina /Library/Caches e /Library/Logs. Sono selezionabili solo corpi di cache HTTP riconosciuti e log ruotati da almeno sette giorni, attribuiti a un’app terza installata. Cartelle Apple, database e file senza attribuzione restano esclusi.");
        if let Some(job) = &self.job {
            match job.rx.try_recv() {
                Ok(result) => {
                    self.job = None;
                    match result {
                        Ok(Outcome::Scanned(r)) => {
                            self.status = format!(
                                "{} file selezionabili, {} voci escluse o illeggibili. {}",
                                r.items.len(),
                                r.skipped,
                                if r.partial {
                                    "Inventario parziale."
                                } else {
                                    ""
                                }
                            );
                            self.inventory = r;
                        }
                        Ok(Outcome::Moved(receipts)) => {
                            self.receipt = receipts;
                            self.status = "Esiti della selezione".into();
                            self.inventory = Inventory::default();
                        }
                        Err(e) => self.status = e,
                    }
                }
                Err(mpsc::TryRecvError::Empty) => {
                    ui.spinner();
                    if ui.button("Interrompi").clicked() {
                        job.cancel.store(true, Ordering::Release);
                    }
                    ctx.request_repaint_after(Duration::from_millis(200));
                    return;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.job = None;
                    self.status = "Operazione interrotta.".into();
                }
            }
        }
        if let Some(list) = &self.review {
            ui.strong(format!("Conferma {} file esatti", list.len()));
            ui.label("Lindo verifica nuovamente attribuzione, identità, app in esecuzione e file aperti visibili. Per i file di proprietà del sistema, Finder può richiedere autorizzazione. Puoi rifiutarla.");
            if ui.button("Annulla").clicked() {
                self.review = None;
                return;
            }
            if ui.button("Conferma spostamento nel Cestino").clicked() {
                let list = self.review.take().unwrap();
                self.chosen.clear();
                self.start(ctx, move |stop| {
                    let roots = Roots::default();
                    let mut receipts = Vec::new();
                    for item in list {
                        if stop.load(Ordering::Acquire) {
                            receipts.push(format!("Non avviato: {}", item.path.display()));
                            continue;
                        }
                        match removal::move_one(&roots, &item, stop) {
                            Ok(_) => receipts.push(format!("Nel Cestino: {}", item.path.display())),
                            Err(e) => receipts.push(format!("{} · {e}", item.path.display())),
                        }
                    }
                    Ok(Outcome::Moved(receipts))
                });
            }
            if let Some(list) = &self.review {
                egui::ScrollArea::vertical()
                    .id_salt("system_junk_review")
                    .max_height(230.)
                    .show(ui, |ui| {
                        for item in list {
                            ui.label(item.path.display().to_string());
                        }
                    });
            }
            return;
        }
        if ui.button("Analizza cache e log condivisi").clicked() {
            self.chosen.clear();
            self.receipt.clear();
            self.inventory = Inventory::default();
            self.start(ctx, |stop| {
                scope::scan(&Roots::default(), stop).map(Outcome::Scanned)
            });
        }
        ui.label(&self.status);
        if !self.receipt.is_empty() {
            egui::ScrollArea::vertical()
                .id_salt("system_junk_receipts")
                .show(ui, |ui| {
                    for line in &self.receipt {
                        ui.label(line);
                    }
                });
        }
        if !self.inventory.items.is_empty() {
            if ui
                .add_enabled(
                    !self.chosen.is_empty(),
                    egui::Button::new(format!("Rivedi {} file", self.chosen.len())),
                )
                .clicked()
            {
                self.review = Some(
                    self.chosen
                        .iter()
                        .map(|i| self.inventory.items[*i].clone())
                        .collect(),
                );
            }
            egui::ScrollArea::vertical()
                .id_salt("system_junk_candidates")
                .max_height(230.)
                .show_rows(ui, 22., self.inventory.items.len(), |ui, range| {
                    for i in range {
                        let item = &self.inventory.items[i];
                        let mut marked = self.chosen.contains(&i);
                        let channel = if item.uid == 0 { "Finder" } else { "Cestino" };
                        if ui
                            .add_enabled(
                                marked || self.chosen.len() < 64,
                                egui::Checkbox::new(
                                    &mut marked,
                                    format!(
                                        "{} · {} B · {}",
                                        item.path.display(),
                                        item.bytes,
                                        channel
                                    ),
                                ),
                            )
                            .changed()
                        {
                            if marked {
                                self.chosen.insert(i);
                            } else {
                                self.chosen.remove(&i);
                            }
                        }
                    }
                });
        }
    }
}
