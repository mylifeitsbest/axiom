//! Панель ИИ и вкладки: двойной поиск привязывает панель к своей вкладке,
//! панель, открытая человеком, общая. Решения — в `ai_panel::follow`, здесь
//! они применяются к панели. Страницы не из AI Mode панель отдаёт вкладкам
//! (`ai_panel::pages`).

use anyhow::Result;

use super::App;
use crate::{
    ai_panel::{
        follow::{self, Mode, Step},
        pages,
    },
    events::UserEvent,
    tabs::{TabId, omnibox, script},
};

impl App {
    /// Команда интерфейса: панель открыли (Ctrl+K, кнопка, «Спросить ИИ»,
    /// повтор двойного поиска) или закрыли.
    pub(super) fn ai_command(&mut self, open: bool, query: Option<&str>) -> Result<()> {
        if !open {
            return self.ai_step(Step::Close);
        }
        let query = query.map(str::trim).filter(|q| !q.is_empty());
        let tab = self.tabs.active_tab().and_then(|t| t.ai_query.as_deref());
        let (mode, step) = follow::on_open(
            self.ai.mode,
            self.ai.open,
            query,
            tab,
            self.ai.shown_query(),
        );
        self.ai.mode = mode;
        self.ai_step(step)
    }

    /// Панель не пустила к себе `url` — страницу не из AI Mode. Если человек сам
    /// по ней щёлкнул, она открывается во вкладке, как ссылка из панели в новом окне.
    /// Переадресация и скрипт страницы без клика, а также всё, что случилось
    /// в спрятанной панели (новый разговор грузится в ней сам), вкладок не открывают.
    pub(super) fn ai_left(&mut self, url: String) {
        let Some(webview) = self.ai.webview.as_ref().filter(|_| self.ai.open) else {
            return;
        };
        let events = self.events.clone();
        script::ask(webview, pages::CLICK_PROBE, move |raw| {
            if pages::clicked(&raw) {
                let _ = events.send_event(UserEvent::OpenTab {
                    url: url.clone(),
                    ai: true,
                    from: None,
                });
            } else {
                eprintln!("панель ИИ: переход без клика остановлен");
            }
        });
    }

    /// Панель следует за активной вкладкой: панель двойного поиска видна только
    /// на вкладках со своим запросом и при возврате ничего не перезагружает.
    pub(super) fn ai_follow_tab(&mut self) {
        let query = self.tabs.active_tab().and_then(|t| t.ai_query.as_deref());
        let step = follow::on_activate(self.ai.mode, query, self.ai.shown_query());
        self.ai_step_logged(step);
    }

    /// Закрыли вкладку, у которой был запрос `query`.
    pub(super) fn ai_tab_closed(&mut self, query: Option<&str>) {
        let step = follow::on_close(self.ai.mode, query, self.ai.shown_query());
        self.ai_step_logged(step);
    }

    /// Двойной поиск: вкладка `id` перешла на выдачу с запросом `query`.
    pub(super) fn ai_search(&mut self, id: TabId, query: &str) {
        let active = self.tabs.active == Some(id);
        let Some(tab) = self.tabs.get_mut(id) else {
            return;
        };
        // Эту выдачу уже спрашивали: «Назад» на неё, пробуждение, перезагрузка.
        if tab.ai_query.as_deref() == Some(query) {
            return;
        }
        tab.ai_query = Some(query.to_string());
        let (mode, step) = follow::on_search(self.ai.mode, query, self.ai.shown_query(), active);
        self.ai.mode = mode;
        if self.ai_step_logged(step) {
            self.push_state();
        }
    }

    /// Поиск из адресной строки в новой вкладке (Alt+Enter). Запрос — до открытия
    /// вкладки: панель сразу покажет его, не прячась на миг.
    pub(super) fn ai_search_new_tab(&mut self, id: TabId, input: Option<&str>) {
        if !self.settings.double_search {
            return;
        }
        if let Some(omnibox::Target::Search(query)) = input.and_then(omnibox::resolve) {
            self.ai_search(id, &query);
        }
    }

    /// `true` — панель открылась, спряталась или сменила запрос.
    fn ai_step_logged(&mut self, step: Step) -> bool {
        let changed = step != Step::Keep;
        if let Err(err) = self.ai_step(step) {
            eprintln!("панель ИИ: {err:#}");
        }
        changed
    }

    fn ai_step(&mut self, step: Step) -> Result<()> {
        match step {
            Step::Keep => {}
            Step::Show(query) => self.open_ai(query.as_deref())?,
            Step::Hide | Step::Close => {
                // Панель показывала что-то с прошлого закрытия.
                let used = self.ai.mode != Mode::Closed;
                if step == Step::Close {
                    self.ai.mode = Mode::Closed;
                }
                if self.ai.open {
                    self.ai.hide();
                    // Видео из панели на весь экран не должно остаться без панели.
                    if self.fullscreen.is_on() {
                        self.sync_fullscreen();
                    }
                }
                // «Новый разговор при каждом открытии»: закрытая панель (человеком или
                // вместе с вкладкой своего поиска) откроется с чистого листа. Спрятанная
                // при переходе на другую вкладку — нет: она ещё следует за поиском.
                if step == Step::Close && used && self.settings.ai_new_chat {
                    self.ai.forget();
                }
            }
        }
        Ok(())
    }
}
