//! Local menu-bar sampling. No network requests or interface names are needed.
use crate::system_tools::command;
use eframe::egui;
use std::{
    collections::BTreeMap,
    mem::{size_of, MaybeUninit},
    ptr,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    time::{Duration, Instant},
};

pub(super) const PERIOD: Duration = Duration::from_secs(15);
type Counters = BTreeMap<u16, (u64, u64)>;

fn rates(previous: &Counters, current: &Counters, elapsed: Duration) -> Option<(u64, u64)> {
    // Interface additions/removals and reset counters start a fresh baseline.
    if elapsed.is_zero() || current.is_empty() || current.len() != previous.len() {
        return None;
    }
    let mut rx = 0u64;
    let mut tx = 0u64;
    for (index, (received, sent)) in current {
        let (before_rx, before_tx) = previous.get(index)?;
        rx = rx.checked_add(received.checked_sub(*before_rx)?)?;
        tx = tx.checked_add(sent.checked_sub(*before_tx)?)?;
    }
    let per_second =
        |bytes: u64| u64::try_from(u128::from(bytes) * 1_000_000_000 / elapsed.as_nanos()).ok();
    Some((per_second(rx)?, per_second(tx)?))
}

fn network() -> Option<Counters> {
    // NET_RT_IFLIST2 exposes 64-bit byte counters. Only headers for running
    // Ethernet-type interfaces (including Wi-Fi) are decoded. Associated
    // addresses, interface names and route payloads are ignored.
    let mut mib = [libc::CTL_NET, libc::PF_ROUTE, 0, 0, libc::NET_RT_IFLIST2, 0];
    let mut n = 0usize;
    let mut query = |out: *mut libc::c_void, len: &mut usize| unsafe {
        libc::sysctl(mib.as_mut_ptr(), 6, out, len, ptr::null_mut(), 0)
    };
    if query(ptr::null_mut(), &mut n) != 0 || n == 0 || n > 4 * 1024 * 1024 {
        return None;
    }
    let mut buffer = vec![0u8; n];
    if query(buffer.as_mut_ptr().cast(), &mut n) != 0 || n > buffer.len() {
        return None;
    }
    interface_counts(&buffer[..n])
}

fn interface_counts(mut slice: &[u8]) -> Option<Counters> {
    let mut result = Counters::new();
    while !slice.is_empty() {
        if slice.len() < 4 {
            return None;
        }
        let size = u16::from_ne_bytes([slice[0], slice[1]]) as usize;
        if size < 4 || size > slice.len() {
            return None;
        }
        if slice[3] == libc::RTM_IFINFO2 as u8 {
            if size < size_of::<libc::if_msghdr2>() {
                return None;
            }
            // Routing messages are variable-length and need not be aligned.
            let h = unsafe { slice.as_ptr().cast::<libc::if_msghdr2>().read_unaligned() };
            let flags = h.ifm_flags;
            let up = libc::IFF_UP | libc::IFF_RUNNING;
            if flags & up == up
                && flags & libc::IFF_LOOPBACK == 0
                && h.ifm_data.ifi_type == 6
                && result
                    .insert(h.ifm_index, (h.ifm_data.ifi_ibytes, h.ifm_data.ifi_obytes))
                    .is_some()
            {
                return None;
            }
        }
        slice = &slice[size..];
    }
    Some(result)
}

unsafe extern "C" {
    fn mach_host_self() -> libc::mach_port_t;
    static mach_task_self_: libc::mach_port_t;
    fn mach_port_deallocate(
        task: libc::mach_port_t,
        name: libc::mach_port_t,
    ) -> libc::kern_return_t;
}
fn memory() -> Option<String> {
    let mut total = 0u64;
    let mut len = size_of::<u64>();
    if unsafe {
        libc::sysctlbyname(
            c"hw.memsize".as_ptr(),
            (&mut total as *mut u64).cast(),
            &mut len,
            ptr::null_mut(),
            0,
        )
    } != 0
        || len != 8
        || total == 0
    {
        return None;
    }
    let page_size = u64::try_from(unsafe { libc::sysconf(libc::_SC_PAGESIZE) })
        .ok()
        .filter(|s| *s > 0)?;
    let mut stats = MaybeUninit::<libc::vm_statistics64>::zeroed();
    let mut count = libc::HOST_VM_INFO64_COUNT;
    // Pair mach_host_self's send right with a deallocation on every sample.
    let result = unsafe {
        let host = mach_host_self();
        let code = libc::host_statistics64(
            host,
            libc::HOST_VM_INFO64,
            stats.as_mut_ptr().cast(),
            &mut count,
        );
        mach_port_deallocate(mach_task_self_, host);
        code
    };
    let required =
        std::mem::offset_of!(libc::vm_statistics64, compressor_page_count) + size_of::<u32>();
    if result != 0 || (count as usize) * size_of::<libc::integer_t>() < required {
        return None;
    }
    let stats = unsafe { stats.assume_init() };
    let free = u64::from(stats.free_count).checked_mul(page_size)?;
    let compressed = u64::from(stats.compressor_page_count).checked_mul(page_size)?;
    if free > total || compressed > total {
        return None;
    }
    Some(format!(
        "RAM · {} liberi su {} · {} compressi",
        crate::ui::size(free),
        crate::ui::size(total),
        crate::ui::size(compressed)
    ))
}

