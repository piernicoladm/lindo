use crate::model::{Mode, Progress, Request};
use crate::scan;
use std::fs::{self, File};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

#[test]
fn cancel_during_hash_of_large_files() {
    // The test-only root policy is injected into a separate test process.
    let fixture = tempfile::Builder::new()
        .prefix("lindo-hash-")
        .tempdir()
        .unwrap();
    let root = fixture.path().canonicalize().unwrap();
    let result = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "responsiveness_tests::hash_cancellation_worker",
            "--ignored",
        ])
        .env("LINDO_FIXTURE_WORKER", root)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stdout)
    );
}

#[test]
#[ignore = "invoked by cancel_during_hash_of_large_files on a private temporary root"]
fn hash_cancellation_worker() {
    let home = std::path::PathBuf::from(std::env::var_os("LINDO_FIXTURE_WORKER").unwrap());
    let root = home.join("hash-cancel-fixture");
    fs::create_dir(&root).unwrap();
    for name in ["a.bin", "b.bin"] {
        File::create(root.join(name))
            .unwrap()
            .set_len(1 << 30)
            .unwrap();
    }
    let req = Request {
        mode: Mode::Duplicates,
        roots: vec![root],
        min_bytes: 1,
    };
    let stop = AtomicBool::new(false);
    let progress = Progress::default();
    std::thread::scope(|scope| {
        let worker = scope.spawn(|| scan::run(&req, &stop, &progress));
        let start = Instant::now();
        while progress.hashed_bytes.load(Ordering::Acquire) == 0 {
            assert!(
                !worker.is_finished() && start.elapsed() < Duration::from_secs(15),
                "worker finished or never reported hash progress"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        let cancel_start = Instant::now();
        stop.store(true, Ordering::Release);
        let report = worker.join().unwrap();
        assert!(report.cancelled && report.items.is_empty());
        assert_eq!(report.visited, 2);
        assert!(cancel_start.elapsed() < Duration::from_secs(2));
        println!(
            "hash cancellation after {:?}, {} bytes read",
            cancel_start.elapsed(),
            progress.hashed_bytes.load(Ordering::Acquire)
        );
    });
}
