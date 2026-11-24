use super::{command, installed, scanned_clean, signatures, verdict, CLAM};
use crate::{model::Mode, policy};
use std::fs::File;
use std::os::{
    fd::AsRawFd,
    unix::{ffi::OsStrExt, fs::MetadataExt},
};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const MAX_FILE: u64 = 64 * 1024 * 1024;
const MAX_FILES: usize = 2_000;
const MAX_ENTRIES: usize = 20_000;

fn permitted(path: &Path) -> bool {
    policy::allowed_root(Mode::Large, path)
        && !policy::cloud(path)
        && !path.components().any(|c| {
            c.as_os_str()
                .to_string_lossy()
                .eq_ignore_ascii_case("_3_Vault")
        })
}

fn local_handle(path: &Path, directory: bool) -> Result<File, String> {
    if !permitted(path) {
        return Err("Percorso cloud o protetto.".into());
    }
    let input = policy::open_file(path, if directory { libc::O_DIRECTORY } else { 0 })
        .map_err(|e| e.to_string())?;
    let mut name = [0u8; libc::PATH_MAX as usize];
    if unsafe { libc::fcntl(input.as_raw_fd(), libc::F_GETPATH, name.as_mut_ptr()) } < 0 {
        return Err("Percorso effettivo non verificabile.".into());
    }
    let end = name
        .iter()
        .position(|b| *b == 0)
        .ok_or("Percorso effettivo non verificabile.")?;
    let actual = Path::new(std::ffi::OsStr::from_bytes(&name[..end]));
    let meta = input.metadata().map_err(|e| e.to_string())?;
    if !permitted(actual)
        || !policy::local_path(actual).unwrap_or(false)
        || actual
            .ancestors()
            .skip(if directory { 0 } else { 1 })
            .take_while(|p| p.parent().is_some())
            .any(policy::package)
        || (!directory && (!meta.is_file() || meta.len() > MAX_FILE))
    {
        return Err(
            "Elemento non locale, protetto, contenuto in un pacchetto o oltre 64 MiB.".into(),
        );
    }
    Ok(input)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Finding {
    pub(super) path: PathBuf,
    identity: crate::trash::Identity,
    bytes: u64,
}
impl Finding {
    fn capture(path: &Path, f: &File) -> Result<Self, String> {
        let m = f.metadata().map_err(|e| e.to_string())?;
        let identity = crate::trash::Identity::read(path).map_err(|e| e.to_string())?;
        if !identity.matches_metadata(&m) {
            return Err("File cambiato durante la verifica.".into());
        }
        Ok(Self {
            path: path.into(),
            bytes: m.len(),
            identity,
        })
    }
    fn validate(&self, f: &File) -> Result<(), String> {
        if !self.identity.matches(&self.path)
            || !f
                .metadata()
                .is_ok_and(|m| self.identity.matches_metadata(&m))
        {
            return Err("Il file è cambiato. Ripeti la scansione iniziale.".into());
        }
        Ok(())
    }
}
pub(super) fn trash_detected(
    finding: &Finding,
    cancel: &AtomicBool,
) -> Result<Option<PathBuf>, String> {
    let scanner = installed(CLAM).ok_or("ClamAV non disponibile.")?;
    crate::trash::move_reviewed_file(&finding.path, &finding.identity, |file| {
        finding.validate(&local_handle(&finding.path, false)?)?;
        let output = invoke(
            scanner,
            file.try_clone().map_err(|e| e.to_string())?,
            cancel,
            180,
        )?;
        if cancel.load(Ordering::Acquire) {
            return Err("Operazione annullata.".into());
        }
        if output.code != Some(1)
            || output.truncated
            || !output.text.lines().any(|line| {
                line.ends_with(" FOUND") && !line.contains("Heuristics.Limits.Exceeded")
            })
        {
            return Err(format!(
                "Il motore non ha riconfermato una firma. File conservato.\n{}",
                output.text
            ));
        }
        Ok(())
    })
}
pub(super) fn recheck(finding: &Finding, cancel: &AtomicBool) -> Result<String, String> {
    let scan = installed(CLAM).ok_or("ClamAV non disponibile.")?;
    let _local = policy::LocalOnly::enter().map_err(|e| e.to_string())?;
    let file = local_handle(&finding.path, false)?;
    finding.validate(&file)?;
    let output = invoke(scan, file, cancel, 180)?;
    if output.code != Some(1) {
        return Ok(format!("{}\n{}", verdict(&output), output.text));
    }
    finding.validate(&local_handle(&finding.path, false)?)?;
    let reveal = command::run(
        "/usr/bin/open",
        &["-R", finding.path.to_str().ok_or("Nome non leggibile.")?],
        None,
        cancel,
        12,
    )?;
    if reveal.code != Some(0) {
        return Err(format!(
            "Rilevazione confermata. Apertura Finder non riuscita. {}",
            reveal.text
        ));
    }
    Ok(format!(
        "Rilevazione confermata. Elemento selezionato nel Finder per la revisione.\n{}\n{}",
        finding.path.display(),
        output.text
    ))
}

fn invoke(
    scanner: &str,
    file: File,
    cancel: &AtomicBool,
    timeout: u64,
) -> Result<command::Output, String> {
    let db = signatures::local_database().map(|p| format!("--database={}", p.display()));
    let mut args = vec![
        "--alert-exceeds-max=yes",
        "--max-filesize=64M",
        "--max-scansize=128M",
    ];
    if let Some(path) = db.as_deref() {
        args.push(path);
    }
    args.push("-");
    command::run(scanner, &args, Some(file), cancel, timeout)
}
pub(super) fn scan(path: &Path, cancel: &AtomicBool) -> Result<(String, Vec<Finding>), String> {
    let scanner = installed(CLAM).ok_or("ClamAV assente. Installa il motore e inizializza le firme con freshclam prima della scansione.")?;
    let _local = policy::LocalOnly::enter().map_err(|e| e.to_string())?;
    if !permitted(path) {
        return Err(
            "Seleziona un percorso locale consentito nella cartella utente o su un volume.".into(),
        );
    }
    if path.symlink_metadata().map_err(|e| e.to_string())?.is_dir() {
        scan_tree(path, cancel, |file, seconds| {
            invoke(scanner, file, cancel, seconds)
        })
    } else {
        let input = local_handle(path, false)?;
        let finding = Finding::capture(path, &input)?;
        let result = invoke(scanner, input, cancel, 180)?;
        let findings = if result.code == Some(1) {
            vec![finding]
        } else {
            Vec::new()
        };
        Ok((
            format!(
                "{}\n{}\n\n{}",
                path.display(),
                verdict(&result),
                result.text
            ),
            findings,
        ))
    }
}

fn scan_tree(
    root: &Path,
    cancel: &AtomicBool,
    mut inspect: impl FnMut(File, u64) -> Result<command::Output, String>,
) -> Result<(String, Vec<Finding>), String> {
    let deadline = Instant::now() + Duration::from_secs(900);
    let mut pending = vec![root.to_owned()];
    let (mut entries, mut scanned, mut skipped, mut findings) = (0, 0, 0, 0);
    let mut details = String::new();
    let mut flagged = Vec::new();
    let mut reason = None;
    'tree: while let Some(path) = pending.pop() {
        if cancel.load(Ordering::Acquire) || Instant::now() >= deadline {
            reason = Some("Interruzione o tempo massimo raggiunto.".to_owned());
            break;
        }
        let verified =
            match local_handle(&path, true).and_then(|f| f.metadata().map_err(|e| e.to_string())) {
                Ok(meta) => meta,
                Err(e) if path == root => return Err(e),
                Err(_) => {
                    skipped += 1;
                    continue;
                }
            };
        let mut dir = match policy::children(&path) {
            Ok(d) => d,
            Err(_) => {
                skipped += 1;
                continue;
            }
        };
        if dir.node().ok() != Some((verified.dev(), verified.ino())) {
            skipped += 1;
            continue;
        }
        while let Some(entry) = dir.next() {
            if entries >= MAX_ENTRIES
                || scanned >= MAX_FILES
                || Instant::now() >= deadline
                || cancel.load(Ordering::Acquire)
            {
                reason = Some("Annullamento o limite di scansione raggiunto.".into());
                break 'tree;
            }
            entries += 1;
            let Ok(name) = entry else {
                skipped += 1;
                break;
            };
            let item: PathBuf = path.join(&name);
            if !permitted(&item) {
                skipped += 1;
                continue;
            }
            let Ok(stat) = dir.stat(&name) else {
                skipped += 1;
                continue;
            };
            match stat.st_mode & libc::S_IFMT {
                libc::S_IFDIR => pending.push(item),
                libc::S_IFREG => {
                    let Ok(file) = local_handle(&item, false) else {
                        skipped += 1;
                        continue;
                    };
                    let Ok(candidate) = Finding::capture(&item, &file) else {
                        skipped += 1;
                        continue;
                    };
                    if candidate.bytes == 0 {
                        skipped += 1;
                        continue;
                    }
                    let remaining = deadline
                        .saturating_duration_since(Instant::now())
                        .as_secs()
                        .min(180);
                    let result = match inspect(file, remaining) {
                        Ok(r) => r,
                        Err(e) => {
                            reason = Some(e);
                            break 'tree;
                        }
                    };
                    match result.code {
                        Some(0) if scanned_clean(&result) => scanned += 1,
                        Some(1) => {
                            scanned += 1;
                            findings += 1;
                            flagged.push(candidate);
                            if details.len() < 80_000 {
                                let detection = result
                                    .text
                                    .lines()
                                    .find(|s| s.ends_with(" FOUND"))
                                    .unwrap_or("Firma o limite segnalato.");
                                details.push_str(&format!(
                                    "\n{}\n{}\n",
                                    item.display(),
                                    detection.chars().take(320).collect::<String>()
                                ));
                            }
                        }
                        _ => {
                            reason = Some(format!(
                                "ClamAV non ha completato {}.\n{}",
                                item.display(),
                                result.text.chars().take(1200).collect::<String>()
                            ));
                            break 'tree;
                        }
                    }
                }
                _ => skipped += 1,
            }
        }
    }
    let state = reason.unwrap_or_else(|| {
        if skipped > 0 {
            "Analisi parziale: alcune voci sono state escluse o risultavano illeggibili.".into()
        } else {
            "Scansione completata.".into()
        }
    });
    Ok((format!("{}\n{state}\nVoci visitate: {entries}\nFile analizzati: {scanned}\nSegnalazioni: {findings}\nVoci escluse o illeggibili: {skipped}\n{details}",root.display()),flagged))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn folder_uses_validated_handles_and_excludes_private_paths() {
        if let Some(home) = std::env::var_os("LINDO_SYSTEM_SCAN_FIXTURE") {
            let root = PathBuf::from(home).join("sample");
            let _local = policy::LocalOnly::enter().unwrap();
            let (result, findings) = scan_tree(&root, &AtomicBool::new(false), |mut f, _| {
                use std::io::Read;
                let mut content = String::new();
                f.read_to_string(&mut content).unwrap();
                let (code, line) = match content.as_str() {
                    "clean" => (0, "Scanned files: 1"),
                    "detected" => (1, "stdin: Fixture-Signature FOUND"),
                    _ => panic!("unexpected file"),
                };
                Ok(command::Output {
                    code: Some(code),
                    text: line.into(),
                    truncated: false,
                })
            })
            .unwrap();
            assert_eq!(findings.len(), 1);
            let culprit = &findings[0];
            culprit
                .validate(&local_handle(&culprit.path, false).unwrap())
                .unwrap();
            std::fs::write(&culprit.path, b"replaced").unwrap();
            assert!(culprit
                .validate(&local_handle(&culprit.path, false).unwrap())
                .is_err());
            assert!(result.contains("File analizzati: 2"), "{result}");
            assert!(result.contains("Segnalazioni: 1"), "{result}");
            assert!(result.contains("Fixture-Signature FOUND"));
            return;
        }
        let home = std::env::temp_dir().canonicalize().unwrap().join(format!(
            "lindo-clam-fixture-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let root = home.join("sample");
        for (folder, name, content) in [
            ("", "one", "clean"),
            ("child", "two", "detected"),
            ("_3_Vault", "ignore", "private"),
            ("pack.app", "ignore", "package"),
        ] {
            std::fs::create_dir_all(root.join(folder)).unwrap();
            std::fs::write(root.join(folder).join(name), content).unwrap();
        }
        std::os::unix::fs::symlink(root.join("_3_Vault/ignore"), root.join("link")).unwrap();
        let output=std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact","system_tools::security_scan::tests::folder_uses_validated_handles_and_excludes_private_paths","--nocapture"])
            .env("LINDO_FIXTURE_WORKER",&home).env("LINDO_SYSTEM_SCAN_FIXTURE",&home).output().unwrap();
        std::fs::remove_dir_all(home).unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[cfg(test)]
mod real_engine_tests {
    use super::*;
    #[test]
    #[ignore = "Moves only a newly created synthetic file through Finder Trash and restores the exact reported destination"]
    fn real_detected_fixture_trash_round_trip() {
        let base = PathBuf::from(std::env::var_os("LINDO_FIXTURE_WORKER").unwrap());
        assert!(base.join(".lindo-synthetic-qa").is_file());
        let scratch = tempfile::Builder::new()
            .prefix("lindo-clam-roundtrip-")
            .tempdir_in(&base)
            .unwrap();
        let path = scratch.path().join("synthetic-signature.txt");
        let bytes = b"Lindo benign synthetic test signature sample. Nothing executable.";
        std::fs::write(&path, bytes).unwrap();
        let cancel = AtomicBool::new(false);
        let (_, findings) = scan(&path, &cancel).unwrap();
        assert_eq!(findings.len(), 1);
        let target = trash_detected(&findings[0], &cancel)
            .unwrap()
            .expect("macOS must return an exact restoration path");
        assert!(!path.exists());
        assert_eq!(std::fs::read(&target).unwrap(), bytes);
        std::fs::rename(&target, &path).unwrap();
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }
    #[test]
    #[ignore = "Requires an explicitly prepared isolated fixture home with ClamAV signatures"]
    fn real_engine_scans_synthetic_clean_and_matching_files() {
        let home = PathBuf::from(std::env::var_os("LINDO_FIXTURE_WORKER").unwrap());
        let sample = home.join("fixture-files");
        assert!(home.join(".lindo-synthetic-qa").is_file());
        let flag = AtomicBool::new(false);
        let (summary, findings) = scan(&sample.join("clean.txt"), &flag).unwrap();
        assert!(summary.contains("Nessuna firma rilevata"), "{summary}");
        assert!(findings.is_empty());
        let (summary, findings) = scan(&sample.join("match.txt"), &flag).unwrap();
        assert!(summary.contains("Lindo-Fixture-Test"), "{summary}");
        assert_eq!(findings.len(), 1);
        let (summary, findings) = scan(&sample, &flag).unwrap();
        assert!(summary.contains("File analizzati: 2"), "{summary}");
        assert_eq!(findings.len(), 1);
    }
}
