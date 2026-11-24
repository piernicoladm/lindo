use super::{automation, json_input, Accounts, Reply};
use eframe::egui;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

pub(super) const SCRIPT: &str = r#"
function run(argv) {
    const app = Application("com.google.Chrome");
    if (!app.running()) throw Error("Apri Chrome prima dell’analisi.");
    const windows = app.windows();
    if (argv[0] === "scan") {
        const rows = [];
        let total=0, skipped=0;
        for (const win of windows) {
            if (win.mode() !== "normal") continue;
            const count=win.tabs.length;
            total+=count;
            for(let i=0;i<count && rows.length<300;i++) {
                try {
                    const tab=win.tabs[i], url=String(tab.url());
                    if (!/^https?:\/\//i.test(url) || url.length>2048) { ++skipped; continue; }
                    rows.push({win:String(win.id()),id:String(tab.id()),url,title:String(tab.title()).slice(0,200)});
                } catch(e){ ++skipped; }
            }
        }
        return JSON.stringify({total,skipped,rows});
    }
    if(argv[0]!=="close") throw Error("Operazione sconosciuta.");
    ObjC.import("Foundation");
    const raw=$.NSFileHandle.fileHandleWithStandardInput.readDataToEndOfFile;
    const rows=JSON.parse(ObjC.unwrap($.NSString.alloc.initWithDataEncoding(raw,$.NSUTF8StringEncoding)));
    function verified(row) {
        const win=app.windows.byId(row.win);
        if(!win.exists() || win.mode()!=="normal") throw Error("Finestra cambiata.");
        const tab=win.tabs.byId(row.id), keep=win.tabs.byId(row.keep);
        if(row.id===row.keep || !tab.exists() || !keep.exists() || String(tab.url())!==row.url || String(keep.url())!==row.url)
            throw Error("Scheda o copia conservata cambiata. Ripeti la scansione.");
        return tab;
    }
    for (const row of rows) verified(row);
    let closed=0;
    for (const row of rows) {
        try { verified(row).close(); ++closed; }
        catch(e) { return JSON.stringify({closed,error:String(e)}); }
    }
    return JSON.stringify({closed});
}
"#;
pub(super) struct BrowserTab {
    win: String,
    id: String,
    title: String,
    url: String,
    keeper: Option<String>,
}
pub(super) struct Tabs {
    rows: Vec<BrowserTab>,
    total: u64,
    skipped: u64,
}
pub(super) fn parse(v: Value) -> Result<Tabs, String> {
    let invalid = || "Risposta Chrome incompleta.".to_owned();
    let mut rows = Vec::new();
    let mut ids = HashSet::new();
    let mut anchors = HashMap::new();
    for r in v["rows"].as_array().ok_or_else(invalid)? {
        let read = |k: &str| r[k].as_str().map(str::to_owned).ok_or_else(invalid);
        let win = read("win")?;
        let id = read("id")?;
        let url = read("url")?;
        if rows.len() >= 300
            || win.is_empty()
            || id.is_empty()
            || !ids.insert((win.clone(), id.clone()))
        {
            return Err(invalid());
        }
        let keeper = anchors.get(&(win.clone(), url.clone())).cloned();
        anchors
            .entry((win.clone(), url.clone()))
            .or_insert_with(|| id.clone());
        rows.push(BrowserTab {
            win,
            id,
            title: read("title")?,
            url,
            keeper,
        });
    }
    Ok(Tabs {
        rows,
        total: v["total"].as_u64().ok_or_else(invalid)?,
        skipped: v["skipped"].as_u64().ok_or_else(invalid)?,
    })
}
impl Accounts {
    pub(super) fn browser_tabs(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.separator();
        ui.label("Schede Chrome · finestre normali · fino a 300 schede web. I doppioni devono avere lo stesso URL nella stessa finestra.");
        if ui.button("Analizza schede Chrome").clicked() {
            self.reset();
            self.work(ctx, false, |stop| {
                parse(automation(SCRIPT, "scan", None, stop)?).map(Reply::Tabs)
            });
            return;
        }
        self.review_button(ui);
        if let Some(tabs) = &self.tabs {
            ui.label(format!(
                "{} schede web elencate di {} schede · {} escluse durante la lettura",
                tabs.rows.len(),
                tabs.total,
                tabs.skipped
            ));
            let mut change = None;
            egui::ScrollArea::vertical()
                .id_salt("browser_tabs")
                .show_rows(ui, 23.0, tabs.rows.len(), |ui, range| {
                    for i in range {
                        let t = &tabs.rows[i];
                        ui.push_id(i, |ui| {
                            ui.horizontal(|ui| {
                                if t.keeper.is_some() {
                                    let mut checked = self.selected.contains(&i);
                                    if ui.checkbox(&mut checked, "Doppione").changed() {
                                        change = Some((i, checked));
                                    }
                                } else {
                                    ui.label("Conservata");
                                }
                                ui.label(format!("Finestra {}", t.win));
                                ui.add(egui::Label::new(&t.title).truncate())
                                    .on_hover_text(&t.url);
                            })
                        });
                    }
                });
            if let Some((i, checked)) = change {
                self.select(i, checked);
            }
        }
    }
}
impl Tabs {
    pub(super) fn describe(&self, index: usize) -> String {
        let t = &self.rows[index];
        format!("Finestra {} · {} · {}", t.win, t.title, t.url)
    }
    pub(super) fn close_payload(&self, indices: &[usize]) -> Result<Value, String> {
        let mut rows = Vec::new();
        for &i in indices {
            let t = self.rows.get(i).ok_or("Selezione non valida.")?;
            let keep = t.keeper.as_ref().ok_or("Scheda protetta.")?;
            rows.push(json!({"id":t.id,"win":t.win,"url":t.url,"keep":keep}));
        }
        Ok(Value::Array(rows))
    }
}
pub(super) fn close(
    tabs: Tabs,
    indices: Vec<usize>,
    stop: &std::sync::atomic::AtomicBool,
) -> Result<Reply, String> {
    let f = json_input(&tabs.close_payload(&indices)?)?;
    let reply = automation(SCRIPT, "close", Some(f), stop)
        .map_err(|e| format!("Verifica le schede in Chrome prima di riprovare. {e}"))?;
    Ok(Reply::Action(format!(
        "{} di {} schede chiuse. {}",
        reply["closed"].as_u64().ok_or("Esito sconosciuto.")?,
        indices.len(),
        reply["error"].as_str().unwrap_or("")
    )))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn duplicates_require_exact_url_and_same_window() {
        let row = |w: &str, id: &str, u: &str| json!({"win":w,"id":id,"title":"fixture","url":u});
        let tabs=parse(json!({"rows":[row("1","a","https://fixture.invalid/"),row("2","b","https://fixture.invalid/"),row("1","c","https://fixture.invalid/"),row("1","d","https://fixture.invalid/#other")],"total":4,"skipped":0})).unwrap();
        assert_eq!(tabs.close_payload(&[2]).unwrap()[0]["keep"], "a");
        for i in [0, 1, 3] {
            assert!(tabs.close_payload(&[i]).is_err());
        }
    }
}