fn battery(text: &str) -> Option<String> {
    let mut internal = text
        .lines()
        .filter(|s| s.trim_start().starts_with("-InternalBattery-"));
    let Some(line) = internal.next() else {
        return text
            .lines()
            .any(|l| l.starts_with("Now drawing from "))
            .then(|| "Batteria interna non rilevata".into());
    };
    if internal.next().is_some() {
        return None;
    }
    let (before, after) = line.split_once('%')?;
    let percent = before
        .split_whitespace()
        .next_back()?
        .parse::<u32>()
        .ok()
        .filter(|p| *p <= 100)?;
    let state = after.split(';').map(str::trim).find_map(|p| match p {
        "charging" => Some("in carica"),
        "discharging" => Some("in scarica"),
        "charged" => Some("carica"),
        "AC attached" => Some("alimentatore collegato"),
        "not charging" => Some("carica sospesa"),
        _ => None,
    });
    Some(match state {
        Some(s) => format!("Batteria · {percent}% · {s}"),
        None => format!("Batteria · {percent}%"),
    })
}

fn sample(previous: &mut Option<(Instant, Counters)>, stop: &AtomicBool) -> Vec<String> {
    let mut lines = Vec::new();
    let mut averages = [0.; 3];
    if unsafe { libc::getloadavg(averages.as_mut_ptr(), 3) } == 3 {
        lines.push(format!(
            "Carico medio 1 / 5 / 15 min · {:.2} / {:.2} / {:.2}",
            averages[0], averages[1], averages[2]
        ));
    }
    lines.push(memory().unwrap_or_else(|| "RAM · dati non disponibili".into()));
    let power = command::run("/usr/bin/pmset", &["-g", "batt"], None, stop, 3)
        .ok()
        .filter(|r| r.code == Some(0) && !r.truncated)
        .and_then(|r| battery(&r.text));
    lines.push(power.unwrap_or_else(|| "Batteria · dati non disponibili".into()));
    let now = Instant::now();
    let next = network();
    let net = match &next {
        None => "Rete · contatori non disponibili".into(),
        Some(c) if c.is_empty() => "Rete · nessuna interfaccia Wi-Fi o Ethernet attiva".into(),
        Some(c) => previous
            .as_ref()
            .and_then(|(time, prior)| rates(prior, c, now.duration_since(*time)))
            .map(|(rx, tx)| {
                format!(
                    "Rete · download {}/s · upload {}/s",
                    crate::ui::size(rx),
                    crate::ui::size(tx)
                )
            })
            .unwrap_or_else(|| "Rete · in attesa del secondo campione".into()),
    };
    *previous = next.map(|c| (now, c));
    lines.push(net);
    lines
}

