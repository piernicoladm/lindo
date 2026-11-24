use crate::model::Item;
use eframe::egui::{self, Align2, Color32, FontId, Rect, Sense};

// Split at the cumulative weight closest to half, along the longer side.
fn tiles(weights: &[u64], rect: Rect, output: &mut Vec<Rect>) {
    if weights.is_empty() {
        return;
    }
    if weights.len() == 1 {
        output.push(rect);
        return;
    }
    let total: u128 = weights.iter().copied().map(u128::from).sum();
    let mut left = u128::from(weights[0]);
    let mut split = 1;
    while split < weights.len() - 1 {
        let next = left + u128::from(weights[split]);
        if (next * 2).abs_diff(total) >= (left * 2).abs_diff(total) {
            break;
        }
        left = next;
        split += 1;
    }
    let fraction = left as f32 / total as f32;
    let mut a = rect;
    let mut b = rect;
    if rect.width() > rect.height() {
        a.max.x = rect.min.x + fraction * rect.width();
        b.min.x = a.max.x;
    } else {
        a.max.y = rect.min.y + fraction * rect.height();
        b.min.y = a.max.y;
    }
    tiles(&weights[..split], a, output);
    tiles(&weights[split..], b, output);
}

pub(crate) fn show(ui: &mut egui::Ui, items: &[Item], total: u64) -> Option<usize> {
    let mut weights: Vec<_> = items
        .iter()
        .take(18)
        .map(|row| row.bytes)
        .take_while(|&n| n > 0)
        .collect();
    let named = weights.len();
    let others = total.saturating_sub(weights.iter().fold(0u64, |sum, &w| sum.saturating_add(w)));
    if others > 0 {
        weights.push(others);
    }
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 145.), Sense::hover());
    let mut cells = Vec::new();
    tiles(&weights, rect, &mut cells);
    let mut click = None;
    for (i, cell) in cells.into_iter().enumerate() {
        if cell.width() < 2. || cell.height() < 2. {
            continue;
        }
        let cell = cell.shrink(1.);
        let response = ui.interact(
            cell,
            ui.id().with(("treemap", i)),
            if i < named {
                Sense::click()
            } else {
                Sense::hover()
            },
        );
        let label = if i < named {
            items[i]
                .path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
        } else {
            "Altri elementi".into()
        };
        let color = [
            Color32::from_rgb(30, 90, 110),
            Color32::from_rgb(55, 75, 120),
            Color32::from_rgb(28, 104, 86),
        ][i % 3];
        let painter = ui.painter().with_clip_rect(cell);
        painter.rect_filled(
            cell,
            2.,
            if response.hovered() {
                color.gamma_multiply(1.25)
            } else {
                color
            },
        );
        if cell.width() >= 38. && cell.height() >= 16. {
            painter.text(
                cell.left_center() + egui::vec2(4., 0.),
                Align2::LEFT_CENTER,
                &label,
                FontId::proportional(12.),
                Color32::WHITE,
            );
        }
        if response.clicked() && i < named {
            click = Some(i);
        }
        response.on_hover_text(format!("{} · {}", label, crate::ui::size(weights[i])));
    }
    click
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn area_tracks_bytes_and_fills_the_panel() {
        let rect = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(200., 100.));
        let mut cells = Vec::new();
        tiles(&[8, 5, 4, 3], rect, &mut cells);
        assert_eq!(cells.len(), 4);
        for (r, weight) in cells.iter().zip([8., 5., 4., 3.]) {
            assert!((r.area() - weight * 1000.).abs() < 0.02);
        }
        assert!(cells.iter().all(|r| rect.contains_rect(*r)));
    }
}
