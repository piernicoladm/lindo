use super::*;

#[path = "ui_preview.rs"]
mod preview;

impl Lindo {
    pub(super) fn section(&mut self, ui: &mut egui::Ui, mode: Mode, ctx: &egui::Context) {
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
        let compact_scopes = mode == Mode::Duplicates && self.pages[&mode].result.is_some();
        if !compact_scopes {
            ui.label(if mode == Mode::Duplicates {
                "Scegli quale copia conservare."
            } else {
                hint(mode)
            });
            ui.add_space(8.);
        }
        let available = self.available();
        let page = self.pages.get_mut(&mode).unwrap();
        let mut scan = false;
        let mut choose = false;
        let mut append = false;
        let mut scopes = |ui: &mut egui::Ui, page: &mut Page| {
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
                                            .selectable_label(
                                                page.req.roots.first() == Some(&p),
                                                label,
                                            )
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
                        if matches!(mode, Mode::Large | Mode::Duplicates)
                            && !page.req.roots.is_empty()
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
        };
        let mut options = |ui: &mut egui::Ui, page: &mut Page| {
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
                        page.selected_bytes = 0;
                    }
                    if mode != Mode::Duplicates
                        && ui
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
                    number(page.selected.len()),
                    number(page.selected.len().saturating_sub(page.selected_visible))
                ));
            }
        };
        if compact_scopes {
            let settings_id = ui.id().with("duplicate-options");
            let mut opened = ui
                .data(|d| d.get_temp::<bool>(settings_id))
                .unwrap_or(false);
            let find = ui.is_enabled()
                && ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::F));
            ui.horizontal_wrapped(|ui| {
                ui.label("Scegli quale copia conservare.");
                if ui.button("Opzioni").clicked() {
                    opened = !opened;
                }
                if !page.query.is_empty() {
                    ui.small(format!("Filtro: {}", page.query));
                }
            });
            if find {
                opened = true;
            }
            if opened && ui.is_enabled() {
                egui::Window::new("Opzioni duplicati")
                    .id(settings_id)
                    .open(&mut opened)
                    .collapsible(false)
                    .resizable(false)
                    .default_width(360.)
                    .max_height((ctx.screen_rect().height() - 65.).max(140.))
                    .vscroll(true)
                    .show(ctx, |ui| {
                        search(ui, page, mode, ctx);
                        if find {
                            ui.memory_mut(|m| m.request_focus(egui::Id::new(("ricerca", mode))));
                        }
                        options(ui, page);
                        ui.separator();
                        if let Some(results) = &page.result {
                            let r = &results.report;
                            let groups = results.groups.len();
                            ui.label(format!(
                                "{} {} · {} {}",
                                number(r.items.len()),
                                if r.items.len() == 1 {
                                    "elemento"
                                } else {
                                    "elementi"
                                },
                                number(groups),
                                if groups == 1 { "gruppo" } else { "gruppi" }
                            ));
                            ui.small(format!(
                                "{} visitati · {} esclusi",
                                number(r.visited),
                                number(r.skipped)
                            ));
                            report_notes(ui, r);
                        }
                        ui.separator();
                        scopes(ui, page);
                    });
            }
            ui.data_mut(|d| d.insert_temp(settings_id, opened));
        } else {
            scopes(ui, page);
        }
        if !compact_scopes {
            ui.separator();
        }
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
        if !compact_scopes {
            ui.horizontal_wrapped(|ui| {
                ui.strong(item_count(r.items.len()));
                ui.label(format!(
                    "{} visitati · {} esclusi",
                    number(r.visited),
                    number(r.skipped)
                ));
                if mode == Mode::Duplicates {
                    ui.label(if results.groups.len() == 1 {
                        "1 gruppo".to_owned()
                    } else {
                        format!("{} gruppi", results.groups.len())
                    });
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
        }
        if r.errors > 0 || r.cancelled {
            ui.colored_label(
                ui.visuals().warn_fg_color,
                format!("Copertura parziale · {} errori", r.errors),
            );
        }
        if !compact_scopes {
            report_notes(ui, r);
        }
        if mode == Mode::Duplicates {
            let panel = ui.available_rect_before_wrap();
            let footer_height = 47.;
            let mut body = panel;
            body.max.y = (panel.bottom() - footer_height - 5.).max(body.top());
            let reveal = ui
                .scope_builder(egui::UiBuilder::new().max_rect(body), |ui| {
                    ui.set_clip_rect(ui.clip_rect().intersect(body));
                    duplicate_group(ui, page, &results)
                })
                .inner;
            let hidden = page.selected.len().saturating_sub(page.selected_visible);
            let footer = egui::Rect::from_min_max(
                egui::pos2(panel.left(), panel.bottom() - footer_height),
                panel.right_bottom(),
            );
            ui.painter().hline(
                footer.x_range(),
                footer.top(),
                egui::Stroke::new(1.0_f32, design::LINE),
            );
            let actions = egui::Rect::from_min_max(
                egui::pos2(footer.left(), footer.top() + 6.),
                footer.right_bottom(),
            );
            let count = page.selected.len();
            let summary = if count == 1 {
                "1 copia selezionata".to_owned()
            } else {
                format!("{} copie selezionate", number(count))
            };
            let cta = format!("Rivedi {}", size(page.selected_bytes));
            let mut left = action_cell(ui, actions, false, "selected", 168.);
            left.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 1.;
                ui.add(egui::Label::new(design::display(summary, 17.)).truncate());
                if hidden > 0 {
                    ui.small(format!("{} nascoste dal filtro", number(hidden)));
                }
            });
            let mut right = action_cell(ui, actions, true, "review", 168.);
            if design::primary(&mut right, &cta, count > 0 && available).clicked() {
                self.review = Review::snapshot(page).map(|mut review| {
                    // Keep source paths adjacent to retained paths in every group.
                    let items = &review.data.report.items;
                    for group in review
                        .rows
                        .chunk_by_mut(|a, b| items[a.0].group == items[b.0].group)
                    {
                        let moving = group[1..].partition_point(|(_, keep)| !keep);
                        group[..=moving].rotate_left(1);
                    }
                    review
                });
                self.confirm = self.review.is_some();
            }
            if let Some(path) = reveal {
                self.reveal(path, ctx);
            }
            self.refresh_filter(mode, ctx);
            return;
        }
        options(ui, page);
        search(ui, page, mode, ctx);
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
                        compact_row(ui);
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

    pub(super) fn review_page(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let Some(review) = self.review.as_ref() else {
            return;
        };
        let direct = self.mode == Some(Mode::Duplicates) && review.req.mode == Mode::Duplicates;
        let hidden = if direct {
            let page = &self.pages[&Mode::Duplicates];
            page.selected.len().saturating_sub(page.selected_visible)
        } else {
            0
        };
        let compact = ctx.screen_rect().width() < 590.;
        let padding = if compact { 16 } else { 28 };
        let frame = egui::Frame::new()
            .fill(design::CANVAS)
            .stroke(egui::Stroke::new(1.0_f32, design::MUTED))
            .corner_radius(16.)
            .inner_margin(padding);
        let panel = ui.max_rect();
        let offset = panel.center() - ctx.screen_rect().center();
        let id = egui::Id::new("review-trash-dialog");
        let modal = egui::Modal::new(id)
            .area(egui::Modal::default_area(id).anchor(egui::Align2::CENTER_CENTER, offset))
            .frame(frame)
            .backdrop_color(egui::Color32::from_black_alpha(115));
        let mut back = false;
        let mut next = false;
        let mut reveal = None;
        let response = modal.show(ctx, |ui| {
            ui.set_width((panel.width() - f32::from(padding) * 2. - 12.).clamp(180., 525.));
            let heading = if self.confirm { "Spostare nel Cestino?" } else { "Rivedi la selezione" };
            ui.label(design::display(heading, if compact { 23. } else { 32. }));
            let summary = format!("{} · {}", item_count(review.moves.len()), size(review.total));
            ui.label(egui::RichText::new(summary).size(17.).color(design::CREAM));
            if !direct { ui.small(&review.label); }
            ui.add_space(10.);
            if hidden > 0 {
                let warning = format!("La selezione include {} nascosti dal filtro.", item_count(hidden));
                ui.colored_label(ui.visuals().warn_fg_color, warning);
            }
            if review.cloud {
                let warning = concat!(
                    "Alcuni percorsi appartengono a cartelle sincronizzate, come Dropbox o iCloud. ",
                    "Lo spostamento può rimuoverli anche dagli altri dispositivi e per chi condivide la cartella."
                );
                ui.colored_label(ui.visuals().warn_fg_color, warning);
            }
            let height = (ctx.screen_rect().height() * 0.42).clamp(72., 330.);
            egui::ScrollArea::vertical().id_salt("review-items").max_height(height)
                .show_rows(ui, 54., review.rows.len(), |ui, rows| {
                    for row in rows {
                        let (index, keep) = review.rows[row];
                        let item = &review.data.report.items[index];
                        ui.push_id(index, |ui| {
                            ui.set_min_height(54.);
                            compact_row(ui);
                            ui.horizontal(|ui| {
                                status_icon(ui, keep);
                                ui.vertical(|ui| {
                                    ui.horizontal(|ui| {
                                        let label = if keep { "Resta sul Mac" } else { "Nel Cestino" };
                                        ui.label(design::display(label, 16.));
                                        if let Some(group) = item.group {
                                            if review.keepers.len() > 1 { ui.small(format!("Gruppo {group}")); }
                                            if keep && review.keepers.get(&group) == Some(&index) {
                                                ui.small("Protetta");
                                            }
                                        }
                                    });
                                    if reveal_link(ui, item, &review.data, &review.req.roots).clicked() {
                                        reveal = Some(item.path.clone());
                                    }
                                });
                            });
                            ui.separator();
                        });
                    }
                });
            ui.add_space(8.);
            ui.label("Lo spazio si libera quando svuoti il Cestino.");
            ui.add_space(14.);
            let (actions,_) = ui.allocate_exact_size(egui::vec2(ui.available_width(),38.),egui::Sense::hover());
            let mut left=action_cell(ui,actions,false,"back",170.);
            back=left.add_sized([left.available_width().min(92.),36.],egui::Button::new("Indietro")).clicked();
            let mut right=action_cell(ui,actions,true,"confirm",170.);
            let action=if self.confirm {"Sposta nel Cestino"} else {"Continua"};
            next=design::primary(&mut right,action,self.available()).clicked();
        });
        if back || response.should_close() {
            if self.confirm && !direct {
                self.confirm = false;
            } else {
                self.confirm = false;
                self.review = None;
            }
        } else if next {
            if self.confirm {
                self.confirm = false;
                let snapshot = self.review.take().unwrap();
                self.trash(snapshot, ctx);
            } else {
                self.confirm = true;
            }
        }
        if let Some(path) = reveal {
            self.reveal(path, ctx);
        }
    }
}

