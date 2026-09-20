#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Address {
    pub name: String,
    pub line1: String,
    pub city: String,
    pub state: String,
    pub zip: String,
    pub phone: Option<String>,
}

impl Address {
    pub fn has_required_fields(&self) -> bool {
        !self.name.trim().is_empty()
            && !self.line1.trim().is_empty()
            && !self.city.trim().is_empty()
            && !self.state.trim().is_empty()
            && !self.zip.trim().is_empty()
    }
}
