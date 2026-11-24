use eframe::egui::{
    self, pos2, vec2, Align2, Color32, FontData, FontFamily, FontId, Pos2, Response, RichText,
    Stroke, TextStyle, Ui,
};
use std::sync::Arc;

pub(super) const INK: Color32 = Color32::from_rgb(20, 36, 28);
pub(super) const CREAM: Color32 = Color32::from_rgb(246, 244, 232);
pub(super) const LIME: Color32 = Color32::from_rgb(184, 227, 85);
pub(super) const MUTED: Color32 = Color32::from_rgb(183, 197, 186);
pub(super) const LINE: Color32 = Color32::from_rgb(62, 82, 71);
pub(super) const CANVAS: Color32 = Color32::from_rgb(17, 36, 28);
pub(super) const RAIL: Color32 = Color32::from_rgb(26, 32, 31);
const SURFACE: Color32 = Color32::from_rgb(36, 55, 45);
const HOVER: Color32 = Color32::from_rgb(53, 73, 60);

fn bold() -> FontFamily {
    FontFamily::Name("Lindo Display".into())
}

pub(super) fn display(text: impl Into<String>, size: f32) -> RichText {
    RichText::new(text)
        .font(FontId::new(size, bold()))
        .color(CREAM)
}

pub(super) fn configure(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let mut headline = fonts.families[&FontFamily::Proportional].clone();
    // Use the Mac's own Helvetica Neue regular and bold faces. Keep the bundled
    // fallback stack for missing glyphs and hosts without the system collection.
    if let Ok(bytes) = std::fs::read("/System/Library/Fonts/HelveticaNeue.ttc") {
        let normal = FontData::from_owned(bytes.clone());
        let mut strong = FontData::from_owned(bytes);
        strong.index = 1;
        fonts
            .font_data
            .insert("Helvetica Neue".into(), Arc::new(normal));
        fonts
            .font_data
            .insert("Helvetica Neue Bold".into(), Arc::new(strong));
        fonts
            .families
            .get_mut(&FontFamily::Proportional)
            .unwrap()
            .insert(0, "Helvetica Neue".into());
        headline.insert(0, "Helvetica Neue Bold".into());
    }
    fonts.families.insert(bold(), headline);
    ctx.set_fonts(fonts);
    ctx.set_theme(egui::Theme::Dark);
    let mut style = (*ctx.style()).clone();
    style
        .text_styles
        .insert(TextStyle::Heading, FontId::new(32.0, bold()));
    style
        .text_styles
        .insert(TextStyle::Body, FontId::proportional(15.0));
    style
        .text_styles
        .insert(TextStyle::Button, FontId::proportional(14.0));
    style
        .text_styles
        .insert(TextStyle::Small, FontId::proportional(12.5));
    style.spacing.item_spacing = vec2(8.0, 7.0);
    style.spacing.button_padding = vec2(11.0, 6.0);
    style.spacing.interact_size = vec2(32.0, 27.0);
    let v = &mut style.visuals;
    *v = egui::Visuals::dark();
    v.panel_fill = CANVAS;
    v.window_fill = SURFACE;
    v.window_stroke = Stroke::new(1.0_f32, LINE);
    v.extreme_bg_color = INK;
    v.faint_bg_color = SURFACE;
    v.code_bg_color = RAIL;
    v.hyperlink_color = LIME;
    v.interact_cursor = Some(egui::CursorIcon::PointingHand);
    v.selection.bg_fill = Color32::from_rgb(58, 83, 63);
    v.selection.stroke = Stroke::new(1.6_f32, LIME);
    v.warn_fg_color = Color32::from_rgb(244, 204, 136);
    v.error_fg_color = Color32::from_rgb(251, 163, 153);
    for (widget, fill) in [
        (&mut v.widgets.noninteractive, CANVAS),
        (&mut v.widgets.inactive, SURFACE),
        (&mut v.widgets.hovered, HOVER),
        (&mut v.widgets.active, HOVER),
        (&mut v.widgets.open, SURFACE),
    ] {
        widget.bg_fill = fill;
        widget.weak_bg_fill = fill;
        widget.fg_stroke = Stroke::new(1.4_f32, CREAM);
        widget.bg_stroke = Stroke::new(1.0_f32, LINE);
        widget.corner_radius = egui::CornerRadius::same(7);
        widget.expansion = 0.0;
    }
    v.widgets.hovered.bg_stroke = Stroke::new(1.2_f32, MUTED);
    v.widgets.active.bg_stroke = Stroke::new(1.4_f32, LIME);
    ctx.set_style(style);
}

