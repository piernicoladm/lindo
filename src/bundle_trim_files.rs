//! Compare every data fork and verify every retained Mach-O slice byte for byte.
use crate::policy;
use sha2::{Digest, Sha256};
use std::os::fd::AsRawFd;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::{Read, Seek, SeekFrom},
    os::unix::fs::MetadataExt,
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};
type Result<T> = std::result::Result<T, String>;
type Hash = [u8; 32];
type Slices = BTreeMap<(u32, u32), Hash>;
const ARM64: u32 = 0x0100000c;
const INTEL: [u32; 2] = [7, 0x01000007];
#[derive(Clone, PartialEq, Eq)]
enum Data {
    Directory,
    Link(PathBuf),
    File { raw: Hash, slices: Slices },
}
#[derive(Clone, PartialEq, Eq)]
struct Node {
    mode: u32,
    data: Data,
}
pub(super) struct Manifest {
    nodes: BTreeMap<PathBuf, Node>,
    pub logical: u64,
    pub allocated: u64,
    pub removable_intel: bool,
}
fn digest(file: &mut File, off: u64, size: u64, stop: &AtomicBool) -> Result<Hash> {
    file.seek(SeekFrom::Start(off)).map_err(|e| e.to_string())?;
    let mut buf = [0u8; 65536];
    let mut left = size;
    let mut hash = Sha256::new();
    while left > 0 {
        if stop.load(Ordering::Acquire) {
            return Err("Operazione interrotta.".into());
        }
        let n = (left as usize).min(buf.len());
        file.read_exact(&mut buf[..n]).map_err(|e| e.to_string())?;
        hash.update(&buf[..n]);
        left -= n as u64;
    }
    Ok(hash.finalize().into())
}
fn u32at(b: &[u8], little: bool) -> u32 {
    let bytes = b[..4].try_into().unwrap();
    if little {
        u32::from_le_bytes(bytes)
    } else {
        u32::from_be_bytes(bytes)
    }
}
fn u64at(b: &[u8], little: bool) -> u64 {
    let bytes = b[..8].try_into().unwrap();
    if little {
        u64::from_le_bytes(bytes)
    } else {
        u64::from_be_bytes(bytes)
    }
}
fn thin(header: &[u8]) -> Option<(u32, u32)> {
    let le = match &header[..4] {
        [0xce, 0xfa, 0xed, 0xfe] | [0xcf, 0xfa, 0xed, 0xfe] => true,
        [0xfe, 0xed, 0xfa, 0xce] | [0xfe, 0xed, 0xfa, 0xcf] => false,
        _ => return None,
    };
    Some((u32at(&header[4..], le), u32at(&header[8..], le)))
}
fn inspect_file(p: &Path, size: u64, stop: &AtomicBool) -> Result<Data> {
    let mut f = policy::open_file(p, 0).map_err(|e| e.to_string())?;
    let fork = unsafe {
        libc::fgetxattr(
            f.as_raw_fd(),
            c"com.apple.ResourceFork".as_ptr(),
            std::ptr::null_mut(),
            0,
            0,
            0,
        )
    };
    if fork > 0
        || (fork < 0 && std::io::Error::last_os_error().raw_os_error() != Some(libc::ENOATTR))
    {
        return Err("Risorsa con fork aggiuntivo non verificabile.".into());
    }
    let raw = digest(&mut f, 0, size, stop)?;
    let mut slices = Slices::new();
    if size < 12 {
        return Ok(Data::File { raw, slices });
    }
    let mut hdr = [0u8; 12];
    f.rewind()
        .and_then(|_| f.read_exact(&mut hdr))
        .map_err(|e| e.to_string())?;
    if let Some(kind) = thin(&hdr) {
        slices.insert(kind, raw);
        return Ok(Data::File { raw, slices });
    }
    let (le, wide) = match hdr[..4] {
        [0xca, 0xfe, 0xba, 0xbe] => (false, false),
        [0xbe, 0xba, 0xfe, 0xca] => (true, false),
        [0xca, 0xfe, 0xba, 0xbf] => (false, true),
        [0xbf, 0xba, 0xfe, 0xca] => (true, true),
        _ => return Ok(Data::File { raw, slices }),
    };
    let count = u32at(&hdr[4..], le) as usize;
    let entry = if wide { 32 } else { 20 };
    if count == 0 || count > 64 || size < (8 + count * entry) as u64 {
        return Ok(Data::File { raw, slices });
    }
    let mut table = vec![0; count * entry];
    f.seek(SeekFrom::Start(8))
        .and_then(|_| f.read_exact(&mut table))
        .map_err(|e| e.to_string())?;
    let mut ranges = Vec::new();
    for part in table.chunks_exact(entry) {
        let cpu = (u32at(part, le), u32at(&part[4..], le));
        let (off, len) = if wide {
            (u64at(&part[8..], le), u64at(&part[16..], le))
        } else {
            (u32at(&part[8..], le) as u64, u32at(&part[12..], le) as u64)
        };
        if off < (8 + table.len()) as u64
            || len < 12
            || off.checked_add(len).is_none_or(|end| end > size)
        {
            return Ok(Data::File {
                raw,
                slices: Slices::new(),
            });
        }
        f.seek(SeekFrom::Start(off))
            .and_then(|_| f.read_exact(&mut hdr))
            .map_err(|e| e.to_string())?;
        if thin(&hdr) != Some(cpu) {
            return Ok(Data::File {
                raw,
                slices: Slices::new(),
            });
        }
        ranges.push((off, off + len));
        if slices
            .insert(cpu, digest(&mut f, off, len, stop)?)
            .is_some()
        {
            return Err("Architettura duplicata nel Mach-O.".into());
        }
    }
    ranges.sort_unstable();
    if ranges.windows(2).any(|r| r[0].1 > r[1].0) {
        return Err("Slice Mach-O sovrapposte.".into());
    }
    Ok(Data::File { raw, slices })
}
fn link_permitted(relative: &Path, target: &Path) -> bool {
    if target.is_absolute() {
        return false;
    }
    let mut depth = relative.parent().map_or(0, |p| p.components().count());
    let mut tail = false;
    for c in target.components() {
        match c {
            Component::ParentDir if depth > 0 && !tail => depth -= 1,
            Component::Normal(_) => {
                depth += 1;
                tail = true;
            }
            Component::CurDir => {}
            _ => return false,
        }
    }
    depth > 0
}
pub(super) fn manifest(root: &Path, stop: &AtomicBool) -> Result<Manifest> {
    let owner = unsafe { libc::geteuid() };
    let mut nodes = BTreeMap::new();
    let mut stack = vec![PathBuf::new()];
    let mut links = Vec::new();
    let mut logical = 0u64;
    let mut allocated = 0u64;
    let mut removable_intel = false;
    let base = fs::symlink_metadata(root).map_err(|e| e.to_string())?;
    let dev = base.dev();
    while let Some(r) = stack.pop() {
        if stop.load(Ordering::Acquire) {
            return Err("Operazione interrotta.".into());
        }
        let p = root.join(&r);
        let m = fs::symlink_metadata(&p).map_err(|e| e.to_string())?;
        if m.uid() != owner || m.dev() != dev || m.mode() & 0o6000 != 0 || nodes.len() >= 200_000 {
            return Err("Permessi, volume o dimensione del bundle non idonei.".into());
        }
        let data = if m.file_type().is_symlink() {
            let target = fs::read_link(&p).map_err(|e| e.to_string())?;
            if !link_permitted(&r, &target) {
                return Err("Collegamento esterno o ambiguo nel bundle.".into());
            }
            links.push(p);
            Data::Link(target)
        } else {
            if !policy::local_path(&p).unwrap_or(false) {
                return Err("Bundle non completamente locale.".into());
            }
            if m.is_dir() {
                for name in policy::children(&p).map_err(|e| e.to_string())? {
                    stack.push(r.join(name.map_err(|e| e.to_string())?));
                }
                Data::Directory
            } else if m.is_file() && m.nlink() == 1 {
                logical = logical.saturating_add(m.len());
                allocated = allocated.saturating_add(m.blocks().saturating_mul(512));
                let data = inspect_file(&p, m.len(), stop)?;
                if let Data::File { slices, .. } = &data {
                    removable_intel |= slices.keys().any(|(c, _)| *c == ARM64)
                        && slices.keys().any(|(c, _)| INTEL.contains(c));
                }
                data
            } else {
                return Err("File speciale o hard link nel bundle.".into());
            }
        };
        nodes.insert(
            r,
            Node {
                mode: m.mode() & 0o7777,
                data,
            },
        );
    }
    for link in links {
        if !fs::canonicalize(link).is_ok_and(|p| p.starts_with(root)) {
            return Err("Collegamento non risolvibile dentro il bundle.".into());
        }
    }
    Ok(Manifest {
        nodes,
        logical,
        allocated,
        removable_intel,
    })
}
pub(super) fn compare(
    source: &Manifest,
    copy: &Manifest,
    thin: bool,
    removed: &BTreeSet<PathBuf>,
) -> Result<()> {
    for (path, original) in &source.nodes {
        if removed.iter().any(|r| path.starts_with(r)) {
            if matches!(&original.data, Data::Link(_))
                || matches!(&original.data,Data::File{slices,..} if !slices.is_empty() || original.mode & 0o111!=0)
            {
                return Err(
                    "La localizzazione contiene codice o collegamenti e deve essere conservata."
                        .into(),
                );
            }
            if copy.nodes.contains_key(path) {
                return Err("Localizzazione ancora presente.".into());
            }
            continue;
        }
        let candidate = copy
            .nodes
            .get(path)
            .ok_or("Una voce non selezionata è scomparsa dalla copia.")?;
        if original == candidate {
            continue;
        }
        if thin && original.mode == candidate.mode {
            if let (Data::File { slices: old, .. }, Data::File { slices: new, .. }) =
                (&original.data, &candidate.data)
            {
                let native: Slices = old
                    .iter()
                    .filter(|((cpu, _), _)| *cpu == ARM64)
                    .map(|(k, v)| (*k, *v))
                    .collect();
                if !native.is_empty()
                    && native.len() < old.len()
                    && old
                        .keys()
                        .all(|(cpu, _)| *cpu == ARM64 || INTEL.contains(cpu))
                    && native == *new
                {
                    continue;
                }
            }
        }
        return Err(format!(
            "Contenuto o permessi cambiati fuori dalla selezione: {}",
            path.display()
        ));
    }
    if copy.nodes.keys().any(|p| !source.nodes.contains_key(p)) {
        return Err("La copia contiene elementi aggiuntivi.".into());
    }
    if thin && copy.removable_intel {
        return Err("La copia conserva slice Intel selezionate.".into());
    }
    Ok(())
}
