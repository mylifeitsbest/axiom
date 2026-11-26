//! Модель вкладок. Вкладка без webview — «спящая» или новая: память не занимает.

pub mod external;
pub mod fullscreen;
pub mod identity;
pub mod links;
pub mod media;
pub mod mini;
pub mod omnibox;
pub mod popups;
pub mod script;
pub mod sleep;
pub mod snapshot;
pub mod tools;
pub mod tracking;
pub mod volume;
pub mod waking;
pub mod webview;
pub mod zoom;

use std::time::Instant;

use wry::WebView;

pub type TabId = u64;

const MAX_CLOSED: usize = 25;

/// Внутренний адрес вкладки настроек. Своего webview у неё нет: настройкам нужен
/// IPC, а у вкладок его нет. Пока она активна, настройки показывает интерфейс.
pub const SETTINGS: &str = "axiom://settings";
pub const SETTINGS_TITLE: &str = "Настройки";

pub struct Tab {
    pub id: TabId,
    pub title: String,
    pub url: String,
    pub loading: bool,
    /// Вкладка открыта из панели ИИ и ходит через её прокси.
    pub ai: bool,
    pub webview: Option<WebView>,
    pub last_active: Instant,
    /// Позиция прокрутки, сохранённая при усыплении.
    pub scroll: Option<f64>,
    /// Когда запрошено усыпление: ждём от страницы позицию прокрутки.
    pub sleep_requested: Option<Instant>,
    /// В странице недописанный текст: не усыпляем, пока вкладку снова не откроют.
    pub keep_alive: bool,
    /// Человек попросил эту вкладку не усыплять (кнопка в адресной строке).
    pub no_sleep: bool,
    /// Вкладка, с которой сайт открыл эту (`target=_blank`, `window.open`).
    pub opener: Option<TabId>,
    /// Запрос, с которым двойной поиск открыл для этой вкладки панель ИИ.
    pub ai_query: Option<String>,
    /// Страницу по адресу `url` уже открывали (в этом или прошлом запуске):
    /// её пробуждение показывает заглушку, а не пустой фон.
    pub loaded: bool,
    /// Вкладка просыпается: webview уже грузит страницу, но его не видно, пока
    /// в ней не появится что показать. На его месте заглушка «Просыпается…».
    pub waking: Option<Instant>,
    /// Звук вкладки выключен. Переживает усыпление: новый webview получает его снова.
    pub muted: bool,
    /// Страница сейчас играет звук (по событию движка).
    pub audible: bool,
    /// Громкость вкладки, 0–100 % (см. `volume`). Переживает усыпление, перезапуск
    /// и Ctrl+Shift+T; новая страница во вкладке получает её снова.
    pub volume: u8,
    /// Масштаб страницы (1.0 — 100 %). Новая страница во вкладке получает масштаб
    /// своего сайта (см. `storage::zoom`), адрес без сайта — оставляет этот.
    pub zoom: f64,
}

impl Tab {
    pub fn is_newtab(&self) -> bool {
        self.url == omnibox::NEWTAB
    }

    pub fn is_settings(&self) -> bool {
        self.url == SETTINGS
    }

    /// Служебная вкладка без страницы и webview: новая вкладка или настройки.
    pub fn is_internal(&self) -> bool {
        self.is_newtab() || self.is_settings()
    }

    pub fn is_sleeping(&self) -> bool {
        self.webview.is_none() && !self.is_internal()
    }

    pub fn is_waking(&self) -> bool {
        self.waking.is_some() && self.webview.is_some()
    }

    /// Webview активной вкладки можно показывать. Просыпающаяся страница на Linux
    /// спрятана до загрузки, а на Windows лежит под окном интерфейса
    /// (см. `webview::WAKES_BEHIND`).
    pub fn page_visible(&self) -> bool {
        webview::WAKES_BEHIND || !self.is_waking()
    }

    /// Звук движка выключен: кнопкой или громкостью 0 % (см. `volume::ZERO_IS_MUTE`).
    pub fn engine_muted(&self) -> bool {
        volume::engine_muted(self.muted, self.volume)
    }

    /// Вкладку не усыпляют: попросили для неё самой или её сайт в списке `sites`
    /// (`settings.no_sleep_sites`).
    pub fn kept_awake(&self, sites: &[String]) -> bool {
        self.no_sleep || sleep::site_kept(sites, &self.url)
    }

