//! Состояние браузера и обработка событий главного цикла.

mod tab_actions;
mod translation;

use std::{
    path::Path,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use tao::{
    dpi::{LogicalPosition, LogicalSize},
    event_loop::{EventLoop, EventLoopProxy},
    window::{Fullscreen, ResizeDirection, Window, WindowBuilder},
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
        history::History,
        import::{self, Browser},
        session::{SavedTab, Session},
    },
    system,
    tabs::{
        TabId, TabStore,
        fullscreen::{self, Owner},
        omnibox::{self, NEWTAB},
        sleep::{self, PageState},
        snapshot::{self, Part, Snapshots},
        webview::{self as tab_webview, Look, TabSpec},
    },
    ui::{self, hotkeys::Hotkey},
    update::{UpdateEvent, Updater},
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
    /// Снимок страницы на месте спрятанных webview, чтобы под окном не было пустого фона.
    snapshot: Snapshots,
    shown: bool,
    session_dirty: bool,
    /// Последний запрос, отправленный в AI Mode (чтобы не грузить его дважды).
    last_ai_query: String,
    importing: bool,
    /// Принудительно тёмные сайты: аргумент запуска WebView2, меняется только перезапуском.
    force_dark: bool,
    updater: Updater,
    /// Запущены сразу после обновления: скажем об этом, когда интерфейс будет готов.
    just_updated: bool,
    /// `shutdown` уже сохранил состояние.
    closed: bool,
    /// Цвета темы поменялись, а настройки ещё не записаны.
    settings_dirty: bool,
    /// Что последним сообщили интерфейсу о развёрнутости окна.
    maximized: Option<bool>,
    /// Перевод страниц во вкладках.
    translate: crate::translate::Translator,
    /// Видео или страница (F11) на весь экран.
    fullscreen: fullscreen::State,
}

