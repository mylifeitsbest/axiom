//! «Всегда разрешать» в окошке ссылки на приложение: каким сайтам какие
//! программы можно открывать по клику без вопроса.

use anyhow::Result;
use serde::{Deserialize, Serialize};

use super::{Paths, now_secs, read_json, write_json};

pub const FILE: &str = "app-links.json";
/// Больше разрешений не храним: самые давние вытесняются.
const MAX_ENTRIES: usize = 200;
/// Файл больше этого — не наш (или испорчен): не читаем.
const MAX_FILE: u64 = 256 * 1024;
const MAX_SITE: usize = 300;
const MAX_SCHEME: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// Origin сайта: `https://example.com` (с портом, если он не стандартный).
    pub site: String,
    pub scheme: String,
    /// Когда разрешили (секунды Unix).
    pub at: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AppLinks {
    entries: Vec<Entry>,
}

impl AppLinks {
    pub fn load(paths: &Paths) -> Self {
        let path = paths.root.join(FILE);
        if std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_FILE) {
            return Self::default();
        }
        read_json::<Self>(&path)
            .map(Self::sanitized)
            .unwrap_or_default()
    }

    pub fn save(&self, paths: &Paths) -> Result<()> {
        write_json(&paths.root.join(FILE), self)
    }

    pub fn allows(&self, site: &str, scheme: &str) -> bool {
        self.entries
            .iter()
            .any(|e| e.site == site && e.scheme == scheme)
    }

    pub fn allow(&mut self, site: &str, scheme: &str) {
        if !valid(site, scheme) {
            return;
        }
        self.entries
            .retain(|e| !(e.site == site && e.scheme == scheme));
        self.entries.push(Entry {
            site: site.to_string(),
            scheme: scheme.to_string(),
            at: now_secs(),
        });
        self.trim();
    }

    /// Все «Всегда разрешать» — для Настроек → Безопасность.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// Убирает «Всегда разрешать»: сайт снова спросит перед запуском программы.
    pub fn remove(&mut self, site: &str, scheme: &str) -> bool {
        let before = self.entries.len();
        self.entries
            .retain(|e| !(e.site == site && e.scheme == scheme));
        self.entries.len() != before
    }

    fn sanitized(mut self) -> Self {
        self.entries.retain(|e| valid(&e.site, &e.scheme));
        let mut seen = std::collections::HashSet::new();
        // Повторы — оставляем последнее.
        let mut entries: Vec<Entry> = self
            .entries
            .into_iter()
            .rev()
            .filter(|e| seen.insert((e.site.clone(), e.scheme.clone())))
            .collect();
        entries.reverse();
        self.entries = entries;
        self.trim();
        self
    }

    fn trim(&mut self) {
        if self.entries.len() > MAX_ENTRIES {
            self.entries.sort_by_key(|e| e.at);
            let extra = self.entries.len() - MAX_ENTRIES;
            self.entries.drain(..extra);
        }
    }
}

fn valid(site: &str, scheme: &str) -> bool {
    (site.starts_with("https://") || site.starts_with("http://"))
        && site.len() <= MAX_SITE
        && !scheme.is_empty()
        && scheme.len() <= MAX_SCHEME
        && scheme
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"+.-".contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remembers_and_limits() {
        let mut links = AppLinks::default();
        links.allow("https://example.com", "tg");
        links.allow("https://example.com", "tg");
        assert!(links.allows("https://example.com", "tg"));
        assert!(!links.allows("https://example.com", "mailto"));
        assert!(!links.allows("https://evil.example", "tg"));
        assert_eq!(links.entries.len(), 1);
        // Мусор не сохраняется.
        links.allow("javascript:alert(1)", "tg");
        links.allow("https://example.com", "TG\\..");
        assert_eq!(links.entries.len(), 1);
        for i in 0..MAX_ENTRIES + 20 {
            links.allow(&format!("https://s{i}.example"), "tg");
        }
        assert_eq!(links.entries.len(), MAX_ENTRIES);
    }

    #[test]
    fn round_trip_and_sanitize() {
        let root = std::env::temp_dir().join(format!("axiom-applinks-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&root);
        let file = root.join(FILE);
        let raw = serde_json::json!({ "entries": [
            { "site": "https://a.example", "scheme": "tg", "at": 1 },
            { "site": "https://a.example", "scheme": "tg", "at": 2 },
            { "site": "file:///C:/", "scheme": "tg", "at": 3 },
            { "site": "https://b.example", "scheme": "ms msdt", "at": 4 }
        ]});
        let _ = std::fs::write(&file, raw.to_string());
        let paths = Paths { root: root.clone() };
        let links = AppLinks::load(&paths);
        assert!(links.save(&paths).is_ok());
        let back = AppLinks::load(&paths);
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(links.entries.len(), 1);
        assert_eq!(links.entries[0].at, 2);
        assert_eq!(back.entries, links.entries);
    }
}
