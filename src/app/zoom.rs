//! Масштаб страниц: ступени по клавишам и кнопкам окошка, Ctrl+колёсико,
//! масштаб по сайтам и вырез в странице под окошком (см. `tabs::zoom`).

use std::time::{Duration, Instant};

use super::App;
use crate::{
    ipc::{self, ToUi},
    storage::{Paths, zoom::SiteZoom},
    tabs::{
        TabId,
        zoom::{self, Area, Event, Zoom},
    },
};

/// WebView2 сбрасывает масштаб от Ctrl+колёсика при переходе на новую страницу —
/// к последнему масштабу, заданному нами, и сообщает об этом сразу после
/// `ContentLoading`. Столько после начала страницы такое событие считаем сбросом.
const RESET_WINDOW: Duration = Duration::from_secs(1);

pub(super) struct State {
    sites: SiteZoom,
    /// Масштаб сайтов поменялся, а файл ещё не записан.
    dirty: bool,
    /// Окошко масштаба на экране: где оно (логические px окна) и радиус его углов.
    bubble: Option<(Area, f64)>,
    /// Вкладка, в webview которой прорезан вырез под окошко.
    hole: Option<TabId>,
    /// Когда во вкладках началась новая страница (см. [`RESET_WINDOW`]).
    started: Vec<(TabId, Instant)>,
}

impl State {
    pub(super) fn load(paths: &Paths) -> Self {
        Self {
            sites: SiteZoom::load(paths),
            dirty: false,
            bubble: None,
            hole: None,
            started: Vec::new(),
        }
    }

    /// Вкладка только что начала новую страницу.
    fn just_started(&self, id: TabId) -> bool {
        self.started
            .iter()
            .any(|&(tab, at)| tab == id && at.elapsed() < RESET_WINDOW)
    }

    pub(super) fn save_if_dirty(&mut self, paths: &Paths) {
        if !self.dirty {
            return;
        }
        match self.sites.save(paths) {
            Ok(()) => self.dirty = false,
            Err(err) => eprintln!("масштаб сайтов не сохранён: {err:#}"),
        }
    }

    /// Масштаб для страницы по адресу: запомненный для её сайта (незнакомый — 100 %).
    /// `None` — у адреса нет сайта, остаётся масштаб вкладки.
    pub(super) fn for_url(&self, url: &str) -> Option<f64> {
        zoom::site(url).map(|site| self.sites.get(&site))
    }
}

impl App {
    /// Ступень масштаба: Ctrl+=, Ctrl+−, Ctrl+0, кнопки окошка, Ctrl+колёсико на Linux.
    pub(super) fn zoom_tab(&mut self, id: TabId, step: Zoom) {
        let Some(tab) = self.tabs.get(id).filter(|t| t.webview.is_some()) else {
            return;
        };
        let level = zoom::next(tab.zoom, step);
        self.user_zoom(id, level);
    }

    /// Кнопки окошка масштаба. Щелчок по окошку увёл фокус в интерфейс — отдаём его
    /// обратно странице: окошко не должно забирать клавиатуру у страницы.
    pub(super) fn zoom_from_bubble(&mut self, step: Zoom) {
        let Some(id) = self.tabs.active else {
            return;
        };
        self.zoom_tab(id, step);
        if let Some(webview) = self.tabs.get(id).and_then(|t| t.webview.as_ref()) {
            let _ = webview.focus();
        }
    }

    /// Человек поменял масштаб вкладки: запоминаем его для сайта, как в Chrome сразу
    /// меняем и другие вкладки этого сайта и показываем окошко масштаба.
    fn user_zoom(&mut self, id: TabId, level: f64) {
        let level = level.clamp(zoom::MIN, zoom::MAX);
        let Some(tab) = self.tabs.get_mut(id) else {
            return;
        };
        let changed = !zoom::same(tab.zoom, level);
        tab.zoom = level;
        // WebView2: заданный нами масштаб — масштаб вкладки, его не сбросит переход
        // на другую страницу (масштаб от Ctrl+колёсика сбросил бы). Поэтому задаём
        // и тот, что движок только что выставил сам.
        if let Some(webview) = &tab.webview {
            zoom::set(webview, level);
        }
        if let Some(site) = zoom::site(&tab.url) {
            if self.zoom.sites.set(&site, level) {
                self.zoom.dirty = true;
            }
            for other in self.tabs.iter_mut().filter(|t| t.id != id) {
                if zoom::same(other.zoom, level) || zoom::site(&other.url).as_ref() != Some(&site) {
                    continue;
                }
                other.zoom = level;
                if let Some(webview) = &other.webview {
                    zoom::set(webview, level);
                }
            }
        }
        // Окошко — раньше состояния вкладок: значок в адресной строке при возврате
        // к 100 % тогда не пропадает на миг перед тем, как окошко его покажет.
        if self.tabs.active == Some(id) && (changed || !zoom::same(level, 1.0)) {
            self.send(&ToUi::Zoom {
                id,
                zoom: zoom::percent(level),
            });
        }
        self.push_state();
    }

