//! Боковая панель Google AI Mode.
//!
//! Панель живёт в отдельном окружении WebView2 (своя папка данных),
//! поэтому прокси применяется только к ней, а вкладки ходят напрямую.

pub mod follow;
pub mod proxy;

use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use tao::{event_loop::EventLoopProxy, window::Window};
use wry::{ProxyConfig, Rect, WebContext, WebView, WebViewBuilder};

use crate::{
    downloads,
    events::UserEvent,
    storage::Paths,
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
    context: Option<WebContext>,
    proxy_raw: String,
}

impl AiPanel {
    pub fn new(proxy_raw: &str) -> Self {
        Self {
            open: false,
            webview: None,
            hidden_since: None,
            mode: follow::Mode::Closed,
            query: None,
            context: None,
            proxy_raw: proxy_raw.to_string(),
        }
    }

    pub fn has_proxy(&self) -> bool {
        !self.proxy_raw.is_empty()
    }

    pub fn proxy_config(&self) -> Option<ProxyConfig> {
        proxy::parse(&self.proxy_raw).ok().flatten()
    }

    /// Меняет прокси: старое окружение уничтожается, новое создастся при открытии.
    pub fn set_proxy(&mut self, proxy_raw: &str) {
        if self.proxy_raw != proxy_raw {
            self.proxy_raw = proxy_raw.to_string();
            self.webview = None;
            self.context = None;
        }
    }

    /// Окружение панели ИИ, общее с вкладками «из ИИ».
    ///
    /// Без прокси панель живёт в общем окружении со вкладками: это экономит
    /// целый набор служебных процессов WebView2 (~100 МБ). С прокси — отдельное
    /// окружение со своей папкой данных, иначе прокси применился бы ко всем вкладкам.
    pub fn context<'a>(
        &'a mut self,
        paths: &Paths,
        shared: &'a mut WebContext,
    ) -> &'a mut WebContext {
        if self.proxy_raw.is_empty() {
            return shared;
        }
        let key = proxy::data_key(&self.proxy_raw);
        self.context
            .get_or_insert_with(|| WebContext::new(Some(paths.ai_data(&key))))
    }

    /// Запрос, ответ на который сейчас в панели. Панель без webview (уснула,
    /// сменился прокси, открепилась во вкладку) не показывает ничего.
    pub fn shown_query(&self) -> Option<&str> {
        self.webview.as_ref().and(self.query.as_deref())
    }

    /// Открывает панель; `query` — запрос для AI Mode. Запрос, который панель
    /// уже показывает, не загружается заново: ответ и переписка остаются.
    #[allow(clippy::too_many_arguments)]
    pub fn show(
        &mut self,
        window: &Window,
        paths: &Paths,
        shared: &mut WebContext,
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
                let proxy = self.proxy_config();
                let start = query.map_or_else(|| omnibox::AI_HOME.to_string(), omnibox::ai_url);
                let context = self.context(paths, shared);
                let builder = WebViewBuilder::new_with_web_context(context)
                    .with_url(start)
                    .with_bounds(bounds.unwrap_or_default())
                    .with_visible(bounds.is_some())
                    .with_navigation_handler(|url| omnibox::is_navigable(&url))
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
                let builder = styled(builder, look, proxy.as_ref());
                let builder = downloads::attach(builder, events);
                let webview = crate::tabs::webview::build_child(builder, window)
                    .context("не удалось создать панель ИИ")?;
                hotkeys::install(&webview, events.clone());
                crate::tabs::popups::install(&webview);
                crate::tabs::fullscreen::install(
                    &webview,
                    crate::tabs::fullscreen::Owner::Ai,
                    events.clone(),
                );
                downloads::install(&webview, None, events.clone());
                crate::reports::watch(&webview, crate::reports::Source::Ai);
                self.webview = Some(webview);
                self.query = query.map(String::from);
            }
        }
        Ok(())
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
