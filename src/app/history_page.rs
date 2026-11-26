//! Вкладка истории (`axiom://history`). Своего webview у неё нет: список рисует
//! интерфейс (`ui/history.js`), а записи по страницам, поиск по всей истории
//! и удаление — здесь. Подсказки адресной строки и плитки новой вкладки читают
//! ту же историю, поэтому удалённое пропадает и из них.

use anyhow::Result;

use super::App;
use crate::{
    ipc::ToUi,
    tabs::{HISTORY, HISTORY_TITLE, TabId},
};

impl App {
    /// Ctrl+H и ссылка «История»: вкладка истории одна — есть, так переходим на неё,
    /// нет — открываем рядом с активной (закроют — вернёмся туда же).
    pub(super) fn open_history(&mut self) {
        if let Some(id) = self.tabs.history() {
            self.activate(id);
            return;
        }
        let from = self.tabs.active;
        let id = self
            .tabs
            .add(HISTORY.into(), HISTORY_TITLE.into(), false, None);
        if let Some(tab) = self.tabs.get_mut(id) {
            tab.opener = from;
        }
        self.activate(id);
    }

    /// Активная вкладка — история.
    fn history_tab(&self) -> Option<TabId> {
        self.tabs
            .active
            .filter(|&id| self.tabs.history() == Some(id))
    }

    /// Часть списка: интерфейс подгружает её по мере прокрутки, а не всю историю сразу.
    pub(super) fn send_history(&self, q: &str, offset: usize, limit: usize) {
        let page = self.history.page(q, offset, limit);
        self.send(&ToUi::History {
            q,
            offset,
            items: page.items,
            found: page.found,
            more: page.more,
            total: self.history.len(),
        });
    }

    /// Клик по записи открывает её в этой же вкладке (история становится обычной
    /// вкладкой, как в Chrome), Ctrl+клик и средняя кнопка — в фоне, сразу за ней.
    pub(super) fn history_open(&mut self, url: String, background: bool) -> Result<()> {
        // В историю пишутся только http(s): другое интерфейс прислать не должен.
        if !is_web(&url) {
            return Ok(());
        }
        let Some(id) = self.history_tab() else {
            return Ok(());
        };
        // Адрес мог попасть в историю с метками слежки (после перенаправления сайта).
        let url = self.untracked(url);
        if background {
            self.add_page_tab(url, false, Some(id), true);
            return Ok(());
        }
        self.load_in_tab(id, url)
    }

    /// Крестик у записи: на диск сразу, чтобы удалённое не вернулось после сбоя.
    pub(super) fn history_delete(&mut self, url: &str) {
        if self.history.remove(url) {
            self.save_history();
        }
    }

    /// «Очистить историю» за час, за сегодня (`since` — полночь по часам человека,
    /// её считает интерфейс) или за всё время (`since = 0`).
    pub(super) fn history_clear(&mut self, since: u64) {
        if self.history.remove_since(since) > 0 {
            self.save_history();
        }
        clear_engine_history(&self.ui, since);
    }

    pub(super) fn history_count(&self, since: u64) {
        self.send(&ToUi::HistoryCount {
            since,
            count: self.history.count_since(since),
        });
    }

    fn save_history(&mut self) {
        if let Err(err) = self.history.save_all(&self.paths) {
            eprintln!("история не сохранена: {err:#}");
        }
    }
}

fn is_web(url: &str) -> bool {
    (url.starts_with("https://") || url.starts_with("http://")) && url::Url::parse(url).is_ok()
}

/// Своя история движка (WebView2 ведёт её в папке профиля, рядом с куками) за тот же
/// срок. Профиль у вкладок и интерфейса общий, поэтому хватает webview интерфейса.
/// Куки, кэш и пароли не трогаем: очищается только история.
#[cfg(windows)]
fn clear_engine_history(ui: &wry::WebView, since: u64) {
    use webview2_com::{
        ClearBrowsingDataCompletedHandler,
        Microsoft::Web::WebView2::Win32::{
            COREWEBVIEW2_BROWSING_DATA_KINDS_BROWSING_HISTORY, ICoreWebView2_13,
            ICoreWebView2Profile2,
        },
    };
    use windows_core::Interface;
    use wry::WebViewExtWindows;

    let kinds = COREWEBVIEW2_BROWSING_DATA_KINDS_BROWSING_HISTORY;
    let handler = ClearBrowsingDataCompletedHandler::create(Box::new(|result| {
        if let Err(err) = result {
            eprintln!("история WebView2 не очищена: {err}");
        }
        Ok(())
    }));
    // SAFETY: ядро webview интерфейса живое, вызов идёт в потоке интерфейса;
    // обработчик COM держит сам, пока очистка не закончится.
    let result = unsafe {
        ui.webview()
            .cast::<ICoreWebView2_13>()
            .and_then(|core| core.Profile())
            .and_then(|profile| profile.cast::<ICoreWebView2Profile2>())
            .and_then(|profile| {
                if since == 0 {
                    profile.ClearBrowsingData(kinds, &handler)
                } else {
                    // С запасом на часы: запись, сделанная только что, тоже уходит.
                    let end = crate::storage::now_secs().saturating_add(3600);
                    profile.ClearBrowsingDataInTimeRange(kinds, since as f64, end as f64, &handler)
                }
            })
    };
    if let Err(err) = result {
        eprintln!("история WebView2 недоступна: {err}");
    }
}

/// WebKitGTK своей истории посещений на диске не ведёт: чистить нечего.
#[cfg(not(windows))]
fn clear_engine_history(_ui: &wry::WebView, _since: u64) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_only_web_pages() {
        assert!(is_web("https://a.example/"));
        assert!(is_web("http://127.0.0.1:8080/x"));
        for url in [
            "javascript:alert(1)",
            "file:///C:/Windows/win.ini",
            "axiom://settings",
            "data:text/html,x",
            "https://",
            "",
        ] {
            assert!(!is_web(url), "{url}");
        }
    }
}
