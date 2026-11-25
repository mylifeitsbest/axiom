//! Масштаб страниц, как в Chrome: ступени, Ctrl+колёсико и вырез в странице
//! под окошком масштаба.
//!
//! WebView2 сам меняет масштаб по Ctrl+колёсику и сообщает об этом
//! (`ZoomFactorChanged`). WebKitGTK по колёсику масштаб не меняет вовсе —
//! колёсико с Ctrl ловим здесь сами и переводим в ступени (см. [`Wheel`]).

use serde::Deserialize;
use tao::event_loop::EventLoopProxy;
use url::Url;
use wry::WebView;

use super::TabId;
use crate::events::UserEvent;

/// Ступени масштаба, как у Chrome: по ним же ходит Ctrl+колёсико в WebView2.
const STEPS: [f64; 17] = [
    0.25,
    1.0 / 3.0,
    0.5,
    2.0 / 3.0,
    0.75,
    0.8,
    0.9,
    1.0,
    1.1,
    1.25,
    1.5,
    1.75,
    2.0,
    2.5,
    3.0,
    4.0,
    5.0,
];
pub const MIN: f64 = 0.25;
pub const MAX: f64 = 5.0;
/// Масштабы ближе этого считаются одним (погрешность double, 1/3 из WebView2).
const EPS: f64 = 0.001;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Zoom {
    In,
    Out,
    Reset,
}

/// Следующая ступень масштаба. Масштаб между ступенями округляется
/// к ближайшей ступени в нужную сторону.
pub fn next(current: f64, zoom: Zoom) -> f64 {
    match zoom {
        Zoom::Reset => 1.0,
        Zoom::In => STEPS
            .into_iter()
            .find(|&s| s > current + EPS)
            .unwrap_or(MAX),
        Zoom::Out => STEPS
            .into_iter()
            .rev()
            .find(|&s| s < current - EPS)
            .unwrap_or(MIN),
    }
}

pub fn same(a: f64, b: f64) -> bool {
    (a - b).abs() < EPS
}

/// Масштаб для интерфейса в процентах: 1.25 → 125.
pub fn percent(zoom: f64) -> u16 {
    let zoom = if zoom.is_finite() { zoom } else { 1.0 };
    (zoom.clamp(MIN, MAX) * 100.0).round() as u16
}

/// Сайт, для которого запоминается масштаб, — хост страницы, как в Chrome.
/// У остальных адресов (файлы, `data:`, исходный код) масштаб только свой у вкладки.
pub fn site(url: &str) -> Option<String> {
    let u = Url::parse(url).ok()?;
    if !matches!(u.scheme(), "http" | "https") {
        return None;
    }
    u.host_str().filter(|h| !h.is_empty()).map(str::to_owned)
}

/// Что сообщает движок вкладки о масштабе.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// WebView2 поменял масштаб (Ctrl+колёсико — или это отклик на наш же вызов:
    /// какой масштаб теперь, `App` читает у движка сам).
    #[cfg_attr(not(windows), allow(dead_code))]
    Changed,
    /// Linux: Ctrl+колёсико над страницей — ступень вверх или вниз.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    Step(Zoom),
}

/// Ctrl+колёсико в ступени масштаба: один щелчок колеса — одна ступень.
///
/// Прокрутка приходит в «щелчках» (GDK: 1.0 на щелчок, вниз — больше нуля).
/// Тачпад и колёса с высоким разрешением шлют доли щелчка — они копятся до целой
/// ступени. Щелчок крупнее единицы (Hyprland и другие композиторы на wlroots
/// шлют 1.5) — всё равно одна ступень, без остатка на следующий.
#[derive(Debug, Default)]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub struct Wheel {
    acc: f64,
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
impl Wheel {
    pub fn scroll(&mut self, dy: f64) -> Option<Zoom> {
        if !dy.is_finite() || dy == 0.0 {
            return None;
        }
        // Повернули в другую сторону: накопленное туда не в счёт.
        if self.acc * dy < 0.0 {
            self.acc = 0.0;
        }
        self.acc += dy;
        if self.acc.abs() < 1.0 - EPS {
            return None;
        }
        // От себя (вверх) — крупнее, как в Chrome.
        let zoom = if self.acc < 0.0 { Zoom::In } else { Zoom::Out };
        self.acc = if dy.abs() >= 1.0 - EPS {
            0.0
        } else {
            self.acc - self.acc.signum()
        };
        Some(zoom)
    }

