//! Боковая панель Google AI Mode.
//!
//! Панель живёт в общем окружении со вкладками (те же входы и тот же прокси —
//! кнопка на панели, см. `net`): отдельное стоило бы целого набора служебных
//! процессов WebView2 (~100 МБ).

pub mod follow;
pub mod pages;

use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use tao::{event_loop::EventLoopProxy, window::Window};
use wry::{Rect, WebContext, WebView, WebViewBuilder};

use crate::{
    downloads,
    events::UserEvent,
    tabs::{
        omnibox,
        webview::{Look, styled},
    },
    ui::hotkeys,
};

pub struct AiPanel {
    pub open: bool,
    pub webview: Option<WebView>,
    /// Когда панель закрыли: спрятанная панель тоже усыпляется.
    pub hidden_since: Option<Instant>,
    /// Кто открыл панель: от этого зависит, следует ли она за вкладками.
    pub mode: follow::Mode,
    /// Запрос, загруженный в панель (`None` — стартовая страница AI Mode).
    query: Option<String>,
}

impl Default for AiPanel {
    fn default() -> Self {
        Self::new()
    }
}

impl AiPanel {
    pub fn new() -> Self {
        Self {
            open: false,
            webview: None,
            hidden_since: None,
            mode: follow::Mode::Closed,
            query: None,
        }
    }

    /// Запрос, ответ на который сейчас в панели. Панель без webview (уснула,
    /// открепилась во вкладку) не показывает ничего.
    pub fn shown_query(&self) -> Option<&str> {
        self.webview.as_ref().and(self.query.as_deref())
    }

    /// Открывает панель; `query` — запрос для AI Mode. Запрос, который панель
    /// уже показывает, не загружается заново: ответ и переписка остаются.
    #[allow(clippy::too_many_arguments)]
    pub fn show(
        &mut self,
        window: &Window,
        context: &mut WebContext,
        events: &EventLoopProxy<UserEvent>,
        query: Option<&str>,
        bounds: Option<Rect>,
        look: Look,
    ) -> Result<()> {
        self.open = true;
        self.hidden_since = None;
        let query = query.map(str::trim).filter(|q| !q.is_empty());
        match &self.webview {
            Some(webview) => {
                if let Some(q) = query.filter(|&q| self.query.as_deref() != Some(q)) {
                    webview.load_url(&omnibox::ai_url(q))?;
                    self.query = Some(q.to_string());
                }
            }
            None => {
                let start = query.map_or_else(|| omnibox::AI_HOME.to_string(), omnibox::ai_url);
                // Адрес — после сборки: сначала должен встать запрет на локальную сеть.
                let builder = WebViewBuilder::new_with_web_context(context)
                    .with_bounds(bounds.unwrap_or_default())
                    .with_visible(bounds.is_some())
                    // Когда был клик — для переходов, которые панель отдаёт во вкладки.
                    .with_initialization_script(crate::tabs::links::TRACKER)
                    // В панели — только AI Mode, остальное во вкладках (см. `pages`).
                    .with_navigation_handler({
                        let events = events.clone();
                        move |url| pages::on_navigation(&url, &events)
                    })
                    // Ушла на другую страницу — её запросы разрешений больше не к месту.
                    .with_on_page_load_handler({
                        let events = events.clone();
                        move |event, _| {
                            if matches!(event, wry::PageLoadEvent::Started) {
                                let _ = events.send_event(UserEvent::PermissionPageLeft(
                                    crate::tabs::permissions::AI,
                                ));
                            }
                        }
                    })
                    .with_new_window_req_handler({
                        let events = events.clone();
                        move |url, _| {
                            if crate::tabs::popups::allowed(&url) {
                                let _ = events.send_event(UserEvent::OpenTab {
                                    url,
                                    ai: true,
                                    from: None,
                                });
                            }
                            wry::NewWindowResponse::Deny
                        }
                    });
                let builder = styled(builder, look);
                let builder = downloads::attach(builder, events);
                // Linux: расширение веб-процесса не даёт панели обращаться к локальной сети
                // (как и вкладкам). Окружение общее, и оно там уже есть.
                let builder = crate::tabs::identity::with_extension(builder);
                let webview = crate::tabs::webview::build_child(builder, window)
                    .context("не удалось создать панель ИИ")?;
                crate::tabs::local_net::install(&webview);
                hotkeys::install(&webview, events.clone());
                pages::guard(&webview, events.clone());
                crate::tabs::popups::install(&webview);
                crate::tabs::fullscreen::install(
                    &webview,
                    crate::tabs::fullscreen::Owner::Ai,
                    events.clone(),
                );
                downloads::install(&webview, None, events.clone());
                // Голосовой ввод AI Mode просит микрофон — спрашиваем тем же окошком.
                crate::tabs::permissions::install(
                    &webview,
                    crate::tabs::permissions::AI,
                    events.clone(),
                );
                crate::reports::watch(&webview, crate::reports::Source::Ai);
                crate::tabs::error_page::install(&webview);
                webview.load_url(&start)?;
                self.webview = Some(webview);
                self.query = query.map(String::from);
            }
        }
        Ok(())
    }

    /// Новый разговор: панель забывает ответ и переписку. Спрятанная панель сразу
    /// грузит стартовую страницу AI Mode, чтобы при открытии не мелькнул старый разговор.
    pub fn forget(&mut self) {
        self.query = None;
        if let Some(webview) = &self.webview {
            let _ = webview.load_url(omnibox::AI_HOME);
        }
    }

    pub fn hide(&mut self) {
        self.open = false;
        self.hidden_since = Some(Instant::now());
        if let Some(webview) = &self.webview {
            let _ = webview.set_visible(false);
        }
    }

    /// Применяет геометрию от интерфейса. `None` — панель ещё анимируется.
    pub fn place(&self, bounds: Option<Rect>, overlay: bool) {
        let Some(webview) = &self.webview else { return };
        match bounds {
            Some(rect) if self.open && !overlay => {
                crate::tabs::webview::set_bounds(webview, rect);
                let _ = webview.set_visible(true);
            }
            _ => {
                let _ = webview.set_visible(false);
            }
        }
    }

    pub fn current_url(&self) -> Option<String> {
        self.webview.as_ref().and_then(|w| w.url().ok())
    }

    /// Освобождает память, если панель скрыта дольше `limit` (`None` — никогда).
    pub fn sleep_if_idle(&mut self, limit: Option<Duration>) {
        if self.open {
            return;
        }
        let idle = limit.is_some_and(|limit| self.hidden_for(limit));
        if idle {
            self.webview = None;
        }
    }

    fn hidden_for(&self, limit: Duration) -> bool {
        self.hidden_since.is_some_and(|t| t.elapsed() >= limit)
    }
}
