//! Conservative visual matches against a fixed reference per group.
use crate::{model::Item, policy};
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader, Limits};
use std::{
    io::{self, BufReader, Read},
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

pub(super) const MAX_IMAGES: usize = 2000;
const MAX_FILE: u64 = 32 * 1024 * 1024;
const MAX_PIXELS: u64 = 16_000_000;

pub(super) struct Visual {
    pub width: u32,
    pub height: u32,
    pub preview: Vec<u8>,
    signature: Vec<u8>,
    edges: u64,
}

pub(super) fn supported(path: &Path) -> bool {
    path.extension().and_then(|v| v.to_str()).is_some_and(|v| {
        ["png", "jpeg", "jpg", "heic", "heif"]
            .iter()
            .any(|ext| v.eq_ignore_ascii_case(ext))
    })
}

impl Visual {
    fn from_image(image: &DynamicImage) -> Self {
        // Composite alpha on white to compare the visible image, including PNG transparency.
        let small = image
            .resize_exact(9, 8, image::imageops::FilterType::Triangle)
            .to_rgba8();
        let mut signature = Vec::with_capacity(216);
        let mut edges = 0;
        for (y, row) in small.rows().enumerate() {
            let mut previous = 0;
            for (x, pixel) in row.enumerate() {
                let alpha = u32::from(pixel[3]);
                let rgb = [pixel[0], pixel[1], pixel[2]]
                    .map(|v| ((u32::from(v) * alpha + 255 * (255 - alpha)) / 255) as u8);
                let gray = 3 * u32::from(rgb[0]) + 6 * u32::from(rgb[1]) + u32::from(rgb[2]);
                if x > 0 && gray > previous {
                    edges |= 1 << (8 * y + x - 1);
                }
                previous = gray;
                signature.extend(rgb);
            }
        }
        let thumbnail = image.thumbnail(48, 48).to_rgba8();
        let mut preview = image::RgbaImage::new(48, 48);
        image::imageops::overlay(
            &mut preview,
            &thumbnail,
            i64::from(48 - thumbnail.width()) / 2,
            i64::from(48 - thumbnail.height()) / 2,
        );
        Self {
            width: image.width(),
            height: image.height(),
            preview: preview.into_raw(),
            signature,
            edges,
        }
    }

    pub(super) fn similar(&self, other: &Self) -> bool {
        let a = u64::from(self.width) * u64::from(other.height);
        let b = u64::from(other.width) * u64::from(self.height);
        if 100 * a.abs_diff(b) > 3 * a.max(b) || (self.edges ^ other.edges).count_ones() > 8 {
            return false;
        }
        let distance: usize = self
            .signature
            .iter()
            .zip(&other.signature)
            .map(|(x, y)| usize::from(x.abs_diff(*y)))
            .sum();
        distance <= 12 * self.signature.len()
    }
}

pub(super) fn load(item: &Item, stop: &AtomicBool) -> io::Result<Visual> {
    fn rejected() -> io::Error {
        io::Error::other("Immagine esclusa")
    }
    let _local_only = policy::LocalOnly::enter()?;
    if stop.load(Ordering::Acquire)
        || item.bytes > MAX_FILE
        || !item.identity.matches(&item.path)
        || !policy::local_path(&item.path)?
    {
        return Err(rejected());
    }
    let file = policy::open_file(&item.path, 0)?;
    if !item.identity.matches_metadata(&file.metadata()?) {
        return Err(rejected());
    }
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() != item.bytes || metadata.len() > MAX_FILE {
        return Err(rejected());
    }
    let heif = item
        .path
        .extension()
        .is_some_and(|s| s.eq_ignore_ascii_case("heic") || s.eq_ignore_ascii_case("heif"));
    let visual = if heif {
        heif_thumbnail(&file, metadata.len(), stop)?
    } else {
        let mut reader = ImageReader::new(BufReader::new(&file)).with_guessed_format()?;
        if !matches!(reader.format(), Some(ImageFormat::Png | ImageFormat::Jpeg)) {
            return Err(rejected());
        }
        let mut limits = Limits::default();
        limits.max_image_width = Some(8192);
        limits.max_image_height = Some(8192);
        limits.max_alloc = Some(96 * 1024 * 1024);
        reader.limits(limits);
        let mut decoder = reader.into_decoder().map_err(io::Error::other)?;
        let (w, h) = decoder.dimensions();
        if w == 0
            || h == 0
            || u64::from(w) * u64::from(h) > MAX_PIXELS
            || decoder.total_bytes() > 64 * 1024 * 1024
            || stop.load(Ordering::Acquire)
        {
            return Err(rejected());
        }
        let orientation = decoder.orientation().map_err(io::Error::other)?;
        let mut decoded = DynamicImage::from_decoder(decoder).map_err(io::Error::other)?;
        decoded.apply_orientation(orientation);
        Visual::from_image(&decoded)
    };
    if stop.load(Ordering::Acquire)
        || !item.identity.matches_metadata(&file.metadata()?)
        || !item.identity.matches(&item.path)
        || !policy::local_path(&item.path)?
    {
        return Err(rejected());
    }
    Ok(visual)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};
    #[test]
    fn visually_matches_resizing_and_small_color_changes() {
        let sample = |size, shift| {
            DynamicImage::ImageRgb8(RgbImage::from_fn(size, size, |x, y| {
                Rgb([(180 * x / size) as u8 + shift, (180 * y / size) as u8, 120])
            }))
        };
        let a = Visual::from_image(&sample(120, 0));
        assert!(a.similar(&Visual::from_image(&sample(75, 3))));
        assert!(!a.similar(&Visual::from_image(&DynamicImage::new_rgb8(120, 120))));
        assert!(!a.similar(&Visual::from_image(&sample(120, 0).rotate90())));
    }
}

