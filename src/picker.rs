use crate::{model::Mode, policy};
use eframe::egui;
use std::{
    ffi::{CStr, OsStr},
    fs::File,
    io,
    os::{
        fd::{AsRawFd, FromRawFd, IntoRawFd, RawFd},
        unix::{ffi::OsStrExt, fs::MetadataExt},
    },
    path::{Component, Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    time::{Duration, Instant},
};

const MAX_ENTRIES: usize = 2000;
const MAX_WORKERS: usize = 2;
const FLAG_DATALESS: u32 = 0x40000000;

fn denied() -> io::Error {
    io::Error::other("Cartella non accessibile o esclusa dalle protezioni.")
}
fn check(stop: &AtomicBool) -> io::Result<()> {
    if stop.load(Ordering::Acquire) {
        Err(io::Error::from(io::ErrorKind::Interrupted))
    } else {
        Ok(())
    }
}

fn input_path(value: &str, home: &Path) -> io::Result<PathBuf> {
    let value = Path::new(value);
    let path = if let Ok(relative) = value.strip_prefix("~") {
        home.join(relative)
    } else {
        value.to_owned()
    };
    if !path.is_absolute() || path.components().any(|c| c == Component::ParentDir) {
        return Err(io::Error::other(
            "Inserisci un percorso assoluto oppure usa ~/Cartella.",
        ));
    }
    Ok(path)
}

fn permitted(path: &Path, home: &Path) -> bool {
    if !home.is_absolute()
        || !path.is_absolute()
        || policy::protected(path, Mode::Large, None)
        || path
            .components()
            .any(|c| policy::placeholder_name(Path::new(c.as_os_str())))
    {
        return false;
    }
    if path.starts_with("/Volumes") {
        return true;
    }
    if !path.starts_with(home) {
        return false;
    }
    let relative = path
        .strip_prefix(home)
        .unwrap()
        .to_string_lossy()
        .to_lowercase();
    let relative = Path::new(&relative);
    !relative.starts_with("library")
        || relative.starts_with("library/cloudstorage")
        || relative.starts_with("library/mobile documents")
}

fn mounted(fd: RawFd, path: &Path) -> io::Result<()> {
    let Ok(relative) = path.strip_prefix("/Volumes") else {
        return Ok(());
    };
    let Some(name) = relative.iter().next() else {
        return Ok(());
    };
    let mut st = std::mem::MaybeUninit::<libc::statfs>::uninit();
    if unsafe { libc::fstatfs(fd, st.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let st = unsafe { st.assume_init() };
    let actual = unsafe { CStr::from_ptr(st.f_mntonname.as_ptr()) };
    if actual.to_bytes() != Path::new("/Volumes").join(name).as_os_str().as_bytes() {
        return Err(denied());
    }
    Ok(())
}

fn finder_flags(fd: RawFd) -> io::Result<u16> {
    let mut attrs: libc::attrlist = unsafe { std::mem::zeroed() };
    attrs.bitmapcount = libc::ATTR_BIT_MAP_COUNT;
    attrs.commonattr = libc::ATTR_CMN_FNDRINFO;
    let mut buffer = [0u8; 36];
    if unsafe {
        libc::fgetattrlist(
            fd,
            (&mut attrs as *mut libc::attrlist).cast(),
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            0,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    if u32::from_ne_bytes(buffer[..4].try_into().unwrap()) != 36 {
        return Err(denied());
    }
    Ok(u16::from_be_bytes([buffer[12], buffer[13]]))
}

fn open_child(
    parent: RawFd,
    name: &OsStr,
    path: &Path,
    home: &Path,
    leaf: bool,
) -> io::Result<File> {
    let c = policy::name_c(Path::new(name))?;
    let mut expected = std::mem::MaybeUninit::<libc::stat>::uninit();
    if unsafe {
        libc::fstatat(
            parent,
            c.as_ptr(),
            expected.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    let expected = unsafe { expected.assume_init() };
    if expected.st_mode & libc::S_IFMT != libc::S_IFDIR
        || expected.st_flags & FLAG_DATALESS != 0
        || policy::placeholder_name(path)
    {
        return Err(denied());
    }
    let raw = unsafe {
        libc::openat(
            parent,
            c.as_ptr(),
            libc::O_RDONLY
                | libc::O_DIRECTORY
                | libc::O_NOFOLLOW
                | libc::O_CLOEXEC
                | libc::O_NONBLOCK,
        )
    };
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    let file = unsafe { File::from_raw_fd(raw) };
    let meta = file.metadata()?;
    if meta.dev() != expected.st_dev as u64
        || meta.ino() != expected.st_ino
        || std::os::macos::fs::MetadataExt::st_flags(&meta) & FLAG_DATALESS != 0
    {
        return Err(denied());
    }
    let flags = finder_flags(raw)?;
    let library = path
        .strip_prefix(home)
        .is_ok_and(|r| r.as_os_str().as_bytes().eq_ignore_ascii_case(b"Library"));
    if flags & 0x2000 != 0
        || ((path.starts_with(home) || path.starts_with("/Volumes"))
            && !library
            && (flags & 0x4000 != 0 || expected.st_flags & libc::UF_HIDDEN != 0))
        || (leaf && (meta.mode() & 0o444 == 0 || meta.mode() & 0o111 == 0))
    {
        return Err(denied());
    }
    // Path-based package/provider properties complement the descriptor flags.
    // Refuse ancestry substitutions before and after reading those metadata.
    verify_path(path, &file)?;
    if policy::package(path) || !policy::provider_state(path, true)?.0 {
        return Err(denied());
    }
    verify_path(path, &file)?;
    mounted(raw, path)?;
    Ok(file)
}
fn verify_path(path: &Path, pinned: &File) -> io::Result<()> {
    let from_path = policy::open_file(path, libc::O_DIRECTORY | libc::O_EVTONLY)?.metadata()?;
    let pinned = pinned.metadata()?;
    if (from_path.dev(), from_path.ino()) != (pinned.dev(), pinned.ino()) {
        return Err(denied());
    }
    Ok(())
}
fn open_folder(path: &Path, home: &Path, stop: &AtomicBool) -> io::Result<File> {
    if !permitted(path, home) {
        return Err(denied());
    }
    let mut parent = policy::open_file(Path::new("/"), libc::O_DIRECTORY)?;
    let mut prefix = PathBuf::from("/");
    for part in path.components() {
        check(stop)?;
        if let Component::Normal(name) = part {
            prefix.push(name);
            parent = open_child(parent.as_raw_fd(), name, &prefix, home, prefix == path)?;
        }
    }
    Ok(parent)
}

struct Stream(*mut libc::DIR);
impl Drop for Stream {
    fn drop(&mut self) {
        unsafe { libc::closedir(self.0) };
    }
}
#[derive(Default)]
struct Listing {
    path: PathBuf,
    children: Vec<PathBuf>,
    limited: bool,
    excluded: usize,
    selectable: bool,
}

fn list(path: &Path, home: &Path, stop: &AtomicBool) -> io::Result<Listing> {
    let _local = policy::LocalOnly::enter()?;
    let file = open_folder(path, home, stop)?;
    let fd = file.try_clone()?.into_raw_fd();
    let raw = unsafe { libc::fdopendir(fd) };
    if raw.is_null() {
        let err = io::Error::last_os_error();
        unsafe { libc::close(fd) };
        return Err(err);
    }
    let stream = Stream(raw);
    verify_path(path, &file)?;
    let mut listing = Listing {
        path: path.to_owned(),
        selectable: path != home && path != Path::new("/Volumes"),
        ..Default::default()
    };
    let mut seen = 0usize;
    loop {
        check(stop)?;
        unsafe {
            *libc::__error() = 0;
        }
        let entry = unsafe { libc::readdir(stream.0) };
        if entry.is_null() {
            let errno = unsafe { *libc::__error() };
            if errno != 0 {
                return Err(io::Error::from_raw_os_error(errno));
            }
            break;
        }
        let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) };
        let bytes = name.to_bytes();
        if bytes == b"." || bytes == b".." {
            continue;
        }
        if seen == MAX_ENTRIES {
            listing.limited = true;
            break;
        }
        seen += 1;
        if bytes.starts_with(b".") {
            continue;
        }
        let dtype = unsafe { (*entry).d_type };
        if dtype != libc::DT_DIR && dtype != libc::DT_UNKNOWN {
            continue;
        }
        let name = OsStr::from_bytes(bytes);
        let child = path.join(name);
        if !permitted(&child, home)
            || open_child(file.as_raw_fd(), name, &child, home, true).is_err()
        {
            listing.excluded += 1;
            continue;
        }
        listing.children.push(child);
    }
    check(stop)?;
    verify_path(path, &file)?;
    listing.children.sort_unstable();
    Ok(listing)
}

struct Pending {
    id: u64,
    stop: Arc<AtomicBool>,
    started: Instant,
}
struct Reply {
    id: u64,
    result: io::Result<Listing>,
}

pub(crate) struct Picker {
    mode: Option<Mode>,
    home: PathBuf,
    text: String,
    listing: Option<Listing>,
    error: Option<String>,
    running: usize,
    next_id: u64,
    pending: Option<Pending>,
    tx: mpsc::Sender<Reply>,
    rx: mpsc::Receiver<Reply>,
}

impl Picker {
    pub(crate) fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            mode: None,
            home: policy::home(),
            text: String::new(),
            listing: None,
            error: None,
            running: 0,
            next_id: 0,
            pending: None,
            tx,
            rx,
        }
    }
    pub(crate) fn visible(&self) -> bool {
        self.mode.is_some()
    }
    fn stop(&mut self) {
        if let Some(p) = self.pending.take() {
            p.stop.store(true, Ordering::Release);
        }
    }
    fn close(&mut self) {
        self.stop();
        self.mode = None;
        self.listing = None;
        self.error = None;
    }
    pub(crate) fn open(&mut self, mode: Mode, ctx: &egui::Context) {
        self.mode = Some(mode);
        self.navigate(self.home.join("Downloads"), ctx);
    }
    fn navigate(&mut self, path: PathBuf, ctx: &egui::Context) {
        self.submit(path, ctx, list);
    }
    fn submit<F>(&mut self, path: PathBuf, ctx: &egui::Context, read: F)
    where
        F: FnOnce(&Path, &Path, &AtomicBool) -> io::Result<Listing> + Send + 'static,
    {
        self.stop();
        self.listing = None;
        self.error = None;
        self.text = path.to_string_lossy().into_owned();
        if self.running >= MAX_WORKERS {
            self.error=Some("Due letture sono ancora in attesa del sistema. Puoi annullare ora o riprovare quando una lettura termina.".into());
            return;
        }
        let id = self.next_id;
        self.next_id += 1;
        self.running += 1;
        let stop = Arc::new(AtomicBool::new(false));
        self.pending = Some(Pending {
            id,
            stop: stop.clone(),
            started: Instant::now(),
        });
        let sender = self.tx.clone();
        let repaint = ctx.clone();
        let home = self.home.clone();
        std::thread::spawn(move || {
            let result = read(&path, &home, &stop);
            let _ = sender.send(Reply { id, result });
            repaint.request_repaint();
        });
    }
    pub(crate) fn poll(&mut self) {
        while let Ok(reply) = self.rx.try_recv() {
            self.running -= 1;
            if !self.pending.as_ref().is_some_and(|p| p.id == reply.id) {
                continue;
            }
            self.pending = None;
            match reply.result {
                Ok(value) => self.listing = Some(value),
                Err(e) => self.error = Some(format!("Impossibile aprire la cartella. {e}")),
            }
        }
    }
    pub(crate) fn draw(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
    ) -> Option<(Mode, PathBuf)> {
        let mode = self.mode?;
        ui.heading("Scegli una cartella");
        let mut go = None;
        let mut close = false;
        let mut selected = None;
        ui.horizontal_wrapped(|ui| {
            let field = ui.add(
                egui::TextEdit::singleline(&mut self.text)
                    .desired_width(350.0)
                    .hint_text("Percorso assoluto oppure ~/Cartella"),
            );
            if ui.button("Vai").clicked()
                || (field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
            {
                match input_path(&self.text, &self.home) {
                    Ok(path) => go = Some(path),
                    Err(e) => {
                        self.stop();
                        self.listing = None;
                        self.error = Some(e.to_string());
                    }
                }
            }
        });
        ui.horizontal_wrapped(|ui| {
            for (label, path) in [
                ("Download", self.home.join("Downloads")),
                ("Utente", self.home.clone()),
                ("Volumi", PathBuf::from("/Volumes")),
                ("CloudStorage", self.home.join("Library/CloudStorage")),
                ("iCloud", self.home.join("Library/Mobile Documents")),
            ] {
                if ui.button(label).clicked() {
                    go = Some(path);
                }
            }
        });
        ui.horizontal_wrapped(|ui| {
            if let Some(list) = &self.listing {
                if let Some(parent) = list.path.parent().filter(|p| permitted(p, &self.home)) {
                    if ui.button("Cartella superiore").clicked() {
                        go = Some(parent.to_owned());
                    }
                }
                if ui
                    .add_enabled(
                        list.selectable
                            && input_path(&self.text, &self.home).is_ok_and(|p| p == list.path),
                        egui::Button::new("Seleziona questa cartella"),
                    )
                    .clicked()
                {
                    selected = Some((mode, list.path.clone()));
                    close = true;
                }
            }
            if (self.pending.is_some() || self.error.is_some()) && ui.button("Riprova").clicked() {
                match input_path(&self.text, &self.home) {
                    Ok(path) => go = Some(path),
                    Err(e) => self.error = Some(e.to_string()),
                }
            }
            close |= ui.button("Annulla").clicked()
                || ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
        });
        if let Some(p) = &self.pending {
            ui.horizontal(|ui| {
                ui.label("Lettura cartella…");
            });
            if p.started.elapsed() >= Duration::from_secs(5) {
                ui.label("macOS non ha ancora risposto. Controlla eventuali richieste di accesso, riprova oppure scegli Download.");
            }
            ctx.request_repaint_after(Duration::from_millis(200));
        }
        if let Some(e) = &self.error {
            ui.colored_label(ui.visuals().warn_fg_color, e);
        }
        if let Some(list) = &self.listing {
            ui.small(list.path.to_string_lossy());
            if list.limited {
                ui.label("Visualizzate le cartelle trovate nelle prime 2.000 voci. Puoi inserire il percorso di un’altra cartella.");
            }
            if list.excluded > 0 {
                ui.small(format!("{} voci escluse o non accessibili", list.excluded));
            }
            if list.children.is_empty() {
                ui.label("Nessuna sottocartella accessibile.");
            }
            egui::ScrollArea::vertical()
                .id_salt(("folder-list", &list.path))
                .show_rows(ui, 22.0, list.children.len(), |ui, rows| {
                    for row in rows {
                        let path = &list.children[row];
                        if ui
                            .add_sized(
                                [ui.available_width(), 22.0],
                                egui::Button::new(
                                    path.file_name().unwrap_or_default().to_string_lossy(),
                                )
                                .truncate(),
                            )
                            .clicked()
                        {
                            go = Some(path.clone());
                        }
                    }
                });
        }
        if close {
            self.close();
        } else if let Some(path) = go {
            self.navigate(path, ctx);
        }
        selected
    }
}
impl Drop for Picker {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_expands_home_and_rejects_parent_traversal() {
        let home = Path::new("/demo/home");
        assert_eq!(
            input_path("~/Downloads", home).unwrap(),
            home.join("Downloads")
        );
        assert_eq!(input_path("~", home).unwrap(), home);
        assert!(input_path("relative", home).is_err());
        assert!(input_path("~/Downloads/../hidden", home).is_err());
        assert!(!permitted(&home.join("library/Logs"), home));
        assert!(permitted(&home.join("Library/CloudStorage/example"), home));
        assert!(permitted(Path::new("/Volumes"), home));
    }

    #[test]
    fn directory_listing_excludes_hidden_packages_symlinks_and_private_paths() {
        use std::os::unix::fs::symlink;
        let temporary = tempfile::Builder::new()
            .prefix("lindo-picker-")
            .tempdir()
            .unwrap();
        let home = temporary.path().canonicalize().unwrap();
        let root = home.join("Downloads");
        std::fs::create_dir(&root).unwrap();
        for name in [
            "available",
            ".hidden",
            "Demo.app",
            "offline.icloud",
            "_6_Archivio_Private",
        ] {
            std::fs::create_dir(root.join(name)).unwrap();
        }
        symlink(root.join("available"), root.join("link")).unwrap();
        std::fs::write(root.join("plain-file"), b"synthetic").unwrap();
        let stop = AtomicBool::new(false);
        let result = list(&root, &home, &stop).unwrap();
        assert_eq!(result.children, vec![root.join("available")]);
        assert!(result.selectable);
        assert!(!result.limited);
        assert!(list(&root.join("link"), &home, &stop).is_err());
        assert!(list(&root.join("Demo.app"), &home, &stop).is_err());
        assert!(!list(&home, &home, &stop).unwrap().selectable);
        stop.store(true, Ordering::Release);
        assert!(list(&root, &home, &stop).is_err());
    }

    #[test]
    fn blocked_reads_release_ui_immediately_and_hold_at_most_two_slots() {
        let ctx = egui::Context::default();
        let mut picker = Picker::new();
        let (tx1, rx1) = mpsc::channel();
        let (tx2, rx2) = mpsc::channel();
        let stalled = |rx: mpsc::Receiver<()>| {
            move |path: &Path, _: &Path, _: &AtomicBool| {
                rx.recv_timeout(Duration::from_secs(5)).unwrap();
                Ok(Listing {
                    path: path.to_owned(),
                    ..Default::default()
                })
            }
        };
        picker.mode = Some(Mode::Large);
        picker.submit(PathBuf::from("/first"), &ctx, stalled(rx1));
        let stop = picker.pending.as_ref().unwrap().stop.clone();
        picker.close();
        assert!(!picker.visible() && stop.load(Ordering::Acquire));
        assert_eq!(picker.running, 1);
        picker.mode = Some(Mode::Duplicates);
        picker.submit(PathBuf::from("/second"), &ctx, stalled(rx2));
        assert_eq!(picker.running, 2);
        picker.submit(PathBuf::from("/third"), &ctx, |_, _, _| {
            panic!("unexpected third worker")
        });
        assert!(picker.error.is_some() && picker.pending.is_none());
        tx2.send(()).unwrap();
        tx1.send(()).unwrap();
        let finish = Instant::now() + Duration::from_secs(2);
        while picker.running > 0 && Instant::now() < finish {
            picker.poll();
            std::thread::yield_now();
        }
        assert_eq!(picker.running, 0);
        assert!(picker.listing.is_none());
        picker.submit(PathBuf::from("/new"), &ctx, |path, _, _| {
            Ok(Listing {
                path: path.into(),
                ..Default::default()
            })
        });
        let finish = Instant::now() + Duration::from_secs(2);
        while picker.running > 0 && Instant::now() < finish {
            picker.poll();
            std::thread::yield_now();
        }
        assert_eq!(picker.listing.as_ref().unwrap().path, PathBuf::from("/new"));
    }
}
