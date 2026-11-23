//! Состояние браузера и обработка событий главного цикла.

use std::{
    path::Path,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use tao::{
    dpi::{LogicalPosition, LogicalSize},
    event_loop::{EventLoop, EventLoopProxy},
    window::{ResizeDirection, Window, WindowBuilder},
};
use url::Url;
use wry::{NewWindowResponse, RGBA, Rect, WebContext, WebView, WebViewBuilder};

use crate::{
    ai_panel::{AiPanel, proxy},
    downloads::{self, DownloadEvent, Downloads, Status},
    events::{TICK, UserEvent},
    ipc::{self, DownloadView, Edge, TabView, ToUi, UiCmd, WindowAction},
    settings::Settings,
    storage::{
        Paths,
        bookmarks::Bookmarks,
        chrome,
        history::History,
        session::{SavedTab, Session},
    },
    system,
    tabs::{
        TabId, TabStore,
        omnibox::{self, NEWTAB},
        sleep::{self, PageState},
        webview::{self as tab_webview, Look, TabSpec},
    },
    ui::{self, hotkeys::Hotkey},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    Continue,
    Exit,
}

/// Поля объявлены в порядке уничтожения: сначала webview, окно — последним.
pub struct App {
    tabs: TabStore,
    /// Webview вкладок, закрытых посреди загрузки. Без своего webview WebView2
    /// перестаёт сообщать о загрузке и не может её завершить, поэтому держим
    /// его скрытым, пока загрузка не закончится.
    parked: Vec<(TabId, WebView)>,
    ai: AiPanel,
    ui: WebView,
    context: WebContext,
    window: Window,
    events: EventLoopProxy<UserEvent>,
    paths: Paths,
    settings: Settings,
    history: History,
    bookmarks: Bookmarks,
    downloads: Downloads,
    /// Область контента и панели ИИ от интерфейса (логические px).
    content: Option<Rect>,
    ai_rect: Option<Rect>,
    /// Подсказки, настройки или приветствие поверх контента: webview временно прячутся.
    overlay: bool,
    shown: bool,
    session_dirty: bool,
    /// Последний запрос, отправленный в AI Mode (чтобы не грузить его дважды).
    last_ai_query: String,
    importing: bool,
    /// Принудительно тёмные сайты: аргумент запуска WebView2, меняется только перезапуском.
    force_dark: bool,
}

impl App {
    pub fn new(event_loop: &EventLoop<UserEvent>, paths: Paths, open: &[String]) -> Result<Self> {
        let settings = Settings::load(&paths);
        let history = History::load(&paths);
        let bookmarks = Bookmarks::load(&paths);
        let downloads = Downloads::load(&paths);
        let events = event_loop.create_proxy();
        let window = build_window(event_loop)?;
        #[cfg(target_os = "linux")]
        crate::ui::hotkeys::install_window(&window, events.clone());
        let mut context = WebContext::new(Some(paths.webview_data()));
        let force_dark = settings.force_dark;
        let ui_look = Look {
            background: ipc::parse_hex(&settings.colors.window).unwrap_or((20, 20, 24, 255)),
            theme: settings.colors.theme(),
            force_dark,
        };
        #[cfg(target_os = "linux")]
        tab_webview::apply_theme_globally(ui_look.theme);
        let ui = build_ui(&window, &mut context, &events, ui_look)?;
        let ai = AiPanel::new(&settings.ai_proxy);

        let mut app = Self {
            tabs: TabStore::default(),
            parked: Vec::new(),
            ai,
            ui,
            context,
            window,
            events,
            paths,
            settings,
            history,
            bookmarks,
            downloads,
            content: None,
            ai_rect: None,
            overlay: false,
            shown: false,
            session_dirty: false,
            last_ai_query: String::new(),
            importing: false,
            force_dark,
        };
        app.restore_session();
        app.open_external(open);
        Ok(app)
    }

    pub fn handle(&mut self, event: UserEvent) -> Flow {
        match self.dispatch(event) {
            Ok(flow) => flow,
            Err(err) => {
                eprintln!("ошибка: {err:#}");
                Flow::Continue
            }
        }
    }

    fn dispatch(&mut self, event: UserEvent) -> Result<Flow> {
        match event {
            UserEvent::Ui(raw) => match ipc::parse(&raw) {
                Ok(cmd) => return self.on_ui(cmd),
                Err(err) => eprintln!("IPC отклонён: {err}"),
            },
            UserEvent::Hotkey(hotkey) => self.on_hotkey(hotkey)?,
            UserEvent::TabTitle(id, title) => self.on_title(id, title),
            UserEvent::TabLoad { id, started, url } => self.on_load(id, started, url),
            UserEvent::OpenTab { url, ai } => {
                if omnibox::is_navigable(&url) {
                    let id = self.tabs.add(url.clone(), display_title(&url), ai, None);
                    self.activate(id);
                }
            }
            UserEvent::SleepChecked(id, page) => self.finish_sleep(id, page),
            UserEvent::FindResult(found) => self.send(&ToUi::Find { found }),
            UserEvent::Download(event) => self.on_download(event),
            UserEvent::External(urls) => {
                self.open_external(&urls);
                self.window.set_minimized(false);
                self.window.set_visible(true);
                self.window.set_focus();
            }
            UserEvent::Imported(result) => self.finish_import(result),
            UserEvent::ShowFallback => self.show_window(),
            UserEvent::Tick => self.on_tick(),
        }
        Ok(Flow::Continue)
    }

    // ---------- команды интерфейса ----------

    fn on_ui(&mut self, cmd: UiCmd) -> Result<Flow> {
        match cmd {
            UiCmd::Ready => {
                self.send(&ToUi::Settings {
                    settings: &self.settings,
                });
                self.send(&ToUi::System {
                    status: system::status(),
                });
                self.push_state();
                self.push_downloads();
                self.push_window_state();
                self.show_window();
                if self.tabs.active_tab().is_some_and(|t| t.is_newtab()) {
                    let _ = self.ui.focus();
                }
            }
            UiCmd::Layout { content, ai } => {
                self.content = Some(to_rect(content));
                self.ai_rect = ai.map(to_rect);
                self.apply_layout();
            }
            UiCmd::Navigate { input } => self.navigate(&input)?,
            UiCmd::NewTab { input } => self.new_tab(input.as_deref()),
            UiCmd::CloseTab { id } => self.close_tab(id),
            UiCmd::ActivateTab { id } => {
                if self.tabs.get(id).is_some() {
                    self.activate(id);
                }
            }
            UiCmd::MoveTab { id, to } => {
                self.tabs.move_to(id, to);
                self.session_dirty = true;
                self.push_state();
            }
            UiCmd::RestoreTab => self.restore_tab(),
            UiCmd::Back => self.on_hotkey(Hotkey::Back)?,
            UiCmd::Forward => self.on_hotkey(Hotkey::Forward)?,
            UiCmd::Reload => self.on_hotkey(Hotkey::Reload)?,
            UiCmd::Hotkey { action } => {
                if let Some(hotkey) = Hotkey::parse(&action) {
                    self.on_hotkey(hotkey)?;
                }
            }
            UiCmd::Ai { open, query } => {
                if open {
                    if let Some(q) = query.as_deref().filter(|q| !q.trim().is_empty()) {
                        self.last_ai_query = q.trim().to_string();
                    }
                    self.open_ai(query.as_deref())?;
                } else {
                    self.ai.hide();
                }
                self.push_state();
            }
            UiCmd::AiPopOut => self.pop_out_ai(),
            UiCmd::Overlay { open } => {
                self.overlay = open;
                self.apply_layout();
            }
            UiCmd::Suggest { q } => {
                let mut items = self.bookmarks.search(&q, 3);
                for hit in self.history.search(&q, 8) {
                    if items.len() >= 7 {
                        break;
                    }
                    if !items.iter().any(|s| s.url == hit.url) {
                        items.push(hit);
                    }
                }
                self.send(&ToUi::Suggestions { q: &q, items });
            }
            UiCmd::ToggleBookmark => self.toggle_bookmark(),
            UiCmd::ImportChrome => self.start_import(),
            UiCmd::Install => {
                let result = system::install();
                let error = result
                    .as_ref()
                    .err()
                    .map(|e| format!("{e:#}"))
                    .unwrap_or_default();
                self.send(&ToUi::Installed {
                    ok: result.is_ok(),
                    error: &error,
                });
                self.send(&ToUi::System {
                    status: system::status(),
                });
            }
            UiCmd::OpenDefaultApps => match system::make_default() {
                Ok(true) => {
                    self.send(&ToUi::Toast {
                        text: "Axiom — браузер по умолчанию",
                    });
                    self.send(&ToUi::System {
                        status: system::status(),
                    });
                }
                Ok(false) => {}
                Err(err) => {
                    let text = format!("{err:#}");
                    self.send(&ToUi::Toast { text: &text });
                }
            },
            UiCmd::SaveSettings { settings } => self.save_settings(settings),
            UiCmd::ThemeColors {
                window,
                page,
                scheme,
            } => {
                let colors = &self.settings.colors;
                let scheme_changed = !scheme.is_empty() && colors.scheme != scheme;
                if colors.window != window || colors.page != page || scheme_changed {
                    if let Some(rgba) = ipc::parse_hex(&window) {
                        let _ = self.ui.set_background_color(rgba);
                    }
                    self.settings.colors.window = window;
                    self.settings.colors.page = page;
                    if scheme_changed {
                        self.settings.colors.scheme = scheme;
                        self.apply_site_theme();
                    }
                    self.persist_settings();
                }
            }
            UiCmd::Window { action } => match action {
                WindowAction::Minimize => self.window.set_minimized(true),
                WindowAction::Maximize => {
                    self.window.set_maximized(!self.window.is_maximized());
                    self.push_window_state();
                }
                WindowAction::Close => return Ok(Flow::Exit),
                WindowAction::Drag => {
                    let _ = self.window.drag_window();
                }
            },
            UiCmd::ResizeWindow { dir } => {
                let _ = self.window.drag_resize_window(resize_direction(dir));
            }
            UiCmd::Find { q, backwards } => self.find(&q, backwards),
            UiCmd::DownloadOpen { id } => {
                if self
                    .downloads
                    .get(id)
                    .is_some_and(|d| d.status == Status::Done)
                {
                    self.download_action(id, downloads::open);
                }
            }
            UiCmd::DownloadShow { id } => self.download_action(id, downloads::reveal),
            UiCmd::DownloadCancel { id } => {
                if !downloads::cancel(id) {
                    self.send(&ToUi::Toast {
                        text: "Эту загрузку нельзя отменить",
                    });
                }
            }
            UiCmd::DownloadRemove { id } => {
                if self.downloads.remove(id) {
                    self.push_downloads();
                }
            }
            UiCmd::DownloadsClear => {
                self.downloads.clear();
                self.push_downloads();
            }
            UiCmd::DownloadsFolder => match downloads::folder() {
                Some(dir) => {
                    let _ = std::fs::create_dir_all(&dir);
                    if let Err(err) = downloads::open(&dir) {
                        self.toast_error(&err);
                    }
                }
                None => self.send(&ToUi::Toast {
                    text: "Не найдена папка «Загрузки»",
                }),
            },
        }
        Ok(Flow::Continue)
    }

    fn on_hotkey(&mut self, hotkey: Hotkey) -> Result<()> {
        if hotkey.is_ui() {
            let _ = self.ui.focus();
            self.send(&ToUi::Hotkey {
                action: hotkey.name(),
            });
            return Ok(());
        }
        let active = self.tabs.active;
        match hotkey {
            Hotkey::NewTab => self.new_tab(None),
            Hotkey::CloseTab => {
                if let Some(id) = active {
                    self.close_tab(id);
                }
            }
            Hotkey::RestoreTab => self.restore_tab(),
            Hotkey::NextTab | Hotkey::PrevTab => {
                let step = if hotkey == Hotkey::NextTab { 1 } else { -1 };
                if let Some(next) = active.and_then(|id| self.tabs.neighbor(id, step)) {
                    self.activate(next);
                }
            }
            Hotkey::SelectTab(n) => {
                if let Some(id) = self.tabs.nth(n) {
                    self.activate(id);
                }
            }
            Hotkey::Bookmark => self.toggle_bookmark(),
            Hotkey::Reload | Hotkey::Back | Hotkey::Forward => {
                let Some(id) = active else { return Ok(()) };
                let Some(tab) = self.tabs.get(id) else {
                    return Ok(());
                };
                match &tab.webview {
                    Some(webview) => match hotkey {
                        Hotkey::Reload => webview.reload()?,
                        Hotkey::Back => webview.go_back()?,
                        _ => webview.go_forward()?,
                    },
                    None if !tab.is_newtab() => self.wake(id)?,
                    None => {}
                }
            }
            _ => {}
        }
        Ok(())
    }

    // ---------- вкладки ----------

    fn restore_session(&mut self) {
        let mut active = None;
        if let Some(session) = Session::load(&self.paths) {
            for (i, saved) in session.tabs.iter().enumerate() {
                let id = self
                    .tabs
                    .add(saved.url.clone(), saved.title.clone(), saved.ai, Some(i));
                if i == session.active {
                    active = Some(id);
                }
            }
        }
        let id = active.unwrap_or_else(|| self.tabs.add(NEWTAB.into(), String::new(), false, None));
        // Остальные вкладки остаются спящими до первого клика.
        self.activate(id);
    }

    fn activate(&mut self, id: TabId) {
        if let Some(prev) = self.tabs.active.filter(|&p| p != id)
            && let Some(tab) = self.tabs.get_mut(prev)
        {
            tab.last_active = Instant::now();
            // Заморозится на тике, если не вернуться за `SUSPEND_AFTER`.
            if let Some(webview) = &tab.webview {
                let _ = webview.set_visible(false);
            }
        }
        self.tabs.active = Some(id);
        if self.tabs.get(id).is_some_and(|t| t.is_sleeping())
            && let Err(err) = self.wake(id)
        {
            eprintln!("не удалось разбудить вкладку: {err:#}");
        }
        let mut is_newtab = false;
        if let Some(tab) = self.tabs.get_mut(id) {
            tab.last_active = Instant::now();
            tab.sleep_requested = None;
            tab.keep_alive = false;
            is_newtab = tab.is_newtab();
            if let Some(webview) = &tab.webview {
                tab_webview::resume(webview);
                if let Some(rect) = self.content {
                    let _ = webview.set_bounds(rect);
                }
                let _ = webview.set_visible(!self.overlay);
            }
        }
        if is_newtab {
            let _ = self.ui.focus();
        }
        self.limit_live_tabs();
        self.session_dirty = true;
        self.push_state();
    }

    /// Создаёт webview для спящей вкладки.
    fn wake(&mut self, id: TabId) -> Result<()> {
        let Some(tab) = self.tabs.get(id) else {
            return Ok(());
        };
        if tab.webview.is_some() || tab.is_newtab() {
            return Ok(());
        }
        let (url, ai) = (tab.url.clone(), tab.ai);
        let spec = TabSpec {
            id,
            url: &url,
            bounds: self.content.unwrap_or_default(),
            visible: self.tabs.active == Some(id) && !self.overlay,
            look: self.look(),
            proxy: if ai { self.ai.proxy_config() } else { None },
            ai,
        };
        let webview = if ai {
            let context = self.ai.context(&self.paths, &mut self.context);
            tab_webview::build_tab(&self.window, context, &self.events, spec)?
        } else {
            tab_webview::build_tab(&self.window, &mut self.context, &self.events, spec)?
        };
        if let Some(tab) = self.tabs.get_mut(id) {
            tab.webview = Some(webview);
            tab.loading = true;
        }
        Ok(())
    }

    fn navigate(&mut self, input: &str) -> Result<()> {
        let Some(target) = omnibox::resolve(input) else {
            return Ok(());
        };
        let Some(id) = self.tabs.active else {
            return Ok(());
        };
        self.load_in_tab(id, target.into_url())
    }

    fn load_in_tab(&mut self, id: TabId, url: String) -> Result<()> {
        if url == NEWTAB {
            self.park_if_busy(id);
        }
        let Some(tab) = self.tabs.get_mut(id) else {
            return Ok(());
        };
        if url == NEWTAB {
            tab.url = url;
            tab.title.clear();
            tab.webview = None;
            self.push_state();
            return Ok(());
        }
        tab.title = display_title(&url);
        tab.url = url.clone();
        tab.loading = true;
        tab.scroll = None;
        match &tab.webview {
            Some(webview) => {
                webview.load_url(&url)?;
                let _ = webview.focus();
            }
            None => {
                self.wake(id)?;
                if let Some(webview) = self.tabs.get(id).and_then(|t| t.webview.as_ref()) {
                    let _ = webview.focus();
                }
            }
        }
        self.session_dirty = true;
        self.push_state();
        Ok(())
    }

    fn new_tab(&mut self, input: Option<&str>) {
        let url = input
            .and_then(omnibox::resolve)
            .map(omnibox::Target::into_url)
            .unwrap_or_else(|| NEWTAB.to_string());
        let title = if url == NEWTAB {
            String::new()
        } else {
            display_title(&url)
        };
        let id = self.tabs.add(url, title, false, None);
        self.activate(id);
    }

    fn close_tab(&mut self, id: TabId) {
        self.close_tab_with(id, true);
    }

    fn close_tab_with(&mut self, id: TabId, remember: bool) {
        let Some(index) = self.tabs.index_of(id) else {
            return;
        };
        let was_active = self.tabs.active == Some(id);
        self.park_if_busy(id);
        drop(self.tabs.remove(id, remember));
        self.session_dirty = true;
        if self.tabs.len() == 0 {
            let fresh = self.tabs.add(NEWTAB.into(), String::new(), false, None);
            self.activate(fresh);
        } else if was_active {
            if let Some(next) = self.tabs.successor_for(index) {
                self.activate(next);
            }
        } else {
            self.push_state();
        }
    }

    fn restore_tab(&mut self) {
        match self.tabs.pop_closed() {
            Some(closed) => {
                let id = self
                    .tabs
                    .add(closed.url, closed.title, closed.ai, Some(closed.index));
                self.activate(id);
            }
            None => self.send(&ToUi::Toast {
                text: "Нет закрытых вкладок",
            }),
        }
    }

    fn on_title(&mut self, id: TabId, title: String) {
        let Some(tab) = self.tabs.get_mut(id) else {
            return;
        };
        let title: String = title.chars().take(300).collect();
        // Адрес мог смениться без перезагрузки (history.pushState).
        if let Some(url) = tab.webview.as_ref().and_then(|w| w.url().ok())
            && omnibox::is_navigable(&url)
            && url != "about:blank"
        {
            tab.url = url;
        }
        if !title.trim().is_empty() {
            tab.title = title;
            self.history.retitle(&tab.url, &tab.title);
        }
        self.session_dirty = true;
        self.mirror_search(id);
        self.push_state();
    }

    fn on_load(&mut self, id: TabId, started: bool, url: String) {
        let Some(tab) = self.tabs.get_mut(id) else {
            return;
        };
        tab.loading = started;
        let blank = url == "about:blank";
        if omnibox::is_navigable(&url) && !blank {
            tab.url = url;
        }
        if !started {
            // Вкладка, открытая ради загрузки, страницу так и не показала — в историю её не пишем.
            if !blank {
                self.history.record(&tab.url, &tab.title);
            }
            if let (Some(y), Some(webview)) = (tab.scroll.take(), &tab.webview) {
                let _ = webview.evaluate_script(&format!("window.scrollTo(0,{y})"));
            }
        }
        self.session_dirty = true;
        if started {
            self.mirror_search(id);
        }
        self.push_state();
    }

    /// Двойной поиск работает и для поиска внутри самой страницы DuckDuckGo:
    /// как только активная вкладка перешла на результаты с новым запросом,
    /// тот же запрос уходит в AI Mode.
    fn mirror_search(&mut self, id: TabId) {
        if !self.settings.double_search || self.tabs.active != Some(id) {
            return;
        }
        let Some(query) = self.tabs.get(id).and_then(|t| ddg_query(&t.url)) else {
            return;
        };
        if query == self.last_ai_query {
            return;
        }
        self.last_ai_query = query.clone();
        if let Err(err) = self.open_ai(Some(&query)) {
            eprintln!("панель ИИ: {err:#}");
        }
    }

    // ---------- панель ИИ ----------

    fn open_ai(&mut self, query: Option<&str>) -> Result<()> {
        let look = self.look();
        self.ai.show(
            &self.window,
            &self.paths,
            &mut self.context,
            &self.events,
            query,
            self.ai_rect,
            look,
        )?;
        self.ai.place(self.ai_rect, self.overlay);
        Ok(())
    }

    /// Открепляет панель во вкладку (тот же прокси и куки).
    fn pop_out_ai(&mut self) {
        let url = self
            .ai
            .current_url()
            .unwrap_or_else(|| omnibox::AI_HOME.to_string());
        self.ai.hide();
        self.ai.webview = None;
        let id = self.tabs.add(url, "AI Mode".into(), true, None);
        self.activate(id);
    }

    // ---------- загрузки ----------

    fn on_download(&mut self, event: DownloadEvent) {
        if let DownloadEvent::Progress { id, received, .. } = event {
            self.downloads.apply(event);
            if let Some(d) = self
                .downloads
                .get(id)
                .filter(|d| d.status == Status::Active)
            {
                self.send(&ToUi::DownloadProgress {
                    id,
                    received,
                    total: d.total,
                });
            }
            return;
        }
        let blank_tab = match event {
            DownloadEvent::Started { tab, .. } => tab,
            _ => None,
        };
        let text = self.downloads.apply(event).and_then(|d| match d.status {
            Status::Active => Some(format!("Скачивается {}", d.name())),
            Status::Done => Some(format!("Скачано: {}", d.name())),
            // Отменил сам пользователь — сообщать незачем.
            Status::Cancelled => None,
            Status::Failed => Some(format!("{}: {}", d.error, d.name())),
        });
        if let Some(text) = text {
            self.send(&ToUi::Toast { text: &text });
        }
        if let Some(tab) = blank_tab {
            self.close_blank_tab(tab);
        }
        self.release_parked();
        self.push_downloads();
    }

    /// Прячет webview вкладки, если она ещё что-то скачивает (см. `parked`).
    fn park_if_busy(&mut self, id: TabId) {
        if !self.downloads.is_busy(id) {
            return;
        }
        if let Some(webview) = self.tabs.get_mut(id).and_then(|t| t.webview.take()) {
            let _ = webview.set_visible(false);
            tab_webview::set_low_memory(&webview);
            self.parked.push((id, webview));
        }
    }

    fn release_parked(&mut self) {
        let downloads = &self.downloads;
        self.parked.retain(|(id, _)| downloads.is_busy(*id));
    }

    /// Ссылка с `target=_blank` или адрес файла в новой вкладке: страница так и не
    /// открылась, осталась пустая вкладка. Закрываем её, как Chrome; загрузка
    /// идёт дальше. Последнюю вкладку не закрываем — она становится новой.
    fn close_blank_tab(&mut self, id: TabId) {
        let blank = self.tabs.get(id).is_some_and(|t| {
            t.webview
                .as_ref()
                .and_then(|w| w.url().ok())
                .is_some_and(|u| u.is_empty() || u == "about:blank")
        });
        if !blank {
            return;
        }
        if self.tabs.len() > 1 {
            self.close_tab_with(id, false);
        } else if let Err(err) = self.load_in_tab(id, NEWTAB.into()) {
            eprintln!("вкладка загрузки: {err:#}");
        }
    }

    fn download_action(&mut self, id: u64, action: fn(&Path) -> Result<()>) {
        let Some(path) = self.downloads.get(id).map(|d| d.path.clone()) else {
            return;
        };
        if let Err(err) = action(&path) {
            self.toast_error(&err);
            // Файл могли удалить: список покажет это.
            self.push_downloads();
        }
    }

    fn toast_error(&self, err: &anyhow::Error) {
        let text = format!("{err:#}");
        let mut chars = text.chars();
        let text: String = match chars.next() {
            Some(first) => first.to_uppercase().chain(chars).collect(),
            None => text.clone(),
        };
        self.send(&ToUi::Toast { text: &text });
    }

    fn push_downloads(&self) {
        let items = self
            .downloads
            .iter()
            .map(|d| DownloadView {
                id: d.id,
                name: d.name(),
                url: d.url.clone(),
                received: d.received,
                total: d.total,
                status: d.status,
                error: d.error.clone(),
                at: d.at,
                exists: d.status != Status::Done || d.path.exists(),
            })
            .collect();
        self.send(&ToUi::Downloads {
            items,
            can_cancel: downloads::CAN_CANCEL,
        });
    }

    // ---------- закладки, импорт, ссылки извне ----------

    fn toggle_bookmark(&mut self) {
        let Some(tab) = self.tabs.active_tab() else {
            return;
        };
        if tab.is_newtab() {
            return;
        }
        let (url, title) = (tab.url.clone(), tab.title.clone());
        let added = self.bookmarks.toggle(&url, &title);
        if let Err(err) = self.bookmarks.save(&self.paths) {
            eprintln!("закладки не сохранены: {err:#}");
        }
        let text = if added {
            "Добавлено в закладки"
        } else {
            "Закладка удалена"
        };
        self.send(&ToUi::Toast { text });
        self.push_state();
    }

    /// Импорт читает файлы Chrome в фоновом потоке, чтобы окно не подвисало.
    fn start_import(&mut self) {
        if self.importing {
            return;
        }
        self.importing = true;
        let (paths, events) = (self.paths.clone(), self.events.clone());
        std::thread::spawn(move || {
            let result = chrome::import(&paths).map_err(|e| format!("{e:#}"));
            let _ = events.send_event(UserEvent::Imported(result));
        });
    }

    fn finish_import(&mut self, result: Result<chrome::Imported, String>) {
        self.importing = false;
        match result {
            Ok(data) => {
                let bookmarks = self.bookmarks.merge(data.bookmarks);
                let history = self.history.merge(data.history);
                let _ = self.bookmarks.save(&self.paths);
                let _ = self.history.save_if_dirty(&self.paths);
                self.send(&ToUi::Imported {
                    ok: true,
                    profile: &data.profile,
                    bookmarks,
                    history,
                    error: "",
                });
                self.push_state();
            }
            Err(error) => self.send(&ToUi::Imported {
                ok: false,
                profile: "",
                bookmarks: 0,
                history: 0,
                error: &error,
            }),
        }
    }

    /// Адреса из командной строки: ссылки из других программ, файлы .html.
    fn open_external(&mut self, args: &[String]) {
        for arg in args {
            let path = std::path::Path::new(arg);
            let url = if path.is_absolute() && path.exists() {
                Url::from_file_path(path).ok().map(String::from)
            } else {
                omnibox::resolve(arg).map(omnibox::Target::into_url)
            };
            if let Some(url) = url.filter(|u| u != NEWTAB) {
                let id = self.tabs.add(url.clone(), display_title(&url), false, None);
                self.activate(id);
            }
        }
    }

    // ---------- настройки ----------

    fn save_settings(&mut self, mut incoming: ipc::SettingsIn) {
        let old_proxy = self.settings.ai_proxy.clone();
        if let Err(err) = proxy::parse(&incoming.ai_proxy) {
            let text = format!("Прокси не применён: {err}");
            self.send(&ToUi::Toast { text: &text });
            incoming.ai_proxy = old_proxy.clone();
        }
        self.settings.apply(incoming);
        if self.settings.ai_proxy != old_proxy {
            self.apply_proxy_change();
        }
        self.limit_live_tabs();
        self.persist_settings();
    }

    fn apply_proxy_change(&mut self) {
        let was_open = self.ai.open;
        self.ai.set_proxy(&self.settings.ai_proxy);
        // Вкладки «из ИИ» пересоздадутся с новым прокси при следующем открытии.
        let active = self.tabs.active;
        for tab in self
            .tabs
            .iter_mut()
            .filter(|t| t.ai && Some(t.id) != active)
        {
            tab.webview = None;
        }
        if was_open && let Err(err) = self.open_ai(None) {
            eprintln!("панель ИИ: {err:#}");
        }
        let text = if self.settings.ai_proxy.is_empty() {
            "Панель ИИ работает без прокси"
        } else {
            "Прокси для панели ИИ применён"
        };
        self.send(&ToUi::Toast { text });
        self.push_state();
    }

    fn persist_settings(&self) {
        if let Err(err) = self.settings.save(&self.paths) {
            eprintln!("настройки не сохранены: {err:#}");
        }
    }

    // ---------- раскладка и окно ----------

    fn apply_layout(&self) {
        if let Some(webview) = self.tabs.active_tab().and_then(|t| t.webview.as_ref()) {
            if let Some(rect) = self.content {
                let _ = webview.set_bounds(rect);
            }
            let _ = webview.set_visible(!self.overlay);
        }
        self.ai.place(self.ai_rect, self.overlay);
    }

    pub fn on_resize(&mut self) {
        let _ = self.ui.set_bounds(full_bounds(&self.window));
        self.push_window_state();
    }

    fn show_window(&mut self) {
        if !self.shown {
            self.shown = true;
            self.window.set_visible(true);
            self.window.set_focus();
        }
    }

    fn push_window_state(&self) {
        self.send(&ToUi::Window {
            maximized: self.window.is_maximized(),
        });
    }

    fn find(&self, query: &str, backwards: bool) {
        let Some(webview) = self.tabs.active_tab().and_then(|t| t.webview.as_ref()) else {
            return;
        };
        if query.is_empty() {
            let _ = webview.evaluate_script("window.getSelection()?.removeAllRanges()");
            return;
        }
        let Ok(q) = serde_json::to_string(query) else {
            return;
        };
        let js = format!("window.find({q}, false, {backwards}, true)");
        let events = self.events.clone();
        let _ = webview.evaluate_script_with_callback(&js, move |result| {
            let _ = events.send_event(UserEvent::FindResult(result.trim() == "true"));
        });
    }

    // ---------- память: усыпление ----------

    fn on_tick(&mut self) {
        self.drop_unanswered();
        let idle = self.idle_limit();
        self.sleep_idle_tabs(idle);
        self.limit_live_tabs();
        // Скачивание из панели ИИ прервалось бы вместе с её webview.
        if !self.downloads.busy_untracked() {
            self.ai.sleep_if_idle(idle);
        }
        self.suspend_hidden();
        if self.session_dirty {
            self.save_session();
        }
        if let Err(err) = self.downloads.save_if_dirty(&self.paths) {
            eprintln!("список загрузок не сохранён: {err:#}");
        }
        if let Err(err) = self.history.save_if_dirty(&self.paths) {
            eprintln!("история не сохранена: {err:#}");
        }
    }

    /// Через сколько бездействия усыплять фоновые вкладки и спрятанную панель ИИ:
    /// `sleep_minutes`, а когда системе не хватает памяти — через минуту.
    /// `None` — усыпление выключено.
    fn idle_limit(&self) -> Option<Duration> {
        let minutes = self.settings.sleep_minutes;
        if minutes == 0 {
            return None;
        }
        let limit = Duration::from_secs(u64::from(minutes) * 60);
        Some(if system::memory::is_low() {
            limit.min(sleep::LOW_MEMORY_IDLE)
        } else {
            limit
        })
    }

    /// Усыпляет вкладки, которые не открывали дольше `limit`.
    fn sleep_idle_tabs(&mut self, limit: Option<Duration>) {
        let Some(limit) = limit else {
            return;
        };
        let idle: Vec<TabId> = self
            .sleep_candidates()
            .into_iter()
            .filter(|(seen, _)| seen.elapsed() >= limit)
            .map(|(_, id)| id)
            .collect();
        for id in idle {
            self.request_sleep(id);
        }
    }

    /// Держит загруженными не больше `live_tabs` фоновых вкладок:
    /// лишние, которые открывали давнее всего, засыпают сразу.
    fn limit_live_tabs(&mut self) {
        let keep = self.settings.live_tabs as usize;
        if keep == 0 {
            return;
        }
        for id in sleep::excess(self.sleep_candidates(), keep) {
            self.request_sleep(id);
        }
    }

    /// Замораживает фоновые вкладки, на которые не возвращались `SUSPEND_AFTER`,
    /// и спрятанную панель ИИ. Вкладки, которые ещё грузятся, не трогаем.
    fn suspend_hidden(&self) {
        let active = self.tabs.active;
        for tab in self.tabs.iter() {
            // Вкладку с идущей загрузкой не трогаем, как и при усыплении.
            let hidden = Some(tab.id) != active
                && !tab.loading
                && tab.sleep_requested.is_none()
                && tab.last_active.elapsed() >= sleep::SUSPEND_AFTER
                && !self.downloads.is_busy(tab.id);
            if let Some(webview) = tab.webview.as_ref().filter(|_| hidden) {
                tab_webview::suspend(webview);
            }
        }
        if !self.downloads.busy_untracked() {
            self.ai.suspend_if_hidden(sleep::SUSPEND_AFTER);
        }
    }

    /// Загруженные фоновые вкладки, которые можно усыпить: без звука,
    /// без недописанного текста, без идущих загрузок и без уже отправленного запроса.
    fn sleep_candidates(&self) -> Vec<(Instant, TabId)> {
        let active = self.tabs.active;
        self.tabs
            .iter()
            .filter(|t| Some(t.id) != active && t.sleep_requested.is_none() && !t.keep_alive)
            .filter(|t| !self.downloads.is_busy(t.id))
            .filter(|t| {
                t.webview
                    .as_ref()
                    .is_some_and(|w| !tab_webview::is_playing_audio(w))
            })
            .map(|t| (t.last_active, t.id))
            .collect()
    }

    /// Спрашивает страницу о прокрутке и недописанном тексте; ответ придёт в `finish_sleep`.
    fn request_sleep(&mut self, id: TabId) {
        let events = self.events.clone();
        let Some(tab) = self.tabs.get_mut(id) else {
            return;
        };
        let Some(webview) = &tab.webview else {
            return;
        };
        // Замороженная страница не выполнит скрипт, пока её не разморозить.
        tab_webview::resume(webview);
        let asked = webview.evaluate_script_with_callback(sleep::PROBE, move |raw| {
            let _ = events.send_event(UserEvent::SleepChecked(id, PageState::parse(&raw)));
        });
        if asked.is_ok() {
            tab.sleep_requested = Some(Instant::now());
        } else {
            tab.webview = None;
            self.push_state();
        }
    }

    /// Страница не ответила за целый тик (зависла) — усыпляем без позиции прокрутки.
    fn drop_unanswered(&mut self) {
        let mut changed = false;
        let downloads = &self.downloads;
        for tab in self.tabs.iter_mut() {
            if !tab.sleep_requested.is_some_and(|at| at.elapsed() >= TICK) {
                continue;
            }
            tab.sleep_requested = None;
            // Пока ждали ответа, в странице началась загрузка — не усыпляем.
            if !downloads.is_busy(tab.id) {
                tab.webview = None;
                changed = true;
            }
        }
        if changed {
            self.push_state();
        }
    }

    fn finish_sleep(&mut self, id: TabId, page: PageState) {
        let active = self.tabs.active;
        let Some(tab) = self.tabs.get_mut(id) else {
            return;
        };
        if Some(id) == active || tab.sleep_requested.is_none() {
            return;
        }
        tab.sleep_requested = None;
        // Пока страница отвечала, в ней началась загрузка.
        if self.downloads.is_busy(id) {
            return;
        }
        if page.keep {
            // Недописанный текст: вкладка остаётся, вместо неё усыпим следующую по давности.
            tab.keep_alive = true;
            self.limit_live_tabs();
            return;
        }
        tab.webview = None;
        tab.scroll = (page.y.is_finite() && page.y > 0.0).then_some(page.y);
        self.push_state();
    }

    // ---------- сохранение ----------

    fn save_session(&mut self) {
        let active = self
            .tabs
            .active
            .and_then(|id| self.tabs.index_of(id))
            .unwrap_or(0);
        let session = Session {
            tabs: self
                .tabs
                .iter()
                .map(|t| SavedTab {
                    url: t.url.clone(),
                    title: t.title.clone(),
                    ai: t.ai,
                })
                .collect(),
            active,
        };
        match session.save(&self.paths) {
            Ok(()) => self.session_dirty = false,
            Err(err) => eprintln!("сессия не сохранена: {err:#}"),
        }
    }

    pub fn shutdown(&mut self) {
        self.save_session();
        let _ = self.history.save_if_dirty(&self.paths);
        let _ = self.downloads.save_if_dirty(&self.paths);
        self.persist_settings();
    }

    // ---------- связь с интерфейсом ----------

    fn push_state(&self) {
        let tabs = self
            .tabs
            .iter()
            .map(|t| TabView {
                id: t.id,
                title: &t.title,
                url: &t.url,
                loading: t.loading,
                sleeping: t.is_sleeping(),
                ai: t.ai,
                bookmarked: self.bookmarks.contains(&t.url),
            })
            .collect();
        let on_newtab = self.tabs.active_tab().is_some_and(|t| t.is_newtab());
        self.send(&ToUi::State {
            tabs,
            active: self.tabs.active,
            ai_open: self.ai.open,
            ai_proxy: self.ai.has_proxy(),
            can_restore: self.tabs.can_restore(),
            top_sites: if on_newtab {
                self.history.top_sites(8)
            } else {
                Vec::new()
            },
        });
    }

    fn send(&self, message: &ToUi<'_>) {
        match serde_json::to_string(message) {
            Ok(json) => {
                let js = format!("window.axiom&&window.axiom.recv({json})");
                if let Err(err) = self.ui.evaluate_script(&js) {
                    eprintln!("интерфейс недоступен: {err}");
                }
            }
            Err(err) => eprintln!("не удалось сериализовать сообщение: {err}"),
        }
    }

    fn look(&self) -> Look {
        Look {
            background: self.page_bg(),
            theme: self.settings.colors.theme(),
            force_dark: self.force_dark,
        }
    }

    /// Сайты сразу получают тему Axiom через `prefers-color-scheme`.
    fn apply_site_theme(&self) {
        let theme = self.settings.colors.theme();
        #[cfg(windows)]
        {
            tab_webview::set_theme(&self.ui, theme);
            for webview in self.tabs.iter().filter_map(|t| t.webview.as_ref()) {
                tab_webview::set_theme(webview, theme);
            }
            if let Some(webview) = &self.ai.webview {
                tab_webview::set_theme(webview, theme);
            }
        }
        #[cfg(target_os = "linux")]
        tab_webview::apply_theme_globally(theme);
        #[cfg(not(any(windows, target_os = "linux")))]
        let _ = theme;
    }

    fn page_bg(&self) -> RGBA {
        ipc::parse_hex(&self.settings.colors.page).unwrap_or((25, 25, 30, 255))
    }
}

fn build_window(event_loop: &EventLoop<UserEvent>) -> Result<Window> {
    let builder = WindowBuilder::new()
        .with_title("Axiom")
        .with_inner_size(LogicalSize::new(1280.0, 820.0))
        .with_min_inner_size(LogicalSize::new(560.0, 380.0))
        .with_decorations(false)
        .with_visible(false);
    #[cfg(windows)]
    let builder = {
        use tao::platform::windows::{IconExtWindows, WindowBuilderExtWindows};
        // Иконка встроена в exe как ресурс №1 (см. build.rs).
        let icon = || tao::window::Icon::from_resource(1, None).ok();
        // with_window_icon задаёт только ICON_SMALL. Без ICON_BIG панель задач берёт
        // иконку exe из кэша значков Windows, и после обновления там остаётся старый логотип.
        builder
            .with_undecorated_shadow(true)
            .with_window_icon(icon())
            .with_taskbar_icon(icon())
    };
    builder.build(event_loop).context("не удалось создать окно")
}

/// Запрос из адреса результатов DuckDuckGo.
fn ddg_query(url: &str) -> Option<String> {
    let u = Url::parse(url).ok()?;
    let host = u.host_str()?;
    if host != "duckduckgo.com" && !host.ends_with(".duckduckgo.com") {
        return None;
    }
    u.query_pairs()
        .find(|(k, _)| k == "q")
        .map(|(_, v)| v.trim().to_string())
        .filter(|q| !q.is_empty())
}

fn build_ui(
    window: &Window,
    context: &mut WebContext,
    events: &EventLoopProxy<UserEvent>,
    look: Look,
) -> Result<WebView> {
    let ipc_events = events.clone();
    let builder = WebViewBuilder::new_with_web_context(context)
        .with_custom_protocol("axiom".into(), |_id, request| ui::serve(&request))
        .with_url(ui::url())
        .with_bounds(full_bounds(window))
        .with_devtools(cfg!(debug_assertions))
        .with_hotkeys_zoom(false)
        .with_navigation_handler(|url| ui::is_ui_origin(&url))
        .with_new_window_req_handler(|_, _| NewWindowResponse::Deny)
        .with_ipc_handler(move |request| {
            // Принимаем сообщения только со страницы интерфейса.
            if ui::is_ui_origin(&request.uri().to_string()) {
                let _ = ipc_events.send_event(UserEvent::Ui(request.into_body()));
            }
        });
    #[cfg(windows)]
    let builder = {
        use wry::WebViewBuilderExtWindows;
        builder
            .with_https_scheme(true)
            .with_browser_accelerator_keys(false)
            .with_default_context_menus(cfg!(debug_assertions))
    };
    let builder = tab_webview::styled(builder, look, None);
    // На Linux обработчик загрузок общий для всех webview, а путь выбирает первый
    // зарегистрированный — интерфейс создаётся раньше вкладок.
    let builder = downloads::attach(builder, events);
    tab_webview::build_child(builder, window).context("не удалось создать интерфейс")
}

fn full_bounds(window: &Window) -> Rect {
    let size = window.inner_size().to_logical::<f64>(window.scale_factor());
    Rect {
        position: LogicalPosition::new(0.0, 0.0).into(),
        size: LogicalSize::new(size.width, size.height).into(),
    }
}

fn to_rect(r: ipc::Rect) -> Rect {
    Rect {
        position: LogicalPosition::new(r.x.max(0.0), r.y.max(0.0)).into(),
        size: LogicalSize::new(r.w, r.h).into(),
    }
}

fn resize_direction(edge: Edge) -> ResizeDirection {
    match edge {
        Edge::N => ResizeDirection::North,
        Edge::S => ResizeDirection::South,
        Edge::E => ResizeDirection::East,
        Edge::W => ResizeDirection::West,
        Edge::Ne => ResizeDirection::NorthEast,
        Edge::Nw => ResizeDirection::NorthWest,
        Edge::Se => ResizeDirection::SouthEast,
        Edge::Sw => ResizeDirection::SouthWest,
    }
}

/// Пока страница не прислала заголовок — показываем домен.
fn display_title(url: &str) -> String {
    Url::parse(url)
        .ok()
        .and_then(|u| {
            u.host_str()
                .map(|h| h.trim_start_matches("www.").to_string())
        })
        .unwrap_or_else(|| url.to_string())
}
