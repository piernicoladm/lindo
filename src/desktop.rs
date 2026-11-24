use eframe::egui;
use objc2::{rc::Retained, MainThreadMarker};
use objc2_app_kit::{NSMenu, NSStatusBar, NSStatusItem};
use objc2_foundation::NSString;
use std::time::{Duration, Instant};

struct MenuMeter {
    item: Retained<NSStatusItem>,
    next: Instant,
}
impl Drop for MenuMeter {
    fn drop(&mut self) {
        NSStatusBar::systemStatusBar().removeStatusItem(&self.item);
    }
}
#[derive(Default)]
pub(crate) struct Desktop {
    meter: Option<MenuMeter>,
    cover_until: Option<Instant>,
    previous_fullscreen: bool,
}

impl Desktop {
    fn update_menu(&mut self, disk: Option<(u64, u64)>) {
        let Some(meter) = self.meter.as_mut() else {
            return;
        };
        let Some(main) = MainThreadMarker::new() else {
            return;
        };
        let menu = NSMenu::new(main);
        let mut lines = vec![String::from("Lindo")];
        if let Some((total, free)) = disk {
            lines.push(format!(
                "Disco · {} liberi su {}",
                crate::ui::size(free),
                crate::ui::size(total)
            ));
        }
        let mut load = [0_f64; 3];
        // getloadavg writes up to three averages to a caller-owned buffer.
        if unsafe { libc::getloadavg(load.as_mut_ptr(), 3) } == 3 {
            lines.push(format!(
                "Carico medio 1 / 5 / 15 min · {:.2} / {:.2} / {:.2}",
                load[0], load[1], load[2]
            ));
        }
        for line in lines {
            // Informational items have no action or target.
            unsafe {
                menu.addItemWithTitle_action_keyEquivalent(
                    &NSString::from_str(&line),
                    None,
                    &NSString::from_str(""),
                );
            }
        }
        // arrangeInFront: is the standard NSApplication action for showing its windows.
        unsafe {
            menu.addItemWithTitle_action_keyEquivalent(
                &NSString::from_str("Mostra Lindo"),
                Some(objc2::sel!(arrangeInFront:)),
                &NSString::from_str(""),
            );
        }
        meter.item.setMenu(Some(&menu));
        meter.next = Instant::now() + Duration::from_secs(15);
    }
    pub(crate) fn refresh_due(&self) -> bool {
        self.meter
            .as_ref()
            .is_some_and(|m| Instant::now() >= m.next)
    }
    pub(crate) fn refresh(&mut self, ctx: &egui::Context, disk: Option<(u64, u64)>) {
        if self.refresh_due() {
            self.update_menu(disk);
        }
        if let Some(m) = &self.meter {
            ctx.request_repaint_after(m.next.saturating_duration_since(Instant::now()));
        }
    }
    pub(crate) fn controls(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        disk: Option<(u64, u64)>,
    ) {
        ui.horizontal_wrapped(|ui| {
            let mut enabled = self.meter.is_some();
            if ui
                .checkbox(&mut enabled, "Monitor nella barra dei menu")
                .changed()
            {
                if enabled {
                    if let Some(main) = MainThreadMarker::new() {
                        let item = NSStatusBar::systemStatusBar().statusItemWithLength(-1.);
                        if let Some(btn) = item.button(main) {
                            btn.setTitle(&NSString::from_str("Lindo"));
                        }
                        self.meter = Some(MenuMeter {
                            item,
                            next: Instant::now(),
                        });
                        self.update_menu(disk);
                    }
                } else {
                    self.meter = None;
                }
            }
            if ui
                .button("Pulisci lo schermo")
                .on_hover_text("Schermo nero per 45 secondi. Esc per tornare.")
                .clicked()
            {
                self.previous_fullscreen = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
                self.cover_until = Some(Instant::now() + Duration::from_secs(45));
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
                ctx.request_repaint();
            }
        });
    }
    pub(crate) fn cover(&mut self, ctx: &egui::Context) -> bool {
        let Some(end) = self.cover_until else {
            return false;
        };
        if Instant::now() >= end || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.cover_until = None;
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.previous_fullscreen));
            ctx.request_repaint();
            return false;
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(egui::Color32::BLACK))
            .show(ctx, |ui| {
                ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| {
                    let seconds = end.saturating_duration_since(Instant::now()).as_secs() + 1;
                    ui.colored_label(egui::Color32::GRAY, format!("Esc per uscire · {seconds} s"));
                });
            });
        ctx.request_repaint_after(Duration::from_millis(250));
        true
    }
}
