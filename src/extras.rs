use crate::model::{Item, Request};

pub(crate) struct Proposal {
    pub request: Request,
    pub items: Vec<Item>,
    pub retained: Vec<Item>,
    pub label: String,
}

pub(crate) fn file_checkbox(
    ui: &mut eframe::egui::Ui,
    selected: &mut bool,
    enabled: bool,
    path: &std::path::Path,
) -> eframe::egui::Response {
    let response = ui.add_enabled(enabled, eframe::egui::Checkbox::without_text(selected));
    ui.ctx().accesskit_node_builder(response.id, |node| {
        node.set_label(format!("Seleziona {}", path.display()));
    });
    response
}