    /// Движок сообщил о масштабе (WebView2) или Ctrl+колёсико над страницей (Linux).
    pub(super) fn on_tab_zoom(&mut self, id: TabId, event: Event) {
        match event {
            Event::Step(step) => self.zoom_tab(id, step),
            Event::Changed => {
                let Some(tab) = self.tabs.get(id) else {
                    return;
                };
                let Some(level) = tab.webview.as_ref().and_then(zoom::current) else {
                    return;
                };
                // Отклик на наш же вызов: у движка уже масштаб вкладки. Сравниваем с тем,
                // что у движка сейчас, а не с числом из события: после двух быстрых
                // Ctrl+= первое событие пришло бы с уже устаревшим масштабом.
                if zoom::same(level, tab.zoom) {
                    return;
                }
                // Сброс при переходе (см. `RESET_WINDOW`) — это не человек: возвращаем
                // масштаб вкладки. Обычно до этого не доходит: масштаб от колёсика
                // мы сразу задаём сами, и сбрасывать WebView2 становится не к чему.
                if self.zoom.just_started(id) {
                    if let Some(webview) = &tab.webview {
                        zoom::set(webview, tab.zoom);
                    }
                    return;
                }
                self.user_zoom(id, level);
            }
        }
    }

    /// Во вкладке открылась страница (`started`) или догрузилась: масштаб её сайта.
    /// Сверяемся и с движком: WebView2 мог сам вернуть масштаб при переходе.
    pub(super) fn sync_page_zoom(&mut self, id: TabId, started: bool) {
        if started {
            self.zoom
                .started
                .retain(|&(tab, at)| tab != id && at.elapsed() < RESET_WINDOW);
            self.zoom.started.push((id, Instant::now()));
        }
        let Some(tab) = self.tabs.get_mut(id) else {
            return;
        };
        let level = self.zoom.for_url(&tab.url).unwrap_or(tab.zoom);
        let engine = tab.webview.as_ref().and_then(zoom::current);
        if zoom::same(tab.zoom, level) && engine.is_none_or(|e| zoom::same(e, level)) {
            return;
        }
        tab.zoom = level;
        if let Some(webview) = &tab.webview {
            zoom::set(webview, level);
        }
    }

    /// Окошко масштаба показалось или спряталось.
    pub(super) fn set_zoom_bubble(&mut self, rect: Option<ipc::Rect>, radius: f64) {
        self.zoom.bubble = rect.map(|r| {
            let area = Area {
                x: r.x,
                y: r.y,
                w: r.w,
                h: r.h,
            };
            (area, radius)
        });
        self.sync_zoom_hole();
    }

    /// Вырез под окошком масштаба: только в webview активной вкладки и только пока
    /// окошко на экране. Во весь экран страница закрывает и окошко.
    pub(super) fn sync_zoom_hole(&mut self) {
        let target = match self.zoom.bubble {
            Some(bubble) if !self.fullscreen.is_on() => self
                .tabs
                .active_tab()
                .filter(|t| t.webview.is_some())
                .map(|t| (t.id, bubble)),
            _ => None,
        };
        if let Some(old) = self.zoom.hole.take()
            && target.is_none_or(|(id, _)| id != old)
            && let Some(webview) = self.tabs.get(old).and_then(|t| t.webview.as_ref())
        {
            zoom::set_hole(webview, None);
        }
        let Some((id, (bubble, radius))) = target else {
            return;
        };
        let Some(page) = self.content_area() else {
            return;
        };
        // WebView2 режет окно в физических пикселях, GTK — в логических.
        let scale = if cfg!(windows) {
            self.window.scale_factor()
        } else {
            1.0
        };
        let hole = zoom::hole(bubble, radius, page, scale);
        if let Some(webview) = self.tabs.get(id).and_then(|t| t.webview.as_ref()) {
            zoom::set_hole(webview, hole);
            self.zoom.hole = hole.map(|_| id);
        }
    }

    /// Место страницы в окне (логические px).
    fn content_area(&self) -> Option<Area> {
        let rect = self.content?;
        let scale = self.window.scale_factor();
        let at = rect.position.to_logical::<f64>(scale);
        let size = rect.size.to_logical::<f64>(scale);
        Some(Area {
            x: at.x,
            y: at.y,
            w: size.width,
            h: size.height,
        })
    }
}
