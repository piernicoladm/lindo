use super::{
    design::{self, Destination},
    *,
};

pub(super) fn mode_destination(mode: Mode) -> Destination {
    match mode {
        Mode::Space => Destination::Map,
        Mode::Apps => Destination::Apps,
        _ => Destination::Cleanup,
    }
}
pub(super) fn extra_destination(extra: Extra) -> Destination {
    match extra {
        Extra::Updates | Extra::Reset | Extra::Trim => Destination::Apps,
        Extra::System | Extra::Accounts | Extra::Remote => Destination::Tools,
        _ => Destination::Cleanup,
    }
}

impl Lindo {
    fn selected_destination(&self) -> Destination {
        self.mode
            .map(mode_destination)
            .or_else(|| self.extra.map(extra_destination))
            .unwrap_or(self.destination)
    }

    fn navigate(&mut self, dest: Destination) {
        self.tool_query.clear();
        self.searching = false;
        self.search_origin = None;
        self.extra = None;
        self.mode = (dest == Destination::Map).then_some(Mode::Space);
        self.destination = dest;
    }

    pub(super) fn sidebar(&mut self, ctx: &egui::Context, browsing: bool) {
        use Destination::*;
        if browsing {
            let modes = [
                None,
                Some(Mode::Space),
                Some(Mode::Cache),
                Some(Mode::Logs),
                Some(Mode::Large),
                Some(Mode::Duplicates),
                Some(Mode::Apps),
            ];
            let keys = [
                egui::Key::Num1,
                egui::Key::Num2,
                egui::Key::Num3,
                egui::Key::Num4,
                egui::Key::Num5,
                egui::Key::Num6,
                egui::Key::Num7,
            ];
            for (mode, key) in modes.into_iter().zip(keys) {
                if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, key)) {
                    self.navigate(mode.map(mode_destination).unwrap_or(Home));
                    self.mode = mode;
                }
            }
        }
        let compact = ctx.screen_rect().width() < 830.0;
        let width = if compact {
            160.0
        } else {
            (ctx.screen_rect().width() * 0.188).clamp(170., 216.)
        };
        egui::SidePanel::left("navigazione")
            .resizable(false)
            .exact_width(width)
            .frame(
                egui::Frame::new()
                    .fill(design::RAIL)
                    .inner_margin(egui::Margin::symmetric(11, 10)),
            )
            .show(ctx, |ui| {
                let bounds = ui.max_rect();
                ui.painter().vline(
                    bounds.right() + 10.,
                    bounds.y_range(),
                    egui::Stroke::new(1_f32, design::LINE),
                );
                ui.add_space(27.0);
                ui.horizontal(|ui| {
                    ui.add_space(5.);
                    let sz = if compact { 32. } else { 40. };
                    ui.add(
                        egui::Image::new(&self.artwork.logo).fit_to_exact_size(egui::vec2(sz, sz)),
                    );
                    ui.label(design::display("LINDO", if compact { 19. } else { 23. }));
                });
                ui.add_space(if compact { 15.0 } else { 24.0 });
                ui.add_enabled_ui(browsing, |ui| {
                    for dest in [Home, Map, Cleanup, Apps, Tools] {
                        if design::nav(
                            ui,
                            dest,
                            !self.searching && self.selected_destination() == dest,
                        )
                        .clicked()
                        {
                            self.navigate(dest);
                        }
                    }
                });
                ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                    ui.add_enabled_ui(browsing, |ui| {
                        if design::nav(
                            ui,
                            Settings,
                            !self.searching && self.selected_destination() == Settings,
                        )
                        .clicked()
                        {
                            self.navigate(Settings);
                        }
                    });
                    ui.separator();
                });
            });
    }

    pub(super) fn toolbar(&mut self, ui: &mut egui::Ui, browsing: bool) {
        // The first 28 points also form the native draggable title bar.
        ui.add_space(24.);
        let shortcut = browsing
            && ui.input_mut(|i| {
                i.consume_key(egui::Modifiers::COMMAND, egui::Key::K)
                    || (self.mode.is_none()
                        && self.extra.is_none()
                        && i.consume_key(egui::Modifiers::COMMAND, egui::Key::F))
            });
        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width(), 29.),
            egui::Layout::right_to_left(egui::Align::Center),
            |ui| {
                let query = ui.add_enabled(
                    browsing,
                    egui::TextEdit::singleline(&mut self.tool_query)
                        .hint_text("Cerca")
                        .desired_width(156.)
                        .margin(egui::Margin {
                            left: 27,
                            right: 10,
                            top: 4,
                            bottom: 4,
                        }),
                );
                // TextEdit exposes its inner text rect. Draw the glass in the leading margin.
                let glass = egui::pos2(query.rect.left() - 15., query.rect.center().y - 1.);
                let stroke = egui::Stroke::new(1.35_f32, design::MUTED);
                ui.painter().circle_stroke(glass, 4.2, stroke);
                ui.painter().line_segment(
                    [glass + egui::vec2(3., 3.), glass + egui::vec2(7., 7.)],
                    stroke,
                );
                if shortcut {
                    query.request_focus();
                }
                if query.changed() || shortcut {
                    if !self.searching {
                        self.search_origin = Some((self.destination, self.mode, self.extra));
                    }
                    self.searching = true;
                    self.mode = None;
                    self.extra = None;
                }
                if self.searching
                    && (ui.small_button("Chiudi").clicked()
                        || ui
                            .input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)))
                {
                    self.searching = false;
                    if let Some((destination, mode, extra)) = self.search_origin.take() {
                        self.destination = destination;
                        self.mode = mode;
                        self.extra = extra;
                    }
                    self.tool_query.clear();
                    query.surrender_focus();
                }
            },
        );
    }
}
