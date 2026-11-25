//! Сессия: открытые вкладки восстанавливаются лениво (спящими) при запуске.

use serde::{Deserialize, Serialize};

use super::{Paths, read_json, write_json};
use crate::tabs::SETTINGS;

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
    /// Громкость вкладки, 0–100. Полная в файл не пишется: так её и читают старые версии.
    #[serde(default = "full_volume", skip_serializing_if = "is_full_volume")]
    pub volume: u8,
}

fn full_volume() -> u8 {
    crate::tabs::volume::FULL
}

fn is_full_volume(volume: &u8) -> bool {
    *volume >= crate::tabs::volume::FULL
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
        // Вкладка настроек восстанавливается ею же, но только одна.
        let mut settings = false;
        let keep: Vec<bool> = s
            .tabs
            .iter()
            .map(|t| {
                let extra = t.url == SETTINGS && std::mem::replace(&mut settings, true);
                is_restorable(&t.url) && !extra
            })
            .collect();
        // Выброшенные вкладки левее активной сдвигают её номер.
        let dropped_before = keep[..s.active.min(keep.len())]
            .iter()
            .filter(|k| !**k)
            .count();
        s.active -= dropped_before;
        let mut keep = keep.into_iter();
        s.tabs.retain(|_| keep.next().unwrap_or(false));
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
        || url == SETTINGS
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
            volume: crate::tabs::volume::FULL,
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
    fn volume_is_optional_and_kept() {
        let full = crate::tabs::volume::FULL;
        let old: Result<SavedTab, _> =
            serde_json::from_str(r#"{"url":"https://a.example/","title":"a"}"#);
        assert!(old.is_ok_and(|t| t.volume == full));
        // Полная громкость в файл не пишется, своя — пишется и читается обратно.
        let json = serde_json::to_string(&tab("https://a.example/")).unwrap_or_default();
        assert!(!json.contains("volume"));
        let saved = SavedTab {
            volume: 30,
            ..tab("https://a.example/")
        };
        let back = serde_json::to_string(&saved)
            .ok()
            .and_then(|json| serde_json::from_str::<SavedTab>(&json).ok());
        assert!(back.is_some_and(|t| t.volume == 30));
    }

    #[test]
    fn settings_tab_restores_once() {
        let root = std::env::temp_dir().join(format!("axiom-session-set-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&root);
        let paths = Paths { root: root.clone() };
        let saved = Session {
            tabs: vec![
                tab(SETTINGS),
                tab("https://a.example/"),
                tab(SETTINGS),
                tab("https://b.example/"),
            ],
            active: 3,
        };
        assert!(saved.save(&paths).is_ok());
        let loaded = Session::load(&paths);
        let _ = std::fs::remove_dir_all(&root);
        let loaded = loaded.unwrap_or_default();
        let urls: Vec<&str> = loaded.tabs.iter().map(|t| t.url.as_str()).collect();
        assert_eq!(urls, [SETTINGS, "https://a.example/", "https://b.example/"]);
        assert_eq!(loaded.active, 2);
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
    fn restores_only_pages_and_own_tabs() {
        for url in [
            "https://a.example/",
            "http://127.0.0.1:8080/",
            "file:///C:/page.html",
            crate::tabs::omnibox::NEWTAB,
            SETTINGS,
        ] {
            assert!(is_restorable(url), "{url}");
        }
        // Подменённый session.json не откроет при запуске скрипт или готовую страницу.
        for url in [
            "javascript:alert(1)",
            "data:text/html,<b>x</b>",
            "about:blank",
            "view-source:https://a.example/",
            "blob:https://a.example/1",
            "ms-settings:",
            "HTTPS://a.example/",
        ] {
            assert!(!is_restorable(url), "{url}");
        }
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
