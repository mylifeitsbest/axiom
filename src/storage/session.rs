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
    /// «Не усыплять»: после запуска вкладка всё равно спит до первого клика.
    #[serde(default)]
    pub no_sleep: bool,
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
        // Выброшенные вкладки левее активной сдвигают её номер.
        let dropped_before = s.tabs[..s.active.min(s.tabs.len())]
            .iter()
            .filter(|t| !is_restorable(&t.url))
            .count();
        s.active -= dropped_before;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn tab(url: &str) -> SavedTab {
        SavedTab {
            url: url.into(),
            title: String::new(),
            ai: false,
            no_sleep: false,
        }
    }

    #[test]
    fn no_sleep_is_optional_and_kept() {
        // Сессия старых версий без поля по-прежнему читается.
        let old: Result<SavedTab, _> =
            serde_json::from_str(r#"{"url":"https://a.example/","title":"a"}"#);
        assert!(old.is_ok_and(|t| !t.no_sleep));
        let saved = SavedTab {
            no_sleep: true,
            ..tab("https://a.example/")
        };
        let back = serde_json::to_string(&saved)
            .ok()
            .and_then(|json| serde_json::from_str::<SavedTab>(&json).ok());
        assert!(back.is_some_and(|t| t.no_sleep));
    }

    #[test]
    fn active_survives_dropped_tabs() {
        let root = std::env::temp_dir().join(format!("axiom-session-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&root);
        let paths = Paths { root: root.clone() };
        let saved = Session {
            tabs: vec![
                tab("data:text/html,x"),
                tab("https://a.example/"),
                tab("about:blank"),
                tab("https://b.example/"),
            ],
            active: 3,
        };
        assert!(saved.save(&paths).is_ok());
        let loaded = Session::load(&paths);
        let _ = std::fs::remove_dir_all(&root);
        let loaded = loaded.unwrap_or_default();
        assert_eq!(loaded.tabs.len(), 2);
        assert_eq!(loaded.tabs[loaded.active].url, "https://b.example/");
    }

    #[test]
    fn closed_all_tabs_starts_fresh() {
        // Последняя вкладка закрывает окно, и сессия сохраняется пустой:
        // следующий запуск начинается с новой вкладки, а не со старых.
        let root = std::env::temp_dir().join(format!("axiom-session-empty-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&root);
        let paths = Paths { root: root.clone() };
        let saved = Session {
            tabs: Vec::new(),
            active: 0,
        };
        assert!(saved.save(&paths).is_ok());
        let loaded = Session::load(&paths);
        let _ = std::fs::remove_dir_all(&root);
        assert!(loaded.is_none());
    }
}
