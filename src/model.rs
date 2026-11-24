use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicU8};

/// Sampled by the window. Filesystem workers never enqueue one UI event per entry.
#[derive(Debug, Default)]
pub struct Progress {
    pub visited: AtomicU64,
    pub tree_entries: AtomicU64,
    pub hashed_bytes: AtomicU64,
    pub compared_bytes: AtomicU64,
    pub phase: AtomicU8,
    pub current_directory: std::sync::Mutex<PathBuf>,
}

pub const WALK: u8 = 0;
pub const HASH: u8 = 1;
pub const COMPARE: u8 = 2;
pub const TREE: u8 = 3;

pub use crate::trash::Identity;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Mode {
    Space,
    Large,
    Duplicates,
    Cache,
    Logs,
    Apps,
    Junk,
}

#[derive(Clone, Debug)]
pub struct Request {
    pub mode: Mode,
    pub roots: Vec<PathBuf>,
    pub min_bytes: u64,
}

#[derive(Clone, Debug)]
pub struct Item {
    pub path: PathBuf,
    pub bytes: u64,
    pub identity: Identity,
    pub group: Option<u64>,
    pub cloud: bool,
}

#[derive(Default, Debug, Clone, Copy)]
pub struct SpaceStats {
    pub logical_bytes: u64,
    pub files: u64,
    pub hardlinks: u64,
    pub skipped: u64,
    pub errors: u64,
}

#[derive(Clone, Debug)]
pub struct FolderMeasurement {
    pub root: (u64, u64),
    pub observed: u64,
    pub volume_free: Option<u64>,
    pub measured_at: String,
    pub skipped: u64,
    pub errors: u64,
}

pub(crate) fn local_timestamp(stamp: libc::time_t) -> Option<String> {
    let mut time = std::mem::MaybeUninit::<libc::tm>::uninit();
    // localtime_r writes to our buffer only on success.
    if unsafe { libc::localtime_r(&stamp, time.as_mut_ptr()) }.is_null() {
        return None;
    }
    let t = unsafe { time.assume_init() };
    Some(format!(
        "{:02}/{:02}/{:04} alle {:02}:{:02}:{:02}",
        t.tm_mday,
        t.tm_mon + 1,
        t.tm_year + 1900,
        t.tm_hour,
        t.tm_min,
        t.tm_sec
    ))
}

#[derive(Default, Debug)]
pub struct Report {
    pub items: Vec<Item>,
    pub space: HashMap<PathBuf, SpaceStats>,
    pub measurement: Option<FolderMeasurement>,
    pub visited: u64,
    pub skipped: u64,
    pub errors: u64,
    pub cancelled: bool,
    pub notes: Vec<String>,
}