impl App {
    pub fn new(
        event_loop: &EventLoop<UserEvent>,
        paths: Paths,
        open: &[String],
        just_updated: bool,
    ) -> Result<Self> {
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
            snapshot: Snapshots::default(),
            shown: false,
            session_dirty: false,
            last_ai_query: String::new(),
            importing: false,
            force_dark,
            updater: Updater::new(),
            just_updated,
            closed: false,
            settings_dirty: false,
            maximized: None,
            translate: crate::translate::Translator::default(),
            fullscreen: fullscreen::State::default(),
        };
        app.restore_session(open);
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
                Err(err) => {
                    eprintln!("IPC отклонён: {err}");
                    if let Some(text) = err.user_text() {
                        self.send(&ToUi::Toast { text });
                    }
                }
            },
            UserEvent::Hotkey(hotkey) => self.on_hotkey(hotkey)?,
            UserEvent::TabTitle(id, title) => self.on_title(id, title),
            UserEvent::TabLoad { id, started, url } => self.on_load(id, started, url),
            UserEvent::OpenTab { url, ai, from } => self.open_from_page(url, ai, from),
            UserEvent::PageTab {
                url,
                ai,
                from,
                background,
            } => self.add_page_tab(url, ai, from, background),
            UserEvent::TabAudio { id, playing } => self.on_tab_audio(id, playing),
            UserEvent::SleepChecked(id, page) => self.finish_sleep(id, page),
            UserEvent::FindResult(found) => self.send(&ToUi::Find { found }),
            UserEvent::Fullscreen { owner, on } => self.on_page_fullscreen(owner, on),
            UserEvent::Download(event) => self.on_download(event),
            UserEvent::Update(event) => return Ok(self.on_update(event)),
            UserEvent::External(urls) => {
                self.open_external(&urls);
                self.window.set_minimized(false);
                if !crate::tabs::webview::test_mode() {
                    self.window.set_visible(true);
                    self.window.set_focus();
                }
            }
            UserEvent::Installed(result) => {
                self.send(&ToUi::Installed {
                    ok: result.is_ok(),
                    error: result.as_ref().err().map_or("", String::as_str),
                });
                self.send(&ToUi::System {
                    status: system::status(),
                });
            }
            UserEvent::Imported(browser, result) => self.finish_import(browser, result),
            UserEvent::Translate(event) => self.on_translate(event),
            UserEvent::ShowFallback => self.show_window(),
            UserEvent::Snapshot(event) => {
                let step = self.snapshot.on_event(event, &self.events);
                self.snapshot_step(step);
            }
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
                self.push_update(false);
                self.push_window_state();
                self.push_translate_all();
                self.show_window();
                if std::mem::take(&mut self.just_updated) {
                    let text = format!("Axiom обновлён до версии {}", env!("CARGO_PKG_VERSION"));
                    self.send(&ToUi::Toast { text: &text });
                }
                // Ссылка на файл при запуске: загрузка могла начаться раньше, чем
                // интерфейс загрузился, и её сообщение не дошло.
                if let Some(d) = self.downloads.iter().find(|d| d.status == Status::Active) {
                    let text = if d.ask {
                        format!("Файл может быть опасен: {}", d.name())
                    } else {
                        format!("Скачивается {}", d.name())
                    };
                    self.send(&ToUi::Toast { text: &text });
                }
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
            UiCmd::Overlay { open } => self.set_overlay(open),
            UiCmd::SnapshotShown { seq } => {
                if self.snapshot.shown(seq) {
                    self.apply_layout();
                }
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
            UiCmd::ToggleNoSleep { id } => self.toggle_no_sleep(id),
            UiCmd::ReloadTab { id } => self.reload_tab(id),
            UiCmd::DuplicateTab { id } => self.duplicate_tab(id),
            UiCmd::ToggleMute { id } => self.toggle_mute(id),
            UiCmd::CloseOtherTabs { id } => self.close_other_tabs(id),
            UiCmd::CloseTabsRight { id } => self.close_tabs_right(id),
            UiCmd::Import { browser } => self.start_import(browser),
            UiCmd::Install => {
                let events = self.events.clone();
                std::thread::spawn(move || {
                    let result = system::install().map(drop).map_err(|e| format!("{e:#}"));
                    let _ = events.send_event(UserEvent::Installed(result));
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
                    // Ползунок цвета шлёт это каждый кадр: на диск — по таймеру и при выходе.
                    self.settings_dirty = true;
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
            UiCmd::DownloadKeep { id } => downloads::keep(id),
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
            UiCmd::UpdateCheck => {
                if self.updater.check(&self.events, true) {
                    self.push_update(false);
                }
            }
            UiCmd::UpdateInstall => {
                if self.updater.install(&self.events) {
                    self.push_update(false);
                }
            }
            UiCmd::Translate => self.translate_active(),
            UiCmd::TranslateOriginal => self.translate_original(),
            UiCmd::TranslatePrefs { prefs } => self.translate_prefs(prefs),
        }
        Ok(Flow::Continue)
    }

    fn on_hotkey(&mut self, hotkey: Hotkey) -> Result<()> {
        if self.fullscreen.leaves_on(hotkey) {
            self.leave_fullscreen();
        }
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
            Hotkey::Fullscreen => self.toggle_fullscreen(),
            Hotkey::Escape => {
                if self.fullscreen.leaves_on_escape() {
                    self.leave_fullscreen();
                }
            }
            _ if hotkey.is_page_tool() => self.page_hotkey(hotkey),
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

    /// `open` — адреса из командной строки (клик по ссылке в другой программе).
    fn restore_session(&mut self, open: &[String]) {
        let mut restored = None;
        if let Some(session) = Session::load(&self.paths) {
            for (i, saved) in session.tabs.iter().enumerate() {
                let id = self
                    .tabs
                    .add(saved.url.clone(), saved.title.clone(), saved.ai, Some(i));
                if let Some(tab) = self.tabs.get_mut(id) {
                    tab.no_sleep = saved.no_sleep;
                }
                if i == session.active {
                    restored = Some(id);
                }
            }
        }
        // Адреса снаружи встают после активной вкладки сессии, а она сама
        // остаётся спящей: незачем грузить страницу, которую сразу закроет другая.
        self.tabs.active = restored;
        self.open_external(open);
        if self.tabs.active == restored {
            let id = restored
                .unwrap_or_else(|| self.tabs.add(NEWTAB.into(), String::new(), false, None));
            // Остальные вкладки остаются спящими до первого клика.
            self.activate(id);
        }
    }

    fn activate(&mut self, id: TabId) {
        if let Some(prev) = self.tabs.active.filter(|&p| p != id)
            && let Some(tab) = self.tabs.get_mut(prev)
        {
            tab.last_active = Instant::now();
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
        let covered = self.covered();
        if let Some(tab) = self.tabs.get_mut(id) {
            tab.last_active = Instant::now();
            tab.sleep_requested = None;
            tab.keep_alive = false;
            is_newtab = tab.is_newtab();
            if let Some(webview) = &tab.webview {
                if let Some(rect) = self.content {
                    let _ = webview.set_bounds(rect);
                }
                let _ = webview.set_visible(!covered);
            }
        }
        if is_newtab {
            let _ = self.ui.focus();
        }
        if self.fullscreen.is_on() {
            self.sync_fullscreen();
        }
        self.limit_live_tabs();
        self.session_dirty = true;
        self.push_state();
        // Под открытым окном был снимок прежней вкладки.
        if self.overlay {
            let targets = self.snapshot_targets();
            let step = self.snapshot.retake(targets, &self.events);
            self.snapshot_step(step);
        }
    }

    /// Создаёт webview для спящей вкладки.
    fn wake(&mut self, id: TabId) -> Result<()> {
        let Some(tab) = self.tabs.get(id) else {
            return Ok(());
        };
        if tab.webview.is_some() || tab.is_newtab() {
            return Ok(());
        }
        let (url, ai, muted) = (tab.url.clone(), tab.ai, tab.muted);
        let spec = TabSpec {
            id,
            url: &url,
            bounds: self.content.unwrap_or_default(),
            visible: self.tabs.active == Some(id) && !self.covered(),
            look: self.look(),
            proxy: if ai { self.ai.proxy_config() } else { None },
            ai,
            muted,
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
            // Звук прежней страницы до усыпления к новой не относится.
            tab.audible = false;
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
            tab.loading = false;
            tab.scroll = None;
            tab.sleep_requested = None;
            tab.keep_alive = false;
            self.session_dirty = true;
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
        self.close_tab_then(id, remember, None);
    }

    /// `prefer` — какую вкладку открыть вместо закрытой, если она ещё есть.
    fn close_tab_then(&mut self, id: TabId, remember: bool, prefer: Option<TabId>) {
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
            let next = prefer
                .filter(|&p| self.tabs.get(p).is_some())
                .or_else(|| self.tabs.successor_for(index));
            if let Some(next) = next {
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
                if let Some(tab) = self.tabs.get_mut(id) {
                    tab.no_sleep = closed.no_sleep;
                }
                self.activate(id);
            }
            None => self.send(&ToUi::Toast {
                text: "Нет закрытых вкладок",
            }),
        }
    }

    fn on_title(&mut self, id: TabId, title: String) {
        // Вкладка без webview: событие от уже выгруженной или припаркованной страницы.
        let Some(tab) = self.tabs.get_mut(id).filter(|t| t.webview.is_some()) else {
            return;
        };
        let title: String = title.chars().take(300).collect();
        // Адрес мог смениться без перезагрузки (history.pushState).
        if let Some(url) = tab.webview.as_ref().and_then(|w| w.url().ok())
            && (omnibox::is_navigable(&url) || crate::tabs::tools::is_source(&url))
            && url != "about:blank"
        {
            tab.url = crate::tabs::tools::keep_source(&tab.url, url);
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
        let Some(tab) = self.tabs.get_mut(id).filter(|t| t.webview.is_some()) else {
            return;
        };
        tab.loading = started;
        let blank = url == "about:blank";
        if (omnibox::is_navigable(&url) || crate::tabs::tools::is_source(&url)) && !blank {
            tab.url = crate::tabs::tools::keep_source(&tab.url, url);
        }
        // Исходный код (Ctrl+U) — тоже адрес вкладки, но не страница: без истории и перевода.
        let source = crate::tabs::tools::is_source(&tab.url);
        if !started {
            // Вкладка, открытая ради загрузки, страницу так и не показала — в историю её не пишем.
            if !blank && !source {
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
        if !source {
            self.translate_load(id, started);
        }
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
        // Через раскладку: поверх страницы на весь экран панель не показываем.
        self.apply_layout();
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
            Status::Active if d.ask => Some(format!("Файл может быть опасен: {}", d.name())),
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
            // Возвращаемся туда, где кликнули по ссылке, а не на соседнюю справа.
            let opener = self.tabs.get(id).and_then(|t| t.opener);
            self.close_tab_then(id, false, opener);
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
                ask: d.ask,
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

    /// Импорт читает файлы браузера в фоновом потоке, чтобы окно не подвисало.
    /// По одному браузеру за раз: интерфейс сам не даёт начать второй.
    fn start_import(&mut self, browser: Browser) {
        if self.importing {
            return;
        }
        self.importing = true;
        let (paths, events) = (self.paths.clone(), self.events.clone());
        std::thread::spawn(move || {
            let result = import::import(browser, &paths).map_err(|e| format!("{e:#}"));
            let _ = events.send_event(UserEvent::Imported(browser, result));
        });
    }

    fn finish_import(&mut self, browser: Browser, result: Result<import::Imported, String>) {
        self.importing = false;
        match result {
            Ok(data) => {
                let bookmarks = self.bookmarks.merge(data.bookmarks);
                let history = self.history.merge(data.history);
                let _ = self.bookmarks.save(&self.paths);
                let _ = self.history.save_if_dirty(&self.paths);
                self.send(&ToUi::Imported {
                    ok: true,
                    browser,
                    name: browser.name(),
                    profile: &data.profile,
                    bookmarks,
                    history,
                    error: "",
                });
                self.push_state();
            }
            Err(error) => self.send(&ToUi::Imported {
                ok: false,
                browser,
                name: browser.name(),
                profile: "",
                bookmarks: 0,
                history: 0,
                error: &error,
            }),
        }
    }

    /// Адреса из командной строки: ссылки из других программ, файлы .html.
    fn open_external(&mut self, args: &[String]) {
        // Ссылка может оказаться файлом: тогда её пустая вкладка закроется
        // и вернёт туда, где был пользователь (при запуске — на вкладку из сессии).
        let from = self.tabs.active;
        for arg in args {
            let path = std::path::Path::new(arg);
            let url = if path.is_absolute() && path.exists() {
                Url::from_file_path(path).ok().map(String::from)
            } else {
                omnibox::resolve(arg).map(omnibox::Target::into_url)
            };
            if let Some(url) = url.filter(|u| u != NEWTAB) {
                let id = self.tabs.add(url.clone(), display_title(&url), false, None);
                if let Some(tab) = self.tabs.get_mut(id) {
                    tab.opener = from;
                }
                self.activate(id);
            }
        }
    }

    // ---------- настройки ----------

    fn save_settings(&mut self, mut incoming: ipc::SettingsIn) {
        let old_proxy = self.settings.ai_proxy.clone();
        let rejected = match proxy::parse(&incoming.ai_proxy) {
            Ok(_) => None,
            Err(err) => {
                incoming.ai_proxy = old_proxy.clone();
                Some(format!("Прокси не применён: {err}"))
            }
        };
        self.settings.apply(incoming);
        if self.settings.ai_proxy != old_proxy {
            self.apply_proxy_change();
        }
        self.limit_live_tabs();
        self.persist_settings();
        if let Some(text) = rejected {
            // Интерфейс держит отклонённый адрес у себя и слал бы его с каждым
            // сохранением — возвращаем ему настоящие настройки.
            self.send(&ToUi::Settings {
                settings: &self.settings,
            });
            self.send(&ToUi::Toast { text: &text });
        }
    }

    fn apply_proxy_change(&mut self) {
        let was_open = self.ai.open;
        self.ai.set_proxy(&self.settings.ai_proxy);
        // Вкладки «из ИИ» пересоздадутся с новым прокси при следующем открытии.
        // Вкладки с недописанным текстом и «не усыплять» не трогаем (сменят прокси,
        // когда уснут), а идущую загрузку не обрываем — webview паркуется до её конца.
        let active = self.tabs.active;
        let ids: Vec<TabId> = self
            .tabs
            .iter()
            .filter(|t| t.ai && Some(t.id) != active && t.webview.is_some() && !t.keep_alive)
            .filter(|t| !t.no_sleep)
            .map(|t| t.id)
            .collect();
        for id in ids {
            self.park_if_busy(id);
            if let Some(tab) = self.tabs.get_mut(id) {
                tab.webview = None;
                tab.sleep_requested = None;
            }
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

    // ---------- полный экран ----------

    /// Страница развернула элемент на весь экран или свернула его (в том числе по Esc).
    fn on_page_fullscreen(&mut self, owner: Owner, on: bool) {
        if !on {
            if self.fullscreen.page == Some(owner) {
                self.fullscreen.page = None;
                self.sync_fullscreen();
            }
            return;
        }
        let shown = match owner {
            Owner::Tab(id) => self.tabs.active == Some(id),
            Owner::Ai => self.ai.open,
        };
        if !shown {
            // Невидимая страница не может занять собой весь экран.
            self.exit_page(owner);
            return;
        }
        if let Some(old) = self.fullscreen.page.filter(|&o| o != owner) {
            self.exit_page(old);
        }
        self.fullscreen.page = Some(owner);
        self.sync_fullscreen();
    }

    /// F11: включает режим или выходит из любого полного экрана.
    fn toggle_fullscreen(&mut self) {
        if self.fullscreen.is_on() {
            self.leave_fullscreen();
            return;
        }
        if self.tabs.active_tab().is_some_and(|t| t.webview.is_some()) {
            self.fullscreen.browser = true;
            self.sync_fullscreen();
        } else {
            self.send(&ToUi::Toast {
                text: "Откройте сайт, чтобы развернуть его на весь экран",
            });
        }
    }

    fn leave_fullscreen(&mut self) {
        if let Some(owner) = self.fullscreen.page.take() {
            self.exit_page(owner);
        }
        self.fullscreen.browser = false;
        self.sync_fullscreen();
    }

    fn exit_page(&self, owner: Owner) {
        let webview = match owner {
            Owner::Tab(id) => self.tabs.get(id).and_then(|t| t.webview.as_ref()),
            Owner::Ai => self.ai.webview.as_ref(),
        };
        if let Some(webview) = webview {
            fullscreen::exit_page(webview);
        }
    }

    /// Приводит окно и раскладку к `self.fullscreen`. Страница, которую больше
    /// не видно (переключили или закрыли вкладку, спрятали панель ИИ), выходит
    /// из полного экрана: окно не должно остаться без рамки и без интерфейса.
    fn sync_fullscreen(&mut self) {
        let visible = match self.fullscreen.page {
            Some(Owner::Tab(id)) => {
                self.tabs.active == Some(id)
                    && self.tabs.get(id).is_some_and(|t| t.webview.is_some())
            }
            Some(Owner::Ai) => self.ai.open && self.ai.webview.is_some(),
            None => true,
        };
        if !visible && let Some(owner) = self.fullscreen.page.take() {
            self.exit_page(owner);
        }
        // F11 растягивает страницу, а новая вкладка — это сам интерфейс.
        if self.fullscreen.browser && !self.tabs.active_tab().is_some_and(|t| t.webview.is_some()) {
            self.fullscreen.browser = false;
        }
        let on = self.fullscreen.is_on();
        let entering = on && self.window.fullscreen().is_none();
        self.window
            .set_fullscreen(on.then_some(Fullscreen::Borderless(None)));
        self.apply_layout();
        if entering {
            // Фокус мог остаться в спрятанном интерфейсе — тогда Esc и клавиши
            // уходили бы туда, а не странице.
            let webview = match self.fullscreen.page {
                Some(Owner::Ai) => self.ai.webview.as_ref(),
                _ => self.tabs.active_tab().and_then(|t| t.webview.as_ref()),
            };
            if let Some(webview) = webview {
                let _ = webview.focus();
            }
        }
    }

    // ---------- раскладка и окно ----------

    fn apply_layout(&self) {
        // На весь экран: страница на всё окно поверх интерфейса, остальное спрятано.
        let full = self.fullscreen.is_on().then(|| full_bounds(&self.window));
        let ai_full = self.fullscreen.page == Some(Owner::Ai);
        let covered = self.covered();
        if let Some(webview) = self.tabs.active_tab().and_then(|t| t.webview.as_ref()) {
            if let Some(rect) = full.or(self.content) {
                let _ = webview.set_bounds(rect);
            }
            let visible = if full.is_some() { !ai_full } else { !covered };
            let _ = webview.set_visible(visible);
        }
        match full {
            Some(rect) => self.ai.place(ai_full.then_some(rect), false),
            None => self.ai.place(self.ai_rect, covered),
        }
    }

    /// Webview спрятаны под окном интерфейса: окно открыто, и снимок уже
    /// на их месте (или его не будет).
    fn covered(&self) -> bool {
        self.overlay && !self.snapshot.holding()
    }

    /// Окно интерфейса поверх контента открылось или закрылось.
    fn set_overlay(&mut self, open: bool) {
        self.overlay = open;
        if open {
            // Webview прячутся, когда интерфейс положит на их место снимок.
            let targets = self.snapshot_targets();
            let step = self.snapshot.begin(targets, &self.events);
            self.snapshot_step(step);
        } else {
            // Сначала webview снова на экране, потом интерфейс убирает снимок.
            let view = self.snapshot.end();
            self.apply_layout();
            self.send(&ToUi::Snapshot(view));
        }
    }

    fn snapshot_step(&self, step: snapshot::Step) {
        match step {
            snapshot::Step::Wait => {}
            snapshot::Step::Show(view) => self.send(&ToUi::Snapshot(view)),
            snapshot::Step::Hide(view) => {
                self.apply_layout();
                if let Some(view) = view {
                    self.send(&ToUi::Snapshot(view));
                }
            }
        }
    }

    /// Нативные webview, которые сейчас на экране: страница активной вкладки и панель ИИ.
    fn snapshot_targets(&self) -> Vec<snapshot::Target> {
        let page = self
            .tabs
            .active_tab()
            .and_then(|t| snapshot::target(t.webview.as_ref()?, Part::Page, Some(t.id)));
        let ai = self
            .ai
            .webview
            .as_ref()
            .filter(|_| self.ai.open && self.ai_rect.is_some())
            .and_then(|w| snapshot::target(w, Part::Ai, None));
        page.into_iter().chain(ai).collect()
    }

    pub fn on_resize(&mut self) {
        let _ = self.ui.set_bounds(full_bounds(&self.window));
        // Окно разворачивается на монитор не сразу, а размер монитора может смениться.
        if self.fullscreen.is_on() {
            self.apply_layout();
        }
        // Resized приходит на каждый шаг перетаскивания рамки — шлём только перемену.
        if self.maximized != Some(self.window.is_maximized()) {
            self.push_window_state();
        }
    }

    fn show_window(&mut self) {
        if !self.shown {
            self.shown = true;
            if crate::tabs::webview::test_mode() {
                show_inactive(&self.window);
            } else {
                self.window.set_visible(true);
                self.window.set_focus();
            }
        }
    }

    fn push_window_state(&mut self) {
        let maximized = self.window.is_maximized();
        self.maximized = Some(maximized);
        self.send(&ToUi::Window { maximized });
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
        self.updater.tick(self.settings.check_updates, &self.events);
        // Страховка: webview страницы на весь экран мог пропасть без нашего ведома.
        if self.fullscreen.is_on() {
            self.sync_fullscreen();
        }
        self.drop_unanswered();
        let idle = self.idle_limit();
        self.sleep_idle_tabs(idle);
        self.limit_live_tabs();
        // Скачивание из панели ИИ прервалось бы вместе с её webview.
        if !self.downloads.busy_untracked() {
            self.ai.sleep_if_idle(idle);
        }
        if self.session_dirty {
            self.save_session();
        }
        if std::mem::take(&mut self.settings_dirty) {
            self.persist_settings();
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
        let candidates = self.sleep_candidates();
        // Вкладки, которые усыплять нельзя (черновик, звук, загрузка, «не усыплять»), всё
        // равно живые и занимают место в лимите: вместо них засыпают следующие по давности.
        let active = self.tabs.active;
        let live = self
            .tabs
            .iter()
            .filter(|t| Some(t.id) != active && t.webview.is_some() && t.sleep_requested.is_none())
            .count();
        let keep = sleep::room(keep, live, candidates.len());
        for id in sleep::excess(candidates, keep) {
            self.request_sleep(id);
        }
    }

    /// Загруженные фоновые вкладки, которые можно усыпить: без звука, без недописанного
    /// текста, без идущих загрузок, без «не усыплять» и без уже отправленного запроса.
    fn sleep_candidates(&self) -> Vec<(Instant, TabId)> {
        let active = self.tabs.active;
        self.tabs
            .iter()
            .filter(|t| Some(t.id) != active && t.may_sleep())
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

    /// «Не усыплять»: вкладку не усыпят ни таймер, ни нехватка памяти, ни лимит живых вкладок.
    fn toggle_no_sleep(&mut self, id: TabId) {
        let Some(tab) = self.tabs.get_mut(id).filter(|t| !t.is_newtab()) else {
            return;
        };
        tab.no_sleep = !tab.no_sleep;
        // Запрос на усыпление мог уже уйти: без него ответ страницы ничего не сделает.
        if tab.no_sleep {
            tab.sleep_requested = None;
        }
        let text = if tab.no_sleep {
            "Вкладка не будет засыпать"
        } else {
            "Вкладка снова может засыпать"
        };
        self.session_dirty = true;
        self.send(&ToUi::Toast { text });
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
                    no_sleep: t.no_sleep,
                })
                .collect(),
            active,
        };
        match session.save(&self.paths) {
            Ok(()) => self.session_dirty = false,
            Err(err) => eprintln!("сессия не сохранена: {err:#}"),
        }
    }

    /// Сохраняет всё на диск. Повторный вызов (после `LoopDestroyed`) ничего не делает.
    pub fn shutdown(&mut self) {
        if std::mem::replace(&mut self.closed, true) {
            return;
        }
        self.save_session();
        let _ = self.history.save_all(&self.paths);
        let _ = self.downloads.save_if_dirty(&self.paths);
        self.persist_settings();
    }

    // ---------- обновления ----------

    fn on_update(&mut self, event: UpdateEvent) -> Flow {
        match event {
            UpdateEvent::Checked(latest) => {
                let manual = self.updater.on_checked(latest);
                self.push_update(manual);
            }
            UpdateEvent::Progress(received) => {
                self.updater.on_progress(received);
                self.push_update(false);
            }
            UpdateEvent::Downloaded(result) => {
                let restart = self.updater.on_downloaded(result);
                self.push_update(false);
                if restart {
                    // Новая версия уже запущена и ждёт, пока этот процесс закроется.
                    return Flow::Exit;
                }
            }
        }
        Flow::Continue
    }

    fn push_update(&self, manual: bool) {
        let busy = self.downloads.iter().any(|d| d.status == Status::Active);
        self.send(&ToUi::Update(self.updater.view(manual, busy)));
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
                no_sleep: t.no_sleep,
                // У спящей вкладки звука нет, но выключенный звук она помнит.
                audible: t.audible && t.webview.is_some(),
                muted: t.muted,
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

/// Режим автотестов: окно появляется, не становясь активным (`SW_SHOWNOACTIVATE`).
#[cfg(windows)]
fn show_inactive(window: &Window) {
    use tao::platform::windows::WindowExtWindows;
    #[link(name = "user32")]
    unsafe extern "system" {
        fn ShowWindow(hwnd: isize, cmd: i32) -> i32;
    }
    // SAFETY: hwnd принадлежит живому окну tao.
    unsafe { ShowWindow(window.hwnd(), 4) };
}

#[cfg(not(windows))]
fn show_inactive(window: &Window) {
    window.set_visible(true);
}

fn build_window(event_loop: &EventLoop<UserEvent>) -> Result<Window> {
    let builder = WindowBuilder::new()
        .with_title("Axiom")
        .with_inner_size(LogicalSize::new(1280.0, 820.0))
        .with_min_inner_size(LogicalSize::new(560.0, 380.0))
        .with_decorations(false)
        .with_visible(false)
        .with_focused(!crate::tabs::webview::test_mode());
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
