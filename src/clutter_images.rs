//! Conservative visual matches against a fixed reference per group.
use crate::{model::Item, policy};
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader, Limits};
use std::{
    io::{self, BufReader},
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
        ["png", "jpeg", "jpg"]
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
    if stop.load(Ordering::Acquire)
        || !item.identity.matches_metadata(&file.metadata()?)
        || !item.identity.matches(&item.path)
        || !policy::local_path(&item.path)?
    {
        return Err(rejected());
    }
    decoded.apply_orientation(orientation);
    Ok(Visual::from_image(&decoded))
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
