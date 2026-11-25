//! Масштаб по сайтам, как в Chrome: увеличили страницу — так же откроются и другие
//! страницы этого сайта, в других вкладках и после перезапуска. 100 % не храним.

use anyhow::Result;
use serde::{Deserialize, Serialize};

use super::{Paths, now_secs, read_json, write_json};
use crate::tabs::zoom::{MAX, MIN, same};

pub const FILE: &str = "zoom.json";
/// Больше сайтов не храним: самые давние вытесняются.
const MAX_ENTRIES: usize = 500;
/// Файл больше этого — не наш (или испорчен): не читаем.
const MAX_FILE: u64 = 128 * 1024;
/// Самое длинное имя хоста в DNS.
const MAX_HOST: usize = 253;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    /// Хост сайта: `docs.example.com`, `localhost`, `[::1]`.
    pub host: String,
    /// Масштаб: 1.25 — это 125 %.
    pub zoom: f64,
    /// Когда поменяли (секунды Unix).
    pub at: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SiteZoom {
    entries: Vec<Entry>,
}

impl SiteZoom {
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

    /// Масштаб сайта; незнакомый сайт — 100 %.
    pub fn get(&self, host: &str) -> f64 {
        self.entries
            .iter()
            .find(|e| e.host == host)
            .map_or(1.0, |e| e.zoom)
    }

    /// Запоминает масштаб сайта (100 % — забывает). `true` — что-то поменялось.
    pub fn set(&mut self, host: &str, zoom: f64) -> bool {
        if !valid_host(host) || !valid_zoom(zoom) {
            return false;
        }
        if same(self.get(host), zoom) {
            return false;
        }
        self.entries.retain(|e| e.host != host);
        if !same(zoom, 1.0) {
            self.entries.push(Entry {
                host: host.to_string(),
                zoom,
                at: now_secs(),
            });
            self.trim();
        }
        true
    }

    fn sanitized(mut self) -> Self {
        self.entries
            .retain(|e| valid_host(&e.host) && valid_zoom(e.zoom) && !same(e.zoom, 1.0));
        let mut seen = std::collections::HashSet::new();
        // Повторы — оставляем последнее.
        let mut entries: Vec<Entry> = self
            .entries
            .into_iter()
            .rev()
            .filter(|e| seen.insert(e.host.clone()))
            .collect();
        entries.reverse();
        self.entries = entries;
        self.trim();
        self
    }

    fn trim(&mut self) {
        if self.entries.len() > MAX_ENTRIES {
            // Устойчивая сортировка: при равном времени давнее то, что раньше в списке.
            self.entries.sort_by_key(|e| e.at);
            let extra = self.entries.len() - MAX_ENTRIES;
            self.entries.drain(..extra);
        }
    }
}

fn valid_host(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= MAX_HOST
        && host
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b".-_[]:".contains(&b))
}

fn valid_zoom(zoom: f64) -> bool {
    zoom.is_finite() && (MIN..=MAX).contains(&zoom)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remembers_forgets_and_limits() {
        let mut z = SiteZoom::default();
        assert_eq!(z.get("example.com"), 1.0);
        assert!(z.set("example.com", 1.25));
        assert!(!z.set("example.com", 1.25));
        assert_eq!(z.get("example.com"), 1.25);
        assert_eq!(z.get("docs.example.com"), 1.0);
        assert!(z.set("example.com", 1.5));
        assert_eq!(z.entries.len(), 1);
        // 100 % не хранится: сайт просто забывается.
        assert!(z.set("example.com", 1.0));
        assert!(z.entries.is_empty());
        assert!(!z.set("example.com", 1.0));
        // Мусор и масштаб за краями не сохраняются.
        assert!(!z.set("", 1.5));
        assert!(!z.set("Example.com", 1.5));
        assert!(!z.set("a b", 1.5));
        assert!(!z.set("example.com", 9.0));
        assert!(!z.set("example.com", f64::NAN));
        assert!(z.set("[::1]", 0.5));
        for i in 0..MAX_ENTRIES + 20 {
            z.set(&format!("s{i}.example"), 1.1);
        }
        assert_eq!(z.entries.len(), MAX_ENTRIES);
        // Вытесняются самые давние.
        assert_eq!(z.get(&format!("s{}.example", MAX_ENTRIES + 19)), 1.1);
    }

    #[test]
    fn round_trip_and_sanitize() {
        let root = std::env::temp_dir().join(format!("axiom-zoom-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&root);
        let file = root.join(FILE);
        let raw = serde_json::json!({ "entries": [
            { "host": "a.example", "zoom": 1.25, "at": 1 },
            { "host": "a.example", "zoom": 1.5, "at": 2 },
            { "host": "b.example", "zoom": 1.0, "at": 3 },
            { "host": "c.example", "zoom": 40.0, "at": 4 },
            { "host": "<script>", "zoom": 1.5, "at": 5 },
            { "host": "d.example", "zoom": 0.9, "at": 6 }
        ]});
        let _ = std::fs::write(&file, raw.to_string());
        let paths = Paths { root: root.clone() };
        let z = SiteZoom::load(&paths);
        assert!(z.save(&paths).is_ok());
        let back = SiteZoom::load(&paths);
        // Слишком большой файл не читается вовсе.
        let _ = std::fs::write(&file, vec![b' '; MAX_FILE as usize + 1]);
        let huge = SiteZoom::load(&paths);
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(z.get("a.example"), 1.5);
        assert_eq!(z.get("d.example"), 0.9);
        assert_eq!(z.entries.len(), 2);
        assert_eq!(back.entries, z.entries);
        assert!(huge.entries.is_empty());
    }
}
