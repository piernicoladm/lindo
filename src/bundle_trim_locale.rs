use crate::policy;
use std::{
    collections::BTreeSet,
    ffi::{c_void, CStr, CString},
    path::Path,
    ptr,
};
type Ref = *const c_void;
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFLocaleCopyPreferredLanguages() -> Ref;
    fn CFLocaleCreateCanonicalLanguageIdentifierFromString(a: Ref, s: Ref) -> Ref;
    fn CFPreferencesCopyAppValue(key: Ref, app: Ref) -> Ref;
    fn CFStringCreateWithCString(a: Ref, s: *const i8, enc: u32) -> Ref;
    fn CFStringGetCString(s: Ref, buf: *mut i8, n: isize, enc: u32) -> u8;
    fn CFGetTypeID(o: Ref) -> usize;
    fn CFStringGetTypeID() -> usize;
    fn CFArrayGetTypeID() -> usize;
    fn CFArrayGetCount(o: Ref) -> isize;
    fn CFArrayGetValueAtIndex(o: Ref, i: isize) -> Ref;
    fn CFArrayCreate(a: Ref, p: *const Ref, n: isize, cb: Ref) -> Ref;
    fn CFBundleCopyLocalizationsForPreferences(l: Ref, p: Ref) -> Ref;
    fn CFRelease(o: Ref);
}
struct Owned(Ref);
impl Owned {
    fn from(o: Ref) -> Result<Self, String> {
        if o.is_null() {
            Err("Lingue preferite non verificabili.".into())
        } else {
            Ok(Self(o))
        }
    }
}
impl Drop for Owned {
    fn drop(&mut self) {
        unsafe { CFRelease(self.0) }
    }
}
fn text(s: &str) -> Result<Owned, String> {
    let v = CString::new(s).map_err(|e| e.to_string())?;
    Owned::from(unsafe { CFStringCreateWithCString(ptr::null(), v.as_ptr(), 0x08000100) })
}
fn extract(s: Ref) -> Option<String> {
    if s.is_null() || unsafe { CFGetTypeID(s) != CFStringGetTypeID() } {
        return None;
    }
    let mut b = [0i8; 256];
    if unsafe { CFStringGetCString(s, b.as_mut_ptr(), 256, 0x08000100) } == 0 {
        return None;
    }
    unsafe { CStr::from_ptr(b.as_ptr()) }
        .to_str()
        .ok()
        .map(str::to_owned)
}
fn array(v: &Owned) -> Result<Vec<String>, String> {
    if unsafe { CFGetTypeID(v.0) != CFArrayGetTypeID() } {
        return Err("Elenco lingue non verificabile.".into());
    }
    let n = unsafe { CFArrayGetCount(v.0) };
    if n > 512 {
        return Err("Elenco lingue oltre i limiti.".into());
    }
    (0..n)
        .map(|i| {
            extract(unsafe { CFArrayGetValueAtIndex(v.0, i) })
                .ok_or_else(|| "Identificatore lingua non verificabile.".into())
        })
        .collect()
}
fn family(s: &str) -> Option<String> {
    let v = text(s).ok()?;
    let canonical = Owned::from(unsafe {
        CFLocaleCreateCanonicalLanguageIdentifierFromString(ptr::null(), v.0)
    })
    .ok()?;
    let canonical = extract(canonical.0)?;
    let base = canonical
        .split(['-', '_', '@'])
        .next()?
        .to_ascii_lowercase();
    ((2..=3).contains(&base.len()) && base.bytes().all(|b| b.is_ascii_lowercase())).then_some(base)
}
pub(super) struct Locale {
    pub name: String,
    pub protected: bool,
}
pub(super) fn list(root: &Path, domain: &str, dev: &str) -> Result<Vec<Locale>, String> {
    let resources = root.join("Contents/Resources");
    let mut names = Vec::new();
    match policy::children(&resources) {
        Ok(list) => {
            for n in list {
                let p = resources.join(n.map_err(|e| e.to_string())?);
                if p.extension().is_some_and(|s| s == "lproj") {
                    let m = p.symlink_metadata().map_err(|e| e.to_string())?;
                    if !m.is_dir() {
                        return Err("Localizzazione non identificabile come directory.".into());
                    }
                    let name = p
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .ok_or("Nome della localizzazione illeggibile.")?;
                    names.push(name.to_owned());
                }
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.to_string()),
    }
    names.sort();
    if names.is_empty() {
        return Ok(Vec::new());
    }
    let name_refs = names
        .iter()
        .map(|s| text(s))
        .collect::<Result<Vec<_>, _>>()?;
    let ptrs = name_refs.iter().map(|s| s.0).collect::<Vec<_>>();
    let available = Owned::from(unsafe {
        CFArrayCreate(ptr::null(), ptrs.as_ptr(), ptrs.len() as isize, ptr::null())
    })?;
    let global = Owned::from(unsafe { CFLocaleCopyPreferredLanguages() })?;
    let d = text(domain)?;
    let key = text("AppleLanguages")?;
    let setting = unsafe { CFPreferencesCopyAppValue(key.0, d.0) };
    let local = if setting.is_null() {
        None
    } else {
        Some(Owned(setting))
    };
    let mut protected = BTreeSet::from(["base".into(), dev.to_ascii_lowercase(), "en".into()]);
    let Some(dev_family) = family(dev) else {
        return Ok(names
            .into_iter()
            .map(|name| Locale {
                name,
                protected: true,
            })
            .collect());
    };
    protected.insert(dev_family);
    for prefs in std::iter::once(&global).chain(local.as_ref()) {
        let pref_names = array(prefs)?;
        for s in pref_names {
            if let Some(f) = family(&s) {
                protected.insert(f);
            }
        }
        let actual =
            Owned::from(unsafe { CFBundleCopyLocalizationsForPreferences(available.0, prefs.0) })?;
        for s in array(&actual)? {
            protected.insert(s.to_ascii_lowercase());
        }
    }
    Ok(names
        .into_iter()
        .map(|name| {
            let keep = protected.contains(&name.to_ascii_lowercase())
                || family(&name).is_none_or(|f| protected.contains(&f));
            Locale {
                name,
                protected: keep,
            }
        })
        .collect())
}
