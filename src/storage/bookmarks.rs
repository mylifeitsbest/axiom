//! Закладки: звёздочка в адресной строке (Ctrl+D), подсказки, импорт из Chrome.

use serde::{Deserialize, Serialize};

use super::{Paths, now_secs, read_json, write_json};
use crate::ipc::Suggestion;

const FILE: &str = "bookmarks.json";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Bookmark {
    pub url: String,
    pub title: String,
    #[serde(default)]
    pub folder: String,
    #[serde(default)]
    pub added: u64,
}

#[derive(Debug, Default)]
pub struct Bookmarks {
    items: Vec<Bookmark>,
}

impl Bookmarks {
    pub fn load(paths: &Paths) -> Self {
        Self {
            items: read_json(&paths.root.join(FILE)).unwrap_or_default(),
        }
    }

    pub fn save(&self, paths: &Paths) -> anyhow::Result<()> {
        write_json(&paths.root.join(FILE), &self.items)
    }

    pub fn contains(&self, url: &str) -> bool {
        self.items.iter().any(|b| b.url == url)
    }

    /// Добавляет или убирает закладку. Возвращает новое состояние.
    pub fn toggle(&mut self, url: &str, title: &str) -> bool {
        if let Some(i) = self.items.iter().position(|b| b.url == url) {
            self.items.remove(i);
            false
        } else {
            self.items.push(Bookmark {
                url: url.to_string(),
                title: title.to_string(),
                folder: String::new(),
                added: now_secs(),
            });
            true
        }
    }

    /// Добавляет закладки, которых ещё нет. Возвращает число новых.
    pub fn merge(&mut self, incoming: Vec<Bookmark>) -> usize {
        let before = self.items.len();
        for b in incoming {
            if !self.contains(&b.url) {
                self.items.push(b);
            }
        }
        self.items.len() - before
    }

    pub fn search(&self, query: &str, limit: usize) -> Vec<Suggestion> {
        let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        if words.is_empty() {
            return Vec::new();
        }
        self.items
            .iter()
            .filter(|b| {
                let hay = format!("{} {} {}", b.url, b.title, b.folder).to_lowercase();
                words.iter().all(|w| hay.contains(w.as_str()))
            })
            .take(limit)
            .map(|b| Suggestion {
                title: b.title.clone(),
                url: b.url.clone(),
                kind: "bookmark",
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggle_merge_search() {
        let mut b = Bookmarks::default();
        assert!(b.toggle("https://a.example/", "Alpha"));
        assert!(b.contains("https://a.example/"));
        let added = b.merge(vec![
            Bookmark {
                url: "https://a.example/".into(),
                title: "dup".into(),
                folder: String::new(),
                added: 0,
            },
            Bookmark {
                url: "https://b.example/".into(),
                title: "Beta docs".into(),
                folder: "Rust".into(),
                added: 0,
            },
        ]);
        assert_eq!(added, 1);
        assert_eq!(b.search("rust beta", 5).len(), 1);
        assert!(!b.toggle("https://a.example/", ""));
    }
}
