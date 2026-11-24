//! Действия с вкладкой: меню вкладки, звук и громкость, исходный код, печать, масштаб,
//! инструменты разработчика и ссылки, открытые в фоне.

use super::{App, display_title};
use crate::{
    events::UserEvent,
    ipc::ToUi,
    tabs::{
        Tab, TabId, links, omnibox, script,
        tools::{self, Zoom},
        volume,
    },
    ui::hotkeys::Hotkey,
};

impl App {
    /// Сочетания для страницы активной вкладки (Ctrl+U, Ctrl+P, Ctrl+M, F12, масштаб…).
    pub(super) fn page_hotkey(&mut self, hotkey: Hotkey) {
        let Some(id) = self.tabs.active else {
            return;
        };
        match hotkey {
            Hotkey::ViewSource => return self.view_source(id),
            Hotkey::Mute => return self.toggle_mute(id),
            Hotkey::HardReload => return self.reload_tab_with(id, true),
            _ => {}
        }
        let Some(webview) = self.tabs.get(id).and_then(|t| t.webview.as_ref()) else {
            return;
        };
        match hotkey {
            Hotkey::DevTools => webview.open_devtools(),
            Hotkey::Print => {
                // Окно печати — внутри страницы: Esc и Enter должны попасть туда.
                let _ = webview.focus();
                tools::print(webview);
            }
            Hotkey::ZoomIn => tools::zoom(webview, Zoom::In),
            Hotkey::ZoomOut => tools::zoom(webview, Zoom::Out),
            Hotkey::ZoomReset => tools::zoom(webview, Zoom::Reset),
            _ => {}
        }
    }

    // ---------- меню вкладки ----------

    pub(super) fn reload_tab(&mut self, id: TabId) {
        self.reload_tab_with(id, false);
    }

    /// Перезагрузка; `hard` — без кэша. Спящая вкладка просто просыпается.
    fn reload_tab_with(&mut self, id: TabId, hard: bool) {
        let Some(tab) = self.tabs.get(id) else {
            return;
        };
        match &tab.webview {
            Some(webview) if hard => tools::reload_ignoring_cache(webview),
            Some(webview) => {
                let _ = webview.reload();
            }
            None if !tab.is_newtab() => {
                if let Err(err) = self.wake(id) {
                    eprintln!("не удалось разбудить вкладку: {err:#}");
                }
                // Её только что попросили обновить: по лимиту живых уснёт не она,
                // а та, что открывали давнее всех.
                if let Some(tab) = self.tabs.get_mut(id) {
                    tab.last_active = std::time::Instant::now();
                }
                self.limit_live_tabs();
                self.push_state();
            }
            None => {}
        }
    }

    /// Копия вкладки сразу за ней, как в Chrome.
    pub(super) fn duplicate_tab(&mut self, id: TabId) {
        let Some(tab) = self.tabs.get(id) else {
            return;
        };
        let (url, title, ai, volume) = (tab.url.clone(), tab.title.clone(), tab.ai, tab.volume);
        let at = self.tabs.index_of(id).map(|i| i + 1);
        let copy = self.tabs.add(url, title, ai, at);
        // Копия той же страницы звучит так же.
        if let Some(tab) = self.tabs.get_mut(copy) {
            tab.volume = volume;
        }
        self.activate(copy);
    }

    /// Значок динамика на вкладке, Ctrl+M и пункт меню.
    pub(super) fn toggle_mute(&mut self, id: TabId) {
        let Some(tab) = self.tabs.get_mut(id).filter(|t| !t.is_newtab()) else {
            return;
        };
        tab.muted = !tab.muted;
        if let Some(webview) = &tab.webview {
            tools::set_muted(webview, tab.engine_muted());
        }
        self.push_state();
    }

    /// Ползунок в меню вкладки и колесо над значком динамика. Звук не включает и не
    /// выключает: это по-прежнему клик по значку и Ctrl+M.
    pub(super) fn set_volume(&mut self, id: TabId, volume: u8) {
        let Some(tab) = self.tabs.get_mut(id).filter(|t| !t.is_internal()) else {
            return;
        };
        let volume = volume::clamp(volume);
        if tab.volume == volume {
            return;
        }
        let was_muted = tab.engine_muted();
        tab.volume = volume;
        if let Some(webview) = &tab.webview {
            script::run(webview, &volume::apply(volume));
            // Через ноль: сначала скрипт, потом звук движка (см. `volume::ZERO_IS_MUTE`).
            if tab.engine_muted() != was_muted {
                tools::set_muted(webview, tab.engine_muted());
            }
        }
        self.session_dirty = true;
        self.push_state();
    }

