//! Native menu lifecycle fixture. Every callback runs on the application's main queue.
use objc2::{rc::Retained, MainThreadMarker};
use objc2_app_kit::{NSApplication, NSMenu, NSStatusItem};
use objc2_foundation::NSString;
use std::ffi::c_void;
use std::io::Write;

extern "C" {
    static _dispatch_main_q: u8;
    fn dispatch_time(when: u64, delta: i64) -> u64;
    fn dispatch_after_f(
        when: u64,
        queue: *const c_void,
        ctx: *mut c_void,
        callback: extern "C" fn(*mut c_void),
    );
}

fn later<F: FnOnce() + 'static>(ms: i64, action: F) {
    extern "C" fn invoke<F: FnOnce()>(ptr: *mut c_void) {
        // Each dispatch_after_f submission invokes this callback exactly once.
        let action = unsafe { Box::from_raw(ptr.cast::<F>()) };
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(action)).is_err() {
            fail("callback panic");
        }
    }
    // Retained AppKit objects captured by action stay on the same main thread.
    unsafe {
        dispatch_after_f(
            dispatch_time(0, ms * 1_000_000),
            (&raw const _dispatch_main_q).cast(),
            Box::into_raw(Box::new(action)).cast(),
            invoke::<F>,
        );
    }
}

fn fail(reason: &str) -> ! {
    eprintln!("LINDO_MENU_QA ok=false reason={reason}");
    std::process::exit(1);
}

fn app(pid: u32) -> Retained<NSApplication> {
    if std::process::id() != pid {
        fail("PID changed");
    }
    NSApplication::sharedApplication(
        MainThreadMarker::new().unwrap_or_else(|| fail("off main thread")),
    )
}

fn menu_action(item: &NSStatusItem, label: &str) -> (Retained<NSMenu>, isize) {
    let menu = item
        .menu(MainThreadMarker::new().unwrap())
        .unwrap_or_else(|| fail("missing menu"));
    let index = menu.indexOfItemWithTitle(&NSString::from_str(label));
    if index < 0 {
        fail("missing menu action");
    }
    (menu, index)
}

pub(super) fn start(item: Retained<NSStatusItem>) {
    let pid = std::process::id();
    later(900, move || {
        let app_before = app(pid);
        let windows = app_before.windows();
        let mut matching = windows.iter().filter(|w| w.title().to_string() == "Lindo");
        let window = matching
            .next()
            .unwrap_or_else(|| fail("missing Lindo window"));
        if matching.next().is_some() || app_before.isHidden() || !window.isVisible() {
            fail("unexpected initial window state");
        }
        window.performClose(None);
        later(900, move || {
            if !app(pid).isHidden() {
                fail("close did not hide the application");
            }
            let (hidden_menu, _) = menu_action(&item, "Mostra Lindo");
            later(18_000, move || {
                if !app(pid).isHidden() {
                    fail("window did not remain hidden");
                }
                let (menu, index) = menu_action(&item, "Mostra Lindo");
                if std::ptr::eq(&*menu, &*hidden_menu) {
                    fail("menu did not refresh while hidden");
                }
                let titles: Vec<_> = menu
                    .itemArray()
                    .iter()
                    .map(|i| i.title().to_string())
                    .collect();
                for prefix in ["RAM ·", "Batteria ·", "Rete ·"] {
                    if !titles.iter().any(|s| s.starts_with(prefix)) {
                        fail("missing sampled metric");
                    }
                }
                menu.performActionForItemAtIndex(index);
                later(700, move || {
                    if app(pid).isHidden() || !window.isVisible() {
                        fail("show did not restore the window");
                    }
                    let (menu, index) = menu_action(&item, "Esci da Lindo");
                    println!("LINDO_MENU_QA ok=true pid={pid}");
                    std::io::stdout().flush().unwrap();
                    later(3000, || fail("quit did not terminate"));
                    menu.performActionForItemAtIndex(index);
                });
            });
        });
    });
}
