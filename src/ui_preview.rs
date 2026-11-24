use super::*;
use crate::model::Item;
use std::fs::File;
use std::os::unix::fs::MetadataExt;
use std::sync::Mutex;

// Only one user-requested thumbnail or worker is retained in a window.
#[derive(Default)]
struct Preview {
    source: Option<(PathBuf, crate::trash::Identity)>,
    worker: Option<mpsc::Receiver<Option<egui::ColorImage>>>,
    texture: Option<egui::TextureHandle>,
}

fn media(item: &Item) -> Option<(&'static str, &'static str)> {
    let ext = item.path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "jpg" | "jpeg" => ("Foto", "public.jpeg"),
        "png" => ("Foto", "public.png"),
        "heic" => ("Foto", "public.heic"),
        "tif" | "tiff" => ("Foto", "public.tiff"),
        "webp" => ("Foto", "org.webmproject.webp"),
        "gif" => ("Foto", "com.compuserve.gif"),
        "mov" => ("Video", "com.apple.quicktime-movie"),
        "mp4" | "m4v" => ("Video", "public.mpeg-4"),
        _ => return None,
    })
}

fn thumbnail(item: &Item, content_type: &str) -> Option<egui::ColorImage> {
    let _local = crate::policy::LocalOnly::enter().ok()?;
    if item.cloud
        || crate::policy::protected(&item.path, Mode::Duplicates, None)
        || !crate::policy::local_path(&item.path).ok()?
        || crate::trash::Identity::read(&item.path).ok()? != item.identity
    {
        return None;
    }
    let file = crate::policy::open_file(&item.path, 0).ok()?;
    let meta = file.metadata().ok()?;
    if !meta.is_file() || !item.identity.matches_metadata(&meta) {
        return None;
    }
    // Darwin's volume/inode path names the node held open by this descriptor.
    // A rename or replacement of the user-visible path cannot redirect Quick
    // Look or cause the source media to be copied into temporary storage.
    let pinned = PathBuf::from(format!("/.vol/{}/{}", meta.dev(), meta.ino()));
    if !item
        .identity
        .matches_metadata(&std::fs::metadata(&pinned).ok()?)
    {
        return None;
    }
    let dir = tempfile::tempdir().ok()?;
    let result = crate::system_tools::command::run(
        "/usr/bin/qlmanage",
        &[
            "-t",
            "-s",
            "280",
            "-c",
            content_type,
            "-o",
            dir.path().to_str()?,
            pinned.to_str()?,
        ],
        None,
        &AtomicBool::new(false),
        6,
    )
    .ok()?;
    if result.code != Some(0) || !item.identity.matches_metadata(&file.metadata().ok()?) {
        return None;
    }
    let bytes = File::open(dir.path().join(format!("{}.png", meta.ino()))).ok()?;
    if bytes.metadata().ok()?.len() > 2 * 1024 * 1024 {
        return None;
    }
    let mut reader =
        image::ImageReader::with_format(std::io::BufReader::new(bytes), image::ImageFormat::Png);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(560);
    limits.max_image_height = Some(560);
    limits.max_alloc = Some(4 * 1024 * 1024);
    reader.limits(limits);
    let image = reader.decode().ok()?.to_rgba8();
    Some(egui::ColorImage::from_rgba_unmultiplied(
        [image.width() as usize, image.height() as usize],
        &image,
    ))
}

