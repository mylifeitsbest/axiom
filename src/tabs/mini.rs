//! Видео внутри островка: навёл мышь на чёлку — из неё вниз выезжает чёрный остров,
//! а в нём видео той вкладки; увёл мышь — он уезжает обратно, звук играет дальше.
//!
//! Никаких отдельных окон: под чёлку встаёт сама вкладка с видео (её webview — дочернее
//! окно нашего окна, оно двигается вместе с браузером). Страница на это время выводит
//! своё видео поверх всего на чёрном фоне с отступом и скруглёнными углами
//! (`tabs::media`, режим `mini`), а у webview скруглены нижние углы — вместе с чёлкой
//! это один чёрный остров, видео в нём как в обводке.
//!
//! Мышь над видео интерфейс не видит (там webview страницы), поэтому «увёл мышь» решает
//! [`watch`] по положению курсора: курсор не над чёлкой и не над островом дольше
//! [`LEAVE`] — показ кончается.
//!
//! Linux: webview вкладки — дочернее окно GDK внутри окна браузера; остров — его форма
//! (`gdk_window_shape_combine_region`: вне неё окно не рисуется и мышь не ловит), а
//! курсор берётся у GDK относительно своего окна — координат экрана на Wayland нет.

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

/// Чёрная рамка вокруг видео, логические px (её же рисует страница, см. `tabs::media`).
pub const PAD: f64 = 8.0;
/// Скругление острова снизу, логические px.
pub const RADIUS: f64 = 22.0;
/// Сколько курсор может быть вне чёлки и острова, прежде чем видео спрячется.
#[cfg(any(windows, target_os = "linux"))]
pub const LEAVE: Duration = Duration::from_millis(350);
/// Кадров анимации выезда и уезда.
pub const FRAMES: u8 = 14;
pub const FRAME: Duration = Duration::from_millis(14);

/// Прямоугольник в логических px окна (или в px экрана на Windows — см. [`Shared`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Area {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Area {
    // Курсор отслеживается там, где есть видео в островке: Windows и Linux.
    #[cfg(any(windows, target_os = "linux", test))]
    pub fn contains(self, x: f64, y: f64, margin: f64) -> bool {
        x >= self.x - margin
            && x < self.x + self.w + margin
            && y >= self.y - margin
            && y < self.y + self.h + margin
    }

    /// Сверху вниз выезжает: та же ширина, высота — доля `k` (с пружинкой может
    /// чуть перелететь).
    pub fn grown(self, k: f64) -> Self {
        Self {
            h: (self.h * k).max(1.0),
            ..self
        }
    }
}

/// Остров под чёлкой: середина чёлки, верх острова (он заходит под низ чёлки), ширина и
/// верх самой чёлки. Логические px окна, от интерфейса.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Dock {
    pub cx: f64,
    pub top: f64,
    pub width: f64,
    pub notch_top: f64,
}

impl Dock {
    /// Остров для видео `vw × vh`: видео во всю ширину внутри рамки [`PAD`].
    pub fn island(self, vw: f64, vh: f64) -> Area {
        let inner = self.width - 2.0 * PAD;
        let video = if vw > 0.0 && vh > 0.0 {
            inner * vh / vw
        } else {
            inner * 9.0 / 16.0
        };
        // Вертикальное видео — не выше квадрата, совсем узкая полоска — не ниже трети.
        let video = video.clamp(inner / 3.0, inner);
        Area {
            x: self.cx - self.width / 2.0,
            y: self.top,
            w: self.width,
            h: video + 2.0 * PAD,
        }
    }

    /// Сама чёлка над островом (шириной с него) — для «мышь ещё здесь».
    pub fn notch(self) -> Area {
        Area {
            x: self.cx - self.width / 2.0,
            y: self.notch_top,
            w: self.width,
            h: (self.top - self.notch_top).max(1.0),
        }
    }
}