fn status_icon(ui: &mut egui::Ui, keep: bool) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(28., 32.), egui::Sense::hover());
    let c = rect.center();
    let p = ui.painter();
    let stroke = egui::Stroke::new(1.6_f32, design::CREAM);
    let body = egui::Rect::from_center_size(c + egui::vec2(0., 3.), egui::vec2(17., 16.));
    p.rect_stroke(body, 3., stroke, egui::StrokeKind::Inside);
    if keep {
        let shackle = egui::Rect::from_center_size(c - egui::vec2(0., 6.), egui::vec2(12., 12.));
        p.rect_stroke(shackle, 5., stroke, egui::StrokeKind::Inside);
        p.rect_filled(body.shrink(2.), 1., design::CANVAS);
        p.circle_filled(body.center(), 2., design::LIME);
    } else {
        p.line_segment(
            [
                body.left_top() - egui::vec2(2., 1.),
                body.right_top() + egui::vec2(2., -1.),
            ],
            stroke,
        );
        p.line_segment([c - egui::vec2(4., 9.), c + egui::vec2(4., -9.)], stroke);
        for x in [-3., 3.] {
            p.line_segment([c + egui::vec2(x, 0.), c + egui::vec2(x, 6.)], stroke);
        }
    }
}

fn duplicate_group(ui: &mut egui::Ui, page: &mut Page, results: &Results) -> Option<PathBuf> {
    let count = page.count();
    if count == 0 {
        ui.label(if page.query.is_empty() {
            "Nessuna copia identica trovata."
        } else {
            "Nessun gruppo corrisponde alla ricerca."
        });
        return None;
    }
    // Cursor is a visible row offset. Both sort orders and filter results keep
    // copies contiguous and preserve their within-group order.
    let cursor_id = ui.id().with("duplicate-group-cursor");
    let cursor = ui
        .data(|d| d.get_temp::<usize>(cursor_id))
        .unwrap_or(0)
        .min(count - 1);
    let index = page.row_id(cursor);
    let group = results.report.items[index].group?;
    let ids = results.groups.get(&group)?;
    let first = cursor.saturating_sub(index - ids[0]);
    let after = first + ids.len();
    let mut next = cursor;
    if first > 0 || after < count {
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(first > 0, egui::Button::new("‹ Precedente"))
                .clicked()
            {
                next = first - 1;
            }
            if ui
                .add_enabled(after < count, egui::Button::new("Successivo ›"))
                .clicked()
            {
                next = after;
            }
            egui::ComboBox::from_id_salt("jump-to-copy")
                .selected_text("Vai a una copia…")
                .show_ui(ui, |ui| {
                    ui.set_min_width(210.);
                    egui::ScrollArea::vertical().max_height(200.).show_rows(
                        ui,
                        22.,
                        count,
                        |ui, range| {
                            for row in range {
                                let item = &results.report.items[page.row_id(row)];
                                let path = item.path.to_string_lossy();
                                if ui
                                    .add(
                                        egui::Button::new(path.as_ref())
                                            .selected(row == cursor)
                                            .truncate(),
                                    )
                                    .on_hover_text(path.as_ref())
                                    .clicked()
                                {
                                    next = row;
                                    ui.close_menu();
                                }
                            }
                        },
                    );
                });
        });
    }
    ui.data_mut(|d| d.insert_temp(cursor_id, next));
    if next != cursor {
        ui.ctx().request_repaint();
    }
    let mut reveal = None;
    let keeper = page.keeper(group)?;
    let representative = &results.report.items[keeper];
    let compact = ui.available_width() < 490.;
    let margin = if compact { 10 } else { 16 };
    ui.add_space(4.);
    egui::Frame::new()
        .fill(design::CANVAS)
        .stroke(egui::Stroke::new(1.0_f32, design::LINE))
        .corner_radius(14.)
        .inner_margin(margin)
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                preview::show(ui, representative, if compact { 83. } else { 144. });
                ui.add_space(4.);
                ui.vertical(|ui| {
                    let name = representative
                        .path
                        .file_name()
                        .unwrap_or(representative.path.as_os_str())
                        .to_string_lossy();
                    ui.add(
                        egui::Label::new(design::display(
                            name.as_ref(),
                            if compact { 19. } else { 24. },
                        ))
                        .truncate(),
                    )
                    .on_hover_text(name.as_ref());
                    ui.label(format!("{} copie identiche", number(ids.len())));
                });
            });
            ui.add_space(4.);
            ui.separator();
            ui.label(design::display("Conserva", 16.));
            let height = ui.available_height().max(20.);
            egui::ScrollArea::vertical()
                .id_salt(("copies", group))
                .max_height(height)
                .show_rows(
                    ui,
                    if compact { 70. } else { 52. },
                    ids.len(),
                    |ui, rows| {
                        for row in rows {
                            let i = ids[row];
                            let item = &results.report.items[i];
                            let protected = page.keeper(group) == Some(i);
                            let chosen = page.selected.contains(&i);
                            ui.push_id(i, |ui| {
                                ui.set_min_height(if compact { 70. } else { 52. });
                                compact_row(ui);
                                ui.horizontal(|ui| {
                                    let radio = ui.radio(protected, "").on_hover_text(format!(
                                        "Conserva questa copia\n{}",
                                        item.path.display()
                                    ));
                                    radio.widget_info(|| {
                                        egui::WidgetInfo::selected(
                                            egui::WidgetType::RadioButton,
                                            ui.is_enabled(),
                                            protected,
                                            format!("Conserva {}", item.path.display()),
                                        )
                                    });
                                    if radio.clicked() {
                                        page.keepers.insert(group, i);
                                        page.select(i, false);
                                    }
                                    let width = (ui.available_width()
                                        - if compact { 4. } else { 190. })
                                    .max(40.);
                                    ui.allocate_ui_with_layout(
                                        egui::vec2(width, 32.),
                                        egui::Layout::top_down(egui::Align::Min),
                                        |ui| {
                                            let name = item
                                                .path
                                                .file_name()
                                                .unwrap_or(item.path.as_os_str())
                                                .to_string_lossy();
                                            ui.add(
                                                egui::Label::new(design::display(
                                                    name.as_ref(),
                                                    15.,
                                                ))
                                                .truncate(),
                                            );
                                            if reveal_link(ui, item, results, &page.req.roots)
                                                .clicked()
                                            {
                                                reveal = Some(item.path.clone());
                                            }
                                        },
                                    );
                                    if !compact {
                                        copy_action(ui, page, i, protected, chosen, item);
                                    }
                                });
                                if compact {
                                    ui.horizontal(|ui| {
                                        ui.add_space(24.);
                                        copy_action(ui, page, i, protected, chosen, item);
                                    });
                                }
                                ui.separator();
                            });
                        }
                    },
                );
        });
    reveal
}

