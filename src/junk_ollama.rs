use crate::system_tools::command;
use eframe::egui;
use serde_json::Value;
use std::{
    collections::HashSet,
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
        Arc,
    },
    time::Duration,
};

#[derive(Clone, Debug, PartialEq, Eq)]
struct Model {
    name: String,
    digest: String,
    bytes: u64,
}
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_' | b':' | b'/'))
        && name.split('/').all(|part| {
            !part.is_empty()
                && part.as_bytes()[0].is_ascii_alphanumeric()
                && part != "."
                && part != ".."
        })
        && name.matches(':').count() <= 1
        && !name.ends_with(':')
}
fn parse(json: &str) -> Result<Vec<Model>, String> {
    let v: Value = serde_json::from_str(json).map_err(|_| "Risposta Ollama incompleta.")?;
    let mut models = Vec::new();
    let mut seen = HashSet::new();
    for row in v["models"].as_array().ok_or("Elenco Ollama incompleto.")? {
        let name = row["name"].as_str().ok_or("Nome modello mancante.")?;
        let digest = row["digest"].as_str().ok_or("Digest modello mancante.")?;
        let bytes = row["size"].as_u64().ok_or("Dimensione modello mancante.")?;
        if !valid_name(name)
            || digest.len() != 64
            || !digest.bytes().all(|b| b.is_ascii_hexdigit())
            || !seen.insert(name.to_ascii_lowercase())
        {
            return Err("Nomi o digest Ollama ambigui.".into());
        }
        models.push(Model {
            name: name.into(),
            digest: digest.to_ascii_lowercase(),
            bytes,
        });
    }
    models.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(models)
}
fn cli() -> Result<&'static str, String> {
    [
        "/opt/homebrew/bin/ollama",
        "/usr/local/bin/ollama",
        "/Applications/Ollama.app/Contents/Resources/ollama",
    ]
    .into_iter()
    .find(|path| Path::new(path).is_file())
    .ok_or_else(|| "CLI Ollama non installata nelle posizioni supportate.".into())
}
fn api(endpoint: &str, stop: &AtomicBool) -> Result<Vec<Model>, String> {
    let url = match endpoint {
        "tags" => "http://127.0.0.1:11434/api/tags",
        "ps" => "http://127.0.0.1:11434/api/ps",
        _ => return Err("Endpoint non supportato.".into()),
    };
    let out = command::run_stdout(
        "/usr/bin/curl",
        &[
            "--disable",
            "--noproxy",
            "*",
            "--fail",
            "--silent",
            "--connect-timeout",
            "2",
            "--max-time",
            "8",
            url,
        ],
        None,
        stop,
        10,
    )?;
    if out.code != Some(0) || out.truncated {
        return Err("Servizio Ollama locale non raggiungibile o elenco incompleto.".into());
    }
    parse(&out.text)
}
fn remove(
    model: &Model,
    read: impl Fn(&str) -> Result<Vec<Model>, String>,
    rm: impl FnOnce(&str) -> Result<(), String>,
) -> Result<(), String> {
    if !valid_name(&model.name) {
        return Err("Nome modello non valido.".into());
    }
    if read("ps")?
        .iter()
        .any(|m| m.name == model.name || m.digest == model.digest)
    {
        return Err("Il modello è caricato. Interrompine l’uso in Ollama e riprova.".into());
    }
    if !read("tags")?.contains(model) {
        return Err("Il modello è cambiato. Aggiorna l’elenco.".into());
    }
    rm(&model.name)
        .map_err(|e| format!("Verifica lo stato del modello prima di riprovare. {e}"))?;
    if read("tags")
        .map_err(|_| "Comando inviato. Aggiorna l’elenco per verificarne l’esito.")?
        .iter()
        .any(|m| m.name == model.name)
    {
        return Err("Ollama elenca ancora questo nome. Aggiorna l’inventario.".into());
    }
    Ok(())
}
enum Outcome {
    List(Vec<Model>),
    Removed(String),
}
struct Job {
    stop: Arc<AtomicBool>,
    rx: Receiver<Result<Outcome, String>>,
}
#[derive(Default)]
pub(super) struct Ollama {
    models: Vec<Model>,
    review: Option<Model>,
    job: Option<Job>,
    status: String,
}
impl Drop for Ollama {
    fn drop(&mut self) {
        if let Some(j) = &self.job {
            j.stop.store(true, Ordering::Release);
        }
    }
}
impl Ollama {
    fn start(
        &mut self,
        ctx: &egui::Context,
        task: impl FnOnce(&AtomicBool) -> Result<Outcome, String> + Send + 'static,
    ) {
        let (tx, rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        self.job = Some(Job {
            stop: stop.clone(),
            rx,
        });
        self.status = "Consultazione di Ollama…".into();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(task(&stop));
            ctx.request_repaint();
        });
    }
    pub(super) fn show(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.label("Modelli gestiti dal servizio Ollama locale su 127.0.0.1:11434.");
        if let Some(j) = &self.job {
            ctx.request_repaint_after(Duration::from_millis(120));
            if ui.button("Interrompi").clicked() {
                j.stop.store(true, Ordering::Release);
            }
            match j.rx.try_recv() {
                Ok(out) => {
                    self.job = None;
                    match out {
                        Ok(Outcome::List(rows)) => {
                            self.status = format!("{} modelli disponibili", rows.len());
                            self.models = rows;
                        }
                        Ok(Outcome::Removed(name)) => {
                            self.status = format!("Modello {name} rimosso e verificato.");
                            self.models.retain(|m| m.name != name);
                        }
                        Err(e) => {
                            self.models.clear();
                            self.status = e;
                        }
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.job = None;
                    self.models.clear();
                    self.status = "Operazione interrotta. Aggiorna l’elenco.".into();
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        ui.label(&self.status);
        if self.job.is_some() {
            return;
        }
        if let Some(model) = self.review.clone() {
            ui.heading("Rivedi la rimozione da Ollama");
            ui.label(format!(
                "{} · {:.2} GiB",
                model.name,
                model.bytes as f64 / 1073741824.
            ));
            ui.label(format!("Digest SHA-256 {}", model.digest));
            ui.label("Ollama rimuove il modello dal catalogo locale senza passare dal Cestino. Per ripristinarlo dovrai riscaricarlo o ricrearlo. I dati condivisi con altri modelli possono restare su disco.");
            if ui.button("Conferma rimozione da Ollama").clicked() {
                self.review = None;
                self.start(ctx, move |stop| {
                    let executable = cli()?;
                    remove(
                        &model,
                        |endpoint| api(endpoint, stop),
                        |name| {
                            let output = command::run(
                                "/usr/bin/env",
                                &[
                                    "OLLAMA_HOST=http://127.0.0.1:11434",
                                    "NO_PROXY=*",
                                    "no_proxy=*",
                                    executable,
                                    "rm",
                                    name,
                                ],
                                None,
                                stop,
                                60,
                            )?;
                            if output.code != Some(0) {
                                return Err("Comando ollama rm non completato.".into());
                            }
                            Ok(())
                        },
                    )?;
                    Ok(Outcome::Removed(model.name))
                });
            } else if ui.button("Indietro").clicked() {
                self.review = None;
            }
            return;
        }
        if ui.button("Elenca i modelli Ollama").clicked() {
            self.models.clear();
            self.start(ctx, |stop| {
                cli()?;
                api("tags", stop).map(Outcome::List)
            });
            return;
        }
        egui::ScrollArea::vertical().show_rows(ui, 26., self.models.len(), |ui, range| {
            for model in &self.models[range] {
                ui.horizontal(|ui| {
                    if ui.button("Rivedi rimozione").clicked() {
                        self.review = Some(model.clone());
                    }
                    ui.label(format!(
                        "{} · {:.2} GiB",
                        model.name,
                        model.bytes as f64 / 1073741824.
                    ));
                });
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    #[test]
    fn remove_checks_full_digest_and_running_aliases_before_single_cli_call() {
        let m = Model {
            name: "demo/test:v1".into(),
            digest: "a".repeat(64),
            bytes: 900,
        };
        let deleted = Cell::new(false);
        let read = |key: &str| {
            Ok(if key == "tags" && !deleted.get() {
                vec![m.clone()]
            } else {
                vec![]
            })
        };
        remove(&m, read, |name| {
            assert_eq!(name, m.name);
            deleted.set(true);
            Ok(())
        })
        .unwrap();
        assert!(deleted.get());
        let stale = Model {
            digest: "b".repeat(64),
            ..m.clone()
        };
        assert!(remove(
            &m,
            |k| Ok(if k == "tags" {
                vec![stale.clone()]
            } else {
                vec![]
            }),
            |_| panic!("unexpected rm")
        )
        .is_err());
        let alias = Model {
            name: "other:latest".into(),
            ..m.clone()
        };
        assert!(remove(&m, |_| Ok(vec![alias.clone()]), |_| panic!("loaded alias")).is_err());
    }
    #[test]
    fn rejects_duplicate_names_partial_digest_and_options() {
        let m = serde_json::json!({"name":"demo:latest","size":20,"digest":"a".repeat(64)});
        assert_eq!(
            parse(&serde_json::json!({"models":[m]}).to_string())
                .unwrap()
                .len(),
            1
        );
        assert!(parse(&serde_json::json!({"models":[m,m]}).to_string()).is_err());
        assert!(parse(r#"{"models":[{"name":"x","size":5,"digest":"deadbeef"}]}"#).is_err());
        for bad in ["-v", "../../other", ":80", "tag:", "a b", "a/b/c;rm"] {
            assert!(!valid_name(bad));
        }
    }
}
