use super::command;
use std::sync::atomic::AtomicBool;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Mutation {
    FlushDns,
    Spotlight,
    EmptyTrash,
    Detected(super::security_scan::Finding),
    Snapshot(String),
    Brew { cask: bool, name: String },
}

impl Mutation {
    pub(super) fn label(&self) -> String {
        match self {
            Self::Brew { name, .. } => format!("Aggiorna {name}"),
            Self::FlushDns => "Svuota cache DNS".into(),
            Self::Spotlight => "Ricostruisci indice Spotlight".into(),
            Self::Detected(f) => format!("Sposta nel Cestino {}", f.path.display()),
            Self::EmptyTrash => "Svuota definitivamente il Cestino".into(),
            Self::Snapshot(date) => format!("Elimina snapshot del {date}"),
        }
    }
    pub(super) fn explanation(&self) -> &'static str {
        match self {
            Self::Brew { .. } => "Homebrew scaricherà e installerà il pacchetto selezionato e le dipendenze necessarie. Chiudi l’app prima di procedere. Se interrompi l’operazione, ricontrolla lo stato del pacchetto.",
            Self::FlushDns => "macOS richiederà l’autorizzazione amministratore per svuotare la cache DNS e ricaricare il resolver. Le prossime richieste ripeteranno la risoluzione dei nomi.",
            Self::Spotlight => "macOS richiederà l’autorizzazione per ricostruire l’indice del disco di avvio. I risultati di ricerca saranno temporaneamente incompleti. L’indicizzazione prosegue in macOS.",
            Self::Detected(_) => "Lindo ricontrollerà l’identità del file e ripeterà la scansione. Se ClamAV conferma la rilevazione, il singolo file verrà spostato nel Cestino di macOS.",
            Self::EmptyTrash => "Finder eliminerà definitivamente tutti gli elementi nel Cestino del tuo utente, inclusi quelli sui volumi collegati. L’operazione è irreversibile.",
            Self::Snapshot(_) => "Time Machine eliminerà lo snapshot con questa data da tutti i volumi montati in cui è presente. Perderai quel punto di ripristino locale. macOS richiederà l’autorizzazione.",
        }
    }
}

pub(super) fn valid_package(name: &str) -> bool {
    let parts: Vec<_> = name.split('/').collect();
    name.len() <= 128
        && matches!(parts.len(), 1 | 3)
        && parts.iter().all(|part| {
            part.as_bytes()
                .first()
                .is_some_and(u8::is_ascii_alphanumeric)
                && part.bytes().all(|b| {
                    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'+' | b'@')
                })
        })
}

fn valid_date(s: &str) -> bool {
    s.len() == 17
        && s.bytes().enumerate().all(|(i, b)| {
            if matches!(i, 4 | 7 | 10) {
                b == b'-'
            } else {
                b.is_ascii_digit()
            }
        })
}
pub(super) fn snapshots(list: &str) -> Vec<String> {
    list.lines()
        .filter_map(|line| {
            let s = line.trim().strip_prefix("com.apple.TimeMachine.")?;
            let date = s.strip_suffix(".local").unwrap_or(s);
            valid_date(date).then(|| date.to_owned())
        })
        .take(256)
        .collect()
}

