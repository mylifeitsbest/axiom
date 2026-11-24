//! Пробуждение вкладки. До первой отрисовки WebView2 показывает только фон,
//! и тяжёлый сайт подолгу стоял бы пустым. Поэтому, пока уснувшая страница
//! грузится, на её месте заглушка интерфейса с иконкой и названием сайта
//! («Просыпается…»), а страница появляется, когда в ней есть что показать
//! (см. `tabs::waking`).

use std::{thread, time::Instant};

use super::App;
use crate::{
    events::UserEvent,
    tabs::{
        TabId,
        waking::{self, Event, LIMIT, STEP},
        webview as tab_webview,
    },
};

impl App {
    /// Webview уснувшей вкладки только что создан спрятанным.
    pub(super) fn begin_waking(&mut self, id: TabId) {
        let covered = self.covered();
        let Some(tab) = self.tabs.get_mut(id) else {
            return;
        };
        tab.waking = Some(Instant::now());
        if let Some(webview) = &tab.webview {
            tab_webview::set_behind(webview, true);
            // Фокус — как у открытой страницы: адресная строка не остаётся
            // в фокусе, а клавиши после пробуждения уходят странице.
            if !covered {
                let _ = webview.focus();
            }
        }
        // Под окном интерфейса страница «видима» и грузится как открытая.
        self.apply_layout();
        let events = self.events.clone();
        thread::spawn(move || {
            let mut waited = STEP;
            while waited <= LIMIT {
                thread::sleep(STEP);
                if events
                    .send_event(UserEvent::Waking(id, Event::Check))
                    .is_err()
                {
                    return;
                }
                waited += STEP;
            }
        });
    }

    /// Страница нарисовалась, её сменили или ждать надоело: показываем webview
    /// вместо заглушки.
    pub(super) fn end_waking(&mut self, id: TabId) {
        let Some(tab) = self.tabs.get_mut(id) else {
            return;
        };
        if tab.waking.take().is_none() {
            return;
        }
        if let Some(webview) = &tab.webview {
            tab_webview::set_behind(webview, false);
        }
        if self.tabs.active == Some(id) {
            self.apply_layout();
        }
        self.push_state();
    }

    pub(super) fn on_waking(&mut self, id: TabId, event: Event) {
        let Some(tab) = self.tabs.get(id).filter(|t| t.is_waking()) else {
            return;
        };
        match event {
            Event::Check if tab.waking.is_some_and(|at| at.elapsed() >= LIMIT) => {
                self.end_waking(id);
            }
            Event::Check => {
                let Some(webview) = &tab.webview else {
                    return;
                };
                let events = self.events.clone();
                let _ = webview.evaluate_script_with_callback(waking::PROBE, move |raw| {
                    let ready = waking::parse(&raw);
                    let _ = events.send_event(UserEvent::Waking(id, Event::Probed(ready)));
                });
            }
            Event::Probed(true) => self.end_waking(id),
            Event::Probed(false) => {}
        }
    }
}
