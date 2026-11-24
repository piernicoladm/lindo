use std::fs::File;
use std::io::{self, Read};
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const LIMIT: usize = 96 * 1024;

#[derive(Debug)]
pub(crate) struct Output {
    pub(crate) code: Option<i32>,
    pub(crate) text: String,
    pub(crate) truncated: bool,
}

fn nonblocking(fd: i32) -> io::Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn drain(source: &mut impl Read, data: &mut Vec<u8>, clipped: &mut bool) -> io::Result<()> {
    let mut buffer = [0; 8192];
    for _ in 0..32 {
        match source.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => {
                let keep = n.min(LIMIT - data.len());
                data.extend_from_slice(&buffer[..keep]);
                *clipped |= keep < n;
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

fn terminate(child: &mut Child) {
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    let _ = child.kill();
    let _ = child.wait();
}

pub(crate) fn run(
    executable: &str,
    args: &[&str],
    input: Option<File>,
    cancel: &AtomicBool,
    seconds: u64,
) -> Result<Output, String> {
    capture(executable, args, input, cancel, seconds, false)
}

pub(crate) fn run_stdout(
    executable: &str,
    args: &[&str],
    input: Option<File>,
    cancel: &AtomicBool,
    seconds: u64,
) -> Result<Output, String> {
    capture(executable, args, input, cancel, seconds, true)
}

fn capture(
    executable: &str,
    args: &[&str],
    input: Option<File>,
    cancel: &AtomicBool,
    seconds: u64,
    stdout_on_success: bool,
) -> Result<Output, String> {
    if cancel.load(Ordering::Acquire) {
        return Err("Operazione annullata.".into());
    }
    let mut command = Command::new(executable);
    command
        .args(args)
        .process_group(0)
        .env("LC_ALL", "C")
        .env("HOMEBREW_NO_AUTO_UPDATE", "1")
        .env("HOMEBREW_NO_ANALYTICS", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if input.is_some() {
        unsafe extern "C" {
            fn setiopolicy_np(kind: i32, scope: i32, policy: i32) -> i32;
        }
        unsafe {
            command.pre_exec(|| {
                if setiopolicy_np(3, 0, 1) == 0 {
                    Ok(())
                } else {
                    Err(io::Error::last_os_error())
                }
            });
        }
    }
    command.stdin(input.map(Stdio::from).unwrap_or_else(Stdio::null));
    let mut child = command.spawn().map_err(|e| format!("{executable}: {e}"))?;
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let deadline = Instant::now() + Duration::from_secs(seconds);
    let mut data = Vec::new();
    let mut err = Vec::new();
    let mut clipped = false;
    let mut stderr_clipped = false;
    let result = (|| {
        nonblocking(stdout.as_raw_fd())
            .and_then(|_| nonblocking(stderr.as_raw_fd()))
            .map_err(|e| e.to_string())?;
        loop {
            if cancel.load(Ordering::Acquire) {
                return Err("Operazione annullata.".into());
            }
            if Instant::now() >= deadline {
                return Err("Tempo massimo superato. Operazione interrotta.".into());
            }
            let status = child.try_wait().map_err(|e| e.to_string())?;
            drain(&mut stdout, &mut data, &mut clipped)
                .and_then(|_| drain(&mut stderr, &mut err, &mut stderr_clipped))
                .map_err(|e| e.to_string())?;
            if let Some(status) = status {
                if (!stdout_on_success || !status.success()) && !err.is_empty() {
                    clipped |= stderr_clipped;
                    data.push(b'\n');
                    data.extend(err);
                }
                let text = String::from_utf8_lossy(&data).trim().to_owned();
                return Ok(Output {
                    code: status.code(),
                    text,
                    truncated: clipped,
                });
            }
            std::thread::sleep(Duration::from_millis(35));
        }
    })();
    if result.is_err() {
        terminate(&mut child);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structured_capture_bounds_stdout_independently_of_stderr() {
        let stop = AtomicBool::new(false);
        let out = run_stdout(
            "/bin/sh",
            &["-c", "echo '[]'; /usr/bin/head -c 110000 /dev/zero >&2"],
            None,
            &stop,
            5,
        )
        .unwrap();
        assert_eq!(out.code, Some(0));
        assert_eq!(out.text, "[]");
        assert!(!out.truncated);
        let out = run_stdout(
            "/usr/bin/head",
            &["-c", "110000", "/dev/zero"],
            None,
            &stop,
            5,
        )
        .unwrap();
        assert!(out.truncated);
        assert_eq!(out.text.len(), LIMIT);
    }

    #[test]
    fn structured_capture_keeps_success_json_and_failure_diagnostics() {
        let stop = AtomicBool::new(false);
        let script = r#"echo '{"rows":[]}'; echo warning >&2; exit "$1""#;
        for code in ["0", "3"] {
            let out =
                run_stdout("/bin/sh", &["-c", script, "fixture", code], None, &stop, 5).unwrap();
            assert_eq!(out.code, code.parse().ok());
            if code == "0" {
                assert_eq!(
                    serde_json::from_str::<serde_json::Value>(&out.text).unwrap()["rows"],
                    serde_json::json!([])
                );
            } else {
                assert!(out.text.contains("warning"));
            }
        }
    }
}
