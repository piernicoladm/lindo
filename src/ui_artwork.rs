use eframe::egui;

pub(super) struct Artwork {
    pub(super) home: egui::TextureHandle,
    pub(super) scan: egui::TextureHandle,
    pub(super) logo: egui::TextureHandle,
}

impl Artwork {
    pub(super) fn new(ctx: &egui::Context) -> Self {
        let load = |name, bytes: &[u8]| {
            let pixels = image::load_from_memory_with_format(bytes, image::ImageFormat::Png)
                .expect("embedded Lindo artwork must be a valid PNG")
                .into_rgba8();
            let size = [pixels.width() as usize, pixels.height() as usize];
            let image = egui::ColorImage::from_rgba_unmultiplied(size, pixels.as_raw());
            ctx.load_texture(name, image, egui::TextureOptions::LINEAR)
        };
        Self {
            home: load("lindo-home", include_bytes!("artwork/home.png")),
            scan: load("lindo-scan", include_bytes!("artwork/scan.png")),
            logo: load("lindo-logo", include_bytes!("artwork/logo.png")),
        }
    }
}