    /// Флаги самой вкладки и список сайтов «не усыплять» не мешают её усыпить.
    /// Активность, звук и загрузки проверяет `App::sleep_candidates`: им нужен webview.
    pub fn may_sleep(&self, sites: &[String]) -> bool {
        !self.kept_awake(sites) && !self.keep_alive && self.sleep_requested.is_none()
    }
}

#[derive(Debug, Clone)]
pub struct ClosedTab {
    pub url: String,
    pub title: String,
    pub ai: bool,
    /// Ctrl+Shift+T возвращает вкладку вместе с «не усыплять» и громкостью.
    pub no_sleep: bool,
    pub volume: u8,
    pub index: usize,
}

#[derive(Default)]
pub struct TabStore {
    tabs: Vec<Tab>,
    pub active: Option<TabId>,
    next_id: TabId,
    closed: Vec<ClosedTab>,
}

impl TabStore {
    pub fn iter(&self) -> impl Iterator<Item = &Tab> {
        self.tabs.iter()
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut Tab> {
        self.tabs.iter_mut()
    }

    pub fn len(&self) -> usize {
        self.tabs.len()
    }

    pub fn get(&self, id: TabId) -> Option<&Tab> {
        self.tabs.iter().find(|t| t.id == id)
    }

    pub fn get_mut(&mut self, id: TabId) -> Option<&mut Tab> {
        self.tabs.iter_mut().find(|t| t.id == id)
    }

    pub fn active_tab(&self) -> Option<&Tab> {
        self.active.and_then(|id| self.get(id))
    }

    pub fn index_of(&self, id: TabId) -> Option<usize> {
        self.tabs.iter().position(|t| t.id == id)
    }

    /// Вкладка настроек, если она открыта (она всегда одна).
    pub fn settings(&self) -> Option<TabId> {
        self.tabs.iter().find(|t| t.is_settings()).map(|t| t.id)
    }

    /// Добавляет вкладку без webview. `at = None` — после активной.
    pub fn add(&mut self, url: String, title: String, ai: bool, at: Option<usize>) -> TabId {
        self.next_id += 1;
        let id = self.next_id;
        let index = at
            .or_else(|| self.active.and_then(|a| self.index_of(a)).map(|i| i + 1))
            .unwrap_or(self.tabs.len())
            .min(self.tabs.len());
        self.tabs.insert(
            index,
            Tab {
                id,
                title,
                url,
                loading: false,
                ai,
                webview: None,
                last_active: Instant::now(),
                scroll: None,
                sleep_requested: None,
                keep_alive: false,
                no_sleep: false,
                opener: None,
                ai_query: None,
                loaded: false,
                waking: None,
                muted: false,
                audible: false,
                volume: volume::FULL,
                zoom: 1.0,
            },
        );
        id
    }

    /// Удаляет вкладку (webview уничтожается вместе с ней). `remember` — запомнить
    /// для Ctrl+Shift+T (вкладку, открытую ради загрузки, возвращать незачем).
    pub fn remove(&mut self, id: TabId, remember: bool) -> Option<Tab> {
        let index = self.index_of(id)?;
        let tab = self.tabs.remove(index);
        if remember && !tab.is_internal() {
            self.closed.push(ClosedTab {
                url: tab.url.clone(),
                title: tab.title.clone(),
                ai: tab.ai,
                no_sleep: tab.no_sleep,
                volume: tab.volume,
                index,
            });
            if self.closed.len() > MAX_CLOSED {
                self.closed.remove(0);
            }
        }
        if self.active == Some(id) {
            self.active = None;
        }
        Some(tab)
    }

    pub fn pop_closed(&mut self) -> Option<ClosedTab> {
        self.closed.pop()
    }

    pub fn can_restore(&self) -> bool {
        !self.closed.is_empty()
    }

    pub fn move_to(&mut self, id: TabId, to: usize) {
        if let Some(from) = self.index_of(id) {
            let tab = self.tabs.remove(from);
            let to = to.min(self.tabs.len());
            self.tabs.insert(to, tab);
        }
    }

    /// Соседняя вкладка по кругу: `step = 1` — следующая, `-1` — предыдущая.
    pub fn neighbor(&self, id: TabId, step: isize) -> Option<TabId> {
        let len = self.tabs.len() as isize;
        if len == 0 {
            return None;
        }
        let i = self.index_of(id)? as isize;
        let j = (i + step).rem_euclid(len) as usize;
        self.tabs.get(j).map(|t| t.id)
    }

    /// Ctrl+1..8 — по номеру, Ctrl+9 — последняя.
    pub fn nth(&self, n: u8) -> Option<TabId> {
        if n >= 9 {
            return self.tabs.last().map(|t| t.id);
        }
        self.tabs
            .get(usize::from(n.saturating_sub(1)))
            .map(|t| t.id)
    }

    /// Куда поставить вкладку, открытую со страницы `opener`: сразу за ней и за теми,
    /// что она уже открыла, — несколько ссылок в фоне встают по порядку, как в Chrome.
    pub fn child_index(&self, opener: TabId) -> Option<usize> {
        let start = self.index_of(opener)? + 1;
        let children = self
            .tabs
            .iter()
            .skip(start)
            .take_while(|t| t.opener == Some(opener))
            .count();
        Some(start + children)
    }

    /// Какую вкладку активировать после закрытия вкладки с индексом `index`.
    pub fn successor_for(&self, index: usize) -> Option<TabId> {
        self.tabs
            .get(index)
            .or_else(|| self.tabs.get(index.saturating_sub(1)))
            .map(|t| t.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(n: usize) -> TabStore {
        let mut s = TabStore::default();
        for i in 0..n {
            let id = s.add(
                format!("https://t{i}.example/"),
                format!("t{i}"),
                false,
                None,
            );
            s.active = Some(id);
        }
        s
    }

    #[test]
    fn order_and_neighbors() {
        let mut s = store(3);
        let ids: Vec<TabId> = s.iter().map(|t| t.id).collect();
        assert_eq!(s.neighbor(ids[2], 1), Some(ids[0]));
        assert_eq!(s.neighbor(ids[0], -1), Some(ids[2]));
        assert_eq!(s.nth(9), Some(ids[2]));
        s.move_to(ids[2], 0);
        assert_eq!(s.iter().next().map(|t| t.id), Some(ids[2]));
    }

    #[test]
    fn close_and_restore() {
        let mut s = store(2);
        let first = s.iter().next().map(|t| t.id).unwrap_or_default();
        assert!(s.remove(first, true).is_some());
        assert_eq!(s.len(), 1);
        let closed = s.pop_closed();
        assert_eq!(closed.map(|c| c.index), Some(0));
        assert!(!s.can_restore());
    }

    #[test]
    fn settings_tab_is_internal() {
        let mut s = store(1);
        let id = s.add(SETTINGS.into(), SETTINGS_TITLE.into(), false, None);
        assert_eq!(s.settings(), Some(id));
        let tab = s.get(id);
        assert!(tab.is_some_and(|t| t.is_internal() && !t.is_sleeping()));
        // Закрытые настройки не возвращаются по Ctrl+Shift+T как обычная страница.
        assert!(s.remove(id, true).is_some());
        assert!(!s.can_restore());
        assert_eq!(s.settings(), None);
    }

    #[test]
    fn children_open_in_order() {
        let mut s = store(3);
        let ids: Vec<TabId> = s.iter().map(|t| t.id).collect();
        let opener = ids[0];
        assert_eq!(s.child_index(opener), Some(1));
        let a = s.add(
            "https://a/".into(),
            String::new(),
            false,
            s.child_index(opener),
        );
        if let Some(t) = s.get_mut(a) {
            t.opener = Some(opener);
        }
        let b = s.add(
            "https://b/".into(),
            String::new(),
            false,
            s.child_index(opener),
        );
        if let Some(t) = s.get_mut(b) {
            t.opener = Some(opener);
        }
        let order: Vec<TabId> = s.iter().map(|t| t.id).collect();
        assert_eq!(order, vec![opener, a, b, ids[1], ids[2]]);
        // У последней вкладки детей нет — новая встаёт в конец.
        assert_eq!(s.child_index(ids[2]), Some(5));
        assert_eq!(s.child_index(999), None);
    }

    #[test]
    fn closed_tab_remembers_no_sleep() {
        let mut s = store(2);
        let first = s.iter().next().map(|t| t.id).unwrap_or_default();
        if let Some(tab) = s.get_mut(first) {
            tab.no_sleep = true;
        }
        assert!(s.remove(first, true).is_some());
        assert_eq!(s.pop_closed().map(|c| c.no_sleep), Some(true));
    }

    #[test]
    fn closed_tab_remembers_volume() {
        let mut s = store(2);
        let ids: Vec<TabId> = s.iter().map(|t| t.id).collect();
        // Новая вкладка — на полной громкости.
        assert!(s.iter().all(|t| t.volume == volume::FULL));
        if let Some(tab) = s.get_mut(ids[1]) {
            tab.volume = 35;
        }
        assert!(s.remove(ids[1], true).is_some());
        assert_eq!(s.pop_closed().map(|c| c.volume), Some(35));
    }
}
