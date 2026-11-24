use crate::model::Mode;
use objc2::rc::autoreleasepool;
use std::{
    ffi::{c_void, CString},
    fs::File,
    io::{self, Read},
    os::unix::{ffi::OsStrExt, fs::OpenOptionsExt},
    path::{Component, Path, PathBuf},
    ptr,
};

type Object = *const c_void;
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFURLCreateFromFileSystemRepresentation(a: Object, b: *const u8, n: isize, d: u8) -> Object;
    fn CFURLCopyResourcePropertyForKey(
        u: Object,
        k: Object,
        out: *mut Object,
        err: *mut Object,
    ) -> u8;
    fn CFRelease(o: Object);
    fn CFEqual(a: Object, b: Object) -> u8;
    fn CFGetTypeID(o: Object) -> usize;
    fn CFDictionaryGetTypeID() -> usize;
    fn CFStringGetTypeID() -> usize;
    fn CFDataCreate(a: Object, b: *const u8, n: isize) -> Object;
    fn CFPropertyListCreateWithData(
        a: Object,
        d: Object,
        options: usize,
        fmt: *mut isize,
        err: *mut Object,
    ) -> Object;
    fn CFDictionaryGetValue(d: Object, k: Object) -> Object;
    fn CFStringCreateWithCString(a: Object, s: *const i8, e: u32) -> Object;
    fn CFStringGetCString(s: Object, b: *mut i8, n: isize, e: u32) -> u8;
    static kCFBooleanTrue: Object;
    static kCFBooleanFalse: Object;
    static kCFURLIsPackageKey: Object;
    static kCFURLIsUbiquitousItemKey: Object;
    static kCFURLUbiquitousItemDownloadingStatusKey: Object;
    static kCFURLUbiquitousItemDownloadingStatusCurrent: Object;
}
struct Owned(Object);
impl Owned {
    fn new(value: Object) -> io::Result<Self> {
        if value.is_null() {
            Err(io::Error::other("Metadati non verificabili"))
        } else {
            Ok(Self(value))
        }
    }
}
impl Drop for Owned {
    fn drop(&mut self) {
        unsafe { CFRelease(self.0) }
    }
}
fn url(path: &Path, is_dir: bool) -> io::Result<Owned> {
    let b = path.as_os_str().as_bytes();
    Owned::new(unsafe {
        CFURLCreateFromFileSystemRepresentation(
            ptr::null(),
            b.as_ptr(),
            b.len() as isize,
            is_dir as u8,
        )
    })
}
fn property(u: &Owned, key: Object) -> io::Result<Option<Owned>> {
    let mut value = ptr::null();
    if unsafe { CFURLCopyResourcePropertyForKey(u.0, key, &mut value, ptr::null_mut()) } == 0 {
        return Err(io::Error::other("Attributo del Finder non accessibile"));
    }
    Ok((!value.is_null()).then(|| Owned(value)))
}
// sys/resource.h exposes a per-thread ban on materializing File Provider data.
// Keeping this guard alive also covers a file becoming dataless after its stat.
unsafe extern "C" {
    fn getiopolicy_np(kind: i32, scope: i32) -> i32;
    fn setiopolicy_np(kind: i32, scope: i32, policy: i32) -> i32;
}
const MATERIALIZE_DATALESS: i32 = 3;
const THREAD_SCOPE: i32 = 1;
pub(crate) struct LocalOnly(i32, std::marker::PhantomData<std::rc::Rc<()>>);
impl LocalOnly {
    pub(crate) fn enter() -> io::Result<Self> {
        let policy = unsafe { getiopolicy_np(MATERIALIZE_DATALESS, THREAD_SCOPE) };
        if policy < 0 || unsafe { setiopolicy_np(MATERIALIZE_DATALESS, THREAD_SCOPE, 1) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(policy, std::marker::PhantomData))
    }
}
impl Drop for LocalOnly {
    fn drop(&mut self) {
        unsafe {
            setiopolicy_np(MATERIALIZE_DATALESS, THREAD_SCOPE, self.0);
        }
    }
}
const NOFOLLOW_ANY: i32 = 0x20000000;
pub(crate) fn open_file(path: &Path, extra: i32) -> io::Result<File> {
    File::options()
        .read(true)
        .custom_flags(NOFOLLOW_ANY | libc::O_NONBLOCK | extra)
        .open(path)
}
pub(crate) fn name_c(path: &Path) -> io::Result<CString> {
    CString::new(path.as_os_str().as_bytes()).map_err(io::Error::other)
}
pub(crate) fn provider_state(path: &Path, dir: bool) -> io::Result<(bool, bool)> {
    // CFURL creates autoreleased Objective-C temporaries even though the
    // returned CF values have explicit ownership. Rust worker threads have
    // no event-loop pool to drain those temporaries.
    autoreleasepool(|_| {
        let u = url(path, dir)?;
        unsafe {
            let Some(value) = property(&u, kCFURLIsUbiquitousItemKey)? else {
                return Ok((true, false));
            };
            if CFEqual(value.0, kCFBooleanFalse) != 0 {
                return Ok((true, false));
            }
            if CFEqual(value.0, kCFBooleanTrue) == 0 {
                return Err(io::Error::other("Stato iCloud sconosciuto"));
            }
            Ok((
                CFEqual(
                    property(&u, kCFURLUbiquitousItemDownloadingStatusKey)?
                        .ok_or_else(|| io::Error::other("Stato iCloud sconosciuto"))?
                        .0,
                    kCFURLUbiquitousItemDownloadingStatusCurrent,
                ) != 0,
                true,
            ))
        }
    })
}
pub(crate) fn placeholder_name(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|s| s.as_bytes().ends_with(b".icloud"))
}
pub(crate) fn local_provider_state(path: &Path) -> io::Result<(bool, bool)> {
    use std::os::macos::fs::MetadataExt;
    if placeholder_name(path) {
        return Ok((false, false));
    }
    let m = open_file(path, libc::O_EVTONLY)?.metadata()?;
    if m.st_flags() & 0x40000000 != 0 {
        return Ok((false, false));
    }
    provider_state(path, m.is_dir())
}
pub fn local_path(path: &Path) -> io::Result<bool> {
    local_provider_state(path).map(|(local, _)| local)
}
pub fn package(path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|x| x.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    const PACKAGES:&str="app bundle framework plugin photoslibrary photolibrary pages numbers key rtfd band logicx sparsebundle playground xcassets vmwarevm pvm utm backupdb aplibrary migratedphotolibrary imovielibrary tvlibrary musiclibrary theater fcpbundle dvdmedia xcarchive xcodeproj xcworkspace xcdatamodeld dsym docset playgroundbook kext prefpane saver qlgenerator mdimporter appex xpc component vst vst3 workflow action wdgt pkg mpkg download scriv textbundle graffle oo3 ofocus dtbase2 lrdata lrcat-data";
    if PACKAGES.split_ascii_whitespace().any(|s| s == ext) {
        return true;
    }
    autoreleasepool(|_| unsafe {
        url(path, true)
            .and_then(|u| property(&u, kCFURLIsPackageKey))
            .map_or(true, |v| {
                v.is_some_and(|v| CFEqual(v.0, kCFBooleanFalse) == 0)
            })
    })
}
pub fn protected(path: &Path, mode: Mode, app_root: Option<&Path>) -> bool {
    let space = mode == Mode::Space;
    let mut prev_work = false;
    let app_start = app_root
        .filter(|a| path.starts_with(a))
        .map(|a| a.components().count())
        .unwrap_or(usize::MAX);
    for (index, part) in path.components().enumerate() {
        if matches!(part, Component::ParentDir | Component::CurDir) {
            return true;
        }
        let name = part.as_os_str().to_string_lossy().to_lowercase();
        if prev_work && name == "clients" {
            return true;
        }
        prev_work = name == "_2_work";
        if matches!(
            name.as_str(),
            "_6_archivio_private"
                | "piernicola de maria"
                | ".ssh"
                | ".gnupg"
                | ".aws"
                | ".codex"
                | "keychains"
                | ".trash"
                | ".trashes"
                | "trash"
        ) {
            return true;
        }
        if index < app_start
            && (matches!(name.as_str(), "mail" | "messages")
                || (!space
                    && (matches!(name.as_str(), "photos" | "system" | "apple")
                        || name.starts_with("com.apple."))))
        {
            return true;
        }
        if matches!(mode, Mode::Large | Mode::Duplicates | Mode::Logs) && name.starts_with('.') {
            return true;
        }
    }
    let folded = path.to_string_lossy().to_lowercase();
    !space
        && ["/library/apple", "/applications/utilities"]
            .iter()
            .any(|p| Path::new(&folded).starts_with(p))
}
pub(crate) fn volume_available(dir: &File) -> io::Result<u64> {
    use std::os::fd::AsRawFd;
    let mut st = std::mem::MaybeUninit::<libc::statfs>::uninit();
    if unsafe { libc::fstatfs(dir.as_raw_fd(), st.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let st = unsafe { st.assume_init() };
    Ok(st.f_bavail.saturating_mul(st.f_bsize as u64))
}
pub(crate) fn home() -> PathBuf {
    #[cfg(test)]
    if let Some(fixture) = std::env::var_os("LINDO_FIXTURE_WORKER") {
        return fixture.into();
    }
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
}
pub(crate) fn cloud(path: &Path) -> bool {
    cloud_under_home(path, &home())
}

fn cloud_under_home(path: &Path, home: &Path) -> bool {
    if !home.is_absolute() || !path.is_absolute() {
        return false;
    }
    let mut parts = path.iter().map(OsStrExt::as_bytes);
    if !home.iter().all(|part| {
        parts
            .next()
            .is_some_and(|p| p.eq_ignore_ascii_case(part.as_bytes()))
    }) {
        return false;
    }
    let Some(first) = parts.next() else {
        return false;
    };
    if first.eq_ignore_ascii_case(b"library") {
        return parts.next().is_some_and(|dir| {
            dir.eq_ignore_ascii_case(b"cloudstorage")
                || dir.eq_ignore_ascii_case(b"mobile documents")
        });
    }
    [
        b"dropbox".as_slice(),
        b"onedrive",
        b"google drive",
        b"googledrive",
    ]
    .iter()
    .any(|provider| {
        first.eq_ignore_ascii_case(provider)
            || (first.get(provider.len()) == Some(&b'-')
                && first[..provider.len()].eq_ignore_ascii_case(provider))
    })
}

pub fn allowed_root(mode: Mode, path: &Path) -> bool {
    let home = home();
    if !home.is_absolute()
        || !path.is_absolute()
        || protected(path, mode, None)
        || path
            .components()
            .any(|c| placeholder_name(Path::new(c.as_os_str())))
    {
        return false;
    }
    let under_user = path.starts_with(&home);
    let volumes = path.starts_with("/Volumes");
    match mode {
        Mode::Junk => crate::junk::allowed_root(path),
        Mode::Space => under_user || path.starts_with("/Applications") || volumes,
        Mode::Apps => path == Path::new("/Applications") || path == home.join("Applications"),
        Mode::Cache => cache_branch(path) || cache_owner(path).is_some(),
        Mode::Logs => {
            path.starts_with(home.join("Library/Logs")) || path.starts_with("/Library/Logs")
        }
        Mode::Large | Mode::Duplicates => {
            let folded = path.to_string_lossy().to_lowercase();
            let h = PathBuf::from(home.to_string_lossy().to_lowercase());
            let p = Path::new(&folded);
            (under_user || volumes)
                && p != h
                && (!p.starts_with(h.join("library"))
                    || p.starts_with(h.join("library/cloudstorage"))
                    || p.starts_with(h.join("library/mobile documents")))
        }
    }
}
fn cache_parts(path: &Path) -> Option<Vec<&str>> {
    path.strip_prefix(home().join("Library/Caches"))
        .ok()?
        .iter()
        .map(|s| s.to_str())
        .collect()
}
fn profile(s: &str) -> bool {
    s == "Default"
        || s.strip_prefix("Profile ")
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}
pub fn cache_owner(path: &Path) -> Option<&'static str> {
    match cache_parts(path)?.as_slice() {
        ["Google", "Chrome", p, "Cache"] if profile(p) => Some("com.google.Chrome"),
        ["Chromium", p, "Cache"] if profile(p) => Some("org.chromium.Chromium"),
        ["Firefox", "Profiles", p, "cache2"] if !p.starts_with('.') => Some("org.mozilla.firefox"),
        _ => None,
    }
}
pub(crate) fn cache_branch(path: &Path) -> bool {
    let Some(parts) = cache_parts(path) else {
        return false;
    };
    match parts.as_slice() {
        []
        | ["Google"]
        | ["Google", "Chrome"]
        | ["Chromium"]
        | ["Firefox"]
        | ["Firefox", "Profiles"] => true,
        ["Google", "Chrome", p] | ["Chromium", p] => profile(p),
        ["Firefox", "Profiles", p] => !p.starts_with('.'),
        _ => false,
    }
}
pub fn app_identifier(app: &Path) -> io::Result<String> {
    let path = app.join("Contents/Info.plist");
    if !local_path(&path)? {
        return Err(io::Error::other("Info.plist non locale"));
    }
    let f = open_file(&path, 0)?;
    let m = f.metadata()?;
    if !m.is_file() || m.len() >= 1_000_000 {
        return Err(io::Error::other("Info.plist non valido"));
    }
    let mut data = Vec::new();
    f.take(1_000_000).read_to_end(&mut data)?;
    if data.len() as u64 != m.len() {
        return Err(io::Error::other("Info.plist modificato"));
    }
    const UTF8: u32 = 0x08000100;
    autoreleasepool(|_| unsafe {
        let bytes = Owned::new(CFDataCreate(
            ptr::null(),
            data.as_ptr(),
            data.len() as isize,
        ))?;
        let plist = Owned::new(CFPropertyListCreateWithData(
            ptr::null(),
            bytes.0,
            0,
            ptr::null_mut(),
            ptr::null_mut(),
        ))?;
        if CFGetTypeID(plist.0) != CFDictionaryGetTypeID() {
            return Err(io::Error::other("Info.plist non valido"));
        }
        let key = Owned::new(CFStringCreateWithCString(
            ptr::null(),
            c"CFBundleIdentifier".as_ptr(),
            UTF8,
        ))?;
        let value = CFDictionaryGetValue(plist.0, key.0);
        if value.is_null() || CFGetTypeID(value) != CFStringGetTypeID() {
            return Err(io::Error::other("Identificatore assente"));
        }
        let mut buf = [0i8; 1024];
        if CFStringGetCString(value, buf.as_mut_ptr(), buf.len() as isize, UTF8) == 0 {
            return Err(io::Error::other("Identificatore non valido"));
        }
        let id = std::ffi::CStr::from_ptr(buf.as_ptr())
            .to_str()
            .map_err(io::Error::other)?;
        if id.is_empty() {
            return Err(io::Error::other("Identificatore assente"));
        }
        Ok(id.to_owned())
    })
}

/// Owns the POSIX directory stream. Each iteration copies one name before readdir
/// reuses its buffer. Relative metadata queries stay anchored to this stream.
pub(crate) struct Directory(*mut libc::DIR);
impl Drop for Directory {
    fn drop(&mut self) {
        unsafe {
            libc::closedir(self.0);
        }
    }
}
pub(crate) fn children(path: &Path) -> io::Result<Directory> {
    use std::os::fd::IntoRawFd;
    let fd = open_file(path, libc::O_DIRECTORY)?.into_raw_fd();
    let stream = unsafe { libc::fdopendir(fd) };
    if stream.is_null() {
        let error = io::Error::last_os_error();
        unsafe { libc::close(fd) };
        return Err(error);
    }
    Ok(Directory(stream))
}
impl Directory {
    pub(crate) fn stat(&self, name: &std::ffi::OsStr) -> io::Result<libc::stat> {
        let name = name_c(Path::new(name))?;
        let mut result = std::mem::MaybeUninit::<libc::stat>::uninit();
        if unsafe {
            libc::fstatat(
                libc::dirfd(self.0),
                name.as_ptr(),
                result.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        } != 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(unsafe { result.assume_init() })
    }
    pub(crate) fn node(&self) -> io::Result<(u64, u64)> {
        let mut s = std::mem::MaybeUninit::uninit();
        if unsafe { libc::fstat(libc::dirfd(self.0), s.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let s = unsafe { s.assume_init() };
        Ok((s.st_dev as u64, s.st_ino))
    }
}
impl Iterator for Directory {
    type Item = io::Result<std::ffi::OsString>;
    fn next(&mut self) -> Option<Self::Item> {
        use std::os::unix::ffi::OsStringExt;
        unsafe {
            loop {
                *libc::__error() = 0;
                let entry = libc::readdir(self.0);
                if entry.is_null() {
                    return (*libc::__error() != 0).then(|| Err(io::Error::last_os_error()));
                }
                let bytes = std::ffi::CStr::from_ptr((*entry).d_name.as_ptr()).to_bytes();
                if bytes != b"." && bytes != b".." {
                    return Some(Ok(std::ffi::OsString::from_vec(bytes.to_vec())));
                }
            }
        }
    }
}

#[cfg(test)]
mod location_tests {
    use super::*;

    #[test]
    fn reviewed_file_location_preserves_mail_and_logs_and_rejects_bundles_and_self() {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().canonicalize().unwrap();
        let exe = std::env::current_exe().unwrap();
        let guard = LocalOnly::enter().unwrap();
        let mut fixtures = vec![(exe, false)];
        for (dir, allowed) in [
            (
                "Library/Containers/com.apple.mail/Data/Library/Mail Downloads",
                true,
            ),
            ("Library/Logs/Sample", true),
            ("My.app/Contents/Resources", false),
        ] {
            let parent = base.join(dir);
            std::fs::create_dir_all(&parent).unwrap();
            let file = parent.join("entry.txt");
            std::fs::write(&file, b"fixture").unwrap();
            fixtures.push((file, allowed));
        }
        for (path, allowed) in fixtures {
            assert!(local_path(&path).unwrap());
            let identity = crate::trash::Identity::read(&path).unwrap();
            let mut reached_feature = false;
            let result = crate::trash::move_reviewed_file(&path, &identity, |_| {
                reached_feature = true;
                Err("Synthetic feature veto".into())
            });
            assert!(result.is_err());
            assert_eq!(reached_feature, allowed, "{}", path.display());
            assert!(path.exists());
        }
        drop(guard);
    }

    #[test]
    fn cloud_roots_use_complete_case_insensitive_home_components() {
        let home = Path::new("/Users/demo");
        for rel in [
            "lIbRaRy/cLoUdStOrAgE/Service/file",
            "Library/mobile documents/x",
            "Dropbox-Team/file",
            "oNeDrIvE/file",
            "Google Drive/file",
            "GoogleDrive-Team/file",
        ] {
            assert!(cloud_under_home(&Path::new("/users/demo").join(rel), home));
        }
        for path in [
            "/Users/demo/Library/CloudStorage Backup/x",
            "/Users/demo/Documents/Dropbox/x",
            "/Users/demo/DropboxNotes/x",
            "/Users/demolition/Dropbox/x",
            "/Volumes/Test/OneDrive/x",
        ] {
            assert!(!cloud_under_home(Path::new(path), home));
        }
    }
}