/// Доля высоты на кадре `step` из [`FRAMES`]: выезд с лёгким перелётом.
pub fn open_k(step: u8) -> f64 {
    let t = (f64::from(step) / f64::from(FRAMES)).clamp(0.0, 1.0);
    let (c1, c3) = (1.2, 2.2);
    1.0 + c3 * (t - 1.0).powi(3) + c1 * (t - 1.0).powi(2)
}

/// Доля высоты на кадре `step` уезда: быстро вверх, без перелёта.
pub fn close_k(step: u8) -> f64 {
    let t = (f64::from(step) / f64::from(FRAMES)).clamp(0.0, 1.0);
    1.0 - t * t
}

/// Идущий показ видео в островке.
#[derive(Debug)]
pub struct Show {
    pub id: crate::tabs::TabId,
    /// Размер видео; `None` — страница ещё не ответила.
    pub video: Option<(f64, f64)>,
    pub shared: Handle,
    /// Уже уезжает.
    pub closing: bool,
    /// Куда страница уже поставила видео (x, y, w, h, масштаб) — не слать то же самое.
    pub sent: Option<(f64, f64, f64, f64, f64)>,
}

impl Show {
    pub fn new(id: crate::tabs::TabId) -> Self {
        Self {
            id,
            video: None,
            shared: Handle::default(),
            closing: false,
            sent: None,
        }
    }

    /// Остановить слежение за мышью: показ закончится (`watch` позовёт `done`).
    pub fn stop(&self) {
        if let Ok(mut s) = self.shared.lock() {
            s.stop = true;
        }
    }
}

/// Ответ страницы на [`crate::tabs::media::Action::Peek`]: размер видео.
pub fn video_size(raw: &str) -> Option<(f64, f64)> {
    #[derive(serde::Deserialize)]
    struct Size {
        w: f64,
        h: f64,
    }
    let s: Size = serde_json::from_str(raw).ok()?;
    (s.w.is_finite() && s.h.is_finite() && s.w > 0.0 && s.h > 0.0).then_some((s.w, s.h))
}

/// Общее с потоком слежения за мышью: где чёлка и остров на экране и не пора ли уйти.
#[derive(Debug, Default)]
pub struct Shared {
    /// Чёлка и остров: на Windows в px экрана, на Linux в логических px окна (как
    /// [`crate::tabs::webview::pointer`]); `None` — показа больше нет.
    pub areas: Option<(Area, Area)>,
    pub stop: bool,
}

pub type Handle = Arc<Mutex<Shared>>;

/// Следит за курсором: ушёл с чёлки и острова дольше [`LEAVE`] (или показ остановили) —
/// зовёт `done` (из своего потока).
#[cfg(windows)]
pub fn watch(shared: Handle, done: impl FnOnce() + Send + 'static) {
    use std::{thread, time::Instant};
    thread::spawn(move || {
        let mut inside = Instant::now();
        loop {
            thread::sleep(Duration::from_millis(30));
            let (areas, stop) = match shared.lock() {
                Ok(s) => (s.areas, s.stop),
                Err(_) => (None, true),
            };
            let Some((notch, island)) = areas.filter(|_| !stop) else {
                break;
            };
            let over = cursor()
                .is_some_and(|(x, y)| notch.contains(x, y, 6.0) || island.contains(x, y, 6.0))
                || hold_for_tests();
            if over {
                inside = Instant::now();
            } else if inside.elapsed() >= LEAVE {
                log_for_tests(&format!(
                    "leave: cursor {:?}, notch {notch:?}, island {island:?}",
                    cursor()
                ));
                break;
            }
        }
        done();
    });
}