fn heif_thumbnail(mut file: &std::fs::File, len: u64, stop: &AtomicBool) -> io::Result<Visual> {
    // Read the pinned descriptor in bounded chunks. ImageIO never reopens a
    // path or maps a File Provider placeholder through a URL.
    if len == 0 || len > MAX_FILE {
        return Err(io::Error::other("Dimensione HEIF esclusa"));
    }
    let mut data = Vec::new();
    data.try_reserve_exact(len as usize)
        .map_err(io::Error::other)?;
    let mut buffer = [0u8; 65536];
    while data.len() < len as usize {
        if stop.load(Ordering::Acquire) {
            return Err(io::ErrorKind::Interrupted.into());
        }
        let cap = buffer.len().min(len as usize - data.len());
        let n = file.read(&mut buffer[..cap])?;
        if n == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        data.extend_from_slice(&buffer[..n]);
    }
    native_thumbnail::decode(&data, stop)
}

mod native_thumbnail {
    use super::{Visual, MAX_PIXELS};
    use image::{DynamicImage, RgbaImage};
    use std::{
        ffi::{c_void, CStr},
        io, ptr,
        sync::atomic::{AtomicBool, Ordering},
    };

    type Ref = *const c_void;
    #[repr(C)]
    struct Rect {
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFRelease(value: Ref);
        fn CFDataCreateWithBytesNoCopy(
            alloc: Ref,
            data: *const u8,
            length: isize,
            deallocator: Ref,
        ) -> Ref;
        fn CFDictionaryCreate(
            alloc: Ref,
            keys: *const Ref,
            values: *const Ref,
            count: isize,
            key_callbacks: Ref,
            value_callbacks: Ref,
        ) -> Ref;
        fn CFDictionaryGetValue(dict: Ref, key: Ref) -> Ref;
        fn CFNumberCreate(alloc: Ref, kind: i32, value: *const c_void) -> Ref;
        fn CFNumberGetValue(number: Ref, kind: i32, value: *mut c_void) -> u8;
        fn CFGetTypeID(value: Ref) -> usize;
        fn CFNumberGetTypeID() -> usize;
        fn CFStringGetCString(string: Ref, buf: *mut i8, size: isize, encoding: u32) -> u8;
        static kCFAllocatorNull: Ref;
        static kCFBooleanTrue: Ref;
        static kCFBooleanFalse: Ref;
    }
    #[link(name = "ImageIO", kind = "framework")]
    unsafe extern "C" {
        fn CGImageSourceCreateWithData(data: Ref, options: Ref) -> Ref;
        fn CGImageSourceGetType(source: Ref) -> Ref;
        fn CGImageSourceGetCount(source: Ref) -> usize;
        fn CGImageSourceGetPrimaryImageIndex(source: Ref) -> usize;
        fn CGImageSourceCopyPropertiesAtIndex(source: Ref, index: usize, options: Ref) -> Ref;
        fn CGImageSourceCreateThumbnailAtIndex(source: Ref, index: usize, options: Ref) -> Ref;
        static kCGImagePropertyPixelWidth: Ref;
        static kCGImagePropertyPixelHeight: Ref;
        static kCGImagePropertyOrientation: Ref;
        static kCGImageSourceShouldCache: Ref;
        static kCGImageSourceCreateThumbnailWithTransform: Ref;
        static kCGImageSourceCreateThumbnailFromImageAlways: Ref;
        static kCGImageSourceThumbnailMaxPixelSize: Ref;
    }
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGImageGetWidth(image: Ref) -> usize;
        fn CGImageGetHeight(image: Ref) -> usize;
        fn CGColorSpaceCreateWithName(name: Ref) -> Ref;
        static kCGColorSpaceSRGB: Ref;
        fn CGBitmapContextCreate(
            data: *mut c_void,
            w: usize,
            h: usize,
            bits: usize,
            stride: usize,
            space: Ref,
            flags: u32,
        ) -> Ref;
        fn CGContextDrawImage(ctx: Ref, rect: Rect, image: Ref);
    }
    struct Owned(Ref);
    impl Owned {
        fn new(r: Ref) -> io::Result<Self> {
            if r.is_null() {
                Err(bad())
            } else {
                Ok(Self(r))
            }
        }
    }
    impl Drop for Owned {
        fn drop(&mut self) {
            unsafe { CFRelease(self.0) }
        }
    }
    fn bad() -> io::Error {
        io::Error::other("Formato HEIF non verificabile o fuori limite")
    }
    fn number(dict: Ref, key: Ref) -> Option<i32> {
        unsafe {
            let n = CFDictionaryGetValue(dict, key);
            let mut value = 0i32;
            (!n.is_null()
                && CFGetTypeID(n) == CFNumberGetTypeID()
                && CFNumberGetValue(n, 3, (&mut value as *mut i32).cast()) != 0)
                .then_some(value)
        }
    }
    fn options<const N: usize>(keys: [Ref; N], values: [Ref; N]) -> io::Result<Owned> {
        // All referenced values outlive this non-retaining dictionary.
        Owned::new(unsafe {
            CFDictionaryCreate(
                ptr::null(),
                keys.as_ptr(),
                values.as_ptr(),
                N as isize,
                ptr::null(),
                ptr::null(),
            )
        })
    }
    pub(super) fn decode(data: &[u8], stop: &AtomicBool) -> io::Result<Visual> {
        objc2::rc::autoreleasepool(|_| unsafe {
            let limit = 32i32;
            let max = Owned::new(CFNumberCreate(
                ptr::null(),
                3,
                (&limit as *const i32).cast(),
            ))?;
            let config = options(
                [
                    kCGImageSourceShouldCache,
                    kCGImageSourceCreateThumbnailWithTransform,
                    kCGImageSourceCreateThumbnailFromImageAlways,
                    kCGImageSourceThumbnailMaxPixelSize,
                ],
                [kCFBooleanFalse, kCFBooleanTrue, kCFBooleanTrue, max.0],
            )?;
            // The Rust slice remains alive until all source and image references drop.
            let bytes = Owned::new(CFDataCreateWithBytesNoCopy(
                ptr::null(),
                data.as_ptr(),
                data.len() as isize,
                kCFAllocatorNull,
            ))?;
            let source = Owned::new(CGImageSourceCreateWithData(bytes.0, config.0))?;
            let kind = CGImageSourceGetType(source.0);
            let mut buf = [0i8; 128];
            if kind.is_null()
                || CFStringGetCString(kind, buf.as_mut_ptr(), buf.len() as isize, 0x08000100) == 0
                || !matches!(
                    CStr::from_ptr(buf.as_ptr()).to_bytes(),
                    b"public.heic" | b"public.heif"
                )
            {
                return Err(bad());
            }
            let count = CGImageSourceGetCount(source.0);
            if count == 0 || count > 256 {
                return Err(bad());
            }
            let index = CGImageSourceGetPrimaryImageIndex(source.0);
            if index >= count {
                return Err(bad());
            }
            let props = Owned::new(CGImageSourceCopyPropertiesAtIndex(
                source.0, index, config.0,
            ))?;
            let mut w = number(props.0, kCGImagePropertyPixelWidth).ok_or_else(bad)?;
            let mut h = number(props.0, kCGImagePropertyPixelHeight).ok_or_else(bad)?;
            if !(1..=8192).contains(&w)
                || !(1..=8192).contains(&h)
                || (w as u64) * (h as u64) > MAX_PIXELS
            {
                return Err(bad());
            }
            let orient = number(props.0, kCGImagePropertyOrientation).unwrap_or(1);
            if !(1..=8).contains(&orient) {
                return Err(bad());
            }
            if orient >= 5 {
                std::mem::swap(&mut w, &mut h);
            }
            if stop.load(Ordering::Acquire) {
                return Err(io::ErrorKind::Interrupted.into());
            }
            let img = Owned::new(CGImageSourceCreateThumbnailAtIndex(
                source.0, index, config.0,
            ))?;
            let width = CGImageGetWidth(img.0);
            let height = CGImageGetHeight(img.0);
            if !(1..=32).contains(&width) || !(1..=32).contains(&height) {
                return Err(bad());
            }
            let space = Owned::new(CGColorSpaceCreateWithName(kCGColorSpaceSRGB))?;
            let mut pixels = vec![255u8; width * height * 4];
            // RGBA byte order, opaque white backdrop. The alpha byte is skipped by Core Graphics.
            let ctx = Owned::new(CGBitmapContextCreate(
                pixels.as_mut_ptr().cast(),
                width,
                height,
                8,
                width * 4,
                space.0,
                0x4000 | 5,
            ))?;
            CGContextDrawImage(
                ctx.0,
                Rect {
                    x: 0.,
                    y: 0.,
                    width: width as f64,
                    height: height as f64,
                },
                img.0,
            );
            drop(ctx);
            for rgba in pixels.as_chunks_mut::<4>().0 {
                rgba[3] = 255;
            }
            let preview =
                RgbaImage::from_raw(width as u32, height as u32, pixels).ok_or_else(bad)?;
            let mut visual = Visual::from_image(&DynamicImage::ImageRgba8(preview));
            visual.width = w as u32;
            visual.height = h as u32;
            Ok(visual)
        })
    }
}