pub(super) fn apply(
    action: Mutation,
    mut run: impl FnMut(&str, &[&str], u64) -> Result<command::Output, String>,
) -> Result<String, String> {
    let (script, success) = match action {
        Mutation::Detected(_) => return Err("Scansione di conferma richiesta.".into()),
        Mutation::Brew { cask, name } => {
            let brew = super::installed(super::BREW).ok_or("Homebrew non disponibile.")?;
            if !valid_package(&name) { return Err("Pacchetto non valido.".into()); }
            let kind = if cask { "--cask" } else { "--formula" };
            let list = run(brew,&["outdated",kind,"--quiet"],60)?;
            if list.code!=Some(0) || list.truncated || !list.text.lines().any(|line| line.trim()==name) {
                return Err("Pacchetto non più presente nell’elenco aggiornabile. Ripeti il controllo.".into());
            }
            let output = run(brew,&["upgrade",kind,&name],900)
                .map_err(|e| format!("{e} Verifica lo stato in Homebrew prima di ripetere."))?;
            if output.code!=Some(0) { return Err(format!("Aggiornamento incompleto.\n{}",output.text)); }
            return Ok(format!("Homebrew ha completato l’aggiornamento di {name}.\n{}",output.text));
        }
        Mutation::FlushDns => (
            r#"do shell script "/usr/bin/dscacheutil -flushcache && /usr/bin/killall -HUP mDNSResponder" with administrator privileges"#.into(),
            "Cache DNS svuotata e resolver ricaricato."
        ),
        Mutation::Spotlight => (
            r#"do shell script "/usr/bin/mdutil -E /" with administrator privileges"#.into(),
            "macOS ha accettato la richiesta di ricostruzione dell’indice."
        ),
        Mutation::EmptyTrash => (
            "tell application \"Finder\"\nempty trash\nreturn (count of items of trash) as string\nend tell".into(),
            "Finder ha verificato che il Cestino è vuoto."
        ),
        Mutation::Snapshot(ref date) => {
            if !valid_date(date) { return Err("Identificativo snapshot non valido.".into()); }
            let list = run("/usr/bin/tmutil",&["listlocalsnapshots","/"],15)?;
            if list.code != Some(0) || list.truncated || !snapshots(&list.text).contains(date) {
                return Err("Lo snapshot non è più verificabile. Aggiorna l’elenco.".into());
            }
            (format!(r#"do shell script "/usr/bin/tmutil deletelocalsnapshots {date}" with administrator privileges"#),
             "Time Machine ha eliminato lo snapshot selezionato.")
        }
    };
    let output = run("/usr/bin/osascript", &["-e", &script], 180)
        .map_err(|e| format!("{e} Verifica lo stato in macOS prima di ripetere."))?;
    if output.code != Some(0) {
        return Err(format!(
            "macOS non ha confermato il completamento.\n{}",
            output.text
        ));
    }
    if action == Mutation::EmptyTrash {
        return match output.text.trim().parse::<usize>() {
            Ok(0) => Ok(success.into()),
            Ok(n) => Ok(format!(
                "Finder ha ricevuto la richiesta. Elementi ancora presenti al controllo: {n}."
            )),
            Err(_) => Err(
                "Richiesta inviata. Il numero di elementi rimasti nel Cestino non è verificabile."
                    .into(),
            ),
        };
    }
    Ok(format!("{success}\n{}", output.text))
}

pub(super) fn execute(action: Mutation, cancel: &AtomicBool) -> Result<String, String> {
    if let Mutation::Detected(finding) = action {
        super::security_scan::trash_detected(&finding, cancel)?;
        return Ok(format!(
            "Rilevazione riconfermata. File spostato nel Cestino.\n{}",
            finding.path.display()
        ));
    }
    apply(action, |exe, args, time| {
        command::run(exe, args, None, cancel, time)
    })
}

pub(super) fn verify_volume(
    volume: &str,
    mut run: impl FnMut(&str, &[&str], u64) -> Result<command::Output, String>,
) -> Result<String, String> {
    let result = run("/usr/sbin/diskutil", &["verifyVolume", volume], 600)?;
    let status = if result.code == Some(0) {
        "Verifica del filesystem completata."
    } else {
        "macOS ha segnalato un errore o non ha potuto completare la verifica. Consulta il dettaglio e Utility Disco."
    };
    let limit = if result.truncated {
        "\nRapporto parziale per limite di visualizzazione."
    } else {
        ""
    };
    Ok(format!("{status}\n{}{limit}", result.text))
}

pub(super) fn periodic_status(binary: bool, scripts: bool) -> &'static str {
    match (binary,scripts) {
        (true,true)=>"Questo Mac include il comando periodic e la cartella degli script /etc/periodic. La loro presenza non determina quando sono stati eseguiti.",
        (false,false)=>"Questa installazione di macOS non include il comando periodic né la cartella /etc/periodic per i tradizionali script giornalieri, settimanali e mensili.",
        _=>"Installazione incompleta degli strumenti periodici tradizionali. Manca /usr/sbin/periodic oppure /etc/periodic.",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn output(code: i32, text: &str) -> command::Output {
        command::Output {
            code: Some(code),
            text: text.into(),
            truncated: false,
        }
    }
    #[test]
    fn filesystem_check_only_invokes_the_requested_readonly_verb() {
        for code in [0, 8] {
            let report = verify_volume("/", |exe, args, timeout| {
                assert_eq!(exe, "/usr/sbin/diskutil");
                assert_eq!(args, ["verifyVolume", "/"]);
                assert_eq!(timeout, 600);
                Ok(output(code, "fixture check"))
            })
            .unwrap();
            assert!(report.contains("fixture check"));
            assert_eq!(report.contains("completata."), code == 0);
        }
        assert!(periodic_status(false, false).contains("non include"));
        assert!(periodic_status(true, true).contains("include il comando"));
        assert!(periodic_status(true, false).contains("incompleta"));
    }
    #[test]
    fn rejects_stale_or_untrusted_snapshot_inputs_before_elevation() {
        let snapshot = "2026-11-24-093500";
        let mut calls = 0;
        assert!(apply(Mutation::Snapshot(";bad".into()), |_, _, _| panic!()).is_err());
        assert!(apply(Mutation::Snapshot(snapshot.into()), |_, _, _| {
            calls += 1;
            Ok(output(0, ""))
        })
        .is_err());
        assert_eq!(calls, 1);
        let result = apply(Mutation::Snapshot(snapshot.into()), |exe, args, _| {
            if exe.ends_with("tmutil") {
                return Ok(output(
                    0,
                    &format!("com.apple.TimeMachine.{snapshot}.local"),
                ));
            }
            assert_eq!(exe, "/usr/bin/osascript");
            assert!(args[1].contains("deletelocalsnapshots 2026-11-24-093500"));
            Ok(output(0, ""))
        });
        assert!(result.is_ok());
    }
    #[test]
    fn brew_checks_inventory_and_passes_one_package_as_a_literal_argument() {
        assert!(valid_package("pkg-config"));
        assert!(valid_package("vendor/tap/tool@3"));
        for invalid in ["-f", "pkg;id", "../x", "foo bar", "vendor//tool"] {
            assert!(!valid_package(invalid));
        }
        if super::super::installed(super::super::BREW).is_none() {
            return;
        }
        let action = Mutation::Brew {
            cask: true,
            name: "example-app".into(),
        };
        assert!(apply(action.clone(), |_, args, _| {
            assert_eq!(args[0], "outdated");
            Ok(output(0, "different-app"))
        })
        .is_err());
        let mut calls = 0;
        assert!(apply(action, |_, args, _| {
            calls += 1;
            if calls == 1 {
                assert_eq!(args, ["outdated", "--cask", "--quiet"]);
                Ok(output(0, "example-app"))
            } else {
                assert_eq!(args, ["upgrade", "--cask", "example-app"]);
                Ok(output(0, "done"))
            }
        })
        .is_ok());
        assert_eq!(calls, 2);
    }
    #[test]
    fn trash_outcome_uses_finder_count_and_command_status() {
        for (code, count, expected) in [
            (0, "0", "è vuoto"),
            (0, "3", "presenti"),
            (1, "denied", "non ha confermato"),
            (0, "", "non è verificabile"),
        ] {
            let result = apply(Mutation::EmptyTrash, |exe, args, _| {
                assert_eq!(exe, "/usr/bin/osascript");
                assert!(args[1].contains("empty trash"));
                Ok(output(code, count))
            })
            .unwrap_or_else(|e| e);
            assert!(result.contains(expected));
        }
    }
}