pub(super) fn primary(ui: &mut Ui, label: &str, enabled: bool) -> Response {
    ui.add_enabled(
        enabled,
        egui::Button::new(display(label, 16.0).color(INK))
            .fill(LIME)
            .corner_radius(11)
            .min_size(vec2(112.0, 38.0)),
    )
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Destination {
    Home,
    Map,
    Cleanup,
    Apps,
    Tools,
    Settings,
}
impl Destination {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Home => "Panoramica",
            Self::Map => "Mappa spazio",
            Self::Cleanup => "Pulizia",
            Self::Apps => "Applicazioni",
            Self::Tools => "Strumenti",
            Self::Settings => "Impostazioni",
        }
    }
}

fn symbol(painter: &egui::Painter, origin: Pos2, dest: Destination) {
    let stroke = Stroke::new(1.6_f32, CREAM);
    let p = |x, y| origin + vec2(x, y);
    let line = |points: &[(f32, f32)]| {
        painter.add(egui::Shape::line(
            points.iter().map(|&(x, y)| p(x, y)).collect(),
            stroke,
        ));
    };
    match dest {
        Destination::Home => {
            line(&[(1., 10.), (11., 1.), (21., 10.)]);
            line(&[
                (4., 8.),
                (4., 20.),
                (9., 20.),
                (9., 14.),
                (14., 14.),
                (14., 20.),
                (18., 20.),
                (18., 8.),
            ]);
        }
        Destination::Map => {
            painter.circle_stroke(p(10., 12.), 8.5, stroke);
            line(&[(10., 3.5), (10., 12.), (18.5, 12.)]);
            line(&[(14., 1.), (14., 8.), (21., 8.), (19., 4.), (14., 1.)]);
        }
        Destination::Cleanup => {
            line(&[
                (3., 12.),
                (8., 8.),
                (11., 11.),
                (18., 3.),
                (21., 5.),
                (14., 14.),
                (14., 17.),
                (10., 20.),
                (3., 15.),
                (3., 12.),
            ]);
            line(&[(5., 12.), (11., 16.)]);
        }
        Destination::Apps => {
            line(&[(3., 19.), (13., 2.)]);
            line(&[(8., 2.), (18.5, 19.)]);
            line(&[(1., 15.), (21., 15.)]);
        }
        Destination::Tools => {
            line(&[
                (2., 17.),
                (10., 9.),
                (10., 5.),
                (13., 2.),
                (18., 2.),
                (14., 6.),
                (17., 9.),
                (21., 5.),
                (21., 10.),
                (18., 13.),
                (14., 13.),
                (6., 21.),
                (2., 17.),
            ]);
        }
        Destination::Settings => {
            let center = p(11., 11.);
            painter.circle_stroke(center, 7., stroke);
            painter.circle_stroke(center, 2.6, stroke);
            for i in 0..8 {
                let angle = i as f32 * std::f32::consts::FRAC_PI_4;
                let dir = vec2(angle.cos(), angle.sin());
                painter.line_segment(
                    [center + dir * 7., center + dir * 9.5],
                    Stroke::new(2.5_f32, CREAM),
                );
            }
        }
    }
}

pub(super) fn nav(ui: &mut Ui, dest: Destination, selected: bool) -> Response {
    let label = dest.label();
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), 38.0), egui::Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::SelectableLabel,
            ui.is_enabled(),
            selected,
            label,
        )
    });
    let paint = ui.painter();
    if selected || response.hovered() || response.has_focus() {
        let fill = if selected {
            Color32::from_rgb(57, 70, 60)
        } else {
            SURFACE
        };
        paint.rect_filled(rect, 8.0, fill);
        if response.has_focus() {
            paint.rect_stroke(
                rect,
                8.0,
                Stroke::new(1.0_f32, LIME),
                egui::StrokeKind::Inside,
            );
        }
    }
    symbol(paint, pos2(rect.left() + 10., rect.center().y - 11.), dest);
    paint.text(
        pos2(rect.left() + 41., rect.center().y),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(16.),
        CREAM,
    );
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}
