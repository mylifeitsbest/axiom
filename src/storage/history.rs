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
    /// Прошлые поисковые запросы для подсказок (и свой адрес поиска, по которому
    /// их искали): разбирать каждый адрес истории на каждую букву — дорого.
    queries: RefCell<Option<(String, Vec<Query>)>>,
}

/// Поисковый запрос из истории: все его страницы результатов вместе.
#[derive(Debug, Clone)]
struct Query {
    text: String,
    low: String,
    visits: u32,
    last: u64,
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
        self.queries.get_mut().take();
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

    /// Сколько адресов в истории.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Страница истории (`axiom://history`): свежие посещения сверху. С запросом —
    /// только записи, где все его слова есть в адресе или заголовке, по всей истории.
    pub fn page(&self, query: &str, offset: usize, limit: usize) -> Page<'_> {
        let words = words(query);
        let mut found: Vec<&Entry> = self
            .entries
            .iter()
            .filter(|e| words.is_empty() || matches(e, &words))
            .collect();
        // При равном времени — по адресу: иначе соседние страницы списка могли бы
        // повторить или пропустить запись.
        found.sort_by(|a, b| b.last.cmp(&a.last).then_with(|| a.url.cmp(&b.url)));
        let total = found.len();
        let items: Vec<&Entry> = found.into_iter().skip(offset).take(limit).collect();
        Page {
            more: offset.saturating_add(items.len()) < total,
            items,
            found: total,
        }
    }

    /// Крестик в истории: адрес пропадает из истории, подсказок и плиток.
    pub fn remove(&mut self, url: &str) -> bool {
        let before = self.entries.len();
        self.entries.retain(|e| e.url != url);
        let removed = self.entries.len() != before;
        if removed {
            self.changed();
        }
        removed
    }

    /// Сколько адресов открывали начиная с `since` (Unix-секунды; 0 — вся история).
    pub fn count_since(&self, since: u64) -> usize {
        self.entries.iter().filter(|e| e.last >= since).count()
    }

    /// «Очистить историю»: убирает адреса, открытые начиная с `since` (0 — всё).
    /// Время у записи одно — последнее посещение, поэтому адрес, открытый и раньше,
    /// уходит целиком: после очистки от этого часа не должно остаться следов.
    pub fn remove_since(&mut self, since: u64) -> usize {
        let before = self.entries.len();
        self.entries.retain(|e| e.last < since);
        let removed = before - self.entries.len();
        if removed > 0 {
            self.changed();
        }
        removed
    }

    /// Подсказки: все слова запроса должны встретиться в адресе или заголовке.
    pub fn search(&self, query: &str, limit: usize) -> Vec<Suggestion> {
        let words = words(query);
        if words.is_empty() {
            return Vec::new();
        }
        let now = now_secs();
        let mut hits: Vec<(f64, &Entry)> = self
            .entries
            .iter()
            .filter(|e| !is_search_page(&e.url))
            .filter_map(|e| {
                if !matches(e, &words) {
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

    /// Прошлые поисковые запросы, которые начинаются с набранного (или одно из
    /// их слов): набрали «пр» — «привет», который искали вчера. `custom` — свой
    /// адрес поиска из настроек. Частые и свежие выше, сам набранный текст не входит.
    pub fn searches(&self, typed: &str, custom: &str, limit: usize) -> Vec<String> {
        let typed = words(typed).join(" ");
        if typed.is_empty() || limit == 0 {
            return Vec::new();
        }
        let mut cache = self.queries.borrow_mut();
        if cache.as_ref().is_none_or(|(c, _)| c != custom) {
            *cache = Some((custom.to_string(), self.collect_queries(custom)));
        }
        let Some((_, all)) = cache.as_ref() else {
            return Vec::new();
        };
        let inner = format!(" {typed}");
        let now = now_secs();
        let mut hits: Vec<(f64, &Query)> = all
            .iter()
            .filter(|q| q.low != typed)
            .filter_map(|q| {
                let prefix = q.low.starts_with(&typed);
                if !prefix && !q.low.contains(&inner) {
                    return None;
                }
                let age_days = now.saturating_sub(q.last) as f64 / 86_400.0;
                let score = f64::from(q.visits).ln_1p() * 2.0 + if prefix { 3.0 } else { 0.0 }
                    - age_days.min(90.0) / 15.0;
                Some((score, q))
            })
            .collect();
        hits.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.low.cmp(&b.1.low)));
        hits.into_iter()
            .take(limit)
            .map(|(_, q)| q.text.clone())
            .collect()
    }

    fn collect_queries(&self, custom: &str) -> Vec<Query> {
        let mut by_text: HashMap<String, Query> = HashMap::new();
        for e in &self.entries {
            let Some(raw) = crate::tabs::omnibox::search_query(&e.url, custom) else {
                continue;
            };
            let text = raw.split_whitespace().collect::<Vec<_>>().join(" ");
            // Длинное — скорее вставленный текст, чем запрос, который набирают снова.
            if text.is_empty() || text.chars().count() > 200 {
                continue;
            }
            let low = text.to_lowercase();
            let q = by_text.entry(low.clone()).or_insert_with(|| Query {
                text,
                low,
                visits: 0,
                last: 0,
            });
            q.visits = q.visits.saturating_add(e.visits);
            q.last = q.last.max(e.last);
        }
        by_text.into_values().collect()
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

/// Часть списка для страницы истории.
#[derive(Debug)]
pub struct Page<'a> {
    pub items: Vec<&'a Entry>,
    /// Сколько всего записей подходит под запрос.
    pub found: usize,
    /// Дальше есть ещё.
    pub more: bool,
}

/// Слова запроса в нижнем регистре.
fn words(query: &str) -> Vec<String> {
    query.split_whitespace().map(str::to_lowercase).collect()
}

/// Все слова есть в адресе или заголовке.
fn matches(e: &Entry, words: &[String]) -> bool {
    let hay = format!("{} {}", e.url.to_lowercase(), e.title.to_lowercase());
    words.iter().all(|w| hay.contains(w.as_str()))
}

fn host_of(url: &str) -> Option<String> {
    Url::parse(url).ok()?.host_str().map(str::to_lowercase)
}

/// Страницы результатов поиска не засоряют подсказки и плитки.
fn is_search_page(url: &str) -> bool {
    url.starts_with("https://duckduckgo.com/?")
        || url.starts_with("https://www.google.com/search")
        || url.starts_with("https://google.com/search")
        || crate::tabs::omnibox::search_query(url, "").is_some()
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
        h.record("https://yandex.ru/search/?text=wry", "wry — Яндекс");

        let hits = h.search("wry", 10);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].url, "https://github.com/tauri-apps/wry");
        assert!(h.search("wry github", 10).len() == 1);
        assert!(h.search("   ", 10).is_empty());

        let top = h.top_sites(5);
        assert_eq!(top[0].url, "https://github.com");
    }

    #[test]
    fn past_searches_complete_the_typed_text() {
        let mut h = History::default();
        h.record(
            "https://yandex.ru/search/?text=%D0%BF%D1%80%D0%B8%D0%B2%D0%B5%D1%82",
            "привет — Яндекс",
        );
        h.record(
            "https://duckduckgo.com/?q=%D0%9F%D1%80%D0%B8%D0%B2%D0%B5%D1%82",
            "Привет at DuckDuckGo",
        );
        h.record("https://www.google.com/search?q=%D0%BF%D1%80%D0%BE%D0%B3%D0%BD%D0%BE%D0%B7+%D0%BF%D0%BE%D0%B3%D0%BE%D0%B4%D1%8B", "прогноз погоды");
        h.record("https://www.google.com/search?q=%D1%87%D0%B5%D0%BC+%D0%BF%D1%80%D0%BE%D0%BA%D0%B0%D1%87%D0%B0%D1%82%D1%8C", "чем прокачать");
        h.record(
            "https://www.google.com/search?udm=50&q=%D0%BF%D1%80%D0%BE+AI",
            "AI Mode",
        );
        h.record("https://example.com/%D0%BF%D1%80", "пр — не поиск");
        h.record("https://www.google.com/search?q=%D0%BF%D1%80", "пр");

        let found = h.searches("пр", "", 10);
        // «привет» дважды (Яндекс и DuckDuckGo, разный регистр) — одна подсказка и выше всех.
        assert_eq!(found[0].to_lowercase(), "привет");
        assert_eq!(
            found
                .iter()
                .filter(|q| q.to_lowercase() == "привет")
                .count(),
            1
        );
        assert!(found.contains(&"прогноз погоды".to_string()));
        // Слово внутри запроса тоже подходит, но ниже начала.
        assert_eq!(found.last().map(String::as_str), Some("чем прокачать"));
        // Сам набранный текст, AI Mode и обычные страницы — не подсказки.
        assert_eq!(found.len(), 3, "{found:?}");
        assert_eq!(h.searches("  ПРИ  ", "", 10).len(), 1);
        assert_eq!(h.searches("пр", "", 1).len(), 1);
        assert!(h.searches("", "", 10).is_empty());

        // Удалили из истории — пропало и из подсказок.
        h.remove_since(0);
        assert!(h.searches("пр", "", 10).is_empty());

        // Свой адрес поиска.
        h.record(
            "https://search.example/find/%D0%BF%D1%80%D0%B8%D0%B2%D0%B5%D1%82",
            "x",
        );
        assert!(h.searches("пр", "", 10).is_empty());
        assert_eq!(
            h.searches("пр", "https://search.example/find/%s", 10),
            ["привет"]
        );
    }

    fn with(entries: &[(&str, &str, u64)]) -> History {
        History {
            entries: entries
                .iter()
                .map(|&(url, title, last)| Entry {
                    url: url.into(),
                    title: title.into(),
                    visits: 1,
                    last,
                })
                .collect(),
            ..History::default()
        }
    }

    fn urls(page: &Page<'_>) -> Vec<String> {
        page.items.iter().map(|e| e.url.clone()).collect()
    }

    #[test]
    fn page_is_newest_first_and_pages() {
        let h = with(&[
            ("https://a.example/", "A", 100),
            ("https://c.example/", "C", 300),
            ("https://b.example/", "B", 200),
            ("https://d.example/", "D", 300),
        ]);
        let first = h.page("", 0, 3);
        // Свежие сверху, при равном времени — по адресу.
        assert_eq!(
            urls(&first),
            [
                "https://c.example/",
                "https://d.example/",
                "https://b.example/"
            ]
        );
        assert_eq!(first.found, 4);
        assert!(first.more);
        let rest = h.page("", 3, 3);
        assert_eq!(urls(&rest), ["https://a.example/"]);
        assert!(!rest.more);
        // За концом — пусто, без паники.
        let past = h.page("", 10, 3);
        assert!(past.items.is_empty() && !past.more && past.found == 4);
        assert_eq!(h.len(), 4);
    }

    #[test]
    fn page_searches_whole_history() {
        let mut entries: Vec<(String, String, u64)> = (0..300)
            .map(|i| {
                (
                    format!("https://filler.example/{i}"),
                    format!("Страница {i}"),
                    1000 + i,
                )
            })
            .collect();
        // Старая запись далеко за первой страницей списка.
        entries.push(("https://github.com/tauri-apps/wry".into(), "Wry".into(), 5));
        entries.push(("https://docs.rs/wry/".into(), "Документация WRY".into(), 50));
        let refs: Vec<(&str, &str, u64)> = entries
            .iter()
            .map(|(u, t, l)| (u.as_str(), t.as_str(), *l))
            .collect();
        let h = with(&refs);
        let hits = h.page("wry", 0, 200);
        assert_eq!(
            urls(&hits),
            ["https://docs.rs/wry/", "https://github.com/tauri-apps/wry"]
        );
        assert_eq!(hits.found, 2);
        // Без учёта регистра, по заголовку и адресу, все слова сразу.
        assert_eq!(h.page("ДОКУМЕНТАЦИЯ", 0, 10).found, 1);
        assert_eq!(h.page("wry github", 0, 10).found, 1);
        assert_eq!(h.page("wry gitlab", 0, 10).found, 0);
        assert_eq!(h.page("  ", 0, 10).found, 302);
    }

    #[test]
    fn remove_url_leaves_others() {
        let mut h = with(&[
            ("https://github.com/a", "a", 10),
            ("https://github.com/b", "b", 20),
        ]);
        assert_eq!(h.top_sites(8)[0].url, "https://github.com");
        assert!(h.remove("https://github.com/a"));
        assert!(!h.remove("https://github.com/a"));
        assert_eq!(urls(&h.page("", 0, 10)), ["https://github.com/b"]);
        // Подсказки и плитки читают ту же историю.
        assert!(h.search("github.com/a", 10).is_empty());
        assert!(h.remove("https://github.com/b"));
        assert!(h.top_sites(8).is_empty());
        assert!(h.dirty);
    }

    #[test]
    fn remove_since_clears_range() {
        let mut h = with(&[
            ("https://old.example/", "old", 100),
            ("https://hour.example/", "hour", 5_000),
            ("https://now.example/", "now", 9_000),
        ]);
        assert_eq!(h.count_since(5_000), 2);
        assert_eq!(h.count_since(0), 3);
        assert_eq!(h.count_since(10_000), 0);
        // Ничего не подошло — история не помечается изменённой.
        assert_eq!(h.remove_since(10_000), 0);
        assert!(!h.dirty);
        assert_eq!(h.remove_since(5_000), 2);
        assert_eq!(urls(&h.page("", 0, 10)), ["https://old.example/"]);
        assert!(h.dirty);
        assert_eq!(h.remove_since(0), 1);
        assert_eq!(h.len(), 0);
        assert!(h.top_sites(8).is_empty());
    }

    #[test]
    fn removal_is_saved() {
        let root = std::env::temp_dir().join(format!("axiom-history-rm-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&root);
        let paths = Paths { root: root.clone() };
        let mut h = with(&[
            ("https://a.example/", "a", 1),
            ("https://b.example/", "b", 2),
        ]);
        assert!(h.remove("https://a.example/"));
        assert!(h.save_all(&paths).is_ok());
        let loaded = History::load(&paths);
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(urls(&loaded.page("", 0, 10)), ["https://b.example/"]);
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
