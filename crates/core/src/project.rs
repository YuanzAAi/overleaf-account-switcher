#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectAccessLevel {
    Owner,
    Collaborator,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectSource {
    Token,
    Direct,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub access_level: ProjectAccessLevel,
    pub source: ProjectSource,
    pub owner_email: Option<String>,
    pub last_updated: Option<String>,
    pub trashed: bool,
}

impl Project {
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            access_level: ProjectAccessLevel::Unknown,
            source: ProjectSource::Unknown,
            owner_email: None,
            last_updated: None,
            trashed: false,
        }
    }

    pub fn is_owner(&self) -> bool {
        self.access_level == ProjectAccessLevel::Owner
    }

    pub fn is_link_sharing_collaboration(&self) -> bool {
        !self.is_owner() && self.source == ProjectSource::Token
    }
}
