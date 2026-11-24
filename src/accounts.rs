//! On-demand integrations with Apple Mail, Finder and local browser settings.
#[path = "accounts_attachments.rs"]
pub(crate) mod attachments;
#[path = "accounts_browser.rs"]
mod browser;

use crate::{
    model::{Item, Mode, Progress, Request},
    picker::Picker,
    policy, scan,
    system_tools::command,
};
use eframe::egui;
use objc2::{msg_send, runtime::Bool};
use objc2_foundation::{
    NSFileManager, NSURLResourceKey, NSURLUbiquitousItemHasUnresolvedConflictsKey,
    NSURLUbiquitousItemIsUploadedKey, NSURLUbiquitousItemIsUploadingKey, NSURL,
};
use serde_json::Value;
use std::{
    collections::HashSet,
    fs::File,
    io::{Seek, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    time::Duration,
};

const MAIL_SCRIPT: &str = r#"
function run(argv) {
    const app = Application("com.apple.mail");
    const viewers = app.messageViewers();
    if (!viewers.length) throw Error("Apri una finestra di Mail e seleziona una casella.");
    const boxes = viewers[0].selectedMailboxes();
    if (boxes.length !== 1) throw Error("Seleziona una sola casella di un account in Mail.");
    const box = boxes[0], account = box.account();
    if (!account || !account.id()) throw Error("Seleziona una casella di un singolo account.");
    const accountID = String(account.id()), boxName = String(box.name());
    function request() {
        ObjC.import("Foundation");
        const raw = $.NSFileHandle.fileHandleWithStandardInput.readDataToEndOfFile;
        return JSON.parse(ObjC.unwrap($.NSString.alloc.initWithDataEncoding(raw,$.NSUTF8StringEncoding)));
    }
    function sameBox(input) {
        if (input.accountID !== accountID || input.boxName !== boxName) throw Error("La casella selezionata è cambiata.");
    }
    if (argv[0] === "scan" || argv[0] === "page") {
        const total = box.messages.length, rows = [];
        let offset=0;
        if (argv[0] === "page") {
            const page=request(); sameBox(page);
            if (!Number.isSafeInteger(page.offset) || page.offset < 0 || page.offset % 100 !== 0) throw Error("Pagina non valida.");
            offset=Math.min(page.offset, Math.floor(Math.max(total-1,0)/100)*100);
        }
        const end=Math.min(total,offset+100);
        let skipped = 0;
        for (let i=offset; i<end; ++i) {
            try {
                const m = box.messages[i], subject = String(m.subject() || "");
                const promotion = /newsletter|promotion|promozion|offert[ae]|sconto|special offer|sale ends/i.test(subject);
                const parts = m.mailAttachments();
                let local = 0;
                for (let p of parts) if (p.downloaded()) ++local;
                let unsubscribe = "";
                try {
                    const h = m.headers.whose({name:"List-Unsubscribe"})();
                    if (h.length === 1) unsubscribe = String(h[0].content()).slice(0,2048);
                } catch(e) {}
                if (!promotion && !local && !unsubscribe) continue;
                const id = Number(m.id()), mid = String(m.messageId() || "");
                if (!id || !mid) { ++skipped; continue; }
                rows.push({id,mid,subject:subject.slice(0,240),local,promotion,unsubscribe});
            } catch(e) { ++skipped; }
        }
        return JSON.stringify({accountID,boxName,total,offset,skipped,rows});
    }
    if (argv[0] !== "move") throw Error("Azione sconosciuta.");
    const input=request(); sameBox(input);
    // Resolve the account's configured Trash via Mail's standard aggregated Trash.
    const top = app.trashMailbox(), destinations = [];
    for (const child of top.mailboxes()) {
        try { if (String(child.account().id()) === accountID) destinations.push(child); } catch(e) {}
    }
    if (destinations.length === 0) {
        try { if (String(top.account().id()) === accountID) destinations.push(top); } catch(e) {}
    }
    if (destinations.length !== 1 || String(destinations[0].name()) === boxName)
        throw Error("Cestino separato di questo account non identificabile.");
    const targets = [];
    for (const row of input.rows) {
        const found = box.messages.whose({id:row.id})();
        if (found.length !== 1 || String(found[0].messageId()) !== row.mid)
            throw Error("Un messaggio è cambiato. Ripeti la scansione.");
        targets.push(found[0]);
    }
    let moved=0, verified=0;
    for (let i=0;i<targets.length;++i) {
        try {
            const row=input.rows[i], dest=destinations[0];
            const selected=app.messageViewers()[0].selectedMailboxes();
            if (selected.length!==1 || String(selected[0].account().id())!==accountID || String(selected[0].name())!==boxName)
                throw Error("La casella selezionata è cambiata.");
            const before=box.messages.whose({id:row.id})();
            if (before.length!==1 || String(before[0].messageId())!==row.mid)
                throw Error("Il messaggio è cambiato.");
            const existing=dest.messages.whose({messageId:row.mid})().length;
            app.move(before[0],{to:dest}); ++moved;
            // Mail may assign a new local id in Trash. Verify the original local
            // id disappeared and the RFC Message-ID count increased in this account.
            if (box.messages.whose({id:row.id})().some(m=>String(m.messageId())===row.mid) ||
                String(dest.account().id())!==accountID || dest.messages.whose({messageId:row.mid})().length!==existing+1)
                throw Error("Mail non ha ancora confermato il trasferimento in questa casella. Controlla il Cestino.");
            ++verified;
        } catch(e) {return JSON.stringify({moved,verified,total:targets.length,error:String(e)});}
    }
    return JSON.stringify({moved,verified,total:targets.length});
}
"#;

fn json_input(value: &Value) -> Result<File, String> {
    let mut f = tempfile::tempfile().map_err(|e| e.to_string())?;
    f.write_all(value.to_string().as_bytes())
        .and_then(|_| f.rewind())
        .map_err(|e| e.to_string())?;
    Ok(f)
}
fn unsubscribe_link(header: &str) -> Option<url::Url> {
    header.split(',').find_map(|part| {
        let target = part.trim().strip_prefix('<')?.strip_suffix('>')?;
        if target
            .chars()
            .any(|c| c.is_control() || c.is_whitespace() || c == '\\')
        {
            return None;
        }
        let url = url::Url::parse(target).ok()?;
        (url.scheme() == "https"
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none())
        .then_some(url)
    })
}
fn automation(
    script: &str,
    operation: &str,
    input: Option<File>,
    stop: &AtomicBool,
) -> Result<Value, String> {
    let r = command::run(
        "/usr/bin/osascript",
        &["-l", "JavaScript", "-e", script, "--", operation],
        input,
        stop,
        120,
    )?;
    if r.code != Some(0) {
        return Err(r.text);
    }
    if r.truncated {
        return Err("Risposta del client troppo lunga. Riduci gli elementi aperti.".into());
    }
    serde_json::from_str(&r.text).map_err(|_| "Risposta del client illeggibile.".into())
}
#[derive(Debug, Clone)]
struct Message {
    id: u64,
    mid: String,
    subject: String,
    local: u64,
    promotion: bool,
    unsubscribe: Option<url::Url>,
}
struct Mailbox {
    account: String,
    name: String,
    rows: Vec<Message>,
    total: u64,
    offset: u64,
    skipped: u64,
}
fn parse_mail(value: Value) -> Result<Mailbox, String> {
    let invalid = || "Metadati di Mail incompleti.".to_owned();
    let string = |v: &Value, field: &str| v[field].as_str().map(str::to_owned).ok_or_else(invalid);
    let integer = |v: &Value, field: &str| v[field].as_u64().ok_or_else(invalid);
    let mut rows = Vec::new();
    let mut ids = HashSet::new();
    for row in value["rows"].as_array().ok_or_else(invalid)? {
        let id = integer(row, "id")?;
        let mid = string(row, "mid")?;
        if id == 0 || mid.is_empty() || !ids.insert(id) || rows.len() == 100 {
            return Err(invalid());
        }
        rows.push(Message {
            id,
            mid,
            subject: string(row, "subject")?,
            local: integer(row, "local")?,
            unsubscribe: row["unsubscribe"].as_str().and_then(unsubscribe_link),
            promotion: row["promotion"].as_bool().ok_or_else(invalid)?,
        });
    }
    let total = integer(&value, "total")?;
    let offset = integer(&value, "offset")?;
    if !offset.is_multiple_of(100)
        || (offset > 0 && offset >= total)
        || rows.len() as u64 > total.saturating_sub(offset).min(100)
    {
        return Err(invalid());
    }
    Ok(Mailbox {
        account: string(&value, "accountID")?,
        name: string(&value, "boxName")?,
        total,
        offset,
        skipped: integer(&value, "skipped")?,
        rows,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Provider {
    ICloud,
    Dropbox,
    Google,
    OneDrive,
}
impl Provider {
    fn title(self) -> &'static str {
        match self {
            Self::ICloud => "iCloud Drive",
            Self::Dropbox => "Dropbox",
            Self::Google => "Google Drive",
            Self::OneDrive => "OneDrive",
        }
    }
}
fn provider(home: &Path, path: &Path) -> Option<Provider> {
    let library = home.join("Library");
    if path.starts_with(library.join("Mobile Documents/com~apple~CloudDocs")) {
        return Some(Provider::ICloud);
    }
    let domain = path
        .strip_prefix(library.join("CloudStorage"))
        .ok()?
        .components()
        .next()?
        .as_os_str()
        .to_str()?;
    for (prefix, name) in [
        ("Dropbox", Provider::Dropbox),
        ("GoogleDrive", Provider::Google),
        ("OneDrive", Provider::OneDrive),
    ] {
        if domain == prefix
            || domain
                .strip_prefix(prefix)
                .is_some_and(|v| v.starts_with('-'))
        {
            return Some(name);
        }
    }
    None
}
fn ready(url: &NSURL) -> bool {
    fn boolean(url: &NSURL, key: &NSURLResourceKey) -> Option<bool> {
        let mut value = None;
        unsafe { url.getResourceValue_forKey_error(&mut value, key) }.ok()?;
        let value = value?;
        // These three documented URL resource keys return NSNumber booleans.
        let flag: Bool = unsafe { msg_send![&*value, boolValue] };
        Some(flag.as_bool())
    }
    unsafe {
        boolean(url, NSURLUbiquitousItemIsUploadedKey) == Some(true)
            && boolean(url, NSURLUbiquitousItemIsUploadingKey) == Some(false)
            && boolean(url, NSURLUbiquitousItemHasUnresolvedConflictsKey) == Some(false)
    }
}
fn evict(root: &Path, item: &Item) -> Result<(), String> {
    let fail = || "File cambiato, non locale oppure sincronizzazione incompleta.".to_owned();
    let _guard = policy::LocalOnly::enter().map_err(|_| fail())?;
    if !item.path.starts_with(root)
        || provider(&policy::home(), root) != Some(Provider::ICloud)
        || !policy::allowed_root(Mode::Large, &item.path)
        || !item.identity.matches(&item.path)
        || !policy::provider_state(&item.path, false).is_ok_and(|s| s == (true, true))
    {
        return Err(fail());
    }
    objc2::rc::autoreleasepool(|_| {
        let url = NSURL::from_file_path(&item.path).ok_or_else(fail)?;
        if !ready(&url) || !item.identity.matches(&item.path) {
            return Err(fail());
        }
        NSFileManager::defaultManager()
            .evictUbiquitousItemAtURL_error(&url)
            .map_err(|err| err.localizedDescription().to_string())
    })
}
struct Cloud {
    root: PathBuf,
    provider: Provider,
    items: Vec<Item>,
    status: String,
}
fn cloud_scan(root: PathBuf, stop: &AtomicBool) -> Result<Cloud, String> {
    let p = provider(&policy::home(), &root)
        .ok_or("Scegli una cartella iCloud Drive, Dropbox, Google Drive o OneDrive.")?;
    let report = scan::run(
        &Request {
            mode: Mode::Large,
            roots: vec![root.clone()],
            min_bytes: 0,
        },
        stop,
        &Progress::default(),
    );
    if stop.load(Ordering::Acquire) {
        return Err("Analisi annullata.".into());
    }
    let mut items = report.items;
    items.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.path.cmp(&b.path)));
    let status = format!(
        "{} file locali · {} esclusioni · {} errori. {}",
        items.len(),
        report.skipped,
        report.errors,
        report.notes.first().map(String::as_str).unwrap_or("")
    );
    Ok(Cloud {
        root,
        provider: p,
        items,
        status,
    })
}
enum Reply {
    Mail(Mailbox),
    Cloud(Cloud),
    Tabs(browser::Tabs),
    Copies(attachments::Copies),
    Action(String),
}
struct Work {
    stop: Arc<AtomicBool>,
    rx: mpsc::Receiver<Result<Reply, String>>,
    mutation: bool,
}
impl Drop for Work {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}
#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Tab {
    #[default]
    Mail,
    Cloud,
    Privacy,
    Attachments,
}
#[derive(Default)]
pub(crate) struct Accounts {
    tab: Tab,
    picker: Option<Picker>,
    root: Option<PathBuf>,
    job: Option<Work>,
    mail: Option<Mailbox>,
    cloud: Option<Cloud>,
    tabs: Option<browser::Tabs>,
    copies: Option<attachments::Copies>,
    link: Option<url::Url>,
    selected: HashSet<usize>,
    review: Option<Vec<usize>>,
    notice: String,
}
impl Accounts {
    fn reset(&mut self) {
        self.job.take();
        self.mail.take();
        self.cloud.take();
        self.tabs.take();
        self.copies.take();
        self.link.take();
        self.review.take();
        self.selected.clear();
        self.notice.clear();
    }
    fn work(
        &mut self,
        ctx: &egui::Context,
        mutation: bool,
        f: impl FnOnce(&AtomicBool) -> Result<Reply, String> + Send + 'static,
    ) {
        let (tx, rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        self.job = Some(Work {
            rx,
            stop: stop.clone(),
            mutation,
        });
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(f(&stop));
            ctx.request_repaint();
        });
    }
    fn poll(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let Some(job) = &self.job else {
            return;
        };
        match job.rx.try_recv() {
            Ok(result) => {
                self.job.take();
                match result {
                    Ok(Reply::Mail(m)) => self.mail = Some(m),
                    Ok(Reply::Cloud(c)) => self.cloud = Some(c),
                    Ok(Reply::Copies(c)) => self.copies = Some(c),
                    Ok(Reply::Tabs(t)) => self.tabs = Some(t),
                    Ok(Reply::Action(s)) => self.notice = s,
                    Err(e) => self.notice = e,
                }
            }
            Err(mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(Duration::from_millis(160));
                ui.horizontal(|ui| {
                    ui.spinner();
                    if !job.mutation && ui.button("Interrompi").clicked() {
                        job.stop.store(true, Ordering::Release);
                    }
                });
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.job.take();
                self.notice = "Operazione interrotta. Controlla il risultato nel client.".into();
            }
        }
    }
    fn select(&mut self, i: usize, checked: bool) {
        if checked {
            self.selected.insert(i);
        } else {
            self.selected.remove(&i);
        }
    }
    fn review_button(&mut self, ui: &mut egui::Ui) {
        if ui
            .add_enabled(
                !self.selected.is_empty(),
                egui::Button::new(format!("Rivedi {} selezionati", self.selected.len())),
            )
            .clicked()
        {
            let mut indices: Vec<_> = self.selected.iter().copied().collect();
            indices.sort_unstable();
            self.review = Some(indices);
        }
    }
    fn confirm(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let Some(indices) = self.review.as_ref() else {
            return;
        };
        let action = if self.tab == Tab::Mail {
            "Sposta i messaggi interi nel Cestino di Mail. L’azione si sincronizza con l’account e include tutti gli allegati."
        } else if self.tab == Tab::Attachments {
            "Sposta queste copie locali nel Cestino del Mac. Le modifiche fatte alle copie possono essere diverse dagli allegati conservati in Mail."
        } else if self.tab == Tab::Privacy {
            "Chiudi queste schede Chrome. Ogni indirizzo conserva una scheda nella stessa finestra. Eventuali modifiche non salvate nella scheda possono andare perse."
        } else {
            "Rimuovi solo queste copie scaricate dal Mac. iCloud Drive conserva i file sincronizzati."
        };
        ui.label(action);
        let mut confirm = false;
        let mut cancel = false;
        ui.horizontal(|ui| {
            confirm = ui.button("Conferma operazione").clicked();
            cancel = ui.button("Indietro").clicked();
        });
        egui::ScrollArea::vertical()
            .id_salt("account_review")
            .show_rows(ui, 22.0, indices.len(), |ui, range| {
                for n in range {
                    let i = indices[n];
                    if let Some(m) = &self.mail {
                        ui.label(&m.rows[i].subject);
                    }
                    if let Some(c) = &self.copies {
                        ui.label(c.items[i].path.display().to_string());
                    }
                    if let Some(t) = &self.tabs {
                        ui.label(t.describe(i));
                    }
                    if let Some(c) = &self.cloud {
                        ui.label(c.items[i].path.display().to_string());
                    }
                }
            });
        if cancel {
            self.review.take();
            return;
        }
        if !confirm {
            return;
        }
        let indices = self.review.take().unwrap();
        if let Some(mail) = self.mail.take() {
            let rows: Vec<_> = indices
                .iter()
                .map(|i| {
                    let m = &mail.rows[*i];
                    serde_json::json!({"id":m.id,"mid":m.mid})
                })
                .collect();
            let payload =
                serde_json::json!({"accountID":mail.account,"boxName":mail.name,"rows":rows});
            self.selected.clear();
            self.work(ctx, true, move |stop| {
                let reply = automation(MAIL_SCRIPT, "move", Some(json_input(&payload)?), stop)
                    .map_err(|e| format!("Verifica l’esito in Mail prima di riprovare. {e}"))?;
                let moved = reply["moved"]
                    .as_u64()
                    .ok_or("Esito di Mail sconosciuto.")?;
                let verified = reply["verified"].as_u64().ok_or("Verifica del Cestino sconosciuta.")?;
                Ok(Reply::Action(format!(
                    "Mail ha spostato {moved} di {} messaggi. {verified} verificati nel Cestino dell’account. {}",
                    rows.len(),
                    reply["error"].as_str().unwrap_or("")
                )))
            });
        } else if let Some(c) = self.copies.take() {
            self.selected.clear();
            self.work(ctx, true, move |stop| {
                attachments::move_copies(c, indices, stop)
            });
        } else if let Some(t) = self.tabs.take() {
            self.selected.clear();
            self.work(ctx, true, move |stop| browser::close(t, indices, stop));
        } else if let Some(c) = self.cloud.take() {
            let items: Vec<_> = indices.into_iter().map(|i| c.items[i].clone()).collect();
            self.selected.clear();
            self.work(ctx, true, move |stop| {
                let mut moved = 0;
                let mut issue = String::new();
                for item in &items {
                    if stop.load(Ordering::Acquire) {
                        break;
                    }
                    match evict(&c.root, item) {
                        Ok(()) => moved += 1,
                        Err(e) => {
                            issue = format!("{}: {e}", item.path.display());
                            break;
                        }
                    }
                }
                Ok(Reply::Action(format!(
                    "{moved} di {} copie locali rimosse. {issue}",
                    items.len()
                )))
            });
        }
    }
    pub(crate) fn show(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        self.poll(ui, ctx);
        if self.job.is_some() {
            return;
        }
        if let Some(picker) = self.picker.as_mut() {
            picker.poll();
            if picker.visible() {
                if let Some((_, path)) = picker.draw(ui, ctx) {
                    self.reset();
                    self.root = Some(path);
                }
                return;
            }
        }
        if let Some(url) = &self.link {
            ui.heading("Apri il link di disiscrizione");
            ui.label(format!("Dominio: {}", url.host_str().unwrap_or("")));
            ui.label(url.as_str());
            ui.label("Il mittente può registrare la visita o completare subito la disiscrizione.");
            let mut open = false;
            let mut cancel = false;
            ui.horizontal(|ui| {
                open = ui.button("Apri nel browser").clicked();
                cancel = ui.button("Annulla").clicked();
            });
            if open {
                let link = self.link.take().unwrap();
                self.work(ctx, false, move |stop| open_client(&[link.as_str()], stop));
            }
            if cancel {
                self.link.take();
            }
            return;
        }
        if self.review.is_some() {
            self.confirm(ui, ctx);
            return;
        }
        let old = self.tab;
        ui.horizontal_wrapped(|ui| {
            for (tab, title) in [
                (Tab::Mail, "Apple Mail"),
                (Tab::Attachments, "Copie allegati Mail"),
                (Tab::Cloud, "Copie locali cloud"),
                (Tab::Privacy, "Browser e privacy"),
            ] {
                ui.selectable_value(&mut self.tab, tab, title);
            }
        });
        if self.tab != old {
            self.reset();
        }
        if !self.notice.is_empty() {
            ui.label(&self.notice);
        }
        match self.tab {
            Tab::Attachments => self.attachments(ui, ctx),
            Tab::Mail => {
                ui.label("Seleziona una casella di un singolo account in Apple Mail. Lindo esamina 100 messaggi per pagina nell’ordine corrente di Mail e mostra quelli con allegati scaricati, un oggetto promozionale o un’intestazione di disiscrizione.");
                if ui.button("Analizza casella selezionata in Mail").clicked() {
                    self.reset();
                    self.work(ctx, false, |stop| {
                        parse_mail(automation(MAIL_SCRIPT, "scan", None, stop)?).map(Reply::Mail)
                    });
                    return;
                }
                self.review_button(ui);
                if let Some(mail) = &self.mail {
                    let pages = mail.total.div_ceil(100).max(1);
                    let examined = mail.total.saturating_sub(mail.offset).min(100);
                    ui.label(format!("{} · pagina {} di {pages} · {} messaggi totali · {examined} esaminati · {} illeggibili",
                        mail.name, mail.offset / 100 + 1, mail.total, mail.skipped));
                    let mut requested = None;
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(mail.offset >= 100, egui::Button::new("Pagina precedente"))
                            .clicked()
                        {
                            requested = Some(mail.offset - 100);
                        }
                        if ui
                            .add_enabled(
                                mail.offset.saturating_add(100) < mail.total,
                                egui::Button::new("Pagina successiva"),
                            )
                            .clicked()
                        {
                            requested = Some(mail.offset + 100);
                        }
                    });
                    if let Some(offset) = requested {
                        let payload = serde_json::json!({"accountID":mail.account,"boxName":mail.name,"offset":offset});
                        self.reset();
                        self.work(ctx, false, move |stop| {
                            parse_mail(automation(
                                MAIL_SCRIPT,
                                "page",
                                Some(json_input(&payload)?),
                                stop,
                            )?)
                            .map(Reply::Mail)
                        });
                        return;
                    }
                    let mut change = None;
                    egui::ScrollArea::vertical()
                        .id_salt("mail_results")
                        .show_rows(ui, 24.0, mail.rows.len(), |ui, range| {
                            for i in range {
                                let m = &mail.rows[i];
                                let mut checked = self.selected.contains(&i);
                                let label = format!(
                                    "{} · {} allegati locali{}",
                                    m.subject,
                                    m.local,
                                    if m.promotion {
                                        " · promozione possibile"
                                    } else {
                                        ""
                                    }
                                );
                                ui.push_id(i, |ui| {
                                    ui.horizontal(|ui| {
                                        if let Some(link) = &m.unsubscribe {
                                            if ui.small_button("Disiscriviti…").clicked() {
                                                self.link = Some(link.clone());
                                            }
                                        }
                                        if ui.checkbox(&mut checked, label).changed() {
                                            change = Some((i, checked));
                                        }
                                    })
                                });
                            }
                        });
                    if let Some((i, v)) = change {
                        self.select(i, v);
                    }
                }
            }
            Tab::Cloud => {
                let root = self
                    .root
                    .get_or_insert_with(|| {
                        policy::home().join("Library/Mobile Documents/com~apple~CloudDocs")
                    })
                    .clone();
                ui.label(root.display().to_string());
                ui.horizontal(|ui| {
                    if ui.button("Scegli cartella").clicked() {
                        self.reset();
                        self.picker
                            .get_or_insert_with(Picker::new)
                            .open(Mode::Large, ctx);
                    }
                    if ui.button("Analizza file locali").clicked() {
                        self.reset();
                        self.work(ctx, false, move |stop| {
                            cloud_scan(root, stop).map(Reply::Cloud)
                        });
                    }
                });
                if self
                    .cloud
                    .as_ref()
                    .is_some_and(|c| c.provider == Provider::ICloud)
                {
                    self.review_button(ui);
                }
                if let Some(c) = &self.cloud {
                    ui.label(format!("{} · {}", c.provider.title(), c.status));
                    let direct = c.provider == Provider::ICloud;
                    if !direct {
                        ui.label("Mostra il file nel Finder e usa il comando del provider per renderlo disponibile solo online.");
                    }
                    let mut change = None;
                    let mut reveal = None;
                    egui::ScrollArea::vertical()
                        .id_salt("cloud_results")
                        .show_rows(ui, 24.0, c.items.len(), |ui, range| {
                            for i in range {
                                let item = &c.items[i];
                                ui.push_id(i, |ui| {
                                    ui.horizontal(|ui| {
                                        if direct {
                                            let mut checked = self.selected.contains(&i);
                                            if crate::extras::file_checkbox(
                                                ui,
                                                &mut checked,
                                                true,
                                                &item.path,
                                            )
                                            .changed()
                                            {
                                                change = Some((i, checked));
                                            }
                                        }
                                        if ui.small_button("Finder").clicked() {
                                            reveal = Some(item.clone());
                                        }
                                        ui.label(format!("{:.1} MB", item.bytes as f64 / 1e6));
                                        ui.add(
                                            egui::Label::new(item.path.display().to_string())
                                                .truncate(),
                                        )
                                        .on_hover_text(item.path.display().to_string());
                                    })
                                });
                            }
                        });
                    if let Some((i, v)) = change {
                        self.select(i, v);
                    }
                    if let Some(item) = reveal {
                        self.work(ctx, false, move |stop| {
                            if !item.identity.matches(&item.path) {
                                return Err("File cambiato.".into());
                            }
                            open_client(
                                &[
                                    "-R",
                                    item.path.to_str().ok_or("Percorso non rappresentabile.")?,
                                ],
                                stop,
                            )
                        });
                    }
                }
            }
            Tab::Privacy => {
                ui.label("Apri i controlli del browser per scegliere cronologia, cookie e intervallo di tempo. Nel browser aperto verifica profilo e sincronizzazione prima di cancellare.");
                for (title, args) in [
                    (
                        "Safari · Cronologia > Cancella cronologia",
                        vec!["-b", "com.apple.Safari"],
                    ),
                    (
                        "Chrome · Cancella dati di navigazione",
                        vec![
                            "-b",
                            "com.google.Chrome",
                            "chrome://settings/clearBrowserData",
                        ],
                    ),
                    (
                        "Edge · Cancella dati di navigazione",
                        vec![
                            "-b",
                            "com.microsoft.edgemac",
                            "edge://settings/clearBrowserData",
                        ],
                    ),
                    (
                        "Firefox · Privacy e sicurezza",
                        vec!["-b", "org.mozilla.firefox", "about:preferences#privacy"],
                    ),
                    (
                        "Permessi privacy di macOS",
                        vec![
                            "x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension",
                        ],
                    ),
                ] {
                    if ui.button(title).clicked() {
                        self.work(ctx, false, move |stop| open_client(&args, stop));
                        break;
                    }
                }
                ui.horizontal_wrapped(|ui| {
                    for (title, pane, help) in [
                        ("Gestisci reti Wi-Fi conosciute", "x-apple.systempreferences:com.apple.wifi-settings-extension", "In Wi-Fi scegli Avanzate. Accanto alla rete apri il menu Altro e scegli Rimuovi dall’elenco."),
                        ("Gestisci elementi recenti", "x-apple.systempreferences:com.apple.ControlCenter-Settings.extension", "In Barra dei menu scegli il numero di documenti, applicazioni e server recenti. Per svuotare l’elenco scegli menu Apple > Elementi recenti > Cancella menu."),
                    ] {
                        if ui.button(title).on_hover_text(help).clicked() {
                            self.work(ctx, false, move |stop| {
                                open_client(&[pane], stop)?;
                                Ok(Reply::Action(help.into()))
                            });
                        }
                    }
                });
                self.browser_tabs(ui, ctx);
            }
        }
    }
}
fn open_client(args: &[&str], stop: &AtomicBool) -> Result<Reply, String> {
    let r = command::run("/usr/bin/open", args, None, stop, 12)?;
    if r.code != Some(0) {
        return Err(format!("Client non disponibile. {}", r.text));
    }
    Ok(Reply::Action(
        "Richiesta di apertura inviata a macOS.".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn cloud_domains_are_exact_and_home_scoped() {
        let home = Path::new("/fixture");
        for (p, expected) in [
            (
                "Library/Mobile Documents/com~apple~CloudDocs/x",
                Some(Provider::ICloud),
            ),
            (
                "Library/CloudStorage/Dropbox-team/file",
                Some(Provider::Dropbox),
            ),
            (
                "Library/CloudStorage/GoogleDrive-test/x",
                Some(Provider::Google),
            ),
            (
                "Library/CloudStorage/OneDrive-Personal/x",
                Some(Provider::OneDrive),
            ),
            ("Library/CloudStorage/DropboxFake/x", None),
        ] {
            assert_eq!(provider(home, &home.join(p)), expected);
        }
        assert!(provider(home, Path::new("/other/Library/CloudStorage/Dropbox")).is_none());
    }

    #[test]
    fn unsubscribe_only_offers_explicit_https_links_without_credentials() {
        assert_eq!(
            unsubscribe_link("<mailto:test@example.invalid>, <https://news.invalid/u?id=12>")
                .unwrap()
                .host_str(),
            Some("news.invalid")
        );
        let mut authenticated = url::Url::parse("https://news.invalid/").unwrap();
        authenticated.set_username("fixture").unwrap();
        authenticated.set_password(Some("fixture")).unwrap();
        assert!(unsubscribe_link(&format!("<{authenticated}>")).is_none());
        for bad in [
            "<http://test.invalid/x>",
            "<file:///tmp/no>",
            "<https://news.invalid/\n>",
            "not a link",
        ] {
            assert!(unsubscribe_link(bad).is_none());
        }
    }

    // Substitute the application factory before evaluating JXA. No Apple events
    // go to Mail or Chrome; the exact production scripts run against local objects.
    const MAIL_FIXTURE: &str = r#"
function fixtureApp() {
    const account = {id:()=>"fixture-account"};
    const message={id:()=>3,messageId:()=>"fixture-id",subject:()=>"Offerta fixture",
        mailAttachments:()=>[{downloaded:()=>true},{downloaded:()=>false}],
        headers:{whose:()=>()=>[{content:()=>"<https://fixture.invalid/u>"}]}};
    const messages=[message], trash=[];
    messages.whose=({id})=>()=>messages.filter(m=>m.id()===id);
    trash.whose=({messageId})=>()=>trash.filter(m=>m.messageId()===messageId);
    const source={name:()=>"Fixture",account:()=>account,messages};
    const dest={name:()=>"Trash",account:()=>account,messages:trash};
    return {messageViewers:()=>[{selectedMailboxes:()=>[source]}],
        trashMailbox:()=>({mailboxes:()=>[dest]}),
        move:(m,{to})=>{if(m!==message || to!==dest) throw Error("Wrong target"); messages.splice(messages.indexOf(m),1); trash.push(m);}};
}
"#;
    #[test]
    fn apple_mail_fixture_scans_and_moves_only_matching_messages() {
        let script = format!(
            "{}\n{}",
            MAIL_FIXTURE,
            MAIL_SCRIPT.replace("Application(\"com.apple.mail\")", "fixtureApp()")
        );
        let stop = AtomicBool::new(false);
        let inbox = parse_mail(automation(&script, "scan", None, &stop).unwrap()).unwrap();
        assert_eq!(inbox.rows.len(), 1);
        let m = &inbox.rows[0];
        assert_eq!(m.local, 1);
        assert!(m.promotion);
        assert!(m.unsubscribe.is_some());
        let payload = |mid| json!({"accountID":"fixture-account","boxName":"Fixture","rows":[{"id":3,"mid":mid}]});
        let good = automation(
            &script,
            "move",
            Some(json_input(&payload("fixture-id")).unwrap()),
            &stop,
        )
        .unwrap();
        assert_eq!(good["moved"], 1);
        assert_eq!(good["verified"], 1);
        assert!(automation(
            &script,
            "move",
            Some(json_input(&payload("other-id")).unwrap()),
            &stop
        )
        .is_err());
    }

    #[test]
    fn mail_pagination_scans_later_pages_and_rejects_mailbox_changes() {
        let fixture = r#"
function fixtureApp(){
 const account={id:()=>"fixture"};
 const messages=Array.from({length:220},(_,i)=>({
    id:()=>i+1, messageId:()=>"fixture-"+(i+1), subject:()=>[144,219].includes(i)?"Newsletter":"Ordinary message",
    mailAttachments:()=>[], headers:{whose:()=>()=>[]}
 }));
 const box={name:()=>"Fixture",account:()=>account,messages};
 return {messageViewers:()=>[{selectedMailboxes:()=>[box]}]};
}"#;
        let script = format!(
            "{fixture}\n{}",
            MAIL_SCRIPT.replace("Application(\"com.apple.mail\")", "fixtureApp()")
        );
        let stop = AtomicBool::new(false);
        let first = parse_mail(automation(&script, "scan", None, &stop).unwrap()).unwrap();
        assert_eq!((first.total, first.offset, first.rows.len()), (220, 0, 0));
        let page = |offset, box_name| {
            let payload = json!({"accountID":"fixture","boxName":box_name,"offset":offset});
            parse_mail(automation(
                &script,
                "page",
                Some(json_input(&payload)?),
                &stop,
            )?)
        };
        let second = page(100, "Fixture").unwrap();
        assert_eq!(second.rows[0].id, 145);
        let third = page(200, "Fixture").unwrap();
        assert_eq!(third.offset, 200);
        assert_eq!(third.rows[0].id, 220);
        assert_eq!(page(100, "Fixture").unwrap().rows[0].id, 145);
        assert_eq!(page(300, "Fixture").unwrap().offset, 200);
        assert!(page(101, "Fixture").is_err());
        assert!(page(100, "Other").is_err());
        let mut panel = Accounts {
            mail: Some(second),
            selected: HashSet::from([0]),
            review: Some(vec![0]),
            ..Default::default()
        };
        panel.reset();
        assert!(panel.selected.is_empty() && panel.review.is_none() && panel.mail.is_none());
    }

    const CHROME_FIXTURE: &str = r#"
function fixtureApp(){
    const t = (id)=>({id:()=>id,url:()=>"https://fixture.invalid/",title:()=>"Fixture",exists:()=>true,
        close:()=>{if(id!=="extra")throw Error("Keeper selected");}});
    const tabs=[t("keep"),t("extra")];
    tabs.byId=(id)=>tabs.find(v=>v.id()===id)||{exists:()=>false};
    const win={id:()=>"w",mode:()=>"normal",exists:()=>true,tabs};
    const windows=()=>[win,{mode:()=>"incognito"}]; windows.byId=()=>win;
    return {running:()=>true,windows};
}
"#;
    #[test]
    fn chrome_fixture_rechecks_url_and_keeper_before_close() {
        let script = format!(
            "{}\n{}",
            CHROME_FIXTURE,
            browser::SCRIPT.replace("Application(\"com.google.Chrome\")", "fixtureApp()")
        );
        let stop = AtomicBool::new(false);
        let scanned = automation(&script, "scan", None, &stop).unwrap();
        assert_eq!(scanned["total"], 2);
        let tabs = browser::parse(scanned).unwrap();
        let mut payload = tabs.close_payload(&[1]).unwrap();
        assert_eq!(
            automation(&script, "close", Some(json_input(&payload).unwrap()), &stop).unwrap()
                ["closed"],
            1
        );
        payload[0]["url"] = json!("https://changed.invalid/");
        assert!(automation(&script, "close", Some(json_input(&payload).unwrap()), &stop).is_err());
    }
}
