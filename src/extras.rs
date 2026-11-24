use crate::model::{Item, Request};

pub(crate) struct Proposal {
    pub request: Request,
    pub items: Vec<Item>,
    pub retained: Vec<Item>,
    pub label: String,
}
