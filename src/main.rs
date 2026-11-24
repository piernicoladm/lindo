mod accounts;
mod app_reset;
mod assessment;
mod clutter;
mod desktop;
mod extras;
mod file_versions;
mod junk;
mod legacy_junk;
mod model;
mod picker;
mod policy;
mod remote;
#[cfg(test)]
mod responsiveness_tests;
mod scan;
mod system_tools;
mod trash;
mod treemap;
mod ui;
mod vendor_updates;

use std::env;
use std::io;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

// Exercise the scanner from the actual bundled binary on a private temporary
// directory. The release scripts invoke this command before and after signing.
fn selftest() -> io::Result<()> {
    let parent = PathBuf::from(env::var_os("HOME").ok_or(io::ErrorKind::NotFound)?)
        .join("Library/Developer/Lindo/test-fixtures");
    std::fs::create_dir_all(&parent)?;
    let fixture = tempfile::Builder::new()
        .prefix("rust-bundle-")
        .tempdir_in(parent)?;
    let sample = fixture.path().join("sample");
    std::fs::write(&sample, b"Lindo synthetic bundle check")?;
    let request = model::Request {
        mode: model::Mode::Space,
        roots: vec![fixture.path().to_owned()],
        min_bytes: 0,
    };
    let report = scan::run(
        &request,
        &AtomicBool::new(false),
        &model::Progress::default(),
    );
    if report.errors != 0
        || report.cancelled
        || report.items.len() != 1
        || report.items[0].path != sample
    {
        return Err(io::Error::other(format!(
            "Unexpected scan result: {report:?}"
        )));
    }
    println!("Lindo {} bundle selftest passed", env!("CARGO_PKG_VERSION"));
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args_os().skip(1);
    match args.next().as_deref().and_then(|a| a.to_str()) {
        None => ui::launch(None)?,
        Some("-AppleInterfaceStyle") => {
            let theme = match args.next().as_deref().and_then(|v| v.to_str()) {
                Some("Dark") => eframe::egui::Theme::Dark,
                Some("Light") => eframe::egui::Theme::Light,
                _ => return Err("Expected Dark or Light".into()),
            };
            ui::launch(Some(theme))?;
        }
        Some("--version") => println!("Lindo {}", env!("CARGO_PKG_VERSION")),
        Some("--selftest") => selftest()?,
        #[cfg(feature = "ui-probe")]
        Some("--ui-probe") => {
            let mode = match args.next().as_deref().and_then(|a| a.to_str()) {
                Some("space") => model::Mode::Space,
                Some("duplicates") => model::Mode::Duplicates,
                _ => return Err("Expected space or duplicates".into()),
            };
            let root = PathBuf::from(args.next().ok_or("Expected a synthetic fixture path")?);
            ui::launch_fixture(root, mode)?;
        }
        Some(_) => return Err("Unsupported Lindo command line option".into()),
    }
    Ok(())
}
