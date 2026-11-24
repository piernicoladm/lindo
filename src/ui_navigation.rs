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
                if browsing && (query.clicked() || query.changed() || shortcut) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clicking_search_field_or_glass_opens_catalog_and_restores_origin() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        design::configure(&ctx);
        let mut app = Lindo::new(&ctx);
        app.destination = Destination::Cleanup;
        app.mode = Some(Mode::Duplicates);
        let draw = |app: &mut Lindo, events| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(660., 460.),
                    )),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    app.sidebar(ctx, true);
                    egui::CentralPanel::default().show(ctx, |ui| app.toolbar(ui, true));
                },
            )
            .platform_output
            .accesskit_update
            .unwrap()
        };
        let _ = draw(&mut app, vec![]);
        let tree = draw(&mut app, vec![]);
        let field = tree
            .nodes
            .iter()
            .find(|(_, node)| node.role() == egui::accesskit::Role::TextInput)
            .and_then(|(_, node)| node.bounds())
            .expect("global search input");
        for x in [field.x0 + 12., (field.x0 + field.x1) * 0.5] {
            let pos = egui::pos2(x as f32, ((field.y0 + field.y1) * 0.5) as f32);
            for pressed in [true, false] {
                let _ = draw(
                    &mut app,
                    vec![
                        egui::Event::PointerMoved(pos),
                        egui::Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ],
                );
            }
            assert!(
                app.searching,
                "Clicking the glass or text field should immediately open search"
            );
            assert_eq!(app.mode, None);
            let _ = draw(&mut app, vec![egui::Event::Text("log".into())]);
            assert_eq!(app.tool_query, "log");
            let _ = draw(
                &mut app,
                vec![egui::Event::Key {
                    key: egui::Key::Escape,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
            );
            assert!(!app.searching && app.tool_query.is_empty());
            assert_eq!(app.destination, Destination::Cleanup);
            assert_eq!(app.mode, Some(Mode::Duplicates));
        }

        for destination in [Destination::Cleanup, Destination::Tools] {
            let tree = draw(&mut app, vec![]);
            let bounds = tree
                .nodes
                .iter()
                .find(|(_, node)| node.label() == Some(destination.label()))
                .and_then(|(_, node)| node.bounds())
                .expect("visible sidebar destination");
            let pos = egui::pos2(
                (bounds.x1 - 3.) as f32,
                ((bounds.y0 + bounds.y1) * 0.5) as f32,
            );
            for pressed in [true, false] {
                let _ = draw(
                    &mut app,
                    vec![
                        egui::Event::PointerMoved(pos),
                        egui::Event::PointerButton {
                            pos,
                            pressed,
                            button: egui::PointerButton::Primary,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ],
                );
            }
            assert_eq!(app.destination, destination);
            assert!(app.mode.is_none() && app.extra.is_none());
        }
    }
}