struct State {
    ready: AtomicBool,
    stop: AtomicBool,
    lines: Mutex<Vec<String>>,
}
pub(super) struct Sampler {
    state: Arc<State>,
    _lifetime: mpsc::Sender<()>,
}
impl Sampler {
    pub(super) fn start(ctx: &egui::Context) -> Self {
        let state = Arc::new(State {
            ready: AtomicBool::new(false),
            stop: AtomicBool::new(false),
            lines: Mutex::new(Vec::new()),
        });
        let (lifetime, wait) = mpsc::channel();
        let worker = state.clone();
        let ctx = ctx.clone();
        let started = std::thread::Builder::new()
            .name("Lindo menu monitor".into())
            .spawn(move || {
                let mut previous = None;
                while !worker.stop.load(Ordering::Acquire) {
                    let lines = sample(&mut previous, &worker.stop);
                    if worker.stop.load(Ordering::Acquire) {
                        break;
                    }
                    if let Ok(mut slot) = worker.lines.lock() {
                        *slot = lines;
                        worker.ready.store(true, Ordering::Release);
                    }
                    ctx.request_repaint();
                    if wait.recv_timeout(PERIOD) != Err(mpsc::RecvTimeoutError::Timeout) {
                        break;
                    }
                }
            });
        if started.is_err() {
            if let Ok(mut slot) = state.lines.try_lock() {
                slot.push("Monitor · campionamento non disponibile".into());
            }
        }
        Self {
            state,
            _lifetime: lifetime,
        }
    }
    pub(super) fn ready(&self) -> bool {
        self.state.ready.load(Ordering::Acquire)
    }
    pub(super) fn lines(&self) -> Vec<String> {
        if let Ok(slot) = self.state.lines.try_lock() {
            self.state.ready.store(false, Ordering::Release);
            if !slot.is_empty() {
                return slot.clone();
            }
        }
        vec!["Memoria, batteria e rete · campionamento in corso".into()]
    }
}
impl Drop for Sampler {
    fn drop(&mut self) {
        self.state.stop.store(true, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn monotonic_rate_rebaselines_on_counter_or_interface_changes() {
        let initial = Counters::from([(1, (100, 50)), (4, (900, 450))]);
        let newer = Counters::from([(1, (220, 74)), (4, (980, 466))]);
        let elapsed = Duration::from_secs(2);
        assert_eq!(rates(&initial, &newer, elapsed), Some((100, 20)));
        assert_eq!(rates(&initial, &initial, elapsed), Some((0, 0)));
        assert_eq!(rates(&initial, &newer, Duration::ZERO), None);
        assert_eq!(rates(&newer, &initial, elapsed), None);
        assert_eq!(
            rates(
                &initial,
                &Counters::from([(1, (500, 500)), (5, (900, 700))]),
                elapsed
            ),
            None
        );
        assert_eq!(rates(&Counters::new(), &newer, elapsed), None);
    }
    #[test]
    fn decodes_only_running_link_headers_and_64bit_counters() {
        // Use a synthetic Darwin routing header, without addresses or interfaces
        // from the machine running the test.
        let mut frame = vec![0u8; size_of::<libc::if_msghdr2>()];
        let mut h: libc::if_msghdr2 = unsafe { std::mem::zeroed() };
        h.ifm_msglen = frame.len() as u16;
        h.ifm_type = libc::RTM_IFINFO2 as u8;
        h.ifm_index = 2;
        h.ifm_flags = libc::IFF_UP | libc::IFF_RUNNING;
        h.ifm_data.ifi_type = 6;
        h.ifm_data.ifi_ibytes = 7_000_000_000;
        h.ifm_data.ifi_obytes = 9_000_000_000;
        // Write relevant fields without copying uninitialized struct padding.
        let set = |buf: &mut [u8], offset: usize, bytes: &[u8]| {
            buf[offset..offset + bytes.len()].copy_from_slice(bytes)
        };
        set(&mut frame, 0, &h.ifm_msglen.to_ne_bytes());
        frame[3] = h.ifm_type;
        set(
            &mut frame,
            std::mem::offset_of!(libc::if_msghdr2, ifm_flags),
            &h.ifm_flags.to_ne_bytes(),
        );
        set(
            &mut frame,
            std::mem::offset_of!(libc::if_msghdr2, ifm_index),
            &h.ifm_index.to_ne_bytes(),
        );
        let data = std::mem::offset_of!(libc::if_msghdr2, ifm_data);
        frame[data] = h.ifm_data.ifi_type;
        set(
            &mut frame,
            data + std::mem::offset_of!(libc::if_data64, ifi_ibytes),
            &h.ifm_data.ifi_ibytes.to_ne_bytes(),
        );
        set(
            &mut frame,
            data + std::mem::offset_of!(libc::if_data64, ifi_obytes),
            &h.ifm_data.ifi_obytes.to_ne_bytes(),
        );
        assert_eq!(
            interface_counts(&frame),
            Some(Counters::from([(2, (7_000_000_000, 9_000_000_000))]))
        );
        assert!(interface_counts(&frame[..4]).is_none());
        frame[data] = 24;
        assert_eq!(interface_counts(&frame), Some(Counters::new()));
    }

    #[test]
    fn macos_reports_free_and_compressed_memory() {
        assert!(memory().is_some_and(|s| s.contains("liberi su") && s.contains("compressi")));
    }
    #[test]
    fn battery_distinguishes_absence_malformed_values_and_zero_percent() {
        assert_eq!(
            battery("Now drawing from 'AC Power'").as_deref(),
            Some("Batteria interna non rilevata")
        );
        assert!(battery("unknown").is_none());
        assert_eq!(
            battery(" -InternalBattery-0 (id=123)\t0%; discharging; present: true").as_deref(),
            Some("Batteria · 0% · in scarica")
        );
        assert_eq!(
            battery(" -InternalBattery-0 (id=123)\t80%; charging; present: true").as_deref(),
            Some("Batteria · 80% · in carica")
        );
        assert!(battery(" -InternalBattery-0 (id=123)\t120%; charging").is_none());
    }
}