fn copy_action(
    ui: &mut egui::Ui,
    page: &mut Page,
    index: usize,
    protected: bool,
    chosen: bool,
    item: &crate::model::Item,
) {
    ui.label(size(item.bytes));
    if protected {
        ui.colored_label(design::LIME, "Resta sul Mac");
    } else {
        let mut checked = chosen;
        let response = ui
            .checkbox(&mut checked, "Da spostare")
            .on_hover_text(format!("Seleziona per il Cestino\n{}", item.path.display()));
        response.widget_info(|| {
            egui::WidgetInfo::selected(
                egui::WidgetType::Checkbox,
                ui.is_enabled(),
                checked,
                format!("Da spostare: {}", item.path.display()),
            )
        });
        if response.changed() {
            page.select(index, checked);
        }
    }
}

fn search(ui: &mut egui::Ui, page: &mut Page, mode: Mode, ctx: &egui::Context) {
    let search_id = egui::Id::new(("ricerca", mode));
    ui.horizontal_wrapped(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut page.query)
                .desired_width(if mode == Mode::Duplicates { 155. } else { 200. })
                .id(search_id)
                .hint_text("Cerca nel percorso…"),
        );
        if ui.is_enabled()
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::F))
        {
            ui.memory_mut(|m| m.request_focus(search_id));
        }
        if ui.is_enabled()
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
        {
            page.query.clear();
        }
        ui.small(if page.query != page.applied {
            "Ricerca…".to_owned()
        } else {
            format!("{} visibili", number(page.count()))
        });
    });
}

