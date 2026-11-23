//! Сессия: открытые вкладки восстанавливаются лениво (спящими) при запуске.

use serde::{Deserialize, Serialize};

use super::{Paths, read_json, write_json};

const FILE: &str = "session.json";
const MAX_TABS: usize = 500;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedTab {
    pub url: String,
    pub title: String,
    #[serde(default)]
    pub ai: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Session {
    pub tabs: Vec<SavedTab>,
    pub active: usize,
}

impl Session {
    pub fn load(paths: &Paths) -> Option<Self> {
        let mut s: Session = read_json(&paths.root.join(FILE))?;
        s.tabs.truncate(MAX_TABS);
        s.tabs.retain(|t| is_restorable(&t.url));
        if s.tabs.is_empty() {
            return None;
        }
        s.active = s.active.min(s.tabs.len() - 1);
        Some(s)
    }

    pub fn save(&self, paths: &Paths) -> anyhow::Result<()> {
        write_json(&paths.root.join(FILE), self)
    }
}

fn is_restorable(url: &str) -> bool {
    url == crate::tabs::omnibox::NEWTAB
        || url.starts_with("https://")
        || url.starts_with("http://")
        || url.starts_with("file://")
}
