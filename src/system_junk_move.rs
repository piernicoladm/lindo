use super::scope::{self, Candidate, Roots};
use crate::{policy, system_tools::command, trash};
use std::os::{fd::AsRawFd, unix::fs::MetadataExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

fn opened(path: &Path, own_fd: Option<i32>, cancel: &AtomicBool) -> Result<(), String> {
    let output = command::run(
        "/usr/sbin/lsof",
        &[
            "-nP",
            "-Fpf",
            "--",
            path.to_str().ok_or("Nome non rappresentabile.")?,
        ],
        None,
        cancel,
        10,
    )?;
    if output.code == Some(1) && output.text.is_empty() {
        return Ok(());
    }
    if output.code != Some(0) || output.truncated {
        return Err("Verifica file aperti incompleta. File conservato.".into());
    }
    let mut pid = None;
    let mut seen = false;
    for line in output.text.lines() {
        if let Some(raw) = line.strip_prefix('p') {
            pid = raw.parse::<u32>().ok();
            if pid.is_none() {
                return Err("Verifica processo incompleta.".into());
            }
        } else if let Some(fd) = line.strip_prefix('f') {
            seen = true;
            if pid != Some(std::process::id())
                || own_fd.is_none()
                || fd.parse::<i32>().ok() != own_fd
            {
                return Err("File aperto da un processo. Ripeti dopo averlo chiuso.".into());
            }
        } else {
            return Err("Processi non verificabili. File conservato.".into());
        }
    }
    if !seen {
        return Err("Verifica file aperti incompleta.".into());
    }
    Ok(())
}
const FINDER_TRASH: &str = r#"use framework "Foundation"
use scripting additions
on run argv
set refURL to current application's NSURL's URLWithString:(item 1 of argv)
set currentPath to (refURL's filePathURL()'s |path|()) as text
if currentPath is not (item 2 of argv) then error "Identità o percorso cambiati"
set targetFile to (POSIX file currentPath) as alias
if (POSIX path of targetFile) is not (item 2 of argv) then error "Percorso cambiato"
tell application "Finder" to set movedItem to delete targetFile
return POSIX path of (movedItem as alias)
end run"#;

fn finder_move(item: &Candidate, cancel: &AtomicBool) -> Result<Option<PathBuf>, String> {
    let path = item.path.to_str().ok_or("Nome non rappresentabile.")?;
    let reference = objc2::rc::autoreleasepool(|_| {
        objc2_foundation::NSURL::from_file_path(&item.path)?
            .fileReferenceURL()?
            .absoluteString()
            .map(|s| s.to_string())
    })
    .filter(|p| p.starts_with("file:///.file/id="))
    .ok_or("Riferimento Finder stabile non disponibile. File consultabile.")?;
    if !item.identity.matches(&item.path) {
        return Err("File cambiato prima di contattare Finder.".into());
    }
    let output = command::run_stdout(
        "/usr/bin/osascript",
        &["-e", FINDER_TRASH, &reference, path],
        None,
        cancel,
        90,
    )
    .map_err(|e| format!("{e} Verifica lo stato nel Finder prima di ripetere."))?;
    finder_receipt(output)
}
fn finder_receipt(output: command::Output) -> Result<Option<PathBuf>, String> {
    if output.code != Some(0) || output.truncated {
        return Err(format!(
            "Finder non ha confermato lo spostamento. {}",
            output.text
        ));
    }
    let destination = PathBuf::from(output.text);
    if !destination.is_absolute()
        || !destination
            .components()
            .any(|c| c.as_os_str() == ".Trash" || c.as_os_str() == ".Trashes")
    {
        return Err("Ricevuta del Cestino non verificabile.".into());
    }
    Ok(Some(destination))
}

pub(super) fn move_one(
    roots: &Roots,
    item: &Candidate,
    cancel: &AtomicBool,
) -> Result<PathBuf, String> {
    let _guard = policy::LocalOnly::enter().map_err(|e| e.to_string())?;
    let feature_check = |fd| {
        scope::validate(roots, item)?;
        trash::check_running(&item.owner.app, &item.owner.id, item.cache)?;
        opened(&item.path, fd, cancel)?;
        if cancel.load(Ordering::Acquire) {
            return Err("Interrotto prima dello spostamento.".into());
        }
        scope::validate(roots, item)
    };
    let receipt = if item.uid == unsafe { libc::geteuid() } {
        trash::move_reviewed_file(&item.path, &item.identity, |handle| {
            feature_check(Some(handle.as_raw_fd()))
        })?
    } else if item.uid == 0 {
        feature_check(None)?;
        finder_move(item, cancel)?
    } else {
        return Err("File appartenente a un altro utente. Solo consultazione.".into());
    };
    let target = receipt.ok_or("Spostamento richiesto. Destinazione non verificabile.")?;
    let source_gone = item
        .path
        .symlink_metadata()
        .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound);
    let same = target
        .symlink_metadata()
        .is_ok_and(|m| m.is_file() && (m.dev(), m.ino()) == item.identity.node());
    if !source_gone || !same {
        return Err(
            "Esito non verificabile dopo la richiesta al Cestino. Controlla nel Finder.".into(),
        );
    }
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn finder_denial_and_unverified_receipts_never_count_as_success() {
        for (code, s) in [
            (1, "User canceled (-128)"),
            (1, "Authorization denied"),
            (0, "/private/tmp/untrusted"),
            (0, ""),
        ] {
            assert!(finder_receipt(command::Output {
                code: Some(code),
                text: s.into(),
                truncated: false
            })
            .is_err());
        }
    }
}
