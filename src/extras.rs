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

pub(crate) fn single_drop(
    files: &[eframe::egui::DroppedFile],
) -> Option<Result<&std::path::Path, &'static str>> {
    match files {
        [] => None,
        [file] => Some(file.path.as_deref().ok_or("Scegli un elemento locale.")),
        _ => Some(Err("Trascina un solo elemento alla volta.")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ambiguous_drops_never_choose_a_target() {
        let file = eframe::egui::DroppedFile {
            path: Some("/synthetic/Demo.app".into()),
            ..Default::default()
        };
        assert_eq!(single_drop(&[]), None);
        assert_eq!(
            single_drop(std::slice::from_ref(&file)),
            Some(Ok(file.path.as_deref().unwrap()))
        );
        assert!(single_drop(&[file.clone(), file]).unwrap().is_err());
        assert!(single_drop(&[Default::default()]).unwrap().is_err());
    }
}
