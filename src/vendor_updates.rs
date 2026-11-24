use crate::{model::Mode, policy, system_tools::command};
use eframe::egui;
use quick_xml::{
    events::{BytesStart, Event},
    Reader,
};
use std::{
    cmp::Ordering as Cmp,
    fs,
    net::{IpAddr, Ipv4Addr},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
        Arc,
    },
    time::Duration,
};
use url::{Host, Url};

#[derive(Clone, Debug, PartialEq)]
struct App {
    path: PathBuf,
    id: String,
    build: String,
    version: String,
    feed: Url,
    dev: u64,
    inode: u64,
}
fn endpoint(raw: &str) -> Result<Url, String> {
    let u = Url::parse(raw).map_err(|_| "URL del feed non valido.")?;
    let Some(Host::Domain(h)) = u.host() else {
        return Err("Il feed deve usare un dominio pubblico HTTPS.".into());
    };
    if u.scheme() != "https"
        || u.port_or_known_default() != Some(443)
        || !u.username().is_empty()
        || u.password().is_some()
        || u.query().is_some()
        || u.fragment().is_some()
        || !h.contains('.')
        || h.ends_with('.')
        || [
            ".localhost",
            ".local",
            ".internal",
            ".home",
            ".lan",
            ".test",
            ".invalid",
            ".onion",
        ]
        .iter()
        .any(|suffix| h.ends_with(suffix))
    {
        return Err(
            "Il feed richiede un dominio pubblico HTTPS senza credenziali o parametri.".into(),
        );
    }
    Ok(u)
}
fn public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    !(a == 0
        || a == 10
        || a == 127
        || a >= 224
        || (a == 100 && (64..128).contains(&b))
        || (a == 169 && b == 254)
        || (a == 172 && (16..32).contains(&b))
        || (a == 192 && (b == 168 || b == 0 || (b == 88 && c == 99)))
        || (a == 198 && (b == 18 || b == 19 || (b == 51 && c == 100)))
        || (a == 203 && b == 0 && c == 113))
}
fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => public_v4(v),
        IpAddr::V6(v) => {
            let s = v.segments();
            (s[0] & 0xe000) == 0x2000
                && s[0] != 0x2002
                && !(s[0] == 0x2001 && (s[1] < 0x0200 || s[1] == 0xdb8))
                && (s[0] & 0xfff0) != 0x3ff0
        }
    }
}
fn dns(host: &str, stop: &AtomicBool) -> Result<IpAddr, String> {
    let output = command::run(
        "/usr/bin/dscacheutil",
        &["-q", "host", "-a", "name", host],
        None,
        stop,
        8,
    )?;
    if output.code != Some(0) || output.truncated {
        return Err("Risoluzione del dominio incompleta.".into());
    }
    let ips: Vec<IpAddr> = output
        .text
        .lines()
        .filter_map(|l| l.split_once(':'))
        .filter(|(k, _)| matches!(*k, "ip_address" | "ipv6_address"))
        .flat_map(|(_, v)| v.split_whitespace())
        .map(str::parse)
        .collect::<Result<_, _>>()
        .map_err(|_| "Indirizzo del feed non valido.")?;
    if ips.is_empty() || ips.iter().any(|ip| !public_ip(*ip)) {
        return Err("Dominio locale o non verificabile.".into());
    }
    Ok(*ips.iter().find(|ip| ip.is_ipv4()).unwrap_or(&ips[0]))
}
const LIMIT: u64 = 2 * 1024 * 1024;
fn fetch(mut feed: Url, stop: &AtomicBool) -> Result<Vec<u8>, String> {
    for _ in 0..=3 {
        let host = feed.host_str().ok_or("Dominio mancante.")?;
        let ip = dns(host, stop)?;
        let pin = match ip {
            IpAddr::V4(v) => format!("{host}:443:{v}"),
            IpAddr::V6(v) => format!("{host}:443:[{v}]"),
        };
        let temp = tempfile::NamedTempFile::new()
            .map_err(|_| "Impossibile leggere il feed temporaneo.")?;
        let path = temp
            .path()
            .to_str()
            .ok_or("Percorso temporaneo non valido.")?;
        let result = command::run(
            "/usr/bin/curl",
            &[
                "--disable",
                "--silent",
                "--proto",
                "=https",
                "--noproxy",
                "*",
                "--resolve",
                &pin,
                "--connect-timeout",
                "5",
                "--max-time",
                "16",
                "--max-filesize",
                "2097152",
                "--output",
                path,
                "--write-out",
                "%{http_code}\n%{redirect_url}",
                feed.as_str(),
            ],
            None,
            stop,
            18,
        )?;
        if result.code != Some(0) || result.truncated {
            return Err("Feed non disponibile, troppo grande o certificato TLS non valido.".into());
        }
        let mut lines = result.text.lines();
        match lines.next().unwrap_or("") {
            "200" => {
                if temp
                    .as_file()
                    .metadata()
                    .map_err(|_| "Feed non disponibile.")?
                    .len()
                    > LIMIT
                {
                    return Err("Feed troppo grande.".into());
                }
                return fs::read(temp.path()).map_err(|_| "Lettura del feed non riuscita.".into());
            }
            "301" | "302" | "303" | "307" | "308" => {
                feed = endpoint(lines.next().ok_or("Redirect privo di destinazione.")?)?;
            }
            _ => return Err("L’editore non ha restituito un feed utilizzabile.".into()),
        }
    }
    Err("Troppi reindirizzamenti nel feed.".into())
}
fn plist(path: &Path, key: &str, stop: &AtomicBool) -> Option<String> {
    if !policy::local_path(path).unwrap_or(false) {
        return None;
    }
    let f = policy::open_file(path, 0).ok()?;
    let meta = f.metadata().ok()?;
    if !meta.is_file() || meta.len() > 1024 * 1024 {
        return None;
    }
    let output = command::run(
        "/usr/bin/plutil",
        &["-extract", key, "raw", "-o", "-", "-"],
        Some(f),
        stop,
        4,
    )
    .ok()?;
    (output.code == Some(0) && !output.truncated && !output.text.is_empty()).then_some(output.text)
}
fn read_app(path: &Path, stop: &AtomicBool) -> Option<App> {
    if policy::protected(path, Mode::Apps, None)
        || !policy::local_path(path).ok()?
        || !path
            .extension()
            .is_some_and(|s| s.eq_ignore_ascii_case("app"))
    {
        return None;
    }
    let m = policy::open_file(path, libc::O_DIRECTORY)
        .ok()?
        .metadata()
        .ok()?;
    let info = path.join("Contents/Info.plist");
    let feed = endpoint(&plist(&info, "SUFeedURL", stop)?).ok()?;
    let id = policy::app_identifier(path).ok()?;
    if id.to_ascii_lowercase().starts_with("com.apple.")
        || id.eq_ignore_ascii_case("com.piernicolademaria.lindo")
    {
        return None;
    }
    let build = plist(&info, "CFBundleVersion", stop)?;
    Some(App {
        path: path.into(),
        id,
        version: plist(&info, "CFBundleShortVersionString", stop).unwrap_or_else(|| build.clone()),
        build,
        feed,
        dev: m.dev(),
        inode: m.ino(),
    })
}
fn discover(roots: &[PathBuf], stop: &AtomicBool) -> Result<(Vec<App>, usize), String> {
    let _local = policy::LocalOnly::enter().map_err(|_| "Protezione cloud non disponibile.")?;
    let mut apps = Vec::new();
    let mut omitted = 0;
    for root in roots {
        if !root.exists() {
            continue;
        }
        let Ok(mut dir) = policy::children(root) else {
            omitted += 1;
            continue;
        };
        while let Some(name) = dir.next() {
            if stop.load(Ordering::Acquire) {
                return Err("Ricerca annullata.".into());
            }
            let Ok(name) = name else {
                omitted += 1;
                continue;
            };
            if dir
                .stat(&name)
                .is_ok_and(|s| s.st_mode & libc::S_IFMT == libc::S_IFDIR)
            {
                if let Some(info) = read_app(&root.join(name), stop) {
                    apps.push(info);
                }
            }
        }
    }
    apps.sort_by(|a, b| a.path.file_name().cmp(&b.path.file_name()));
    Ok((apps, omitted))
}
fn current(app: &App, stop: &AtomicBool) -> Result<(), String> {
    let _local = policy::LocalOnly::enter().map_err(|_| "Protezione cloud non disponibile.")?;
    if read_app(&app.path, stop).as_ref() != Some(app) {
        return Err("L’app è cambiata. Aggiorna l’inventario.".into());
    }
    Ok(())
}

