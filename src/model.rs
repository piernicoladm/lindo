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

#[derive(Default, Debug)]
pub struct Report {
    pub items: Vec<Item>,
    pub space: HashMap<PathBuf, SpaceStats>,
    pub visited: u64,
    pub skipped: u64,
    pub errors: u64,
    pub cancelled: bool,
    pub notes: Vec<String>,
}
