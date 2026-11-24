#[path = "desktop_meter.rs"]
mod desktop_meter;
#[cfg(feature = "ui-probe")]
#[path = "menu_probe.rs"]
mod menu_probe;
use eframe::egui;
use objc2::{rc::Retained, MainThreadMarker};
use objc2_app_kit::{NSMenu, NSStatusBar, NSStatusItem};
use objc2_foundation::NSString;
use std::time::{Duration, Instant};

struct MenuMeter {
    sampler: desktop_meter::Sampler,
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
    keyboard_requested: bool,
    keep_monitor: bool,
    keyboard: Option<keyboard::Lock>,
}

impl Desktop {
    #[cfg(feature = "ui-probe")]
    pub(crate) fn start_menu_probe(&mut self, ctx: &egui::Context) {
        self.set_meter(true, ctx, None);
        self.keep_monitor = true;
        menu_probe::start(self.meter.as_ref().expect("main-thread menu").item.clone());
    }

    fn set_meter(&mut self, enabled: bool, ctx: &egui::Context, disk: Option<(u64, u64)>) {
        if enabled {
            if let Some(main) = MainThreadMarker::new() {
                let item = NSStatusBar::systemStatusBar().statusItemWithLength(-1.);
                if let Some(btn) = item.button(main) {
                    btn.setTitle(&NSString::from_str("Lindo"));
                }
                self.meter = Some(MenuMeter {
                    sampler: desktop_meter::Sampler::start(ctx),
                    item,
                    next: Instant::now(),
                });
                self.update_menu(disk);
            }
        } else {
            self.meter = None;
        }
    }
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
        lines.extend(meter.sampler.lines());
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
        let app = objc2_app_kit::NSApplication::sharedApplication(main);
        for (label, action) in [
            ("Mostra Lindo", objc2::sel!(unhide:)),
            ("Esci da Lindo", objc2::sel!(terminate:)),
        ] {
            // NSApplication owns both documented actions and outlives the weak menu target.
            unsafe {
                let item = menu.addItemWithTitle_action_keyEquivalent(
                    &NSString::from_str(label),
                    Some(action),
                    &NSString::from_str(""),
                );
                item.setTarget(Some(&app));
            }
        }
        meter.item.setMenu(Some(&menu));
        meter.next = Instant::now() + desktop_meter::PERIOD;
    }
    pub(crate) fn refresh_due(&self) -> bool {
        self.meter
            .as_ref()
            .is_some_and(|m| Instant::now() >= m.next || m.sampler.ready())
    }
    pub(crate) fn refresh(&mut self, ctx: &egui::Context, disk: Option<(u64, u64)>) {
        if self.keep_monitor
            && self.meter.is_some()
            && ctx.input(|i| i.viewport().close_requested())
        {
            if let Some(main) = MainThreadMarker::new() {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                objc2_app_kit::NSApplication::sharedApplication(main).hide(None);
            }
        }
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
                self.set_meter(enabled, ctx, disk);
            }
            ui.checkbox(&mut self.keyboard_requested,"Blocca anche la tastiera")
                .on_hover_text("Opzionale. Richiede l’autorizzazione Accessibilità di macOS. La tastiera viene sbloccata dopo 45 secondi, con Esc o con un clic.");
            if enabled {
                ui.checkbox(&mut self.keep_monitor, "Tieni il monitor attivo a finestra chiusa").on_hover_text("Mostra Lindo o esci dal menu nella barra di macOS.");
            }
            if ui.button("Pulisci lo schermo")
                .on_hover_text("Schermo nero per 45 secondi. Premi Esc o fai clic per tornare.")
                .clicked()
            {
                self.previous_fullscreen = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
                let end=Instant::now()+Duration::from_secs(45);
                self.cover_until=Some(end);
                self.keyboard=self.keyboard_requested.then(|| keyboard::Lock::start(end));
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
                ctx.request_repaint();
            }
        });
    }
    pub(crate) fn cover(&mut self, ctx: &egui::Context) -> bool {
        let Some(end) = self.cover_until else {
            return false;
        };
        let keyboard_state = self.keyboard.as_ref().map(keyboard::Lock::status);
        let exit = ctx.input(|i| i.key_pressed(egui::Key::Escape) || i.pointer.primary_pressed());
        if Instant::now() >= end || exit || keyboard_state == Some(keyboard::Status::Ended) {
            self.keyboard.take();
            self.cover_until = None;
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.previous_fullscreen));
            ctx.request_repaint();
            return false;
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(egui::Color32::BLACK))
            .show(ctx, |ui| {
                ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| {
                    let seconds = end
                        .saturating_duration_since(Instant::now())
                        .as_millis()
                        .div_ceil(1000);
                    ui.colored_label(
                        egui::Color32::GRAY,
                        format!("Esc o clic per uscire · {seconds} s"),
                    );
                    if let Some(state) = keyboard_state {
                        ui.colored_label(egui::Color32::GRAY, state.label());
                    }
                });
            });
        ctx.request_repaint_after(Duration::from_millis(250));
        true
    }
}