pub(super) fn show(ui: &mut egui::Ui, item: &Item, width: f32) {
    let id = egui::Id::new("duplicate-media-preview");
    let handle = ui
        .ctx()
        .data_mut(|d| d.get_temp_mut_or_default::<Arc<Mutex<Preview>>>(id).clone());
    let mut state = handle.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(receiver) = &state.worker {
        match receiver.try_recv() {
            Ok(image) => {
                state.texture = image.map(|i| {
                    ui.ctx()
                        .load_texture("Anteprima locale", i, egui::TextureOptions::LINEAR)
                });
                state.worker = None;
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                state.worker = None;
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }
    }
    let matches = state
        .source
        .as_ref()
        .is_some_and(|(p, identity)| p == &item.path && identity == &item.identity);
    let classification = media(item);
    let previewable = !item.cloud && !item.identity.is_dir() && classification.is_some();
    let height = (width * 0.53).clamp(54., 94.);
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(width, height),
        if previewable {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        },
    );
    let label = classification.as_ref().map_or("File", |(kind, _)| *kind);
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Button,
            previewable,
            format!("Anteprima {label}"),
        )
    });
    let painter = ui.painter();
    painter.rect_filled(rect, 9., egui::Color32::from_rgb(39, 58, 49));
    if let Some(tex) = state.texture.as_ref().filter(|_| matches) {
        let scale = (rect.width() / tex.size_vec2().x).min(rect.height() / tex.size_vec2().y);
        let image_rect = egui::Rect::from_center_size(rect.center(), tex.size_vec2() * scale);
        painter.image(
            tex.id(),
            image_rect,
            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1., 1.)),
            egui::Color32::WHITE,
        );
    } else {
        let mark =
            egui::Rect::from_center_size(rect.center() - egui::vec2(0., 8.), egui::vec2(29., 22.));
        let stroke = egui::Stroke::new(1.6_f32, design::CREAM);
        painter.rect_stroke(mark, 3., stroke, egui::StrokeKind::Inside);
        if label == "Video" {
            painter.add(egui::Shape::convex_polygon(
                vec![
                    mark.center() + egui::vec2(-3., -5.),
                    mark.center() + egui::vec2(5., 0.),
                    mark.center() + egui::vec2(-3., 5.),
                ],
                design::LIME,
                egui::Stroke::NONE,
            ));
        } else if label == "Foto" {
            painter.line_segment(
                [mark.left_bottom() + egui::vec2(3., -4.), mark.center()],
                stroke,
            );
            painter.line_segment(
                [mark.center(), mark.right_bottom() + egui::vec2(-3., -3.)],
                stroke,
            );
            painter.circle_filled(mark.right_top() + egui::vec2(-7., 6.), 2., design::LIME);
        } else {
            for offset in [-3., 3.] {
                painter.line_segment(
                    [
                        mark.center() + egui::vec2(-8., offset),
                        mark.center() + egui::vec2(8., offset),
                    ],
                    stroke,
                );
            }
        }
        let text = if matches && state.worker.is_some() {
            "Caricamento…"
        } else if matches {
            "Non disponibile"
        } else if previewable {
            "Anteprima"
        } else {
            label
        };
        painter.text(
            rect.center() + egui::vec2(0., 15.),
            egui::Align2::CENTER_CENTER,
            text,
            egui::FontId::proportional(12.),
            design::CREAM,
        );
    }
    let hint = if matches && state.worker.is_none() && state.texture.is_none() {
        "Quick Look non ha restituito un’anteprima locale verificata. Premi per riprovare."
    } else {
        "Mostra l’anteprima locale con Quick Look"
    };
    if previewable && response.on_hover_text(hint).clicked() && state.worker.is_none() {
        let content_type = classification.unwrap().1;
        let item = item.clone();
        state.source = Some((item.path.clone(), item.identity.clone()));
        state.texture = None;
        let (sender, receiver) = mpsc::sync_channel(1);
        state.worker = Some(receiver);
        let ctx = ui.ctx().clone();
        std::thread::spawn(move || {
            let _ = sender.send(thumbnail(&item, content_type));
            ctx.request_repaint();
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires macOS Quick Look services in a graphical session"]
    fn quick_look_uses_verified_local_image_and_rejects_replacement() {
        let temp = tempfile::Builder::new()
            .prefix("lindo-preview-")
            .tempdir()
            .unwrap();
        let path = temp.path().canonicalize().unwrap().join("fixture.png");
        let pixels = image::RgbaImage::from_pixel(50, 25, image::Rgba([22, 98, 41, 255]));
        pixels.save(&path).unwrap();
        let item = Item {
            path: path.clone(),
            identity: crate::trash::Identity::read(&path).unwrap(),
            bytes: std::fs::metadata(&path).unwrap().len(),
            group: Some(1),
            cloud: false,
        };
        let preview =
            thumbnail(&item, "public.png").expect("local PNG should have a Quick Look thumbnail");
        assert!(preview.size.iter().all(|&side| side > 0 && side <= 560));
        std::fs::rename(&path, path.with_extension("old")).unwrap();
        pixels.save(&path).unwrap();
        assert!(thumbnail(&item, "public.png").is_none());
    }
}