/// Linux: то же в главном потоке — GDK знает курсор только там. Курсор над своим окном,
/// без координат экрана: так работает и Wayland.
#[cfg(target_os = "linux")]
pub fn watch(shared: Handle, done: impl FnOnce() + Send + 'static) {
    use gtk::glib;
    use std::time::Instant;
    let mut inside = Instant::now();
    let mut done = Some(done);
    glib::timeout_add_local(Duration::from_millis(30), move || {
        let (areas, stop) = match shared.lock() {
            Ok(s) => (s.areas, s.stop),
            Err(_) => (None, true),
        };
        let leave = match areas.filter(|_| !stop) {
            None => true,
            Some((notch, island)) => {
                let at = crate::tabs::webview::pointer();
                let over = at
                    .is_some_and(|(x, y)| notch.contains(x, y, 6.0) || island.contains(x, y, 6.0))
                    || hold_for_tests();
                if over {
                    inside = Instant::now();
                    false
                } else if inside.elapsed() >= LEAVE {
                    log_for_tests(&format!(
                        "leave: cursor {at:?}, notch {notch:?}, island {island:?}"
                    ));
                    true
                } else {
                    false
                }
            }
        };
        if !leave {
            return glib::ControlFlow::Continue;
        }
        if let Some(done) = done.take() {
            done();
        }
        glib::ControlFlow::Break
    });
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn watch(_shared: Handle, done: impl FnOnce() + Send + 'static) {
    done();
}

/// Только debug-сборка: `AXIOM_PEEK_HOLD=1` держит видео без мыши (тесты через DevTools
/// настоящий курсор не двигают).
#[cfg(any(windows, target_os = "linux"))]
fn hold_for_tests() -> bool {
    cfg!(debug_assertions) && std::env::var_os("AXIOM_PEEK_HOLD").is_some()
}

/// Только debug-сборка: `AXIOM_PEEK_LOG=<файл>` — куда записать, почему видео уехало.
#[cfg(any(windows, target_os = "linux"))]
fn log_for_tests(line: &str) {
    use std::io::Write;
    if !cfg!(debug_assertions) {
        return;
    }
    if let Some(path) = std::env::var_os("AXIOM_PEEK_LOG")
        && let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
    {
        let _ = writeln!(f, "{line}");
    }
}

#[cfg(windows)]
fn cursor() -> Option<(f64, f64)> {
    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetCursorPos(point: *mut [i32; 2]) -> i32;
    }
    let mut p = [0i32; 2];
    // SAFETY: указатель на локальный массив из двух i32 (POINT).
    (unsafe { GetCursorPos(&mut p) } != 0).then(|| (f64::from(p[0]), f64::from(p[1])))
}

/// Оставляет от webview только вырез `w × h` от его левого верхнего угла (логические px)
/// со скруглёнными нижними углами `radius`; `None` — снова весь. Верхние углы прямые: там
/// остров заходит под чёлку. Вне выреза webview не рисуется и мышь не ловит.
#[cfg(windows)]
pub fn round_bottom(webview: &wry::WebView, cut: Option<(f64, f64)>, radius: f64, scale: f64) {
    use windows::Win32::Foundation::HWND;
    use wry::WebViewExtWindows;
    #[link(name = "gdi32")]
    unsafe extern "system" {
        fn CreateRoundRectRgn(x1: i32, y1: i32, x2: i32, y2: i32, w: i32, h: i32) -> isize;
    }
    #[link(name = "user32")]
    unsafe extern "system" {
        fn SetWindowRgn(hwnd: HWND, rgn: isize, redraw: i32) -> i32;
    }
    let px = |v: f64| (v * scale).round() as i32;
    let mut hwnd = HWND::default();
    // SAFETY: контроллер принадлежит живому webview; его родитель — окно-контейнер wry
    // внутри нашего окна. После SetWindowRgn регионом владеет система.
    unsafe {
        if webview.controller().ParentWindow(&mut hwnd).is_err() || hwnd.is_invalid() {
            return;
        }
        let rgn = match cut {
            Some((w, h)) => {
                let d = 2 * px(radius);
                CreateRoundRectRgn(0, -d, px(w) + 1, px(h) + 1, d, d)
            }
            None => 0,
        };
        SetWindowRgn(hwnd, rgn, 1);
    }
}