fn compact_row(ui: &mut egui::Ui) {
    ui.spacing_mut().item_spacing.y = 2.;
    ui.spacing_mut().button_padding.y = 1.;
    ui.spacing_mut().interact_size.y = 20.;
}

fn reveal_link(
    ui: &mut egui::Ui,
    item: &crate::model::Item,
    results: &Results,
    roots: &[PathBuf],
) -> egui::Response {
    let path = item.path.as_path();
    // Group members are sorted by path. The common parent of the first and
    // last entries is therefore also the common parent of every copy.
    let common = item
        .group
        .and_then(|g| results.groups.get(&g))
        .and_then(|ids| {
            let first = &results.report.items[*ids.first()?].path;
            let last = &results.report.items[*ids.last()?].path;
            first.parent()?.ancestors().find(|a| last.starts_with(a))
        });
    let scope = common.or_else(|| {
        roots
            .iter()
            .filter(|p| path.starts_with(p))
            .max_by_key(|p| p.components().count())
            .map(PathBuf::as_path)
    });
    let display = scope
        .map(|root| root.parent().unwrap_or(root))
        .and_then(|parent| path.strip_prefix(parent).ok())
        .map(|relative| relative.to_string_lossy().into_owned())
        .unwrap_or_else(|| {
            let parts: Vec<_> = path.components().rev().take(3).collect();
            let suffix: PathBuf = parts.iter().rev().map(|c| c.as_os_str()).collect();
            if suffix == path {
                suffix.display().to_string()
            } else {
                format!("…/{}", suffix.display())
            }
        });
    let width = ui.available_width().max(1.);
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 19.), egui::Sense::click());
    let paint = ui.painter_at(rect);
    let font = egui::FontId::proportional(14.);
    let render = |text: String| paint.layout_no_wrap(text, font.clone(), design::MUTED);
    let mut galley = render(display.clone());
    if galley.size().x > width {
        let chars: Vec<_> = display.chars().collect();
        let mut lo = 0;
        let mut hi = chars.len();
        while lo < hi {
            let mid = (lo + hi).div_ceil(2);
            let head = mid.div_ceil(2);
            let tail = mid - head;
            let candidate: String = chars[..head]
                .iter()
                .chain(std::iter::once(&'…'))
                .chain(chars[chars.len() - tail..].iter())
                .collect();
            let measured = render(candidate);
            if measured.size().x <= width {
                lo = mid;
                galley = measured;
            } else {
                hi = mid - 1;
            }
        }
        if lo == 0 {
            galley = render("…".into());
        }
    }
    paint.galley(
        rect.left_center() - egui::vec2(0., galley.size().y / 2.),
        galley,
        design::MUTED,
    );
    if response.has_focus() {
        paint.rect_stroke(
            rect,
            3.,
            egui::Stroke::new(1.0_f32, design::LIME),
            egui::StrokeKind::Inside,
        );
    }
    let full = format!("Mostra nel Finder {}", path.display());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), &full)
    });
    ui.ctx()
        .accesskit_node_builder(response.id, |node| node.set_label(full));
    response.on_hover_text(path.display().to_string())
}