#[cfg(test)]
mod heif_tests {
    use super::*;
    use crate::model::Identity;
    use image::{Rgb, RgbImage};
    use std::{fs, process::Command};
    fn candidate(path: &Path) -> Item {
        assert!(policy::local_path(path).unwrap());
        Item {
            path: path.into(),
            bytes: fs::metadata(path).unwrap().len(),
            identity: Identity::read(path).unwrap(),
            group: None,
            cloud: false,
        }
    }
    #[test]
    fn native_heic_thumbnail_matches_original_jpeg_and_rejects_corruption() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let jpeg = root.join("original.jpg");
        let heic = root.join("photo.heic");
        RgbImage::from_fn(160, 96, |x, y| {
            Rgb([(30 + x) as u8, (20 + 2 * y) as u8, (210 - y) as u8])
        })
        .save(&jpeg)
        .unwrap();
        let convert = Command::new("/usr/bin/sips")
            .args(["-s", "format", "heic"])
            .arg(&jpeg)
            .arg("--out")
            .arg(&heic)
            .output()
            .unwrap();
        assert!(
            convert.status.success(),
            "{}",
            String::from_utf8_lossy(&convert.stderr)
        );
        let stop = AtomicBool::new(false);
        let original = load(&candidate(&jpeg), &stop).unwrap();
        let pic = load(&candidate(&heic), &stop).unwrap();
        assert_eq!((pic.width, pic.height), (160, 96));
        assert!(original.similar(&pic));
        let renamed = root.join("image.HEIF");
        fs::copy(&heic, &renamed).unwrap();
        assert!(supported(&renamed));
        assert!(original.similar(&load(&candidate(&renamed), &stop).unwrap()));
        let stale = candidate(&heic);
        fs::write(&heic, b"ftypHEIC").unwrap();
        assert!(load(&stale, &stop).is_err());
        assert!(load(&candidate(&heic), &stop).is_err());
        let disguised = root.join("renamed.heic");
        fs::copy(&jpeg, &disguised).unwrap();
        assert!(load(&candidate(&disguised), &stop).is_err());
    }
}