    /// Жест на тачпаде закончился или Ctrl отпустили.
    pub fn stop(&mut self) {
        self.acc = 0.0;
    }
}

// ---------- движок ----------

/// Масштаб, который сейчас у страницы (WebView2 — последний заданный, пока не применён).
#[cfg(windows)]
pub fn current(webview: &WebView) -> Option<f64> {
    use wry::WebViewExtWindows;
    let mut zoom = 1.0;
    // SAFETY: контроллер принадлежит живому webview, указатель на локальную переменную.
    unsafe { webview.controller().ZoomFactor(&mut zoom) }.ok()?;
    zoom.is_finite().then_some(zoom)
}

#[cfg(target_os = "linux")]
pub fn current(webview: &WebView) -> Option<f64> {
    use gtk::glib::prelude::*;
    use wry::WebViewExtUnix;
    let zoom = webview.webview().property::<f64>("zoom-level");
    zoom.is_finite().then_some(zoom)
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn current(_webview: &WebView) -> Option<f64> {
    None
}

pub fn set(webview: &WebView, zoom: f64) {
    let _ = webview.zoom(zoom.clamp(MIN, MAX));
}

/// Сообщает в главный цикл, когда масштаб меняет сам движок или человек колёсиком.
#[cfg(windows)]
pub fn watch(webview: &WebView, id: TabId, events: EventLoopProxy<UserEvent>) {
    use webview2_com::ZoomFactorChangedEventHandler;
    use wry::WebViewExtWindows;

    let handler = ZoomFactorChangedEventHandler::create(Box::new(move |_, _| {
        let _ = events.send_event(UserEvent::TabZoom(id, Event::Changed));
        Ok(())
    }));
    let mut token = 0i64;
    // SAFETY: контроллер принадлежит webview и живёт столько же, сколько он.
    let result = unsafe {
        webview
            .controller()
            .add_ZoomFactorChanged(&handler, &mut token)
    };
    if let Err(err) = result {
        eprintln!("масштаб страницы не отслеживается: {err}");
    }
}

/// WebKitGTK: Ctrl+колёсико страницу не масштабирует. Ловим `scroll-event` самого
/// webview: наш обработчик срабатывает раньше WebKitGTK, и `Stop` не даёт
/// странице прокрутиться вместо масштаба.
#[cfg(target_os = "linux")]
pub fn watch(webview: &WebView, id: TabId, events: EventLoopProxy<UserEvent>) {
    use gtk::{gdk, glib, prelude::*};
    use std::cell::RefCell;
    use wry::WebViewExtUnix;

    let wheel = RefCell::new(Wheel::default());
    let widget = webview.webview().upcast::<gtk::Widget>();
    widget.connect_scroll_event(move |_, event| {
        let state = event.state();
        let others = gdk::ModifierType::SHIFT_MASK
            | gdk::ModifierType::MOD1_MASK
            | gdk::ModifierType::SUPER_MASK
            | gdk::ModifierType::META_MASK;
        // Только Ctrl: с Shift, Alt и другими клавишами колёсико остаётся странице.
        if !state.contains(gdk::ModifierType::CONTROL_MASK) || state.intersects(others) {
            wheel.borrow_mut().stop();
            return glib::Propagation::Proceed;
        }
        let dy = match event.direction() {
            gdk::ScrollDirection::Up => -1.0,
            gdk::ScrollDirection::Down => 1.0,
            gdk::ScrollDirection::Smooth => {
                if event.is_stop() {
                    wheel.borrow_mut().stop();
                    return glib::Propagation::Stop;
                }
                let (dx, dy) = event.delta();
                // Прокрутка вбок с Ctrl — странице.
                if dy == 0.0 && dx != 0.0 {
                    return glib::Propagation::Proceed;
                }
                dy
            }
            _ => return glib::Propagation::Proceed,
        };
        let step = wheel.borrow_mut().scroll(dy);
        if let Some(step) = step {
            let _ = events.send_event(UserEvent::TabZoom(id, Event::Step(step)));
        }
        glib::Propagation::Stop
    });
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn watch(_webview: &WebView, _id: TabId, _events: EventLoopProxy<UserEvent>) {}

// ---------- вырез под окошком масштаба ----------

/// Прямоугольник в логических пикселях.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Area {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// Вырез в webview страницы: целые пиксели от его угла и радиус скругления.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hole {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub r: i32,
}

/// Окошко масштаба — разметка интерфейса, а страница — нативный webview поверх него.
/// Чтобы окошко было видно, а страница под ним продолжала жить (её масштаб и
/// меняют), в webview страницы прорезается дыра ровно по окошку.
///
/// `bubble` и `page` — в логических пикселях окна, `scale` — во сколько раз
/// пиксели выреза крупнее логических (WebView2 — физические пиксели, GTK — логические).
/// Вырез не больше окошка: лучше спрятать под страницей полпикселя его края,
/// чем показать рядом с ним полоску фона.
pub fn hole(bubble: Area, radius: f64, page: Area, scale: f64) -> Option<Hole> {
    let finite = |a: Area| [a.x, a.y, a.w, a.h].into_iter().all(f64::is_finite);
    if !(finite(bubble) && finite(page) && radius.is_finite() && scale.is_finite() && scale > 0.0) {
        return None;
    }
    let x0 = (bubble.x.max(page.x) - page.x) * scale;
    let y0 = (bubble.y.max(page.y) - page.y) * scale;
    let x1 = ((bubble.x + bubble.w).min(page.x + page.w) - page.x) * scale;
    let y1 = ((bubble.y + bubble.h).min(page.y + page.h) - page.y) * scale;
    let (x0, y0) = (x0.ceil() as i32, y0.ceil() as i32);
    let (x1, y1) = (x1.floor() as i32, y1.floor() as i32);
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let (w, h) = (x1 - x0, y1 - y0);
    let r = ((radius.max(0.0) * scale).round() as i32)
        .min(w / 2)
        .min(h / 2);
    Some(Hole {
        x: x0,
        y: y0,
        w,
        h,
        r,
    })
}

/// Вырез построчно: скругления — ступеньками по строке пикселей, середина — одним
/// прямоугольником. Так его можно вычесть и из региона окна WebView2, и из
/// региона GTK, где есть только прямоугольники.
pub fn rows(hole: Hole) -> Vec<(i32, i32, i32, i32)> {
    let Hole { x, y, w, h, r } = hole;
    let mut out = Vec::with_capacity(2 * r.max(0) as usize + 1);
    let rf = f64::from(r);
    for i in 0..r {
        // Отступ строки от края: где окружность радиуса r пересекает середину строки.
        let dy = rf - f64::from(i) - 0.5;
        let dx = (rf - (rf * rf - dy * dy).max(0.0).sqrt()).round() as i32;
        let width = w - 2 * dx;
        if width > 0 {
            out.push((x + dx, y + i, width, 1));
            out.push((x + dx, y + h - 1 - i, width, 1));
        }
    }
    if h > 2 * r {
        out.push((x, y + r, w, h - 2 * r));
    }
    out
}

/// Прорезает (`Some`) или заделывает (`None`) вырез в webview страницы.
/// Регион — у окна-контейнера wry: Windows обрезает по нему и само содержимое
/// WebView2, и щелчки — в вырезе они достаются интерфейсу под страницей.
#[cfg(windows)]
pub fn set_hole(webview: &WebView, hole: Option<Hole>) {
    use windows::Win32::Foundation::HWND;
    use wry::WebViewExtWindows;

    #[link(name = "gdi32")]
    unsafe extern "system" {
        fn CreateRectRgn(x1: i32, y1: i32, x2: i32, y2: i32) -> isize;
        fn CombineRgn(dst: isize, a: isize, b: isize, mode: i32) -> i32;
        fn DeleteObject(object: isize) -> i32;
    }
    #[link(name = "user32")]
    unsafe extern "system" {
        fn SetWindowRgn(hwnd: HWND, rgn: isize, redraw: i32) -> i32;
    }
    const RGN_DIFF: i32 = 4;
    // Наружная граница региона: заведомо больше любого webview.
    const BIG: i32 = 1 << 15;

    let mut hwnd = HWND::default();
    // SAFETY: контроллер принадлежит живому webview, указатель на локальную переменную.
    if unsafe { webview.controller().ParentWindow(&mut hwnd) }.is_err() || hwnd.is_invalid() {
        return;
    }
    let Some(hole) = hole else {
        // SAFETY: hwnd — живое окно-контейнер webview; пустой регион снимает обрезку.
        unsafe { SetWindowRgn(hwnd, 0, 1) };
        return;
    };
    // SAFETY: регионы создаём и удаляем здесь же; после успешного SetWindowRgn
    // регионом владеет система, и удалять его нельзя.
    unsafe {
        let region = CreateRectRgn(0, 0, BIG, BIG);
        if region == 0 {
            return;
        }
        for (x, y, w, h) in rows(hole) {
            let row = CreateRectRgn(x, y, x + w, y + h);
            if row != 0 {
                CombineRgn(region, region, row, RGN_DIFF);
                DeleteObject(row);
            }
        }
        if SetWindowRgn(hwnd, region, 1) == 0 {
            DeleteObject(region);
        }
    }
}

/// GTK: форма окна GDK у самого webview. Без выреза в форме и отрисовка
/// WebKitGTK, и щелчки — в вырезе виден и нажимается интерфейс под страницей.
#[cfg(target_os = "linux")]
pub fn set_hole(webview: &WebView, hole: Option<Hole>) {
    use gtk::{
        cairo::{RectangleInt, Region},
        prelude::*,
    };
    use wry::WebViewExtUnix;

    const BIG: i32 = 1 << 15;
    let widget = webview.webview().upcast::<gtk::Widget>();
    let Some(hole) = hole else {
        widget.shape_combine_region(None);
        return;
    };
    let region = Region::create_rectangle(&RectangleInt::new(0, 0, BIG, BIG));
    for (x, y, w, h) in rows(hole) {
        let _ = region.subtract_rectangle(&RectangleInt::new(x, y, w, h));
    }
    widget.shape_combine_region(Some(&region));
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn set_hole(_webview: &WebView, _hole: Option<Hole>) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_like_chrome() {
        assert_eq!(next(1.0, Zoom::In), 1.1);
        assert_eq!(next(1.0, Zoom::Out), 0.9);
        assert_eq!(next(1.75, Zoom::Reset), 1.0);
        // Между ступенями — к ближайшей ступени в нужную сторону.
        assert_eq!(next(1.17, Zoom::In), 1.25);
        assert_eq!(next(1.17, Zoom::Out), 1.1);
        // Почти ступень (погрешность double) — считается ступенью.
        assert_eq!(next(1.0999999, Zoom::In), 1.25);
        assert_eq!(next(0.333_333, Zoom::In), 0.5);
        // За краями дальше не идём.
        assert_eq!(next(5.0, Zoom::In), 5.0);
        assert_eq!(next(0.25, Zoom::Out), 0.25);
        assert_eq!(next(0.3, Zoom::Out), 0.25);
        // Ступени в обе стороны возвращают туда же.
        let mut z = 1.0;
        for _ in 0..4 {
            z = next(z, Zoom::In);
        }
        assert_eq!(z, 1.75);
        for _ in 0..4 {
            z = next(z, Zoom::Out);
        }
        assert_eq!(z, 1.0);
    }

    #[test]
    fn percent_and_same() {
        assert_eq!(percent(1.0), 100);
        assert_eq!(percent(1.25), 125);
        assert_eq!(percent(1.0 / 3.0), 33);
        assert_eq!(percent(2.0 / 3.0), 67);
        assert_eq!(percent(9.0), 500);
        assert_eq!(percent(f64::NAN), 100);
        assert!(same(1.1, 1.100_000_1));
        assert!(!same(1.1, 1.25));
    }

    #[test]
    fn site_is_host() {
        assert_eq!(
            site("https://Docs.Example.com/a?b").as_deref(),
            Some("docs.example.com")
        );
        assert_eq!(site("http://localhost:8080/").as_deref(), Some("localhost"));
        assert_eq!(site("http://[::1]:3000/").as_deref(), Some("[::1]"));
        assert_eq!(site("file:///C:/a.html"), None);
        assert_eq!(site("axiom://newtab"), None);
        assert_eq!(site("view-source:https://example.com/"), None);
        assert_eq!(site("data:text/html,x"), None);
    }

    #[test]
    fn wheel_notch_is_one_step() {
        let mut w = Wheel::default();
        // Обычное колесо: щелчок — ступень, от себя — крупнее.
        assert_eq!(w.scroll(-1.0), Some(Zoom::In));
        assert_eq!(w.scroll(-1.0), Some(Zoom::In));
        assert_eq!(w.scroll(1.0), Some(Zoom::Out));
        // Hyprland: 1.5 на щелчок — всё равно по одной ступени, без лишней.
        for _ in 0..4 {
            assert_eq!(w.scroll(1.5), Some(Zoom::Out));
        }
        // Ноль, NaN и бесконечность ничего не делают.
        assert_eq!(w.scroll(0.0), None);
        assert_eq!(w.scroll(f64::NAN), None);
        assert_eq!(w.scroll(f64::INFINITY), None);
    }

    #[test]
    fn wheel_fractions_accumulate() {
        let mut w = Wheel::default();
        // Колесо с высоким разрешением: восьмушки щелчка — ступень на каждой восьмой.
        let steps: Vec<_> = (0..16).filter_map(|_| w.scroll(-0.125)).collect();
        assert_eq!(steps, vec![Zoom::In, Zoom::In]);
        // Тачпад: остаток идёт в следующую ступень.
        assert_eq!(w.scroll(0.6), None);
        assert_eq!(w.scroll(0.6), Some(Zoom::Out));
        assert_eq!(w.scroll(0.6), None);
        assert_eq!(w.scroll(0.6), Some(Zoom::Out));
        // Повернули обратно: накопленное вниз не мешает.
        assert_eq!(w.scroll(-0.6), None);
        assert_eq!(w.scroll(-0.5), Some(Zoom::In));
        // Конец жеста сбрасывает остаток.
        assert_eq!(w.scroll(-0.5), None);
        w.stop();
        assert_eq!(w.scroll(-0.5), None);
        assert_eq!(w.scroll(-0.5), Some(Zoom::In));
    }

    #[test]
    fn hole_fits_inside_bubble_and_page() {
        let page = Area {
            x: 6.0,
            y: 48.0,
            w: 1000.0,
            h: 700.0,
        };
        let bubble = Area {
            x: 600.0,
            y: 44.0,
            w: 220.0,
            h: 46.0,
        };
        // Верх окошка над страницей (в щели между панелью и страницей) — не вырезаем.
        assert_eq!(
            hole(bubble, 12.0, page, 1.0),
            Some(Hole {
                x: 594,
                y: 0,
                w: 220,
                h: 42,
                r: 12
            })
        );
        // 125 %: физические пиксели, края — внутрь окошка.
        assert_eq!(
            hole(bubble, 12.0, page, 1.25),
            Some(Hole {
                x: 743,
                y: 0,
                w: 274,
                h: 52,
                r: 15
            })
        );
        // Окошко целиком над панелью инструментов — выреза нет.
        let above = Area {
            y: 0.0,
            h: 40.0,
            ..bubble
        };
        assert_eq!(hole(above, 12.0, page, 1.0), None);
        assert_eq!(hole(bubble, 12.0, page, f64::NAN), None);
        assert_eq!(hole(bubble, 12.0, page, 0.0), None);
        // Радиус не больше половины выреза.
        let tiny = Area {
            x: 10.0,
            y: 50.0,
            w: 8.0,
            h: 6.0,
        };
        assert_eq!(hole(tiny, 12.0, page, 1.0).map(|h| h.r), Some(3));
    }

    #[test]
    fn rows_cover_rounded_rect() {
        let h = Hole {
            x: 10,
            y: 20,
            w: 100,
            h: 40,
            r: 8,
        };
        let rows = rows(h);
        // 8 строк сверху, 8 снизу и середина.
        assert_eq!(rows.len(), 17);
        let area: i32 = rows.iter().map(|&(_, _, w, h)| w * h).sum();
        // Площадь скруглённого прямоугольника: w·h − (4 − π)·r².
        let exact = 100.0 * 40.0 - (4.0 - std::f64::consts::PI) * 64.0;
        assert!((f64::from(area) - exact).abs() < 12.0, "{area} vs {exact}");
        // Всё внутри выреза, первая строка уже середины, середина — во всю ширину.
        for &(x, y, w, hh) in &rows {
            assert!(x >= 10 && y >= 20 && x + w <= 110 && y + hh <= 60);
        }
        assert!(rows[0].2 < 100);
        assert_eq!(rows.last().copied(), Some((10, 28, 100, 24)));
        // Без скругления — один прямоугольник.
        assert_eq!(rows_of(0), vec![(10, 20, 100, 40)]);
    }

    fn rows_of(r: i32) -> Vec<(i32, i32, i32, i32)> {
        rows(Hole {
            x: 10,
            y: 20,
            w: 100,
            h: 40,
            r,
        })
    }
}
