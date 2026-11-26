//! Действия с вкладкой: меню вкладки, звук и громкость, исходный код, печать, масштаб,
//! инструменты разработчика и ссылки, открытые в фоне.

use wry::{
    Rect,
    dpi::{LogicalPosition, LogicalSize},
};

use super::{App, display_title};
use crate::{
    events::UserEvent,
    ipc::ToUi,
    tabs::{
        Tab, TabId, links, media, mini, omnibox, script, tools, volume, webview as tab_webview,
        zoom::Zoom,
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
            Hotkey::ZoomIn => return self.zoom_tab(id, Zoom::In),
            Hotkey::ZoomOut => return self.zoom_tab(id, Zoom::Out),
            Hotkey::ZoomReset => return self.zoom_tab(id, Zoom::Reset),
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
        let (url, title, ai) = (tab.url.clone(), tab.title.clone(), tab.ai);
        let (volume, zoom) = (tab.volume, tab.zoom);
        let at = self.tabs.index_of(id).map(|i| i + 1);
        let copy = self.tabs.add(url, title, ai, at);
        // Копия той же страницы звучит и выглядит так же.
        if let Some(tab) = self.tabs.get_mut(copy) {
            tab.volume = volume;
            tab.zoom = zoom;
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
        if !volume::SUPPORTED || volume::page_volume(tab.volume) == volume::FULL {
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

    // ---------- островок: что играет и кнопки (tabs::media) ----------

    /// Спросить страницу, что играет, и, если надо, нажать кнопку. Ответ уходит в
    /// интерфейс (`ToUi::Media`). Только живая страница: спящую не будим.
    pub(super) fn media(&mut self, id: TabId, action: Option<media::Action>) {
        match action {
            Some(media::Action::Peek) => {
                self.mini_open(id);
                return;
            }
            Some(media::Action::Unpeek) => {
                if let Some(show) = self.mini.as_ref().filter(|s| s.id == id) {
                    show.stop();
                }
                return;
            }
            // Видео в окошко: из островка сразу (без анимации), потом окошко — по клику.
            Some(media::Action::Pip) => {
                if self.mini.as_ref().is_some_and(|s| s.id == id) {
                    self.mini_finish(id);
                }
                if let Some(webview) = self.tabs.get(id).and_then(|t| t.webview.as_ref()) {
                    script::run_as_click(webview, &media::action(media::Action::Pip));
                }
                self.ask_media(id);
                return;
            }
            _ => {}
        }
        let Some(webview) = self.tabs.get(id).and_then(|t| t.webview.as_ref()) else {
            return;
        };
        if let Some(a) = action {
            script::run(webview, &media::action(a));
        }
        self.ask_media(id);
    }

    fn ask_media(&self, id: TabId) {
        let Some(webview) = self.tabs.get(id).and_then(|t| t.webview.as_ref()) else {
            return;
        };
        let events = self.events.clone();
        script::ask(webview, media::QUERY, move |raw| {
            let _ = events.send_event(UserEvent::TabMedia { id, raw });
        });
    }

    // ---------- видео внутри островка (tabs::mini) ----------

    /// Навели мышь на чёлку: страница готовит видео (`MiniReady`), потом вкладка выезжает
    /// из чёлки. Только вкладка в фоне: на своей вкладке видео и так видно.
    fn mini_open(&mut self, id: TabId) {
        if self.mini.is_some() {
            return;
        }
        let webview = self.tabs.get(id).and_then(|t| t.webview.as_ref());
        let (Some(webview), Some(_), false) =
            (webview, self.island_dock, self.tabs.active == Some(id))
        else {
            self.send(&ToUi::Peek {
                id,
                on: false,
                height: 0.0,
            });
            return;
        };
        let events = self.events.clone();
        let asked = script::ask(webview, &media::action(media::Action::Peek), move |raw| {
            let _ = events.send_event(UserEvent::MiniReady { id, raw });
        });
        if asked {
            self.mini = Some(mini::Show::new(id));
        } else {
            self.send(&ToUi::Peek {
                id,
                on: false,
                height: 0.0,
            });
        }
    }

    /// Страница вывела видео поверх себя: вкладка встаёт под чёлку и выезжает.
    pub(super) fn mini_ready(&mut self, id: TabId, raw: &str) {
        let size = mini::video_size(raw);
        let wanted = self.mini.as_ref().is_some_and(|s| s.id == id && !s.closing);
        let (Some(size), true) = (size, wanted) else {
            // Показывать нечего или уже не нужно: страницу — как была.
            if let Some(webview) = self.tabs.get(id).and_then(|t| t.webview.as_ref()) {
                script::run(webview, &media::action(media::Action::Unpeek));
            }
            if self.mini.as_ref().is_some_and(|s| s.id == id) {
                self.mini = None;
            }
            self.send(&ToUi::Peek {
                id,
                on: false,
                height: 0.0,
            });
            return;
        };
        let shared = match self.mini.as_mut() {
            Some(show) => {
                show.video = Some(size);
                show.shared.clone()
            }
            None => return,
        };
        self.place_mini(0.0);
        if let Some(webview) = self.tabs.get(id).and_then(|t| t.webview.as_ref()) {
            let _ = webview.set_visible(true);
            tab_webview::set_behind(webview, false);
        }
        self.sync_mini_areas();
        let events = self.events.clone();
        mini::watch(shared, move || {
            let _ = events.send_event(UserEvent::MiniEnded(id));
        });
        self.mini_frames(id, true);
        let height = self.mini_dock().map_or(0.0, |d| d.island(size.0, size.1).h);
        self.send(&ToUi::Peek {
            id,
            on: true,
            height,
        });
    }

    /// Кадры выезда (`open`) или уезда: отдельный поток просто тикает, ставит главный.
    fn mini_frames(&self, id: TabId, open: bool) {
        let events = self.events.clone();
        std::thread::spawn(move || {
            for step in 1..=mini::FRAMES {
                std::thread::sleep(mini::FRAME);
                if events
                    .send_event(UserEvent::MiniFrame { id, step, open })
                    .is_err()
                {
                    return;
                }
            }
        });
    }

    pub(super) fn mini_frame(&mut self, id: TabId, step: u8, open: bool) {
        let Some(show) = self.mini.as_ref().filter(|s| s.id == id) else {
            return;
        };
        // Начал уезжать — кадры выезда больше не нужны.
        if open == show.closing {
            return;
        }
        let k = if open {
            mini::open_k(step)
        } else {
            mini::close_k(step)
        };
        self.place_mini(k);
        if !open && step >= mini::FRAMES {
            self.mini_finish(id);
        }
    }

    /// Мышь ушла с чёлки и острова (или показ остановили): остров уезжает в чёлку.
    pub(super) fn mini_ended(&mut self, id: TabId) {
        let Some(show) = self.mini.as_mut().filter(|s| s.id == id && !s.closing) else {
            return;
        };
        show.closing = true;
        show.stop();
        self.mini_frames(id, false);
    }

    /// Показ закончен: страница как была, вкладка — снова на своём месте (в фоне — скрыта).
    pub(super) fn mini_finish(&mut self, id: TabId) {
        let Some(show) = self.mini.take_if(|s| s.id == id) else {
            return;
        };
        show.stop();
        let active = self.tabs.active == Some(id);
        if let Some(webview) = self.tabs.get(id).and_then(|t| t.webview.as_ref()) {
            script::run(webview, &media::action(media::Action::Unpeek));
            mini::round_bottom(webview, None, 0);
            if !active {
                let _ = webview.set_visible(false);
            }
            if let Some(rect) = self.content {
                tab_webview::set_bounds(webview, rect);
            }
        }
        self.send(&ToUi::Peek {
            id,
            on: false,
            height: 0.0,
        });
        self.ask_media(id);
    }

    /// Остров на месте: под чёлкой, высотой `k` от полной (анимация), скруглён снизу.
    ///
    /// Вкладка остаётся своего размера (иначе сайт перестроил бы страницу и унёс видео),
    /// только встаёт левым верхним углом в угол острова (в боковой панели он левее
    /// страницы); видно лишь вырез острова. Видео страница ставит ровно в остров, с рамкой
    /// [`mini::PAD`].
    fn place_mini(&mut self, k: f64) {
        let scale = self.window.scale_factor();
        let (Some(show), Some(dock), Some(content)) = (&self.mini, self.mini_dock(), self.content)
        else {
            return;
        };
        let Some((vw, vh)) = show.video else {
            return;
        };
        let Some(webview) = self.tabs.get(show.id).and_then(|t| t.webview.as_ref()) else {
            return;
        };
        let size = content.size.to_logical::<f64>(scale);
        let island = dock.island(vw, vh);
        tab_webview::set_bounds(
            webview,
            Rect {
                position: LogicalPosition::new(island.x, island.y).into(),
                size: LogicalSize::new(size.width, size.height).into(),
            },
        );
        let px = |v: f64| (v * scale).round() as i32;
        mini::round_bottom(
            webview,
            Some((0, px(island.w), px(island.grown(k).h))),
            px(mini::RADIUS),
        );
        let video = (
            mini::PAD,
            mini::PAD,
            island.w - 2.0 * mini::PAD,
            island.h - 2.0 * mini::PAD,
            scale,
        );
        if show.sent != Some(video) {
            script::run(
                webview,
                &media::mini_script(video.0, video.1, video.2, video.3, scale),
            );
            if let Some(show) = self.mini.as_mut() {
                show.sent = Some(video);
            }
        }
    }

    fn mini_dock(&self) -> Option<mini::Dock> {
        let d = self.island_dock?;
        Some(mini::Dock {
            cx: d.cx,
            top: d.top,
            width: d.width,
            notch_top: d.notch_top,
        })
    }

    /// Чёлка и остров в px экрана — для слежения за мышью.
    fn sync_mini_areas(&self) {
        let Some(show) = &self.mini else {
            return;
        };
        let areas = (|| {
            let (vw, vh) = show.video?;
            let dock = self.mini_dock()?;
            if self.window.is_minimized() {
                return None;
            }
            let scale = self.window.scale_factor();
            let origin = self.window.inner_position().ok()?;
            let screen = |a: mini::Area| mini::Area {
                x: f64::from(origin.x) + a.x * scale,
                y: f64::from(origin.y) + a.y * scale,
                w: a.w * scale,
                h: a.h * scale,
            };
            Some((screen(dock.notch()), screen(dock.island(vw, vh))))
        })();
        if let Ok(mut s) = show.shared.lock() {
            s.areas = areas;
        }
    }

    /// Окно переехало, поменяло размер или чёлка сдвинулась: остров — снова под чёлку.
    pub(super) fn redock_mini(&mut self) {
        if self
            .mini
            .as_ref()
            .is_some_and(|s| !s.closing && s.video.is_some())
        {
            self.place_mini(1.0);
        }
        self.sync_mini_areas();
    }

    /// Остров поверх всего (выше только что показанной вкладки).
    pub(super) fn raise_mini(&self) {
        if let Some(show) = &self.mini
            && let Some(webview) = self.tabs.get(show.id).and_then(|t| t.webview.as_ref())
        {
            tab_webview::set_behind(webview, false);
        }
    }
}

impl App {
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
