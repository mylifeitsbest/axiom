//! История посещений: подсказки в адресной строке и плитки новой вкладки.

use std::{cell::RefCell, collections::HashMap};

use serde::{Deserialize, Serialize};
use url::Url;

use super::{Paths, now_secs, read_json, write_json, write_json_compact};
use crate::ipc::{Site, Suggestion};

const FILE: &str = "history.json";
const MAX_ENTRIES: usize = 5000;
/// Сайты, убранные с плиток новой вкладки.
const HIDDEN_FILE: &str = "hidden_sites.json";
const MAX_HIDDEN: usize = 500;

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
    /// Поменялись только заголовки. Сайт с мигающим заголовком («(1) Почта»)
    /// иначе переписывал бы весь файл каждые 30 с; такое сохраняем при выходе
    /// или вместе со следующим настоящим посещением.
    titles_dirty: bool,
    /// Плитки новой вкладки: пересчёт идёт по всей истории, а просят их
    /// при каждом обновлении полосы вкладок.
    top_cache: RefCell<Option<(usize, Vec<Site>)>>,
    /// Домены, убранные с плиток крестиком. История и подсказки их по-прежнему знают.
    hidden: Vec<String>,
}

impl History {
    pub fn load(paths: &Paths) -> Self {
        Self {
            entries: read_json(&paths.root.join(FILE)).unwrap_or_default(),
            hidden: read_json(&paths.root.join(HIDDEN_FILE)).unwrap_or_default(),
            ..Self::default()
        }
    }

    /// Убирает сайт с плиток новой вкладки или возвращает его («Вернуть»).
    /// `url` — адрес плитки; важен только домен. Возвращает, изменилось ли что-то.
    pub fn set_hidden(&mut self, url: &str, hidden: bool) -> bool {
        let Some(host) = host_of(url) else {
            return false;
        };
        let at = self.hidden.iter().position(|h| *h == host);
        match (hidden, at) {
            (true, None) => {
                self.hidden.push(host);
                if self.hidden.len() > MAX_HIDDEN {
                    self.hidden.remove(0);
                }
            }
            (false, Some(i)) => {
                self.hidden.remove(i);
            }
            _ => return false,
        }
        self.top_cache.get_mut().take();
        true
    }

    /// Список убранных сайтов маленький и меняется по клику: пишем сразу.
    pub fn save_hidden(&self, paths: &Paths) -> anyhow::Result<()> {
        write_json(&paths.root.join(HIDDEN_FILE), &self.hidden)
    }

    /// Сохраняет новые посещения (по таймеру).
    pub fn save_if_dirty(&mut self, paths: &Paths) -> anyhow::Result<()> {
        if self.dirty {
            self.write(paths)?;
        }
        Ok(())
    }

    /// Сохраняет всё, включая обновлённые заголовки (при выходе).
    pub fn save_all(&mut self, paths: &Paths) -> anyhow::Result<()> {
        if self.dirty || self.titles_dirty {
            self.write(paths)?;
        }
        Ok(())
    }

    fn write(&mut self, paths: &Paths) -> anyhow::Result<()> {
        // Без отступов: файл до мегабайта переписывается по таймеру, руками его не правят.
        write_json_compact(&paths.root.join(FILE), &self.entries)?;
        self.dirty = false;
        self.titles_dirty = false;
        Ok(())
    }

    fn changed(&mut self) {
        self.dirty = true;
        self.top_cache.get_mut().take();
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
        self.changed();
    }

    /// Вливает импортированную историю. Возвращает число новых адресов.
    pub fn merge(&mut self, incoming: Vec<Entry>) -> usize {
        let mut added = 0;
        // Индекс по адресу: линейный поиск на каждую из тысяч записей Chrome — O(n·m).
        let mut index: HashMap<String, usize> = self
            .entries
            .iter()
            .enumerate()
            .map(|(i, e)| (e.url.clone(), i))
            .collect();
        for entry in incoming {
            match index.get(&entry.url).and_then(|&i| self.entries.get_mut(i)) {
                Some(e) => {
                    e.visits = e.visits.max(entry.visits);
                    e.last = e.last.max(entry.last);
                    if e.title.is_empty() {
                        e.title = entry.title;
                    }
                }
                None => {
                    index.insert(entry.url.clone(), self.entries.len());
                    self.entries.push(entry);
                    added += 1;
                }
            }
        }
        if self.entries.len() > MAX_ENTRIES {
            self.entries.sort_by_key(|e| std::cmp::Reverse(e.last));
            self.entries.truncate(MAX_ENTRIES);
        }
        self.changed();
        added
    }

    /// Обновляет заголовок уже записанной страницы.
    pub fn retitle(&mut self, url: &str, title: &str) {
        if let Some(e) = self.entries.iter_mut().find(|e| e.url == url)
            && e.title != title
        {
            e.title = title.to_string();
            self.titles_dirty = true;
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
        if let Some((cached_limit, sites)) = self.top_cache.borrow().as_ref()
            && *cached_limit == limit
        {
            return sites.clone();
        }
        let sites = self.compute_top_sites(limit);
        *self.top_cache.borrow_mut() = Some((limit, sites.clone()));
        sites
    }

    fn compute_top_sites(&self, limit: usize) -> Vec<Site> {
        let mut by_host: HashMap<String, (u32, String)> = HashMap::new();
        for e in self.entries.iter().filter(|e| !is_search_page(&e.url)) {
            let Ok(url) = Url::parse(&e.url) else {
                continue;
            };
            let Some(host) = url.host_str().map(str::to_lowercase) else {
                continue;
            };
            if self.hidden.contains(&host) {
                continue;
            }
            let slot = by_host
                .entry(host)
                .or_insert_with(|| (0, url.origin().ascii_serialization()));
            slot.0 = slot.0.saturating_add(e.visits);
        }
        let mut sites: Vec<(String, (u32, String))> = by_host.into_iter().collect();
        // При равном числе посещений — по имени: порядок HashMap случайный,
        // и плитки перемешивались бы при каждом обновлении.
        sites.sort_by(|a, b| b.1.0.cmp(&a.1.0).then_with(|| a.0.cmp(&b.0)));
        sites
            .into_iter()
            .take(limit)
            .map(|(host, (_, url))| Site {
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

    #[test]
    fn hidden_sites_leave_tiles_but_not_history() {
        let mut h = History::default();
        h.record("https://github.com/a", "a");
        h.record("https://github.com/b", "b");
        h.record("https://docs.rs/", "docs");
        assert_eq!(h.top_sites(8)[0].url, "https://github.com");

        // Крестик на плитке: домен пропадает с плиток, следующий сайт встаёт на его место.
        assert!(h.set_hidden("https://GitHub.com", true));
        assert!(!h.set_hidden("https://github.com/", true));
        let top = h.top_sites(8);
        assert_eq!(top.len(), 1);
        assert_eq!(top[0].url, "https://docs.rs");
        assert_eq!(h.search("github", 10).len(), 2);

        // «Вернуть».
        assert!(h.set_hidden("https://github.com", false));
        assert_eq!(h.top_sites(8)[0].url, "https://github.com");
        assert!(!h.set_hidden("not a url", true));
    }
}
