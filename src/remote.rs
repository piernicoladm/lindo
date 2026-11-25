use crate::{model::Mode, policy, system_tools::command};
use eframe::egui;
use serde_json::Value;
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
        Arc,
    },
    time::Duration,
};

#[derive(Clone, Debug, PartialEq, Eq)]
struct Account {
    name: String,
    kind: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct Entry {
    name: String,
    id: String,
    modified: String,
    bytes: i64,
    directory: bool,
}
impl Entry {
    fn identified(&self) -> bool {
        !self.id.trim().is_empty()
            && (self.directory
                || (self.bytes >= 0
                    && !self.modified.is_empty()
                    && !self.modified.starts_with("0001-")
                    && !self.modified.starts_with("2000-01-01T00:00:00")))
    }
    fn same(&self, other: &Self) -> bool {
        self.identified()
            && other.identified()
            && self.name == other.name
            && self.id == other.id
            && self.directory == other.directory
            && (self.directory || (self.modified == other.modified && self.bytes == other.bytes))
    }
}
fn safe_name(s: &str) -> bool {
    !s.is_empty()
        && s.trim() == s
        && !s.starts_with('.')
        && !s.contains([':', '/', '\\'])
        && !s
            .chars()
            .any(|c| c.is_control() || matches!(c,'\u{202a}'..='\u{202e}'|'\u{2066}'..='\u{2069}'))
}
fn safe_path(path: &str) -> bool {
    path.is_empty()
        || (path.split('/').all(safe_name)
            && !policy::protected(Path::new(path), Mode::Space, None)
            && !path.split('/').any(|p| p.eq_ignore_ascii_case("_3_Vault")))
}
fn account_name(s: &str) -> bool {
    safe_name(s)
        && !s.starts_with('-')
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b' ' | b'_' | b'-' | b'.'))
}
fn parse_accounts(v: Value) -> Result<Vec<Account>, String> {
    let list = v.as_array().ok_or("Formato account rclone sconosciuto.")?;
    let mut accounts = Vec::new();
    for item in list {
        let (Some(name), Some(kind)) = (item["name"].as_str(), item["type"].as_str()) else {
            continue;
        };
        if account_name(name) && matches!(kind, "drive" | "dropbox" | "onedrive") {
            accounts.push(Account {
                name: name.into(),
                kind: kind.into(),
            });
        }
    }
    accounts.sort_by_key(|a| a.name.to_lowercase());
    if accounts
        .windows(2)
        .any(|v| v[0].name.eq_ignore_ascii_case(&v[1].name))
    {
        return Err("Nomi account ambigui nella configurazione rclone.".into());
    }
    Ok(accounts)
}
fn binary() -> Result<String, String> {
    let mut candidates = vec![
        PathBuf::from("/opt/homebrew/bin/rclone"),
        PathBuf::from("/usr/local/bin/rclone"),
    ];
    if let Some(parent) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_owned))
    {
        candidates.insert(0, parent.join("rclone"));
    }
    candidates.into_iter().find(|p|p.is_file()).and_then(|p|p.to_str().map(String::from))
        .ok_or_else(||"Installa rclone e configura un account Dropbox, Google Drive o OneDrive con rclone config.".into())
}
type Call<'a> = dyn Fn(&[String]) -> Result<String, String> + 'a;
fn execute(args: &[String], stop: &AtomicBool) -> Result<String, String> {
    let mut argv: Vec<&str> = args.iter().map(String::as_str).collect();
    argv.extend([
        "--retries",
        "1",
        "--low-level-retries",
        "1",
        "--log-level",
        "ERROR",
    ]);
    let result = command::run_stdout(&binary()?, &argv, None, stop, 90)?;
    if result.code != Some(0) {
        return Err(format!(
            "rclone non ha completato {} (codice {}). Verifica la connessione e l’account.",
            args[0],
            result.code.unwrap_or(-1)
        ));
    }
    if result.truncated {
        return Err("Elenco troppo ampio. Inserisci un prefisso più specifico e aggiorna.".into());
    }
    Ok(result.text)
}
fn json(call: &Call<'_>, args: &[String]) -> Result<Value, String> {
    serde_json::from_str(&call(args)?)
        .map_err(|_| "Risposta JSON rclone incompleta. Verifica la versione di rclone.".into())
}
fn accounts(call: &Call<'_>) -> Result<Vec<Account>, String> {
    parse_accounts(json(call, &["listremotes".into(), "--json".into()])?)
}
fn join(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_owned()
    } else {
        format!("{parent}/{name}")
    }
}
fn target(account: &Account, path: &str) -> Result<String, String> {
    if !account_name(&account.name) || !safe_path(path) {
        return Err("Percorso remoto protetto o non valido.".into());
    }
    Ok(format!("{}:{path}", account.name))
}
fn options(account: &Account, args: &mut Vec<String>) {
    if account.kind == "drive" {
        args.extend(["--drive-skip-shortcuts".into(), "--drive-skip-gdocs".into()]);
    }
    if account.kind == "dropbox" {
        args.push("--dropbox-skip-exports".into());
    }
}
fn literal(s: &str) -> String {
    let mut escaped = String::new();
    for c in s.chars() {
        if "*?[]{}\\".contains(c) {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    escaped
}
fn parse_entry(v: &Value) -> Option<Entry> {
    let name = v["Name"].as_str()?;
    if !safe_name(name)
        || v["OrigID"]
            .as_str()
            .is_some_and(|id| !id.is_empty() && Some(id) != v["ID"].as_str())
    {
        return None;
    }
    Some(Entry {
        name: name.into(),
        id: v["ID"].as_str().unwrap_or("").into(),
        modified: v["ModTime"].as_str().unwrap_or("").into(),
        bytes: v["Size"].as_i64().unwrap_or(-1),
        directory: v["IsDir"].as_bool()?,
    })
}
fn list(
    call: &Call<'_>,
    account: &Account,
    parent: &str,
    filter: &str,
    exact: bool,
    min: u64,
) -> Result<Vec<Entry>, String> {
    let mut args = vec![
        "lsjson".into(),
        target(account, parent)?,
        "--max-depth".into(),
        "1".into(),
        "--original".into(),
    ];
    if !filter.is_empty() {
        if !safe_name(filter) {
            return Err("Usa un prefisso di nome senza separatori.".into());
        }
        let suffix = if exact { "" } else { "*" };
        let pattern = format!("/{}{suffix}", literal(filter));
        args.extend([
            "--ignore-case".into(),
            "--filter".into(),
            format!("+ {pattern}/"),
            "--filter".into(),
            format!("+ {pattern}"),
            "--filter".into(),
            "- /**".into(),
        ]);
    }
    if min > 0 {
        args.extend(["--min-size".into(), format!("{min}B")]);
    }
    options(account, &mut args);
    let response = json(call, &args)?;
    let mut rows = Vec::new();
    let folded_filter = filter.to_lowercase();
    for item in response.as_array().ok_or("Elenco remoto incompleto.")? {
        if item["Path"] != item["Name"] {
            return Err("Il provider ha restituito un percorso inatteso.".into());
        }
        let Some(name) = item["Name"].as_str() else {
            if exact {
                return Err("Nome remoto non verificabile.".into());
            }
            continue;
        };
        let folded_name = name.to_lowercase();
        let matches = if exact {
            folded_name == folded_filter
        } else {
            folded_name.starts_with(&folded_filter)
        };
        if !matches {
            continue;
        }
        let entry = parse_entry(item).filter(|row| safe_path(&join(parent, &row.name)));
        if exact
            && (entry.as_ref().is_none_or(|row| !row.identified())
                || item.get("OrigID").is_some_and(|id| !id.is_string()))
        {
            return Err("Il provider ha restituito un’identità incompleta o ambigua.".into());
        }
        if let Some(row) = entry {
            rows.push(row);
        }
    }
    rows.sort_by(|a, b| {
        b.directory
            .cmp(&a.directory)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(rows)
}
fn unique(
    call: &Call<'_>,
    account: &Account,
    parent: &str,
    expected: &Entry,
) -> Result<(), String> {
    let rows = list(call, account, parent, &expected.name, true, 0)?;
    if rows.len() != 1 || !rows[0].same(expected) {
        return Err("Nome ambiguo, identità o versione cambiata. Aggiorna la cartella.".into());
    }
    Ok(())
}
fn verify_chain(call: &Call<'_>, account: &Account, trail: &[Entry]) -> Result<String, String> {
    let mut path = String::new();
    for folder in trail {
        if !folder.directory {
            return Err("Percorso cartella non valido.".into());
        }
        unique(call, account, &path, folder)?;
        path = join(&path, &folder.name);
    }
    Ok(path)
}
fn quota(call: &Call<'_>, a: &Account) -> String {
    let Ok(v) = json(
        call,
        &["about".into(), format!("{}:", a.name), "--json".into()],
    ) else {
        return "Quota non disponibile per questo account".into();
    };
    ["used", "free", "total"]
        .iter()
        .zip(["usati", "liberi", "totali"])
        .filter_map(|(key, label)| {
            v[*key]
                .as_u64()
                .map(|n| format!("{:.1} GiB {label}", n as f64 / 1073741824.))
        })
        .collect::<Vec<_>>()
        .join(" · ")
}
fn review_name() -> String {
    let mut nonce = [0u8; 16];
    unsafe { libc::arc4random_buf(nonce.as_mut_ptr().cast(), nonce.len()) };
    let suffix = nonce.iter().map(|b| format!("{b:02x}")).collect::<String>();
    format!("Lindo revisione {suffix}")
}
fn preflight(
    call: &Call<'_>,
    account: &Account,
    trail: &[Entry],
    selected: &[Entry],
) -> Result<String, String> {
    if selected.is_empty() || !accounts(call)?.contains(account) {
        return Err("Account o selezione cambiati.".into());
    }
    let folder = verify_chain(call, account, trail)?;
    let mut names = HashSet::new();
    for row in selected {
        if row.directory || !row.identified() || !names.insert(row.name.to_lowercase()) {
            return Err("Selezione ambigua.".into());
        }
        unique(call, account, &folder, row)?;
    }
    Ok(folder)
}
fn remove(
    call: &Call<'_>,
    account: &Account,
    trail: &[Entry],
    selected: &[Entry],
) -> Result<String, String> {
    let folder = preflight(call, account, trail, selected)
        .map_err(|e| format!("Operazione non avviata. {e}"))?;
    for (done, row) in selected.iter().enumerate() {
        let mut requested = false;
        let action = (|| {
            verify_chain(call, account, trail)?;
            let mut args = vec![
                "deletefile".into(),
                target(account, &join(&folder, &row.name))?,
            ];
            options(account, &mut args);
            match account.kind.as_str() {
                "drive" => args.push("--drive-use-trash=true".into()),
                "onedrive" => args.push("--onedrive-hard-delete=false".into()),
                "dropbox" => {}
                _ => return Err("Cestino non verificato per questo provider.".into()),
            }
            unique(call, account, &folder, row)?;
            requested = true;
            call(&args)?;
            if !list(call, account, &folder, &row.name, true, 0)?.is_empty() {
                return Err("Il nome è ancora presente nella cartella originale.".into());
            }
            verify_chain(call, account, trail)?;
            Ok::<(), String>(())
        })();
        if let Err(error) = action {
            let state = if requested {
                "Rimozione incerta"
            } else {
                "Rimozione non avviata"
            };
            return Err(format!(
                "{done} rimozioni verificate. {state} per «{}». Controlla il file nella cartella e tra i file eliminati prima di riprovare. {error}",
                row.name
            ));
        }
    }
    Ok(format!(
        "{} file rimossi dalla cartella sul cloud. Per recuperarli consulta i file eliminati del provider.",
        selected.len()
    ))
}
#[derive(Clone)]
enum Review {
    Move(String),
    Trash,
}
fn relocate(
    call: &Call<'_>,
    account: &Account,
    trail: &[Entry],
    selected: &[Entry],
    destination: &str,
) -> Result<String, String> {
    if !destination.starts_with("Lindo revisione ")
        || !safe_name(destination)
        || selected.is_empty()
    {
        return Err("Selezione o destinazione non valida.".into());
    }
    let folder = preflight(call, account, trail, selected)
        .map_err(|e| format!("Operazione non avviata. {e}"))?;
    if !list(call, account, "", destination, true, 0)
        .map_err(|e| format!("Spostamento non avviato. Destinazione non verificabile. {e}"))?
        .is_empty()
    {
        return Err("Spostamento non avviato. La cartella di destinazione esiste già.".into());
    }
    let mut mkdir = vec!["mkdir".into(), target(account, destination)?];
    options(account, &mut mkdir);
    call(&mkdir)
        .map_err(|e| format!("Creazione di «{destination}» incerta. Nessun file spostato. {e}"))?;
    let dirs = list(call, account, "", destination, true, 0).map_err(|e| {
        format!("Identità di «{destination}» non verificata. Nessun file spostato. {e}")
    })?;
    if dirs.len() != 1 || !dirs[0].directory || !dirs[0].identified() {
        return Err("Destinazione creata ma non identificabile. Verificala nel cloud.".into());
    }
    let mut done = 0;
    for row in selected {
        let mut requested = false;
        let action = (|| {
            verify_chain(call, account, trail)?;
            unique(call, account, "", &dirs[0])?;
            if !list(call, account, destination, &row.name, true, 0)?.is_empty() {
                return Err("Il nome di destinazione è occupato.".into());
            }
            unique(call, account, &folder, row)?;
            let mut args = vec![
                "moveto".into(),
                target(account, &join(&folder, &row.name))?,
                target(account, &join(destination, &row.name))?,
                "--immutable".into(),
                "--ignore-existing".into(),
            ];
            options(account, &mut args);
            requested = true;
            call(&args)?;
            unique(call, account, destination, row)?;
            if !list(call, account, &folder, &row.name, true, 0)?.is_empty() {
                return Err("La verifica della posizione originale è incompleta.".into());
            }
            verify_chain(call, account, trail)?;
            unique(call, account, "", &dirs[0])?;
            Ok::<(), String>(())
        })();
        if let Err(e) = action {
            let state = if requested {
                "Spostamento incerto"
            } else {
                "Spostamento non avviato"
            };
            return Err(format!(
                "{done} file verificati in {destination}. {state} per «{}». Verifica la posizione prima di riprovare. {e}",
                row.name
            ));
        }
        done += 1;
    }
    Ok(format!(
        "{done} file spostati e verificati in {}:{destination}.",
        account.name
    ))
}

enum Outcome {
    Accounts(Vec<Account>),
    Listing(Vec<Entry>, String),
    Mutation(Result<String, String>),
}
struct Job {
    stop: Arc<AtomicBool>,
    rx: Receiver<Result<Outcome, String>>,
}
#[derive(Default)]
pub(crate) struct Remote {
    accounts: Vec<Account>,
    active: Option<Account>,
    trail: Vec<Entry>,
    rows: Vec<Entry>,
    selected: HashSet<String>,
    prefix: String,
    min_mib: u64,
    quota: String,
    status: String,
    review: Option<Review>,
    job: Option<Job>,
    mutation_pending: bool,
    mutation_notice: String,
}
impl Drop for Remote {
    fn drop(&mut self) {
        if let Some(job) = &self.job {
            job.stop.store(true, Ordering::Relaxed);
        }
    }
}
impl Remote {
    fn start(
        &mut self,
        ctx: &egui::Context,
        task: impl FnOnce(&Call<'_>) -> Result<Outcome, String> + Send + 'static,
    ) {
        let (tx, rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        self.job = Some(Job {
            stop: stop.clone(),
            rx,
        });
        self.status = "Operazione in corso…".into();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let call = |args: &[String]| execute(args, &stop);
            let _ = tx.send(task(&call));
            ctx.request_repaint();
        });
    }
    fn load(&mut self, ctx: &egui::Context) {
        let Some(a) = self.active.clone() else { return };
        self.quota.clear();
        self.rows.clear();
        self.selected.clear();
        self.review = None;
        let trail = self.trail.clone();
        let prefix = self.prefix.clone();
        let min = self.min_mib.saturating_mul(1048576);
        self.start(ctx, move |call| {
            let p = verify_chain(call, &a, &trail)?;
            let rows = list(call, &a, &p, &prefix, false, min)?;
            Ok(Outcome::Listing(rows, quota(call, &a)))
        });
    }
    pub(crate) fn show(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.heading("Cloud remoti");
        ui.label("Esplora i file online usando gli account Dropbox, Google Drive e OneDrive configurati in rclone.");
        if let Some(job) = &self.job {
            ctx.request_repaint_after(Duration::from_millis(150));
            if ui.button("Interrompi operazione").clicked() {
                job.stop.store(true, Ordering::Relaxed)
            }
            match job.rx.try_recv() {
                Ok(result) => {
                    self.job = None;
                    match result {
                        Ok(Outcome::Accounts(list)) => {
                            self.status = format!("{} account disponibili", list.len());
                            self.accounts = list;
                        }
                        Ok(Outcome::Listing(list, quota)) => {
                            self.status = format!("{} elementi", list.len());
                            self.rows = list;
                            self.quota = quota;
                        }
                        Ok(Outcome::Mutation(result)) => {
                            self.mutation_pending = false;
                            self.mutation_notice = result.unwrap_or_else(|e| e);
                            self.load(ctx);
                        }
                        Err(e) => self.status = e,
                    }
                }
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.job = None;
                    if self.mutation_pending {
                        self.mutation_pending = false;
                        self.mutation_notice =
                            "Esito incerto. Verifica la posizione nel cloud.".into();
                        self.load(ctx);
                    } else {
                        self.status = "Consultazione interrotta.".into();
                    }
                }
            }
        }
        if !self.mutation_notice.is_empty() {
            ui.label(&self.mutation_notice);
        }
        ui.label(&self.status);
        if self.job.is_some() {
            return;
        }
        if let Some(review) = self.review.clone() {
            let Some(a) = &self.active else { return };
            let folder = self
                .trail
                .iter()
                .map(|e| e.name.as_str())
                .collect::<Vec<_>>()
                .join("/");
            let confirm = match &review {
                Review::Move(dest) => {
                    ui.heading("Rivedi lo spostamento");
                    ui.label(format!("Da {}:{folder} a {}:{dest}", a.name, a.name));
                    ui.label("La cartella di revisione occupa spazio nello stesso account. Per ripristinare i file spostali nella posizione originale.");
                    "Conferma spostamento nel cloud"
                }
                Review::Trash => {
                    ui.heading("Rivedi la rimozione dal cloud");
                    ui.label(format!("Account {} · cartella /{folder}", a.name));
                    if a.kind == "dropbox" {
                        ui.label("I file passano ai file eliminati di Dropbox. La possibilità di recupero dipende dal piano, dai permessi e dalle regole di conservazione.");
                    } else {
                        ui.label("I file passano al cestino del provider. Il recupero e i tempi di aggiornamento della quota dipendono dall’account.");
                    }
                    ui.label("La rimozione si sincronizza sugli altri dispositivi e può togliere l’accesso ai collaboratori.");
                    "Conferma rimozione dal cloud"
                }
            };
            let items = self
                .rows
                .iter()
                .filter(|r| self.selected.contains(&r.id))
                .cloned()
                .collect::<Vec<_>>();
            if ui.button(confirm).clicked() {
                self.review = None;
                if let Some(a) = self.active.clone() {
                    let trail = self.trail.clone();
                    self.rows.clear();
                    self.selected.clear();
                    self.mutation_pending = true;
                    self.mutation_notice.clear();
                    self.start(ctx, move |call| {
                        Ok(Outcome::Mutation(match review {
                            Review::Move(dest) => relocate(call, &a, &trail, &items, &dest),
                            Review::Trash => remove(call, &a, &trail, &items),
                        }))
                    });
                    return;
                }
            } else if ui.button("Indietro").clicked() {
                self.review = None;
            }
            if self.review.is_some() {
                egui::ScrollArea::vertical().max_height(180.).show_rows(
                    ui,
                    22.,
                    items.len(),
                    |ui, range| {
                        for row in &items[range] {
                            ui.label(format!("{} · {} B", row.name, row.bytes));
                        }
                    },
                );
            }
            return;
        }
        if ui.button("Aggiorna account rclone").clicked() {
            self.mutation_notice.clear();
            self.active = None;
            self.trail.clear();
            self.rows.clear();
            self.quota.clear();
            self.selected.clear();
            self.accounts.clear();
            self.start(ctx, |call| accounts(call).map(Outcome::Accounts));
            return;
        }
        let mut account = None;
        ui.horizontal_wrapped(|ui| {
            for a in &self.accounts {
                if ui
                    .selectable_label(
                        self.active.as_ref() == Some(a),
                        format!("{} ({})", a.name, a.kind),
                    )
                    .clicked()
                {
                    account = Some(a.clone());
                }
            }
        });
        if let Some(a) = account {
            self.mutation_notice.clear();
            self.active = Some(a);
            self.trail.clear();
            self.prefix.clear();
            self.quota.clear();
            self.load(ctx);
            return;
        }
        let Some(a) = &self.active else { return };
        ui.label(format!(
            "{}:{} · {}",
            a.name,
            self.trail
                .iter()
                .map(|e| e.name.as_str())
                .collect::<Vec<_>>()
                .join("/"),
            self.quota
        ));
        ui.horizontal_wrapped(|ui| {
            ui.label("Prefisso");
            ui.add(egui::TextEdit::singleline(&mut self.prefix).desired_width(115.));
            ui.label("Minimo MiB");
            ui.add(egui::DragValue::new(&mut self.min_mib).range(0..=1_000_000_000));
            if ui.button("Aggiorna cartella").clicked() {
                self.load(ctx);
            }
            if ui
                .add_enabled(
                    !self.trail.is_empty(),
                    egui::Button::new("Cartella superiore"),
                )
                .clicked()
            {
                self.trail.pop();
                self.prefix.clear();
                self.load(ctx);
            }
        });
        if self.job.is_some() {
            return;
        }
        if !self.selected.is_empty() {
            if ui
                .button(format!("Sposta in revisione ({})", self.selected.len()))
                .clicked()
            {
                self.review = Some(Review::Move(review_name()));
            }
            if ui
                .button(format!("Rimuovi dal cloud ({})", self.selected.len()))
                .clicked()
            {
                self.review = Some(Review::Trash);
            }
        }
        let mut enter = None;
        let mut names = HashSet::new();
        let mut ambiguous = HashSet::new();
        let mut ids = HashSet::new();
        let mut duplicate_ids = HashSet::new();
        for row in &self.rows {
            let name = row.name.to_lowercase();
            if !names.insert(name.clone()) {
                ambiguous.insert(name);
            }
            if !ids.insert(&row.id) {
                duplicate_ids.insert(&row.id);
            }
        }
        egui::ScrollArea::vertical().show_rows(ui, 24., self.rows.len(), |ui, range| {
            for row in &self.rows[range] {
                ui.horizontal(|ui| {
                    let usable = row.identified()
                        && !ambiguous.contains(&row.name.to_lowercase())
                        && !duplicate_ids.contains(&row.id);
                    if row.directory {
                        if ui
                            .add_enabled(usable, egui::Button::new(&row.name))
                            .clicked()
                        {
                            enter = Some(row.clone());
                        }
                    } else {
                        let mut yes = self.selected.contains(&row.id);
                        if ui
                            .add_enabled(usable, egui::Checkbox::new(&mut yes, &row.name))
                            .changed()
                        {
                            if yes {
                                self.selected.insert(row.id.clone());
                            } else {
                                self.selected.remove(&row.id);
                            }
                        }
                        ui.label(format!("{} B", row.bytes));
                    }
                    if !usable {
                        ui.weak("Identità o nome ambiguo");
                    }
                });
            }
        });
        if let Some(folder) = enter {
            self.trail.push(folder);
            self.prefix.clear();
            self.load(ctx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::MetadataExt, process::Command};

    fn item() -> Entry {
        Entry {
            name: "budget[1].csv".into(),
            id: "123".into(),
            bytes: 124,
            modified: "2026-01-01T01:00:00Z".into(),
            directory: false,
        }
    }
    #[test]
    fn denies_private_components_and_untrusted_remotes() {
        for name in [
            "_6_Archivio_Private/x",
            "a/Piernicola De Maria/file",
            "a/_2_Work/CLIENTS/x",
            "../a",
            "a/.ssh/file",
            "a:x",
            "a//x",
            "_3_Vault/x",
        ] {
            assert!(!safe_path(name), "{name}")
        }
        let parsed=parse_accounts(serde_json::json!([{"name":"Disk","type":"local"},{"name":"Drive","type":"drive"},{"name":"x,scope=foo","type":"drive"}])).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].name, "Drive");
        assert!(parse_accounts(
            serde_json::json!([{"name":"Drive","type":"drive"},{"name":"DRIVE","type":"onedrive"}])
        )
        .is_err());
    }
    #[test]
    fn changed_versions_and_duplicate_remote_names_block_before_any_move() {
        let a = Account {
            name: "fixture".into(),
            kind: "drive".into(),
        };
        let row = item();
        let fake = serde_json::json!({"Name":row.name,"Path":row.name,"Size":row.bytes,"ID":row.id,"ModTime":row.modified,"IsDir":false});
        for mut results in [vec![fake.clone(), fake.clone()], vec![fake.clone()]] {
            if results.len() == 1 {
                results[0]["ID"] = Value::from("replaced")
            }
            let call = |args: &[String]| match args[0].as_str() {
                "listremotes" => Ok(r#"[{"name":"fixture","type":"drive"}]"#.into()),
                "lsjson" => Ok(serde_json::to_string(&results).unwrap()),
                other => panic!("unexpected mutation {other}"),
            };
            assert!(relocate(&call, &a, &[], std::slice::from_ref(&row), &review_name()).is_err());
        }
    }

    #[test]
    fn malformed_matching_sibling_never_proves_uniqueness_or_absence() {
        let a = Account {
            name: "fixture".into(),
            kind: "drive".into(),
        };
        let row = item();
        let valid = serde_json::json!({"Name":row.name,"Path":row.name,"Size":row.bytes,"ID":row.id,"ModTime":row.modified,"IsDir":false});
        let mut shortcut = valid.clone();
        shortcut["OrigID"] = Value::from("other");
        for rows in [
            vec![shortcut.clone()],
            vec![valid, shortcut],
            vec![Value::Null],
        ] {
            let call = |_: &[String]| Ok(serde_json::json!(rows).to_string());
            assert!(list(&call, &a, "", &row.name, true, 0).is_err());
        }
    }

    #[test]
    fn interrupted_delete_is_uncertain_and_is_never_retried() {
        let a = Account {
            name: "fixture".into(),
            kind: "drive".into(),
        };
        let row = item();
        let sent = std::cell::Cell::new(0);
        let call = |args: &[String]| {
            match args[0].as_str() {
            "listremotes" => Ok(r#"[{"name":"fixture","type":"drive"}]"#.into()),
            "lsjson" => Ok(serde_json::json!([{"Name":row.name,"Path":row.name,"Size":row.bytes,"ID":row.id,"ModTime":row.modified,"IsDir":false}]).to_string()),
            "deletefile" => { sent.set(sent.get()+1); Err("Connessione interrotta".into()) }
            other => panic!("unexpected command {other}"),
        }
        };
        let result = remove(&call, &a, &[], std::slice::from_ref(&row)).unwrap_err();
        assert!(result.contains("Rimozione incerta"));
        assert!(result.contains("0 rimozioni verificate"));
        assert_eq!(sent.get(), 1);
    }

    #[test]
    fn changed_parent_after_mutation_invalidates_file_receipt() {
        let account = Account {
            name: "fixture".into(),
            kind: "drive".into(),
        };
        let file = item();
        let source_data =
            serde_json::json!({"Name":"source", "Path":"source", "ID":"source-id", "IsDir":true});
        let source = parse_entry(&source_data).unwrap();
        let file_data = serde_json::json!({"Name":file.name,"Path":file.name,"ID":file.id,"IsDir":false,"Size":file.bytes,"ModTime":file.modified});
        let destination = "Lindo revisione fixture";
        let dest_data = serde_json::json!({"Name":destination, "Path":destination, "ID":"dest-id", "IsDir":true});
        for (move_file, swap_source) in [(false, true), (true, true), (true, false)] {
            let issued = std::cell::Cell::new(false);
            let created = std::cell::Cell::new(false);
            let call = |args: &[String]| -> Result<String, String> {
                match args[0].as_str() {
                    "listremotes" => return Ok(r#"[{"name":"fixture","type":"drive"}]"#.into()),
                    "mkdir" => {
                        assert!(!created.replace(true));
                        return Ok(String::new());
                    }
                    "deletefile" | "moveto" => {
                        assert!(!issued.replace(true));
                        return Ok(String::new());
                    }
                    "lsjson" => (),
                    other => panic!("unexpected command {other}"),
                }
                let root = args[1] == "fixture:";
                let source_request = root && args.iter().any(|arg| arg == "+ /source");
                let row = if source_request {
                    Some(source_data.clone())
                } else if root {
                    created.get().then(|| dest_data.clone())
                } else if args[1] == "fixture:source" {
                    (!issued.get()).then(|| file_data.clone())
                } else {
                    issued.get().then(|| file_data.clone())
                };
                let rows = row
                    .into_iter()
                    .map(|mut row| {
                        if root && issued.get() && source_request == swap_source {
                            row["ID"] = Value::from("replaced");
                        }
                        row
                    })
                    .collect::<Vec<_>>();
                Ok(serde_json::json!(rows).to_string())
            };
            let trail = std::slice::from_ref(&source);
            let selected = std::slice::from_ref(&file);
            let result = if move_file {
                relocate(&call, &account, trail, selected, destination)
            } else {
                remove(&call, &account, trail, selected)
            };
            assert!(issued.get());
            let error = result.unwrap_err();
            assert!(error.contains(if move_file {
                "Spostamento incerto"
            } else {
                "Rimozione incerta"
            }));
            assert!(error.starts_with("0 "));
        }
    }
    #[test]
    fn local_rclone_fixture_lists_filters_and_moves_without_account_access() {
        let Ok(exe) = binary() else { return };
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let original = root.join("Materials");
        fs::create_dir(&original).unwrap();
        fs::create_dir(root.join("_6_Archivio_Private")).unwrap();
        fs::write(original.join("Budget [1].csv"), b"data").unwrap();
        fs::write(original.join("Budget 2.csv"), b"data").unwrap();
        fs::write(original.join("Other.csv"), b"data").unwrap();
        let a = Account {
            name: "fixture".into(),
            kind: "dropbox".into(),
        };
        let call = |args: &[String]| -> Result<String, String> {
            if args[0] == "listremotes" {
                return Ok(r#"[{"name":"fixture","type":"dropbox"}]"#.into());
            }
            let mapped = args
                .iter()
                .map(|v| {
                    v.strip_prefix("fixture:")
                        .map(|p| root.join(p).display().to_string())
                        .unwrap_or_else(|| v.clone())
                })
                .collect::<Vec<_>>();
            let output = Command::new(&exe)
                .env_clear()
                .args(&mapped)
                .args(["--config", "/dev/null"])
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{mapped:?} {}",
                String::from_utf8_lossy(&output.stderr)
            );
            if args[0] != "lsjson" {
                return Ok(String::new());
            }
            let mut parsed: Value = serde_json::from_slice(&output.stdout).unwrap();
            for row in parsed.as_array_mut().unwrap() {
                // The local backend omits IDs. An inode models a stable provider
                // ID across a same-volume rename for this isolated fixture.
                let file = Path::new(&mapped[1]).join(row["Name"].as_str().unwrap());
                row["ID"] = Value::from(fs::metadata(file).unwrap().ino().to_string());
            }
            Ok(parsed.to_string())
        };
        let top = list(&call, &a, "", "", false, 0).unwrap();
        assert_eq!(top.len(), 1);
        let files = list(&call, &a, "Materials", "budget", false, 0).unwrap();
        assert_eq!(files.len(), 2);
        let selected = files.iter().find(|f| f.name.contains('[')).unwrap();
        let dest = review_name();
        let msg = relocate(&call, &a, &top, std::slice::from_ref(selected), &dest).unwrap();
        assert!(msg.contains("1 file spostati"));
        assert_eq!(
            fs::read(root.join(dest).join(&selected.name)).unwrap(),
            b"data"
        );
        assert!(original.join("Other.csv").is_file() && original.join("Budget 2.csv").is_file());
    }
}

#[cfg(test)]
mod trash_tests {
    use super::*;
    use std::cell::Cell;
    #[test]
    fn requests_only_provider_soft_delete_after_two_checks() {
        for (kind, flag) in [
            ("drive", Some("--drive-use-trash=true")),
            ("onedrive", Some("--onedrive-hard-delete=false")),
            ("dropbox", None),
        ] {
            let a = Account {
                name: "fixture".into(),
                kind: kind.into(),
            };
            let raw = serde_json::json!({"Name":"sample","Path":"sample","Size":29,"ID":"object123","IsDir":false,"ModTime":"2026-01-01T12:12:12Z"});
            let sample = parse_entry(&raw).unwrap();
            let count = Cell::new(0);
            let trash = Cell::new(false);
            let call = |args: &[String]| match args[0].as_str() {
                "listremotes" => {
                    Ok(serde_json::json!([{"name":"fixture","type":kind}]).to_string())
                }
                "lsjson" => {
                    count.set(count.get() + 1);
                    Ok(if trash.get() {
                        "[]".into()
                    } else {
                        serde_json::json!([raw]).to_string()
                    })
                }
                "deletefile" => {
                    assert_eq!(count.get(), 2);
                    assert_eq!(args[1], "fixture:sample");
                    if let Some(flag) = flag {
                        assert!(args.iter().any(|s| s == flag));
                    }
                    trash.set(true);
                    Ok(String::new())
                }
                other => panic!("unexpected command {other}"),
            };
            remove(&call, &a, &[], &[sample]).unwrap();
            assert!(trash.get());
            assert_eq!(count.get(), 3);
        }
    }
}