mod keyboard {
    use std::sync::{
        atomic::{AtomicU8, Ordering},
        Arc,
    };
    use std::thread::JoinHandle;
    use std::time::{Instant, SystemTime};

    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    #[repr(u8)]
    pub(super) enum Status {
        Starting,
        Active,
        Unavailable,
        Ended,
    }
    impl Status {
        pub(super) fn label(self) -> &'static str {
            match self {
                Self::Starting => "Verifica dell’autorizzazione tastiera…",
                Self::Active => {
                    "Tastiera bloccata per la sessione. I controlli hardware riservati restano gestiti da macOS."
                }
                Self::Unavailable => {
                    "Solo schermo nero. Blocco tastiera non disponibile. Per abilitarlo, controlla Lindo in Impostazioni di Sistema > Privacy e sicurezza > Accessibilità e, se richiesto da macOS, Monitoraggio input."
                }
                Self::Ended => "Tastiera sbloccata.",
            }
        }
    }

    struct Gate {
        end: Instant,
        wall_end: SystemTime,
        status: AtomicU8,
    }
    impl Gate {
        fn state(&self) -> Status {
            match self.status.load(Ordering::Acquire) {
                0 => Status::Starting,
                1 => Status::Active,
                2 => Status::Unavailable,
                _ => Status::Ended,
            }
        }
        fn finish(&self) {
            self.status.store(Status::Ended as u8, Ordering::Release);
        }
        fn allow_filter(&self, now: Instant, escape: bool) -> bool {
            if now >= self.end || SystemTime::now() >= self.wall_end || escape {
                self.finish();
                return false;
            }
            self.state() == Status::Active
        }
    }

    pub(super) struct Lock {
        gate: Arc<Gate>,
        worker: Option<JoinHandle<()>>,
    }
    impl Lock {
        pub(super) fn start(end: Instant) -> Self {
            let gate = Arc::new(Gate {
                end,
                wall_end: SystemTime::now() + end.saturating_duration_since(Instant::now()),
                status: AtomicU8::new(Status::Starting as u8),
            });
            let worker = native_worker(&gate);
            if worker.is_none() {
                gate.status
                    .store(Status::Unavailable as u8, Ordering::Release);
            }
            Self { gate, worker }
        }
        pub(super) fn status(&self) -> Status {
            self.gate.state()
        }
    }
    impl Drop for Lock {
        fn drop(&mut self) {
            self.gate.finish();
            if let Some(handle) = self.worker.take() {
                let _ = handle.join();
            }
        }
    }

    // Tests must never install a global input filter, including in UI fixtures.
    #[cfg(test)]
    fn native_worker(_: &Arc<Gate>) -> Option<JoinHandle<()>> {
        None
    }

    #[cfg(not(test))]
    fn native_worker(gate: &Arc<Gate>) -> Option<JoinHandle<()>> {
        if !native::permitted() {
            return None;
        }
        let gate = gate.clone();
        std::thread::Builder::new()
            .name("lindo-keyboard-cover".into())
            .spawn(move || native::run(gate))
            .ok()
    }

    #[cfg(not(test))]
    mod native {
        use super::{Gate, Status};
        use std::{
            ffi::c_void,
            ptr,
            sync::{atomic::Ordering, Arc},
            time::Instant,
        };
        type Object = *mut c_void;
        type Callback = unsafe extern "C" fn(Object, u32, Object, *mut c_void) -> Object;
        const KEY_DOWN: u32 = 10;
        const KEY_UP: u32 = 11;
        const FLAGS_CHANGED: u32 = 12;
        const KEY_MASK: u64 = (1 << KEY_DOWN) | (1 << KEY_UP) | (1 << FLAGS_CHANGED);

        #[link(name = "ApplicationServices", kind = "framework")]
        unsafe extern "C" {
            fn AXIsProcessTrusted() -> bool;
        }
        #[link(name = "CoreGraphics", kind = "framework")]
        unsafe extern "C" {
            fn CGPreflightListenEventAccess() -> bool;
            fn CGEventTapCreate(
                location: u32,
                placement: u32,
                options: u32,
                mask: u64,
                callback: Callback,
                data: *mut c_void,
            ) -> Object;
            fn CGEventTapEnable(tap: Object, enabled: bool);
            fn CGEventTapIsEnabled(tap: Object) -> bool;
            fn CGEventGetIntegerValueField(event: Object, field: u32) -> i64;
        }
        #[link(name = "CoreFoundation", kind = "framework")]
        unsafe extern "C" {
            fn CFMachPortCreateRunLoopSource(
                allocator: Object,
                port: Object,
                order: isize,
            ) -> Object;
            fn CFMachPortInvalidate(port: Object);
            fn CFRunLoopGetCurrent() -> Object;
            fn CFRunLoopAddSource(run_loop: Object, source: Object, mode: Object);
            fn CFRunLoopRemoveSource(run_loop: Object, source: Object, mode: Object);
            fn CFRunLoopRunInMode(mode: Object, seconds: f64, until_handled: u8) -> i32;
            fn CFRelease(object: *const c_void);
            static kCFRunLoopDefaultMode: Object;
        }
        pub(super) fn permitted() -> bool {
            // Preflight only. The user grants consent in System Settings.
            unsafe { AXIsProcessTrusted() && CGPreflightListenEventAccess() }
        }

        unsafe extern "C" fn callback(
            _: Object,
            kind: u32,
            event: Object,
            data: *mut c_void,
        ) -> Object {
            // userInfo points into the Arc held by this very run-loop thread.
            let gate = unsafe { &*data.cast::<Gate>() };
            if kind == u32::MAX || kind == u32::MAX - 1 {
                let _ = gate.status.compare_exchange(
                    Status::Active as u8,
                    Status::Unavailable as u8,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                );
                return event;
            }
            if event.is_null() || !matches!(kind, KEY_DOWN | KEY_UP | FLAGS_CHANGED) {
                return event;
            }
            // Inspect the physical Escape key only. Never copy text or store events.
            let escape = kind == KEY_DOWN && unsafe { CGEventGetIntegerValueField(event, 9) } == 53;
            if gate.allow_filter(Instant::now(), escape) {
                ptr::null_mut()
            } else {
                event
            }
        }

        pub(super) fn run(gate: Arc<Gate>) {
            // Session-level filter; HID-level taps would require root.
            unsafe {
                let tap = CGEventTapCreate(
                    1,
                    0,
                    0,
                    KEY_MASK,
                    callback,
                    Arc::as_ptr(&gate).cast_mut().cast(),
                );
                if tap.is_null() {
                    let _ = gate.status.compare_exchange(
                        Status::Starting as u8,
                        Status::Unavailable as u8,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    );
                    return;
                }
                let source = CFMachPortCreateRunLoopSource(ptr::null_mut(), tap, 0);
                if source.is_null() {
                    CFMachPortInvalidate(tap);
                    CFRelease(tap);
                    let _ = gate.status.compare_exchange(
                        Status::Starting as u8,
                        Status::Unavailable as u8,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    );
                    return;
                }
                let run_loop = CFRunLoopGetCurrent();
                CFRunLoopAddSource(run_loop, source, kCFRunLoopDefaultMode);
                if CGEventTapIsEnabled(tap) && Instant::now() < gate.end {
                    let _ = gate.status.compare_exchange(
                        Status::Starting as u8,
                        Status::Active as u8,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    );
                } else {
                    let _ = gate.status.compare_exchange(
                        Status::Starting as u8,
                        Status::Unavailable as u8,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    );
                }
                while gate.allow_filter(Instant::now(), false) {
                    // Enforce the deadline independently of egui, also with no input.
                    CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.025, 1);
                }
                CGEventTapEnable(tap, false);
                CFRunLoopRemoveSource(run_loop, source, kCFRunLoopDefaultMode);
                CFMachPortInvalidate(tap);
                CFRelease(source);
                CFRelease(tap);
            }
        }
    }
    #[cfg(test)]
    mod tests {
        use super::*;
        use std::time::Duration;
        #[test]
        fn escape_timer_and_manual_stop_release_simulated_filter() {
            let now = Instant::now();
            let end = now + Duration::from_secs(45);
            for action in ["escape", "timer", "mouse"] {
                let gate = Gate {
                    end,
                    wall_end: SystemTime::now() + Duration::from_secs(45),
                    status: AtomicU8::new(Status::Active as u8),
                };
                assert!(gate.allow_filter(now, false));
                match action {
                    "escape" => assert!(!gate.allow_filter(now, true)),
                    "timer" => assert!(!gate.allow_filter(end, false)),
                    _ => gate.finish(),
                }
                assert_eq!(gate.state(), Status::Ended);
                assert!(!gate.allow_filter(now, false));
            }
        }
        #[test]
        fn denied_permission_displays_a_nonblocking_cover() {
            let ctx = eframe::egui::Context::default();
            let end = Instant::now() + Duration::from_secs(45);
            let lock = Lock::start(end);
            assert_eq!(lock.status(), Status::Unavailable);
            assert!(lock.worker.is_none());
            let mut desktop = crate::desktop::Desktop {
                keyboard: Some(lock),
                cover_until: Some(end),
                ..Default::default()
            };
            let _ = ctx.run(Default::default(), |ctx| {
                assert!(desktop.cover(ctx));
            });
            desktop.cover_until = Some(Instant::now());
            let _ = ctx.run(Default::default(), |ctx| {
                assert!(!desktop.cover(ctx));
            });
            assert!(desktop.keyboard.is_none());
        }
    }
}
