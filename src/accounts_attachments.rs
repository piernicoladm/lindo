//! Only user-opened attachment copies under Apple Mail's two download directories.
use super::{Accounts, Reply};
use crate::{
    model::{Identity, Item, Mode},
    policy,
};
use eframe::egui;
use std::{
    io,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

pub(super) struct Copies {
    pub(super) items: Vec<Item>,
    status: String,
}
fn roots(home: &Path) -> [PathBuf; 2] {
    [
        home.join("Library/Containers/com.apple.mail/Data/Library/Mail Downloads"),
        home.join("Library/Mail Downloads"),
    ]
}
fn permitted(root: &Path, path: &Path) -> bool {
    let Ok(rel) = path.strip_prefix(root) else {
        return false;
    };
    if rel.as_os_str().is_empty() || policy::protected(rel, Mode::Large, None) {
        return false;
    }
    !rel.components().any(|c| {
        let n = c.as_os_str().to_string_lossy().to_ascii_lowercase();
        n.starts_with('.')
            || matches!(
                n.as_str(),
                "drafts" | "maildata" | "mailboxes" | "envelope index"
            )
            || n.strip_prefix('v').is_some_and(|digits| {
                !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
            })
            || [
                ".sqlite",
                ".sqlite-shm",
                ".sqlite-wal",
                ".db",
                ".emlx",
                ".emlxpart",
                ".mbox",
            ]
            .iter()
            .any(|ext| n.ends_with(ext))
    })
}
fn local(path: &Path) -> bool {
    policy::local_path(path).is_ok_and(|v| v)
        && policy::provider_state(path, false).is_ok_and(|s| s == (true, false))
}
fn check(home: &Path, item: &Item) -> Result<(), String> {
    let denied =
        || "La copia dell’allegato è cambiata o si trova fuori da Mail Downloads.".to_owned();
    let root = roots(home)
        .into_iter()
        .find(|r| permitted(r, &item.path))
        .ok_or_else(denied)?;
    if item.identity.is_dir() || !item.identity.matches(&item.path) {
        return Err(denied());
    }
    let fd = policy::open_file(&item.path, libc::O_EVTONLY).map_err(|_| denied())?;
    let m = fd.metadata().map_err(|_| denied())?;
    if !m.is_file()
        || m.uid() != unsafe { libc::getuid() }
        || m.nlink() != 1
        || m.len() != item.bytes
        || !item.identity.matches_metadata(&m)
        || !local(&item.path)
    {
        return Err(denied());
    }
    for ancestor in item
        .path
        .ancestors()
        .skip(1)
        .take_while(|p| p.starts_with(&root))
    {
        if policy::package(ancestor) || !policy::local_path(ancestor).is_ok_and(|v| v) {
            return Err(denied());
        }
    }
    if !item.identity.matches(&item.path) {
        return Err(denied());
    }
    Ok(())
}
pub(crate) fn validate(item: &Item) -> Result<(), String> {
    check(&policy::home(), item)
}
fn scan(home: &Path, stop: &AtomicBool) -> Result<Copies, String> {
    let _guard = policy::LocalOnly::enter().map_err(|e| e.to_string())?;
    let mut items = Vec::new();
    let mut missing = 0;
    let mut skipped = 0;
    let mut errors = 0;
    for root in roots(home) {
        let root_id = match Identity::read(&root) {
            Ok(id) if id.is_dir() && policy::local_path(&root).is_ok_and(|v| v) => id,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                missing += 1;
                continue;
            }
            _ => {
                errors += 1;
                continue;
            }
        };
        let mut dirs = vec![(root.clone(), root_id.node())];
        while let Some((dir, node)) = dirs.pop() {
            if stop.load(Ordering::Acquire) {
                return Err("Analisi annullata.".into());
            }
            let Ok(mut listing) = policy::children(&dir) else {
                errors += 1;
                continue;
            };
            if listing.node().ok() != Some(node) {
                errors += 1;
                continue;
            }
            while let Some(name) = listing.next() {
                if stop.load(Ordering::Acquire) {
                    return Err("Analisi annullata.".into());
                }
                let Ok(name) = name else {
                    errors += 1;
                    break;
                };
                let path = dir.join(&name);
                if !permitted(&root, &path) {
                    skipped += 1;
                    continue;
                }
                let Ok(s) = listing.stat(&name) else {
                    errors += 1;
                    continue;
                };
                if s.st_flags & 0x40000000 != 0 || s.st_uid != unsafe { libc::getuid() } {
                    skipped += 1;
                    continue;
                }
                let key = (s.st_dev as u64, s.st_ino);
                match s.st_mode & libc::S_IFMT {
                    libc::S_IFDIR
                        if !policy::package(&path)
                            && policy::local_path(&path).is_ok_and(|v| v) =>
                    {
                        dirs.push((path, key))
                    }
                    libc::S_IFREG if s.st_nlink == 1 && local(&path) => {
                        let Ok(identity) = Identity::read(&path) else {
                            errors += 1;
                            continue;
                        };
                        if identity.node() != key || identity.is_dir() {
                            errors += 1;
                            continue;
                        }
                        items.push(Item {
                            path,
                            bytes: s.st_size as u64,
                            identity,
                            group: None,
                            cloud: false,
                        });
                    }
                    _ => skipped += 1,
                }
            }
        }
    }
    items.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.path.cmp(&b.path)));
    let status = format!(
        "{} copie locali · {missing} cartelle assenti · {skipped} esclusioni · {errors} errori",
        items.len()
    );
    Ok(Copies { items, status })
}
impl Accounts {
    pub(super) fn attachments(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.label("Mail Downloads contiene copie aperte o scaricate da Apple Mail. Una copia modificata sul Mac può essere l’unica versione con le tue modifiche. I messaggi restano nella casella.");
        if ui.button("Analizza Mail Downloads").clicked() {
            self.reset();
            self.work(ctx, false, |stop| {
                scan(&policy::home(), stop).map(Reply::Copies)
            });
            return;
        }
        self.review_button(ui);
        if let Some(c) = &self.copies {
            ui.label(&c.status);
            let mut change = None;
            egui::ScrollArea::vertical()
                .id_salt("mail_local_copies")
                .show_rows(ui, 24.0, c.items.len(), |ui, range| {
                    for i in range {
                        let item = &c.items[i];
                        let mut checked = self.selected.contains(&i);
                        let label = format!(
                            "{:.2} MB · {}",
                            item.bytes as f64 / 1e6,
                            item.path.display()
                        );
                        if ui
                            .push_id(i, |ui| ui.checkbox(&mut checked, label))
                            .inner
                            .changed()
                        {
                            change = Some((i, checked));
                        }
                    }
                });
            if let Some((i, v)) = change {
                self.select(i, v);
            }
        }
    }
}
pub(super) fn move_copies(
    c: Copies,
    indices: Vec<usize>,
    stop: &AtomicBool,
) -> Result<Reply, String> {
    let mut moved = 0;
    let mut issue = String::new();
    for i in &indices {
        if stop.load(Ordering::Acquire) {
            break;
        }
        let item = c.items.get(*i).ok_or("Selezione cambiata.")?;
        match crate::trash::move_mail_download_copy(item) {
            Ok(_) => moved += 1,
            Err(e) => {
                issue = format!("{}: {e}", item.path.display());
                break;
            }
        }
    }
    Ok(Reply::Action(format!(
        "{moved} di {} copie spostate nel Cestino. {issue}",
        indices.len()
    )))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scans_both_exact_roots_and_rejects_changed_copies() {
        let tmp = tempfile::Builder::new()
            .prefix("lindo-mail-copy-")
            .tempdir()
            .unwrap();
        let home = tmp.path().canonicalize().unwrap();
        let paths = roots(&home);
        for root in &paths {
            std::fs::create_dir_all(root.join("attachments")).unwrap();
            std::fs::write(root.join("attachments/document.pdf"), b"fixture").unwrap();
            std::fs::create_dir_all(root.join("V10")).unwrap();
            std::fs::write(root.join("V10/index"), b"protected").unwrap();
            std::fs::write(root.join("database.sqlite"), b"protected").unwrap();
            std::os::unix::fs::symlink(root.join("attachments"), root.join("link")).unwrap();
        }
        let stop = AtomicBool::new(false);
        let c = scan(&home, &stop).unwrap();
        assert_eq!(c.items.len(), 2, "{}", c.status);
        assert!(check(&home, &c.items[0]).is_ok());
        std::fs::write(&c.items[0].path, b"changed").unwrap();
        assert!(check(&home, &c.items[0]).is_err());
        assert!(!permitted(
            &paths[0],
            &home.join("Library/Mail/V10/Inbox/message.emlx")
        ));
        assert!(scan(&home, &AtomicBool::new(true)).is_err());
    }
}