    /// Громкость вкладки — её странице. Новая страница начинает с полной, поэтому
    /// полную не шлём. Без жеста пользователя (см. `tabs::script`).
    pub(super) fn apply_volume(tab: &Tab) {
        if volume::page_volume(tab.volume) == volume::FULL {
            return;
        }
        if let Some(webview) = &tab.webview {
            script::run(webview, &volume::apply(tab.volume));
        }
    }

    /// Закрывает все вкладки, кроме этой; она становится активной.
    /// Справа налево: тогда Ctrl+Shift+T возвращает их по одной на свои места.
    pub(super) fn close_other_tabs(&mut self, id: TabId) {
        if self.tabs.get(id).is_none() {
            return;
        }
        if self.tabs.active != Some(id) {
            self.activate(id);
        }
        let others: Vec<TabId> = self
            .tabs
            .iter()
            .map(|t| t.id)
            .filter(|&t| t != id)
            .collect();
        for other in others.into_iter().rev() {
            self.close_tab(other);
        }
    }

    /// Закрывает вкладки правее этой. Если активная была среди них — активной станет эта.
    pub(super) fn close_tabs_right(&mut self, id: TabId) {
        let Some(index) = self.tabs.index_of(id) else {
            return;
        };
        let right: Vec<TabId> = self.tabs.iter().skip(index + 1).map(|t| t.id).collect();
        if self.tabs.active.is_some_and(|a| right.contains(&a)) {
            self.activate(id);
        }
        for other in right.into_iter().rev() {
            self.close_tab(other);
        }
    }

    // ---------- исходный код ----------

    /// Ctrl+U: исходный код страницы в новой вкладке рядом.
    fn view_source(&mut self, id: TabId) {
        let Some(tab) = self.tabs.get(id) else {
            return;
        };
        let Some(url) = tools::source_url(&tab.url) else {
            return;
        };
        if !tools::CAN_VIEW_SOURCE {
            self.send(&ToUi::Toast {
                text: "Исходный код страницы пока открывается только в Windows",
            });
            return;
        }
        let ai = tab.ai;
        let at = self.tabs.index_of(id).map(|i| i + 1);
        let source = self.tabs.add(url.clone(), display_title(&url), ai, at);
        self.activate(source);
    }

    // ---------- звук ----------

    pub(super) fn on_tab_audio(&mut self, id: TabId, playing: bool) {
        // Событие от уже выгруженной или припаркованной страницы.
        let Some(tab) = self.tabs.get_mut(id).filter(|t| t.webview.is_some()) else {
            return;
        };
        if tab.audible != playing {
            tab.audible = playing;
            self.push_state();
        }
    }

    // ---------- ссылки в новой вкладке ----------

    /// Сайт просит новое окно. Сначала спрашиваем страницу, не средний ли это клик
    /// или Ctrl+клик (см. `tabs::links`): тогда вкладка откроется в фоне.
    pub(super) fn open_from_page(&mut self, url: String, ai: bool, from: Option<TabId>) {
        if !omnibox::is_navigable(&url) {
            return;
        }
        let source = from
            .and_then(|id| self.tabs.get(id))
            .and_then(|t| t.webview.as_ref());
        let Some(webview) = source else {
            self.add_page_tab(url, ai, from, false);
            return;
        };
        let events = self.events.clone();
        let target = url.clone();
        // Без жеста (см. `tabs::script`): иначе этот вопрос сам давал бы странице
        // «клик», и после одного настоящего клика она открывала бы окна цепочкой.
        let asked = script::ask(webview, &links::probe(&url), move |raw| {
            let _ = events.send_event(UserEvent::PageTab {
                url: target.clone(),
                ai,
                from,
                background: links::answer(&raw),
            });
        });
        if !asked {
            self.add_page_tab(url, ai, from, false);
        }
    }

    /// Новая вкладка со страницы: сразу за вкладкой-источником и её прежними «детьми».
    /// В фоне страница грузится сразу, как в Chrome; лишние фоновые уснут по лимиту.
    pub(super) fn add_page_tab(
        &mut self,
        url: String,
        ai: bool,
        from: Option<TabId>,
        background: bool,
    ) {
        if !omnibox::is_navigable(&url) {
            return;
        }
        let at = from.and_then(|f| self.tabs.child_index(f));
        let id = self.tabs.add(url.clone(), display_title(&url), ai, at);
        if let Some(tab) = self.tabs.get_mut(id) {
            tab.opener = from;
        }
        if !background || self.tabs.active.is_none() {
            self.activate(id);
            return;
        }
        if let Err(err) = self.wake(id) {
            eprintln!("не удалось открыть вкладку в фоне: {err:#}");
        }
        self.limit_live_tabs();
        self.session_dirty = true;
        self.push_state();
    }
}