fn report_notes(ui: &mut egui::Ui, r: &Report) {
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
}

fn number(value: impl ToString) -> String {
    let digits = value.to_string();
    let mut result = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            result.push('.');
        }
        result.push(ch);
    }
    result
}

// Fixed sibling cells keep the action and summary on one baseline. Neither
// child's content can advance the parent's cursor or wrap the other cell.
fn action_cell(
    parent: &mut egui::Ui,
    row: egui::Rect,
    right: bool,
    id: &str,
    right_width: f32,
) -> egui::Ui {
    let split = row.right() - right_width.min(row.width() * 0.62);
    let mut rect = row;
    let layout = if right {
        rect.min.x = split;
        egui::Layout::right_to_left(egui::Align::Center)
    } else {
        rect.max.x = (split - 6.).max(rect.left());
        egui::Layout::left_to_right(egui::Align::Center)
    };
    let mut cell = parent.new_child(
        egui::UiBuilder::new()
            .id_salt(id)
            .max_rect(rect)
            .layout(layout),
    );
    cell.set_clip_rect(parent.clip_rect().intersect(rect));
    cell
}

#[cfg(test)]
mod layout_tests {
    use super::*;

    #[test]
    fn compact_review_actions_remain_in_view_and_share_a_row() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let mut report = Report::default();
        for filename in ["one.bin", "two.bin"] {
            let path = root.join(filename);
            std::fs::write(&path, b"example").unwrap();
            report.items.push(crate::model::Item {
                identity: crate::trash::Identity::read(&path).unwrap(),
                path,
                bytes: 7,
                group: Some(1),
                cloud: false,
            });
        }
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        design::configure(&ctx);
        let mut app = Lindo::new(&ctx);
        app.mode = Some(Mode::Duplicates);
        let page = app.pages.get_mut(&Mode::Duplicates).unwrap();
        page.install(Arc::new(Results::prepare(Mode::Duplicates, report)));
        page.select_visible();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(660., 460.));
        for modal in [false, true] {
            if modal {
                app.review = Review::snapshot(&app.pages[&Mode::Duplicates]);
                app.confirm = true;
            }
            // The second frame has the measured sizes of windows and modals.
            for frame in 0..2 {
                let mut bottom = 0.;
                let output = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(screen),
                        ..Default::default()
                    },
                    |ctx| {
                        egui::SidePanel::left("nav")
                            .exact_width(160.)
                            .show(ctx, |_| {});
                        egui::TopBottomPanel::bottom("status")
                            .exact_height(24.)
                            .show(ctx, |_| {});
                        egui::CentralPanel::default().show(ctx, |ui| {
                            ui.add_space(58.);
                            bottom = ui.max_rect().bottom();
                            app.section(ui, Mode::Duplicates, ctx);
                            if modal {
                                app.review_page(ui, ctx);
                            }
                        });
                    },
                );
                if frame == 0 {
                    continue;
                }
                let tree = output.platform_output.accesskit_update.unwrap();
                let bounds = |label: &str| {
                    tree.nodes
                        .iter()
                        .find_map(|(_, node)| {
                            if node.label() == Some(label) || node.value() == Some(label) {
                                node.bounds()
                            } else {
                                None
                            }
                        })
                        .unwrap_or_else(|| panic!("Missing accessible control {label}"))
                };
                let action = bounds(if modal {
                    "Sposta nel Cestino"
                } else {
                    "Rivedi 7 B"
                });
                assert!(action.y1 <= f64::from(bottom) + 1. && action.x1 <= 660.);
                if modal {
                    let back = bounds("Indietro");
                    assert!((action.y0 - back.y0).abs() < 3.);
                    assert!(back.x1 < action.x0);
                } else {
                    let summary = bounds("1 copia selezionata");
                    assert!(summary.y0 >= action.y0 - 2. && summary.y1 <= action.y1 + 2.);
                }
            }
        }
    }
}
