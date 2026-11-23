//! История посещений: подсказки в адресной строке и плитки новой вкладки.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use url::Url;

use super::{Paths, now_secs, read_json, write_json};
use crate::ipc::{Site, Suggestion};

const FILE: &str = "history.json";
const MAX_ENTRIES: usize = 5000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub url: String,
    pub title: String,
    pub visits: u32,
    pub last: u64,
}

#[derive(Debug, Default)]
pub struct History {
    entries: Vec<Entry>,
    dirty: bool,
}

impl History {
    pub fn load(paths: &Paths) -> Self {
        Self {
            entries: read_json(&paths.root.join(FILE)).unwrap_or_default(),
            dirty: false,
        }
    }

    pub fn save_if_dirty(&mut self, paths: &Paths) -> anyhow::Result<()> {
        if self.dirty {
            write_json(&paths.root.join(FILE), &self.entries)?;
            self.dirty = false;
        }
        Ok(())
    }

    /// Учитывает посещение. Записываются только http(s).
    pub fn record(&mut self, url: &str, title: &str) {
        if !(url.starts_with("https://") || url.starts_with("http://")) {
            return;
        }
        let now = now_secs();
        if let Some(e) = self.entries.iter_mut().find(|e| e.url == url) {
            e.visits = e.visits.saturating_add(1);
            e.last = now;
            if !title.is_empty() {
                e.title = title.to_string();
            }
        } else {
            self.entries.push(Entry {
                url: url.to_string(),
                title: title.to_string(),
                visits: 1,
                last: now,
            });
            if self.entries.len() > MAX_ENTRIES {
                self.entries.sort_by_key(|e| std::cmp::Reverse(e.last));
                self.entries.truncate(MAX_ENTRIES);
            }
        }
        self.dirty = true;
    }

    /// Вливает импортированную историю. Возвращает число новых адресов.
    pub fn merge(&mut self, incoming: Vec<Entry>) -> usize {
        let mut added = 0;
        for entry in incoming {
            match self.entries.iter_mut().find(|e| e.url == entry.url) {
                Some(e) => {
                    e.visits = e.visits.max(entry.visits);
                    e.last = e.last.max(entry.last);
                    if e.title.is_empty() {
                        e.title = entry.title;
                    }
                }
                None => {
                    self.entries.push(entry);
                    added += 1;
                }
            }
        }
        if self.entries.len() > MAX_ENTRIES {
            self.entries.sort_by_key(|e| std::cmp::Reverse(e.last));
            self.entries.truncate(MAX_ENTRIES);
        }
        self.dirty = true;
        added
    }

    /// Обновляет заголовок уже записанной страницы.
    pub fn retitle(&mut self, url: &str, title: &str) {
        if let Some(e) = self.entries.iter_mut().find(|e| e.url == url)
            && e.title != title
        {
            e.title = title.to_string();
            self.dirty = true;
        }
    }

    /// Подсказки: все слова запроса должны встретиться в адресе или заголовке.
    pub fn search(&self, query: &str, limit: usize) -> Vec<Suggestion> {
        let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        if words.is_empty() {
            return Vec::new();
        }
        let now = now_secs();
        let mut hits: Vec<(f64, &Entry)> = self
            .entries
            .iter()
            .filter(|e| !is_search_page(&e.url))
            .filter_map(|e| {
                let hay = format!("{} {}", e.url.to_lowercase(), e.title.to_lowercase());
                if !words.iter().all(|w| hay.contains(w.as_str())) {
                    return None;
                }
                let host_hit = host_of(&e.url)
                    .map(|h| {
                        h.starts_with(words[0].as_str()) || h.contains(&format!(".{}", words[0]))
                    })
                    .unwrap_or(false);
                let age_days = now.saturating_sub(e.last) as f64 / 86_400.0;
                let score = f64::from(e.visits).ln_1p() * 2.0 + if host_hit { 4.0 } else { 0.0 }
                    - age_days.min(90.0) / 30.0;
                Some((score, e))
            })
            .collect();
        hits.sort_by(|a, b| b.0.total_cmp(&a.0));
        hits.into_iter()
            .take(limit)
            .map(|(_, e)| Suggestion {
                title: e.title.clone(),
                url: e.url.clone(),
                kind: "history",
            })
            .collect()
    }

    /// Самые посещаемые сайты (по домену) для новой вкладки.
    pub fn top_sites(&self, limit: usize) -> Vec<Site> {
        let mut by_host: HashMap<String, (u32, String, String)> = HashMap::new();
        for e in self.entries.iter().filter(|e| !is_search_page(&e.url)) {
            let Some(host) = host_of(&e.url) else {
                continue;
            };
            let origin = match Url::parse(&e.url) {
                Ok(u) => u.origin().ascii_serialization(),
                Err(_) => continue,
            };
            let slot = by_host.entry(host).or_insert((0, String::new(), origin));
            slot.0 = slot.0.saturating_add(e.visits);
            if slot.1.is_empty() {
                slot.1 = e.title.clone();
            }
        }
        let mut sites: Vec<(String, (u32, String, String))> = by_host.into_iter().collect();
        sites.sort_by_key(|s| std::cmp::Reverse(s.1.0));
        sites
            .into_iter()
            .take(limit)
            .map(|(host, (_, _title, url))| Site {
                title: host.trim_start_matches("www.").to_string(),
                url,
            })
            .collect()
    }
}

fn host_of(url: &str) -> Option<String> {
    Url::parse(url).ok()?.host_str().map(str::to_lowercase)
}

/// Страницы результатов поиска не засоряют подсказки и плитки.
fn is_search_page(url: &str) -> bool {
    url.starts_with("https://duckduckgo.com/?")
        || url.starts_with("https://www.google.com/search")
        || url.starts_with("https://google.com/search")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_and_searches() {
        let mut h = History::default();
        h.record("https://docs.rs/wry/latest/wry/", "wry - Rust");
        h.record("https://github.com/tauri-apps/wry", "tauri-apps/wry");
        h.record("https://github.com/tauri-apps/wry", "tauri-apps/wry");
        h.record("axiom://newtab", "skip me");
        h.record("https://duckduckgo.com/?q=wry", "wry at DuckDuckGo");

        let hits = h.search("wry", 10);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].url, "https://github.com/tauri-apps/wry");
        assert!(h.search("wry github", 10).len() == 1);
        assert!(h.search("   ", 10).is_empty());

        let top = h.top_sites(5);
        assert_eq!(top[0].url, "https://github.com");
    }
}
