use super::{command, installed};
use crate::policy;
use std::fs::{DirBuilder, File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

pub(super) fn directory() -> PathBuf {
    policy::home().join("Library/Application Support/Lindo/ClamAV")
}

pub(super) fn local_database() -> Option<PathBuf> {
    let dir = directory();
    let present = ["daily.cvd", "daily.cld"].iter().any(|s| {
        dir.join(s)
            .symlink_metadata()
            .is_ok_and(|m| m.is_file() && m.len() > 0)
    });
    (present && policy::open_file(&dir, libc::O_DIRECTORY).is_ok()).then_some(dir)
}
pub(super) fn update(cancel: &AtomicBool) -> Result<String, String> {
    let fresh = installed(&[
        "/opt/homebrew/bin/freshclam",
        "/usr/local/bin/freshclam",
        "/usr/local/clamav/bin/freshclam",
    ])
    .ok_or("freshclam non installato.")?;
    let app = policy::home().join("Library/Application Support");
    policy::open_file(&app, libc::O_DIRECTORY).map_err(|e| e.to_string())?;
    let db = directory();
    for dir in [app.join("Lindo"), db.clone()] {
        match DirBuilder::new().mode(0o700).create(&dir) {
            Ok(()) => (),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
            Err(e) => return Err(e.to_string()),
        }
        policy::open_file(&dir, libc::O_DIRECTORY).map_err(|e| e.to_string())?;
    }
    update_in(fresh, &db, cancel)
}

pub(super) fn update_in(fresh: &str, db: &Path, cancel: &AtomicBool) -> Result<String, String> {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    let path = db.join(format!(
        ".lindo-freshclam-{}-{unique}.conf",
        std::process::id()
    ));
    let mut config: File = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .map_err(|e| e.to_string())?;
    std::fs::remove_file(path).map_err(|e| e.to_string())?;
    config.write_all(b"DatabaseMirror database.clamav.net\nForeground yes\nConnectTimeout 20\nReceiveTimeout 60\n").and_then(|_|config.seek(SeekFrom::Start(0))).map_err(|e|e.to_string())?;
    let argument = format!(
        "--datadir={}",
        db.to_str().ok_or("Percorso non leggibile.")?
    );
    let result = command::run(
        fresh,
        &["--config-file=/dev/stdin", &argument],
        Some(config),
        cancel,
        600,
    )?;
    if result.code != Some(0) {
        return Err(format!(
            "Aggiornamento delle firme incompleto.\n{}",
            result.text
        ));
    }
    Ok(format!(
        "Aggiornamento delle firme completato in {}.\n{}",
        db.display(),
        result.text
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "Requires a prepared isolated database and an explicit official mirror network check"]
    fn real_freshclam_updates_the_isolated_qa_database() {
        let base = PathBuf::from(std::env::var_os("LINDO_CLAM_QA_BASE").unwrap());
        assert!(base.join("clamfixture/home/.lindo-synthetic-qa").is_file());
        let text = update_in(
            "/opt/homebrew/bin/freshclam",
            &base.join("clamdb"),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(text.contains("completato"), "{text}");
    }
}