/// Linux: webview WebKitGTK на GTK3 — дочернее окно GDK, оно рисуется вместе с окном
/// браузера. Картинку режет обработчик `draw` самого webview: он срабатывает раньше, чем
/// WebKitGTK рисует страницу, и обрезает кадр по вырезу (полосы — прямоугольники, такую
/// обрезку понимает и рисование через GL). Мышь режет форма окна GDK: вне выреза клики и
/// колесо достаются странице под ним. Координаты GDK — уже логические.
#[cfg(target_os = "linux")]
pub fn round_bottom(webview: &wry::WebView, cut: Option<(f64, f64)>, radius: f64, _scale: f64) {
    use gtk::{cairo, gdk, glib, glib::translate::ToGlibPtr, prelude::*};
    use std::{cell::RefCell, rc::Rc};
    use wry::WebViewExtUnix;

    type Rows = Rc<RefCell<Vec<(i32, i32, i32, i32)>>>;
    thread_local! {
        /// Webview под чёлкой, его обработчик `draw` и вырез, по которому тот режет кадр.
        static CLIP: RefCell<Option<(glib::WeakRef<gtk::Widget>, glib::SignalHandlerId, Rows)>> =
            const { RefCell::new(None) };
    }

    let widget = webview.webview().upcast::<gtk::Widget>();
    let rows =
        cut.map(|(w, h)| rounded_rows(w.round() as i32, h.round() as i32, radius.round() as i32));
    CLIP.with(|clip| {
        let mut clip = clip.borrow_mut();
        let same = clip
            .as_ref()
            .is_some_and(|(w, _, _)| w.upgrade().is_some_and(|w| w == widget));
        // Показ кончился или перешёл на другую вкладку — прежний обработчик долой.
        if (rows.is_none() || !same)
            && let Some((w, id, _)) = clip.take()
            && let Some(w) = w.upgrade()
        {
            w.disconnect(id);
        }
        let Some(rows) = &rows else {
            return;
        };
        if let Some((_, _, now)) = clip.as_ref() {
            *now.borrow_mut() = rows.clone();
            return;
        }
        let now: Rows = Rc::new(RefCell::new(rows.clone()));
        let cut = now.clone();
        let id = widget.connect_draw(move |_, cr| {
            for &(x, y, w, h) in cut.borrow().iter() {
                cr.rectangle(f64::from(x), f64::from(y), f64::from(w), f64::from(h));
            }
            cr.clip();
            glib::Propagation::Proceed
        });
        *clip = Some((widget.downgrade(), id, now));
    });
    widget.queue_draw();
    // Без своего окна `window()` — окно родителя: обрезали бы весь браузер.
    if !widget.has_window() {
        return;
    }
    let Some(window) = widget.window() else {
        return;
    };
    let region = rows.map(|rows| {
        let rects: Vec<_> = rows
            .into_iter()
            .map(|(x, y, w, h)| cairo::RectangleInt::new(x, y, w, h))
            .collect();
        cairo::Region::create_rectangles(&rects)
    });
    window.shape_combine_region(region.as_ref(), 0, 0);
    // SAFETY: живое окно GDK; NULL снимает форму для мыши, регион GDK копирует.
    unsafe {
        let raw = region
            .as_ref()
            .map_or(std::ptr::null_mut(), |r| r.to_raw_none());
        gdk::ffi::gdk_window_input_shape_combine_region(
            window.to_glib_none().0,
            raw.cast_const(),
            0,
            0,
        );
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn round_bottom(_webview: &wry::WebView, _cut: Option<(f64, f64)>, _radius: f64, _scale: f64) {}

/// Вырез `w × h` со скруглёнными нижними углами радиуса `r` — полосами (x, y, ширина,
/// высота): верх целиком, ниже — по строке на каждый пиксель скругления.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn rounded_rows(w: i32, h: i32, r: i32) -> Vec<(i32, i32, i32, i32)> {
    let (w, h) = (w.max(1), h.max(1));
    let r = r.clamp(0, (w / 2).min(h));
    let mut rows = Vec::with_capacity(r as usize + 1);
    if h > r {
        rows.push((0, 0, w, h - r));
    }
    let rf = f64::from(r);
    for i in 0..r {
        // Середина строки — на `dy` ниже центров скруглений.
        let dy = f64::from(i) + 0.5;
        let inset = (rf - (rf * rf - dy * dy).max(0.0).sqrt()).round() as i32;
        rows.push((inset, h - r + i, (w - 2 * inset).max(1), 1));
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOCK: Dock = Dock {
        cx: 800.0,
        top: 22.0,
        width: 400.0,
        notch_top: 0.0,
    };

    #[test]
    fn island_fits_the_video() {
        let a = DOCK.island(1280.0, 720.0);
        assert_eq!((a.x, a.y, a.w), (600.0, 22.0, 400.0));
        // Видео во всю ширину внутри рамки: 384 × 216 и рамка сверху и снизу.
        assert_eq!(a.h, 216.0 + 16.0);
        // Вертикальное — не выше квадрата, неизвестное — 16:9.
        assert_eq!(DOCK.island(720.0, 1280.0).h, 384.0 + 16.0);
        assert_eq!(DOCK.island(0.0, 0.0).h, 216.0 + 16.0);
    }

    #[test]
    fn hover_area() {
        let notch = DOCK.notch();
        assert!(notch.contains(800.0, 10.0, 0.0));
        assert!(!notch.contains(500.0, 10.0, 0.0));
        assert!(notch.contains(597.0, 10.0, 6.0));
        let island = DOCK.island(16.0, 9.0);
        assert!(island.contains(800.0, 200.0, 0.0));
        assert!(!island.contains(800.0, 300.0, 0.0));
        // Карточка в боковой панели: чёлка — от её верха до острова.
        let panel = Dock {
            cx: 120.0,
            top: 120.0,
            width: 228.0,
            notch_top: 86.0,
        };
        assert!(panel.notch().contains(120.0, 100.0, 0.0));
        assert!(!panel.notch().contains(120.0, 50.0, 0.0));
    }

    #[test]
    fn rounded_cut() {
        let rows = rounded_rows(400, 232, 22);
        // Верх целиком, потом 22 строки скругления.
        assert_eq!(rows[0], (0, 0, 400, 210));
        assert_eq!(rows.len(), 23);
        // Сразу под прямой частью — во всю ширину, внизу — уже всего, симметрично.
        assert_eq!(rows[1], (0, 210, 400, 1));
        let last = rows[22];
        assert_eq!(last.1, 231);
        assert!(last.0 > 10 && last.0 + last.2 == 400 - last.0);
        // Пока остров только выезжает (ниже радиуса) — без прямой части и без паники.
        let low = rounded_rows(400, 5, 22);
        assert_eq!(low.len(), 5);
        assert!(
            low.iter()
                .all(|&(x, y, w, h)| x >= 0 && (0..5).contains(&y) && w >= 1 && h == 1)
        );
        assert_eq!(rounded_rows(0, 0, 22), vec![(0, 0, 1, 1)]);
    }

    #[test]
    fn video_size_from_page() {
        assert_eq!(video_size(r#"{"w":1280,"h":720}"#), Some((1280.0, 720.0)));
        assert_eq!(video_size("null"), None);
        assert_eq!(video_size(""), None);
        assert_eq!(video_size(r#"{"w":0,"h":720}"#), None);
    }

    #[test]
    fn animation() {
        assert!(open_k(0).abs() < 1e-9);
        assert!((open_k(FRAMES) - 1.0).abs() < 1e-9);
        // Пружинка: перед концом чуть длиннее.
        assert!(open_k(FRAMES - 3) > 1.0);
        assert!((close_k(0) - 1.0).abs() < 1e-9);
        assert!(close_k(FRAMES).abs() < 1e-9);
        let a = DOCK.island(16.0, 9.0);
        assert_eq!(a.grown(0.5).h, a.h / 2.0);
        assert_eq!(a.grown(0.0).h, 1.0);
    }
}