#[derive(Default)]
struct Release {
    legacy_build: String,
    legacy_version: String,
    build: String,
    version: String,
    min: String,
    max: String,
    channel: String,
    title: String,
    hardware: String,
    mac: bool,
}
fn set_field(r: &mut Release, field: &[u8], text: &str) {
    let dest = match field {
        b"sparkle:version" => &mut r.build,
        b"sparkle:shortVersionString" => &mut r.version,
        b"sparkle:minimumSystemVersion" => &mut r.min,
        b"sparkle:maximumSystemVersion" => &mut r.max,
        b"sparkle:channel" => &mut r.channel,
        b"title" => &mut r.title,
        b"sparkle:hardwareRequirements" => &mut r.hardware,
        _ => return,
    };
    if dest.len() + text.len() < 2048 {
        dest.push_str(text);
    }
}
fn enclosure(r: &mut Release, node: &BytesStart<'_>) -> Result<(), String> {
    let mut os = String::new();
    let mut build = String::new();
    let mut version = String::new();
    let mut url = false;
    for attribute in node.attributes() {
        let attr = attribute.map_err(|_| "Attributi XML non validi.")?;
        let s =
            std::str::from_utf8(&attr.value).map_err(|_| "Codifica del feed non supportata.")?;
        match attr.key.as_ref() {
            b"sparkle:os" => os = s.into(),
            b"sparkle:version" => build = s.into(),
            b"sparkle:shortVersionString" => version = s.into(),
            b"url" => url = !s.is_empty(),
            _ => {}
        }
    }
    if url && (os.is_empty() || os == "macos") {
        r.mac = true;
        r.legacy_build = build;
        r.legacy_version = version;
    }
    Ok(())
}
fn releases(xml: &[u8]) -> Result<Vec<Release>, String> {
    if xml.len() as u64 > LIMIT {
        return Err("Feed troppo grande.".into());
    }
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().trim_text(true);
    let mut stack = Vec::<Vec<u8>>::new();
    let mut result = Vec::new();
    let mut release: Option<Release> = None;
    loop {
        let event = reader
            .read_event()
            .map_err(|_| "XML dell’appcast non valido.")?;
        match event {
            Event::Start(e) => {
                if stack.len() > 40 {
                    return Err("Struttura XML troppo profonda.".into());
                }
                let n = e.name().as_ref().to_vec();
                if n == b"item" && stack.len() == 2 && stack[0] == b"rss" && stack[1] == b"channel"
                {
                    release = Some(Release::default());
                }
                if n == b"enclosure" && stack.len() == 3 {
                    if let Some(r) = &mut release {
                        enclosure(r, &e)?;
                    }
                }
                stack.push(n);
            }
            Event::Empty(e) if e.name().as_ref() == b"enclosure" && stack.len() == 3 => {
                if let Some(r) = &mut release {
                    enclosure(r, &e)?;
                }
            }
            Event::Text(e) if stack.len() == 4 => {
                if let Some(r) = &mut release {
                    set_field(
                        r,
                        &stack[3],
                        &e.decode().map_err(|_| "Codifica XML non valida.")?,
                    );
                }
            }
            Event::CData(e) if stack.len() == 4 => {
                if let Some(r) = &mut release {
                    set_field(
                        r,
                        &stack[3],
                        &e.decode().map_err(|_| "Codifica XML non valida.")?,
                    );
                }
            }
            Event::End(e) => {
                if e.name().as_ref() == b"item" && stack.len() == 3 {
                    if let Some(mut r) = release.take() {
                        if r.build.is_empty() {
                            r.build = std::mem::take(&mut r.legacy_build);
                        }
                        if r.version.is_empty() {
                            r.version = std::mem::take(&mut r.legacy_version);
                        }
                        result.push(r);
                    }
                }
                stack.pop();
            }
            Event::DocType(_) => return Err("Dichiarazioni DTD non ammesse.".into()),
            Event::Eof => break,
            _ => {}
        }
    }
    if !stack.is_empty() || result.is_empty() {
        return Err("Nessuna release riconosciuta nel feed.".into());
    }
    Ok(result)
}
fn numbers(s: &str) -> Option<Vec<u64>> {
    let mut v = s
        .trim()
        .split('.')
        .map(str::parse::<u64>)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    while v.len() > 1 && v.last() == Some(&0) {
        v.pop();
    }
    Some(v)
}
fn prerelease(s: &str) -> bool {
    let s = s.to_ascii_lowercase();
    [
        "alpha",
        "beta",
        "preview",
        "nightly",
        "canary",
        "release candidate",
    ]
    .iter()
    .any(|p| s.contains(p))
        || s.split(|c: char| !c.is_alphanumeric()).any(|s| {
            s == "rc"
                || s.strip_prefix("rc").is_some_and(|tail| {
                    !tail.is_empty() && tail.bytes().all(|c| c.is_ascii_digit())
                })
        })
}
fn latest(xml: &[u8], build: &str, system: &str) -> Result<String, String> {
    let installed = numbers(build)
        .ok_or("L’app usa un confronto di versione personalizzato. Verifica nell’app.")?;
    let os = numbers(system).ok_or("Versione di macOS non disponibile.")?;
    let mut newest: Option<(Vec<u64>, String)> = None;
    for r in releases(xml)? {
        if !r.mac
            || (!r.channel.is_empty() && !r.channel.eq_ignore_ascii_case("stable"))
            || prerelease(&r.version)
            || prerelease(&r.title)
            || (!r.hardware.is_empty() && !(r.hardware == "arm64" && cfg!(target_arch = "aarch64")))
        {
            continue;
        }
        if !r.min.is_empty() && !numbers(&r.min).is_some_and(|v| v <= os) {
            continue;
        }
        if !r.max.is_empty() && !numbers(&r.max).is_some_and(|v| v >= os) {
            continue;
        }
        let Some(version) = numbers(&r.build) else {
            continue;
        };
        if newest.as_ref().is_none_or(|(n, _)| version > *n) {
            let label = if r.version.is_empty() {
                r.build
            } else {
                r.version
            };
            newest = Some((version, label));
        }
    }
    let (v, label) =
        newest.ok_or("Il feed non contiene una versione stabile compatibile e confrontabile.")?;
    Ok(match v.cmp(&installed) {
        Cmp::Greater => format!("Aggiornamento stabile disponibile · {label}"),
        Cmp::Equal => format!("Versione installata allineata al feed · {label}"),
        Cmp::Less => {
            format!("La build installata è più recente della versione stabile nel feed · {label}")
        }
    })
}
fn check(app: &App, stop: &AtomicBool) -> Result<String, String> {
    current(app, stop)?;
    let os = command::run("/usr/bin/sw_vers", &["-productVersion"], None, stop, 5)?;
    if os.code != Some(0) {
        return Err("Versione macOS non disponibile.".into());
    }
    latest(&fetch(app.feed.clone(), stop)?, &app.build, &os.text)
}
fn launch(app: &App, stop: &AtomicBool) -> Result<String, String> {
    current(app, stop)?;
    let p = app.path.to_str().ok_or("Percorso non valido.")?;
    let signed = command::run(
        "/usr/bin/codesign",
        &["--verify", "--strict", "-R=anchor apple generic", p],
        None,
        stop,
        25,
    )?;
    if signed.code != Some(0) {
        return Err(
            "Firma dell’applicazione non verificata. Aprila manualmente dal Finder.".into(),
        );
    }
    current(app, stop)?;
    let out = command::run("/usr/bin/open", &["-a", p], None, stop, 8)?;
    if out.code != Some(0) {
        return Err("Apertura dell’app non riuscita.".into());
    }
    Ok("App aperta. Usa il suo comando Cerca aggiornamenti.".into())
}
enum ResultView {
    Apps(Vec<App>, usize),
    Message(App, String),
}
struct Job {
    stop: Arc<AtomicBool>,
    rx: Receiver<Result<ResultView, String>>,
}
#[derive(Default)]
pub(crate) struct VendorUpdates {
    apps: Vec<App>,
    job: Option<Job>,
    status: String,
    checked: Option<App>,
    confirm: bool,
}
impl Drop for VendorUpdates {
    fn drop(&mut self) {
        if let Some(job) = &self.job {
            job.stop.store(true, Ordering::Release);
        }
    }
}
impl VendorUpdates {
    fn start(
        &mut self,
        ctx: &egui::Context,
        task: impl FnOnce(&AtomicBool) -> Result<ResultView, String> + Send + 'static,
    ) {
        let (tx, rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        self.job = Some(Job {
            stop: stop.clone(),
            rx,
        });
        let ctx = ctx.clone();
        self.status = "Verifica in corso…".into();
        self.confirm = false;
        self.checked = None;
        std::thread::spawn(move || {
            let _ = tx.send(task(&stop));
            ctx.request_repaint();
        });
    }
    pub(crate) fn show(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.heading("Aggiornamenti degli editori");
        ui.label("App di terzi in Applicazioni che pubblicano un feed Sparkle HTTPS. Scegli Verifica per contattare l’editore.");
        if let Some(job) = &self.job {
            ctx.request_repaint_after(Duration::from_millis(150));
            if ui.button("Annulla").clicked() {
                job.stop.store(true, Ordering::Release);
            }
            match job.rx.try_recv() {
                Ok(res) => {
                    self.job = None;
                    match res {
                        Ok(ResultView::Apps(apps, omitted)) => {
                            self.status = format!(
                                "{} app con feed supportato · {} cartelle non accessibili",
                                apps.len(),
                                omitted
                            );
                            self.apps = apps;
                        }
                        Ok(ResultView::Message(app, msg)) => {
                            self.status = format!(
                                "{} · {msg}",
                                app.path.file_name().unwrap_or_default().to_string_lossy()
                            );
                            self.checked = Some(app);
                        }
                        Err(e) => self.status = e,
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.job = None;
                    self.status = "Verifica interrotta.".into();
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        ui.label(&self.status);
        if self.job.is_some() {
            return;
        }
        if let Some(app) = self.checked.clone() {
            if self.confirm {
                ui.label(format!(
                    "Aprire {} per usare la procedura di aggiornamento prevista dall’editore?",
                    app.path.display()
                ));
                if ui.button("Conferma apertura dell’app").clicked() {
                    self.start(ctx, move |stop| {
                        launch(&app, stop).map(|msg| ResultView::Message(app, msg))
                    });
                    return;
                }
                if ui.button("Indietro").clicked() {
                    self.confirm = false;
                }
            } else if ui.button("Apri l’app per aggiornare").clicked() {
                self.confirm = true;
            }
        }
        if ui.button("Cerca app con Sparkle").clicked() {
            self.apps.clear();
            self.start(ctx, |stop| {
                let (apps, omitted) = discover(
                    &[
                        PathBuf::from("/Applications"),
                        policy::home().join("Applications"),
                    ],
                    stop,
                )?;
                Ok(ResultView::Apps(apps, omitted))
            });
            return;
        }
        let mut chosen = None;
        egui::ScrollArea::vertical().show_rows(ui, 42., self.apps.len(), |ui, range| {
            for a in &self.apps[range] {
                ui.horizontal(|ui| {
                    if ui.button("Verifica").clicked() {
                        chosen = Some(a.clone())
                    }
                    ui.label(format!(
                        "{} · {}",
                        a.path.file_name().unwrap_or_default().to_string_lossy(),
                        a.version
                    ));
                });
                ui.small(a.feed.host_str().unwrap_or(""));
            }
        });
        if let Some(app) = chosen {
            self.start(ctx, move |stop| {
                if stop.load(Ordering::Acquire) {
                    return Err("Verifica annullata.".into());
                }
                let message =
                    check(&app, stop).unwrap_or_else(|e| format!("Verifica incompleta. {e}"));
                Ok(ResultView::Message(app, message))
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn checks_signed_appcast_versions_against_os_and_stable_channel() {
        let xml=br#"<rss xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle"><channel>
        <item><title>Stable</title><sparkle:version>9</sparkle:version><sparkle:shortVersionString>2.1</sparkle:shortVersionString>
          <sparkle:minimumSystemVersion>13.0.0</sparkle:minimumSystemVersion><enclosure url="https://publisher.example/release.zip" /></item>
        <item><title>Requires macOS 30</title><sparkle:version>10</sparkle:version><sparkle:minimumSystemVersion>30</sparkle:minimumSystemVersion><enclosure url="https://publisher.example/new.zip"/></item>
        <item><sparkle:version>11</sparkle:version><sparkle:channel>beta</sparkle:channel><enclosure url="https://publisher.example/beta.zip"/></item>
        <item><title>2.2 RC1</title><enclosure url="https://publisher.example/rc.zip" sparkle:version="12"/></item>
        <item><enclosure url="https://publisher.example/win.zip" sparkle:version="20" sparkle:os="windows"/></item>
        </channel></rss>"#;
        assert!(latest(xml, "8", "14.1")
            .unwrap()
            .contains("disponibile · 2.1"));
        assert!(latest(xml, "9.0.0", "14.1").unwrap().contains("allineata"));
        assert!(latest(xml, "git-hash", "14.1").is_err());
        assert!(latest(xml, "8", "12.7").is_err());
        let old=br#"<rss><channel><item><enclosure url="https://example.org/file.zip" sparkle:version="10.0" sparkle:shortVersionString="3.5"/></item></channel></rss>"#;
        assert!(latest(old, "9.9", "14").unwrap().contains("3.5"));
        assert!(releases(b"<!DOCTYPE foo SYSTEM 'file:///etc/passwd'><rss/>").is_err());
    }
    #[test]
    fn refuses_private_destinations_and_credential_urls() {
        for raw in [
            "http://example.org/feed",
            concat!("https://", "user:", "sample", "@example.org/feed"),
            "https://127.0.0.1/f",
            "https://[::1]/",
            "https://test.local/f",
            "https://example.org:9001/f",
            "https://example.org/feed?token=secret",
        ] {
            assert!(endpoint(raw).is_err(), "{raw}")
        }
        assert!(endpoint("https://updates.example.org/stable.xml").is_ok());
        for ip in [
            "127.0.0.1",
            "10.2.3.4",
            "192.168.1.1",
            "169.254.169.254",
            "100.65.0.1",
            "198.51.100.20",
            "::ffff:127.0.0.1",
            "fe80::1",
            "2002:7f00:0001::1",
            "2001:db8::1",
            "fc00::9",
        ] {
            assert!(!public_ip(ip.parse().unwrap()), "{ip}")
        }
        assert!(public_ip("8.8.8.8".parse().unwrap()));
        assert!(public_ip("2606:4700::1111".parse().unwrap()));
    }
    #[test]
    fn discovers_fixture_app_without_contacting_its_feed() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let app = root.join("Fixture.app");
        fs::create_dir_all(app.join("Contents")).unwrap();
        let plist = r#"<?xml version="1.0"?><plist version="1.0"><dict>
        <key>CFBundleIdentifier</key><string>org.example.fixture</string>
        <key>CFBundleVersion</key><string>9</string><key>CFBundleShortVersionString</key><string>2.1</string>
        <key>SUFeedURL</key><string>https://updates.example.org/fixture.xml</string></dict></plist>"#;
        fs::write(app.join("Contents/Info.plist"), plist).unwrap();
        let stop = AtomicBool::new(false);
        let (list, errors) = discover(std::slice::from_ref(&root), &stop).unwrap();
        assert_eq!(errors, 0);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].version, "2.1");
        assert!(current(&list[0], &stop).is_ok());
        fs::write(
            app.join("Contents/Info.plist"),
            plist.replace(">9<", ">10<"),
        )
        .unwrap();
        assert!(current(&list[0], &stop).is_err());
    }
}
