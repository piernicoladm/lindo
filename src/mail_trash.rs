//! Review one explicitly selected Mail account Trash. Apple Mail owns the
//! final erasure; generic scripting "delete" does not attest server expunge.
use crate::system_tools::command;
use eframe::egui;
use serde_json::{json, Value};
use std::{
    collections::BTreeSet,
    io::{Seek, Write},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
        Arc,
    },
    time::Duration,
};

const SCRIPT: &str = r#"
function run(argv) {
    const app=Application('com.apple.mail');
    function spec(box) {
        const value=Automation.getDisplayString(box);
        if (typeof value !== 'string' || value.length>4096 || !/mailbox/i.test(value) || !/Application\(/.test(value)) throw Error("specifier");
        return value;
    }
    function context() {
        const viewers=app.messageViewers(); if (!viewers.length) throw Error("no viewer");
        const viewer=viewers[0], selected=viewer.selectedMailboxes();
        if (selected.length!==1) throw Error("one mailbox");
        const box=selected[0], account=box.account();
        if (!account) throw Error("no account");
        const accountID=String(account.id()||''), viewerID=Number(viewer.id());
        if (!accountID || !Number.isSafeInteger(viewerID) || viewerID<=0) throw Error("no id");
        const top=app.trashMailbox(), trash=[];
        let topAccount;
        try {topAccount=top.account();} catch(e) {}
        if (topAccount && String(topAccount.id())===accountID) trash.push(top);
        for (const child of top.mailboxes()){
            const a=child.account(); if (a && String(a.id())===accountID) trash.push(child);
        }
        if (trash.length!==1 || spec(trash[0])!==spec(box)) throw Error("not selected account Trash");
        return {box,viewer,accountID,viewerID,boxKey:spec(box),accountName:String(account.name()),boxName:String(box.name())};
    }
    function write(result){return JSON.stringify(result);}
    function identity(m){return {id:Number(m.id()),mid:String(m.messageId()||'')};}
    function valid(k){return Number.isSafeInteger(k.id) && k.id>0 && k.mid.length>0 && k.mid.length<=512;}
    function same(a,b){return a.id===b.id && a.mid===b.mid;}
    function locate(box,k){
        const ids=box.messages.whose({id:k.id})(), mids=box.messages.whose({messageId:k.mid})();
        if (!ids.length && !mids.length) return {state:'absent'};
        if (ids.length===1 && mids.length===1 && same(identity(ids[0]),k) && same(identity(mids[0]),k))
            return {state:'present',message:ids[0]};
        return {state:'ambiguous'};
    }
    let ctx;
    try {ctx=context();} catch(e){return write({error:'scope'});}
    const operation=argv[0];
    const key={accountID:ctx.accountID,viewerID:ctx.viewerID,boxKey:ctx.boxKey};
    function matches(c,k){return c.accountID===k.accountID && c.viewerID===k.viewerID && c.boxKey===k.boxKey;}
    let input={};
    if(operation!=='scan'){
        ObjC.import('Foundation');
        const data=$.NSFileHandle.fileHandleWithStandardInput.readDataToEndOfFile;
        input=JSON.parse(ObjC.unwrap($.NSString.alloc.initWithDataEncoding(data,$.NSUTF8StringEncoding)));
        if (!input.key || !matches(ctx,input.key)) return write({error:'changed'});
    }
    if (operation==='scan' || operation==='page'){
        const count=ctx.box.messages.length;
        const requested=operation==='scan'?0:input.offset;
        if (!Number.isSafeInteger(requested)||requested<0||requested%60!==0) return write({error:'changed'});
        const offset=Math.min(requested,Math.floor(Math.max(0,count-1)/60)*60);
        const rows=[];let skipped=0;
        for(let i=offset;i<Math.min(offset+60,count);++i){
            try{
                const m=ctx.box.messages[i],k=identity(m);
                if(!valid(k)){++skipped;continue;}
                rows.push({id:k.id,mid:k.mid,subject:String(m.subject()||'').slice(0,160)});
            }catch(e){++skipped;}
        }
        try{if (!matches(context(),key)) return write({error:'changed'});}catch(e){return write({error:'changed'});}
        return write({key,accountName:ctx.accountName,boxName:ctx.boxName,count,offset,skipped,rows});
    }
    if(!['highlight','verify'].includes(operation) || !Array.isArray(input.rows) || !input.rows.length || input.rows.length>60)
        return write({error:'ambiguous'});
    const ids=new Set(), mids=new Set(), messages=[];
    let present=0,absent=0,ambiguous=0;
    for(const k of input.rows){
        if(!valid(k) || ids.has(k.id) || mids.has(k.mid))return write({error:'ambiguous'});
        ids.add(k.id);mids.add(k.mid);
        try{
            const r=locate(ctx.box,k);
            if(r.state==='present'){++present;messages.push(r.message);}
            else if(r.state==='absent'){++absent;}else{++ambiguous;}
        }catch(e){++ambiguous;}
    }
    try{if(!matches(context(),key)) return write({error:'changed'});}catch(e){return write({error:'changed'});}
    if(operation==='verify')return write({present,absent,ambiguous});
    if(messages.length!==input.rows.length)return write({error:'ambiguous'});
    try{
        const final=context(); if(!matches(final,key))return write({error:'changed'});
        const targets=[];
        for(const k of input.rows){const r=locate(final.box,k);if(r.state!=='present')return write({error:'ambiguous'});targets.push(r.message);}
        final.viewer.selectedMessages=targets;
        const check=context();if(!matches(check,key))return write({error:'changed'});
        const selected=check.viewer.selectedMessages().map(identity);
        if(selected.length!==input.rows.length || input.rows.some(k=>selected.filter(v=>same(k,v)).length!==1))
            return write({error:'ambiguous'});
        app.activate();
    } catch(e){return write({error:'highlight'});}
    return write({highlighted:messages.length});
}
"#;

fn invoke(
    script: &str,
    operation: &str,
    input: Option<&Value>,
    stop: &AtomicBool,
) -> Result<Value, String> {
    let file = if let Some(v) = input {
        let mut f = tempfile::tempfile().map_err(|_| "Input temporaneo non disponibile.")?;
        f.write_all(v.to_string().as_bytes())
            .and_then(|_| f.rewind())
            .map_err(|_| "Input non disponibile.")?;
        Some(f)
    } else {
        None
    };
    let out = command::run_stdout(
        "/usr/bin/osascript",
        &["-l", "JavaScript", "-e", script, "--", operation],
        file,
        stop,
        120,
    )?;
    if out.code != Some(0) || out.truncated {
        return Err(
            "Apple Mail non ha completato la verifica. Controlla l’autorizzazione Automazione."
                .into(),
        );
    }
    let v: Value = serde_json::from_str(&out.text).map_err(|_| "Risposta di Mail incompleta.")?;
    if let Some(code) = v["error"].as_str() {
        return Err(match code{
            "scope"=>"Seleziona in Mail il Cestino di un solo account. Le caselle aggregate o non identificabili sono escluse.",
            "changed"=>"La casella o la finestra selezionata è cambiata. Ripeti la consultazione.",
            "highlight"=>"Mail non ha confermato la selezione dei messaggi. Controlla manualmente la casella.",
            _=>"Identità dei messaggi cambiata o ambigua. Ripeti la consultazione."
        }.into());
    }
    Ok(v)
}
struct Row {
    id: u64,
    mid: String,
    subject: String,
}
struct Preview {
    key: Value,
    account: String,
    box_name: String,
    rows: Vec<Row>,
    count: u64,
    offset: u64,
    skipped: u64,
}
fn parse(v: Value) -> Result<Preview, String> {
    let invalid = "Anteprima di Mail non verificabile.";
    let string = |v: &Value, k: &str| v[k].as_str().map(String::from).ok_or(invalid);
    let int = |v: &Value, k: &str| v[k].as_u64().ok_or(invalid);
    let key = &v["key"];
    if string(key, "accountID")?.is_empty()
        || string(key, "boxKey")?.is_empty()
        || int(key, "viewerID")? == 0
    {
        return Err(invalid.into());
    }
    let mut rows = Vec::new();
    let mut ids = BTreeSet::new();
    let mut mids = BTreeSet::new();
    for r in v["rows"].as_array().ok_or(invalid)? {
        let id = int(r, "id")?;
        let mid = string(r, "mid")?;
        if id == 0 || mid.is_empty() || !ids.insert(id) || !mids.insert(mid.clone()) {
            return Err(invalid.into());
        }
        rows.push(Row {
            id,
            mid,
            subject: string(r, "subject")?,
        });
    }
    let count = int(&v, "count")?;
    let offset = int(&v, "offset")?;
    let skipped = int(&v, "skipped")?;
    if !offset.is_multiple_of(60)
        || offset > count
        || (rows.len() as u64).checked_add(skipped) != Some(count.saturating_sub(offset).min(60))
    {
        return Err(invalid.into());
    }
    Ok(Preview {
        key: key.clone(),
        account: string(&v, "accountName")?,
        box_name: string(&v, "boxName")?,
        rows,
        count,
        offset,
        skipped,
    })
}
enum Outcome {
    Preview(Preview),
    Highlight(usize, Value),
    Verified {
        present: u64,
        absent: u64,
        ambiguous: u64,
    },
}
struct Worker {
    stop: Arc<AtomicBool>,
    rx: Receiver<Result<Outcome, String>>,
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}
#[derive(Default)]
pub(crate) struct MailTrash {
    preview: Option<Preview>,
    chosen: BTreeSet<usize>,
    review: bool,
    receipt: Option<Value>,
    refresh_page: Option<Value>,
    action_notice: String,
    worker: Option<Worker>,
    status: String,
}
impl MailTrash {
    fn start(
        &mut self,
        ctx: &egui::Context,
        task: impl FnOnce(&AtomicBool) -> Result<Outcome, String> + Send + 'static,
    ) {
        let (tx, rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        self.worker = Some(Worker {
            stop: stop.clone(),
            rx,
        });
        self.status = "Verifica in Apple Mail…".into();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(task(&stop));
            ctx.request_repaint();
        });
    }
    fn load(&mut self, ctx: &egui::Context, payload: Option<Value>) {
        self.preview = None;
        self.review = false;
        self.receipt = None;
        self.chosen.clear();
        self.start(ctx, move |stop| {
            let op = if payload.is_some() { "page" } else { "scan" };
            parse(invoke(SCRIPT, op, payload.as_ref(), stop)?).map(Outcome::Preview)
        });
    }
    pub(crate) fn show(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.heading("Cestino di Apple Mail");
        ui.label("In Mail seleziona il Cestino di un singolo account, poi consulta qui i messaggi. L’eliminazione definitiva si conferma in Mail e può sincronizzarsi sugli altri dispositivi.");
        if let Some(w) = &self.worker {
            ctx.request_repaint_after(Duration::from_millis(150));
            if ui.button("Interrompi verifica").clicked() {
                w.stop.store(true, Ordering::Release)
            }
            match w.rx.try_recv() {
                Ok(result) => {
                    self.worker = None;
                    match result {
                        Ok(Outcome::Preview(p)) => {
                            self.status = format!(
                                "{} · {} · {} messaggi totali",
                                p.account, p.box_name, p.count
                            );
                            self.preview = Some(p);
                        }
                        Ok(Outcome::Highlight(n, receipt)) => {
                            self.receipt = Some(receipt);
                            self.action_notice = format!(
                                "{n} messaggi selezionati nella finestra di Mail. Controlla la selezione prima di usare Elimina in Mail."
                            );
                            self.status.clear();
                        }
                        Ok(Outcome::Verified {
                            present,
                            absent,
                            ambiguous,
                        }) => {
                            self.action_notice = format!(
                                "Nel Cestino selezionato · {present} ancora presenti · {absent} assenti · {ambiguous} incerti. La verifica riguarda questa casella sul Mac."
                            );
                            if let Some(page) = self.refresh_page.take() {
                                self.load(ctx, Some(page));
                            }
                        }
                        Err(e) => {
                            self.preview = None;
                            self.chosen.clear();
                            self.review = false;
                            if let Some(page) = self.refresh_page.take() {
                                self.action_notice = format!(
                                    "Esito non confermato. Controlla la selezione in Mail. {e}"
                                );
                                self.load(ctx, Some(page));
                            } else {
                                self.status = e;
                            }
                        }
                    }
                }
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.worker = None;
                    self.preview = None;
                    self.status = "Consultazione interrotta.".into();
                    if let Some(page) = self.refresh_page.take() {
                        self.action_notice =
                            "Esito non confermato. Controlla la selezione in Mail.".into();
                        self.load(ctx, Some(page));
                    }
                }
            }
        }
        if !self.action_notice.is_empty() {
            ui.label(&self.action_notice);
        }
        ui.label(&self.status);
        if self.worker.is_some() {
            return;
        }
        if let Some(receipt) = self.receipt.clone() {
            ui.label(format!(
                "Account {} · casella {}",
                receipt["accountName"].as_str().unwrap_or(""),
                receipt["boxName"].as_str().unwrap_or("")
            ));
            ui.label("Per i soli messaggi evidenziati usa il comando Elimina di Mail. «Cancella elementi eliminati» nel menu Casella riguarda tutto il Cestino dell’account.");
            if ui.button("Verifica i messaggi nel Cestino").clicked() {
                self.start(ctx, move |stop| {
                    let v = invoke(SCRIPT, "verify", Some(&receipt), stop)?;
                    let read = |k: &str| v[k].as_u64().ok_or("Esito incompleto.");
                    let present = read("present")?;
                    let absent = read("absent")?;
                    let ambiguous = read("ambiguous")?;
                    if present
                        .checked_add(absent)
                        .and_then(|n| n.checked_add(ambiguous))
                        != Some(receipt["rows"].as_array().ok_or("Esito incompleto.")?.len() as u64)
                    {
                        return Err("Esito incompleto.".into());
                    }
                    Ok(Outcome::Verified {
                        present,
                        absent,
                        ambiguous,
                    })
                });
                return;
            }
        }
        if ui
            .button("Consulta il Cestino selezionato in Mail")
            .clicked()
        {
            self.refresh_page = None;
            self.action_notice.clear();
            self.load(ctx, None);
            return;
        }
        let Some(p) = &self.preview else { return };
        if self.review {
            ui.heading("Rivedi i messaggi scelti");
            ui.label(format!(
                "Account {} · casella {} · {} messaggi",
                p.account,
                p.box_name,
                self.chosen.len()
            ));
            egui::ScrollArea::vertical()
                .max_height(145.)
                .show(ui, |ui| {
                    for &i in &self.chosen {
                        ui.label(&p.rows[i].subject);
                    }
                });
            if ui.button("Evidenzia questi messaggi in Mail").clicked() {
                let rows = self
                    .chosen
                    .iter()
                    .map(|i| json!({"id":p.rows[*i].id,"mid":p.rows[*i].mid}))
                    .collect::<Vec<_>>();
                let n = rows.len();
                let receipt =
                    json!({"key":p.key,"rows":rows,"accountName":p.account,"boxName":p.box_name});
                self.refresh_page = Some(json!({"key":p.key,"offset":p.offset}));
                self.action_notice.clear();
                self.receipt = None;
                self.preview = None;
                self.review = false;
                self.chosen.clear();
                self.start(ctx, move |stop| {
                    let v = invoke(SCRIPT, "highlight", Some(&receipt), stop)?;
                    if v["highlighted"].as_u64() != Some(n as u64) {
                        return Err("Selezione non confermata da Mail.".into());
                    }
                    Ok(Outcome::Highlight(n, receipt))
                });
            } else if ui.button("Torna all’elenco").clicked() {
                self.review = false;
            }
            return;
        }
        let mut page = None;
        ui.horizontal_wrapped(|ui| {
            ui.label(format!(
                "Pagina {} di {} · {} illeggibili",
                1 + p.offset / 60,
                p.count.div_ceil(60).max(1),
                p.skipped
            ));
            if ui
                .add_enabled(p.offset >= 60, egui::Button::new("Precedente"))
                .clicked()
            {
                page = Some(p.offset - 60);
            }
            if ui
                .add_enabled(p.offset + 60 < p.count, egui::Button::new("Successiva"))
                .clicked()
            {
                page = Some(p.offset + 60);
            }
            if ui
                .add_enabled(
                    !self.chosen.is_empty(),
                    egui::Button::new(format!("Rivedi {} selezionati", self.chosen.len())),
                )
                .clicked()
            {
                self.review = true;
            }
        });
        if let Some(offset) = page {
            let v = json!({"key":p.key,"offset":offset});
            self.load(ctx, Some(v));
            return;
        }
        egui::ScrollArea::vertical().show_rows(ui, 24., p.rows.len(), |ui, range| {
            for i in range {
                let r = &p.rows[i];
                let mut yes = self.chosen.contains(&i);
                if ui
                    .checkbox(
                        &mut yes,
                        if r.subject.is_empty() {
                            "(senza oggetto)"
                        } else {
                            &r.subject
                        },
                    )
                    .changed()
                {
                    if yes {
                        self.chosen.insert(i);
                    } else {
                        self.chosen.remove(&i);
                    }
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const FAKE: &str = r#"
function fixtureApp(){
 const account={id:()=>'account-A',name:()=>'Fixture A'}, second={id:()=>'account-B',name:()=>'Fixture B'};
 const msgs=[['A',11],['B',12],['C',13]].map(([mid,id])=>({id:()=>id,messageId:()=>mid,subject:()=>'Fixture '+mid}));
 if(SCENARIO==='partial'){msgs.splice(0,1);msgs[1].id=()=>88;}
 if(SCENARIO==='duplicate'){msgs[1].messageId=()=>'A';}
 msgs.whose=(match)=>()=>msgs.filter(m=>match.id!==undefined?m.id()===match.id:m.messageId()===match.messageId);
 const box=(a,key)=>({account:()=>a,name:()=>'Deleted Messages',_spec:'Application("Mail").mailboxes.'+key,messages:msgs});
 const own=box(account,'A.Trash'),other=box(second,'B.Trash'),decoy=box(account,'A.NotTrash');
 let calls=0,chosen=[];
 const viewer={id:()=>5,selectedMailboxes:()=>{++calls;
     if(SCENARIO==='swap' && calls>=3)return [other];
     if(SCENARIO==='decoy')return [decoy];
     if(SCENARIO==='combined')return [own,other];
     return [own];}};
 Object.defineProperty(viewer,'selectedMessages',{get:()=>()=>chosen,set:v=>{chosen=v;}});
 return {messageViewers:()=>[viewer],trashMailbox:()=>({account:()=>null,mailboxes:()=>[own,other]}),
   activate:()=>{if(chosen.length!==2 || chosen.some(m=>![11,12].includes(m.id())))throw Error('unexpected selection');},
   delete:()=>{throw Error('forbidden');},move:()=>{throw Error('forbidden');}};
}
"#;
    fn script(scenario: &str) -> String {
        // No application factory or Apple event survives substitution in this fixture.
        format!(
            "const SCENARIO={};\n{}\n{}",
            json!(scenario),
            FAKE,
            SCRIPT
                .replace("Application('com.apple.mail')", "fixtureApp()")
                .replace("Automation.getDisplayString(box)", "box._spec")
        )
    }
    fn fixture_page() -> Preview {
        parse(invoke(&script("ok"), "scan", None, &AtomicBool::new(false)).unwrap()).unwrap()
    }
    fn input(page: &Preview, count: usize) -> Value {
        json!({"key":page.key,"rows":page.rows.iter().take(count).map(|r|json!({"id":r.id,"mid":r.mid})).collect::<Vec<_>>()})
    }
    #[test]
    fn only_highlights_exact_messages_in_the_configured_account_trash() {
        let stop = AtomicBool::new(false);
        let page = fixture_page();
        assert_eq!(page.count, 3);
        assert_eq!(page.account, "Fixture A");
        let selection = input(&page, 2);
        let v = invoke(&script("ok"), "highlight", Some(&selection), &stop).unwrap();
        assert_eq!(v["highlighted"], 2);
        for scenario in ["decoy", "combined", "swap", "duplicate"] {
            assert!(
                invoke(&script(scenario), "highlight", Some(&selection), &stop).is_err(),
                "{scenario}"
            );
        }
        let mut changed = selection;
        changed["key"]["viewerID"] = json!(9);
        assert!(invoke(&script("ok"), "highlight", Some(&changed), &stop).is_err());
    }
    #[test]
    fn reports_partial_results_and_unresolved_ids_without_claiming_erasure() {
        let page = fixture_page();
        let v = invoke(
            &script("partial"),
            "verify",
            Some(&input(&page, 3)),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(v, json!({"present":1,"absent":1,"ambiguous":1}));
    }
}
