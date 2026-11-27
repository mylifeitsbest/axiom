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
//! Выезд и уезд — одно чёрное целое с чёлкой: остров растёт вниз и вширь от ширины
//! чёлки, а чёлка (CSS-переход в `ui/styles.css`, `.isl.vid`) раздаётся до его ширины
//! с той же длительностью и кривой ([`OPEN`], [`CLOSE`]) и в тот же миг — интерфейс
//! узнаёт о начале по `ToUi::Peek`. Доля считается по часам ([`Anim`]), а не по
//! номеру кадра: опоздавший кадр не замедляет и не сбивает анимацию.
//!
//! Linux: webview вкладки — дочернее окно GDK внутри окна браузера; остров — его форма
//! (`gdk_window_shape_combine_region`: вне неё окно не рисуется и мышь не ловит), а
//! курсор берётся у GDK относительно своего окна — координат экрана на Wayland нет.

use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

/// Чёрная рамка вокруг видео, логические px (её же рисует страница, см. `tabs::media`).
pub const PAD: f64 = 8.0;
/// Скругление острова снизу, логические px.
pub const RADIUS: f64 = 22.0;
/// Сколько курсор может быть вне чёлки и острова, прежде чем видео спрячется.
#[cfg(any(windows, target_os = "linux"))]
pub const LEAVE: Duration = Duration::from_millis(350);
/// Сколько ждать, пока страница нарисует видео, прежде чем выехать без её ответа.
pub const PAINT_WAIT: Duration = Duration::from_millis(300);
/// Остров трогается на столько позже, чем интерфейсу уходит `ToUi::Peek`: пока сообщение
/// дойдёт и чёлка начнёт раздаваться, проходит около кадра — так они идут вровень.
pub const UI_LAG: Duration = Duration::from_millis(0);

/// Кривая `cubic-bezier(x1, y1, x2, y2)`, как в CSS.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bezier(pub f64, pub f64, pub f64, pub f64);

impl Bezier {
    /// Значение кривой в момент `x` (0…1), как его считает браузер для CSS-перехода.
    pub fn at(self, x: f64) -> f64 {
        let Self(x1, y1, x2, y2) = self;
        // Края — ровно 0 и 1: начало и конец анимации без остатка от деления пополам.
        if x <= 0.0 {
            return 0.0;
        }
        if x >= 1.0 {
            return 1.0;
        }
        let coord = |a: f64, b: f64, s: f64| {
            let r = 1.0 - s;
            3.0 * r * r * s * a + 3.0 * r * s * s * b + s * s * s
        };
        // x(s) монотонна (x1, x2 в 0…1): параметр — делением пополам.
        let (mut lo, mut hi) = (0.0, 1.0);
        for _ in 0..32 {
            let mid = (lo + hi) / 2.0;
            if coord(x1, x2, mid) < x {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        coord(y1, y2, (lo + hi) / 2.0)
    }
}

/// Длительность (мс) и кривая выезда или уезда.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Motion {
    pub ms: f64,
    pub curve: Bezier,
}

/// Выезд: мягко трогается (не выстреливает четвертью острова за пару кадров), долго
/// и плавно встаёт на место, без перелёта (чёлка шириной ровно с остров). То же —
/// у ширины чёлки в `.isl.vid` (ui/styles.css), у промежутка в ряду вкладок, у сдвига
/// надписи в чёлке (`VID_IN` в ui/island.js) и у проявления видео на странице.
pub const OPEN: Motion = Motion {
    ms: 460.0,
    curve: Bezier(0.3, 0.3, 0.0, 1.0),
};
/// Уезд в чёлку: мягко трогается и мягко прячется. То же — `.isl.vid-out`, `VID_OUT`.
pub const CLOSE: Motion = Motion {
    ms: 300.0,
    curve: Bezier(0.4, 0.0, 0.2, 1.0),
};
/// Видео гаснет быстрее, чем уезжает остров: в чёлку уходит уже чёрное.
pub const FADE_OUT: f64 = 0.7;

/// Идущий выезд или уезд: доля острова от `from` до 1 (выезд) или 0 (уезд) по часам.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Anim {
    pub open: bool,
    pub start: Instant,
    /// С какой доли начали: уезд с середины выезда (и наоборот) — без скачка.
    pub from: f64,
    /// Длительность, мс (уже умножена на скорость анимаций интерфейса).
    pub ms: f64,
    pub curve: Bezier,
}

impl Anim {
    /// `speed` — скорость анимаций интерфейса (0 — без анимации, см. `ui/app.js`).
    ///
    /// Развернулся с полпути — и короче во столько же раз, сколько осталось пройти: так
    /// CSS укорачивает обращённый переход (reversing shortening factor), и чёлка с
    /// островом и тут идут вровень.
    pub fn new(open: bool, from: f64, speed: f64, start: Instant) -> Self {
        let m = if open { OPEN } else { CLOSE };
        let from = from.clamp(0.0, 1.0);
        let left = if open { 1.0 - from } else { from };
        Self {
            open,
            start,
            from,
            ms: m.ms * speed.clamp(0.0, 3.0) * left,
            curve: m.curve,
        }
    }

    /// Пройденная часть времени, 0…1.
    fn t(&self, now: Instant) -> f64 {
        if self.ms <= 0.0 {
            return 1.0;
        }
        let ms = now.saturating_duration_since(self.start).as_secs_f64() * 1000.0;
        (ms / self.ms).clamp(0.0, 1.0)
    }

    /// Доля острова сейчас.
    pub fn k(&self, now: Instant) -> f64 {
        let to = if self.open { 1.0 } else { 0.0 };
        self.from + (to - self.from) * self.curve.at(self.t(now))
    }

    pub fn done(&self, now: Instant) -> bool {
        self.t(now) >= 1.0
    }

    /// Когда кончится.
    pub fn end(&self) -> Instant {
        self.start + Duration::from_secs_f64(self.ms.max(0.0) / 1000.0)
    }
}

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

    /// Видимая часть острова на доле `k` выезда: посередине, шириной от `from` (ширина
    /// чёлки без видео; 0 — сразу во всю) до всей, высотой от нуля до всей.
    pub fn cut(self, k: f64, from: f64) -> Cut {
        let k = k.clamp(0.0, 1.0);
        let from = if from > 0.0 { from.min(self.w) } else { self.w };
        let w = from + (self.w - from) * k;
        Cut {
            x: (self.w - w) / 2.0,
            w,
            h: self.h * k,
        }
    }
}

/// Видимая часть острова от его левого верхнего угла, логические px.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cut {
    pub x: f64,
    pub w: f64,
    pub h: f64,
}

/// Остров под чёлкой: середина чёлки, верх острова (он заходит под низ чёлки), ширина и
/// верх самой чёлки; `from` — ширина чёлки, пока видео нет (из неё остров раздаётся
/// вширь); `speed` — скорость анимаций интерфейса. Логические px окна, от интерфейса.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Dock {
    pub cx: f64,
    pub top: f64,
    pub width: f64,
    pub notch_top: f64,
    pub from: f64,
    pub speed: f64,
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

/// Идущий показ видео в островке.
#[derive(Debug)]
pub struct Show {
    pub id: crate::tabs::TabId,
    /// Размер видео; `None` — страница ещё не ответила.
    pub video: Option<(f64, f64)>,
    pub shared: Handle,
    /// Уже уезжает.
    pub closing: bool,
    /// Выезд или уезд; `None` — остров ещё не тронулся (страница готовит видео).
    pub anim: Option<Anim>,
    /// Номер выезда или уезда: кадры прежнего, начавшегося раньше, не нужны.
    pub run: u32,
    /// Куда страница уже поставила видео (x, y, w, h, масштаб) — не слать то же самое.
    pub sent: Option<(f64, f64, f64, f64, f64)>,
    /// Где уже стоит вкладка (x, y, ширина, высота; логические px) — не двигать зря.
    pub at: Option<(f64, f64, f64, f64)>,
    /// Номер показа: «видео нарисовано» от прежнего показа той же вкладки не трогает
    /// остров нового раньше, чем страница нарисует видео.
    pub serial: u32,
}

static SERIAL: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

impl Show {
    pub fn new(id: crate::tabs::TabId) -> Self {
        Self {
            id,
            serial: SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            video: None,
            shared: Handle::default(),
            closing: false,
            anim: None,
            run: 0,
            sent: None,
            at: None,
        }
    }

    /// Доля острова сейчас: 0 — ещё не выехал (или уже уехал), 1 — весь.
    pub fn k(&self, now: Instant) -> f64 {
        self.anim.map_or(0.0, |a| a.k(now))
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

/// Только debug-сборка: `AXIOM_PEEK_LOG=<файл>` — куда записать, почему видео уехало, и
/// кадры выезда (время от начала эпохи в мс — сверить с чёлкой в интерфейсе).
pub fn log_for_tests(line: &str) {
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
        let ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0.0, |d| d.as_secs_f64() * 1000.0);
        let _ = writeln!(f, "{ms:.1} {line}");
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

/// Ждёт следующего кадра: на Windows — следующего вывода композитора (DWM), чтобы остров
/// менялся ровно раз за кадр экрана, а не то дважды, то ни разу; иначе — 8 мс.
pub fn next_frame() {
    #[cfg(windows)]
    {
        #[link(name = "dwmapi")]
        unsafe extern "system" {
            fn DwmFlush() -> i32;
        }
        let t = Instant::now();
        // SAFETY: без аргументов; только ждёт ближайшего вывода композитора.
        if unsafe { DwmFlush() } >= 0 {
            // Выводить нечего — DWM может вернуть сразу: не крутимся вхолостую.
            if t.elapsed() < Duration::from_millis(2) {
                std::thread::sleep(Duration::from_millis(4));
            }
            return;
        }
    }
    std::thread::sleep(Duration::from_millis(8));
}

/// Оставляет от webview только вырез `cut` (от его левого верхнего угла, логические px)
/// со скруглёнными нижними углами `radius`; `None` — снова весь. Верхние углы прямые: там
/// остров заходит под чёлку. Вне выреза webview не рисуется и мышь не ловит; вырез
/// нулевой высоты — не видно ничего.
#[cfg(windows)]
pub fn round_bottom(webview: &wry::WebView, cut: Option<Cut>, radius: f64, scale: f64) {
    use windows::Win32::Foundation::HWND;
    use wry::WebViewExtWindows;
    #[link(name = "gdi32")]
    unsafe extern "system" {
        fn CreateRoundRectRgn(x1: i32, y1: i32, x2: i32, y2: i32, w: i32, h: i32) -> isize;
        fn CreateRectRgn(x1: i32, y1: i32, x2: i32, y2: i32) -> isize;
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
            Some(c) if px(c.h) > 0 && px(c.w) > 0 => {
                let d = 2 * px(radius.min(c.w / 2.0));
                CreateRoundRectRgn(px(c.x), -d, px(c.x + c.w) + 1, px(c.h) + 1, d, d)
            }
            Some(_) => CreateRectRgn(0, 0, 0, 0),
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
pub fn round_bottom(webview: &wry::WebView, cut: Option<Cut>, radius: f64, _scale: f64) {
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
    let rows = cut.map(|c| {
        rounded_rows(
            c.x.round() as i32,
            c.w.round() as i32,
            c.h.round() as i32,
            radius.round() as i32,
        )
    });
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
pub fn round_bottom(_webview: &wry::WebView, _cut: Option<Cut>, _radius: f64, _scale: f64) {}

/// Вырез `w × h` с левого края `x` со скруглёнными нижними углами радиуса `r` — полосами
/// (x, y, ширина, высота): верх целиком, ниже — по строке на каждый пиксель скругления.
/// Нулевой высоты или ширины — пусто.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn rounded_rows(x: i32, w: i32, h: i32, r: i32) -> Vec<(i32, i32, i32, i32)> {
    if w < 1 || h < 1 {
        return Vec::new();
    }
    let r = r.clamp(0, (w / 2).min(h));
    let mut rows = Vec::with_capacity(r as usize + 1);
    if h > r {
        rows.push((x, 0, w, h - r));
    }
    let rf = f64::from(r);
    for i in 0..r {
        // Середина строки — на `dy` ниже центров скруглений.
        let dy = f64::from(i) + 0.5;
        let inset = (rf - (rf * rf - dy * dy).max(0.0).sqrt()).round() as i32;
        rows.push((x + inset, h - r + i, (w - 2 * inset).max(1), 1));
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
        from: 240.0,
        speed: 1.0,
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
            from: 228.0,
            speed: 1.0,
        };
        assert!(panel.notch().contains(120.0, 100.0, 0.0));
        assert!(!panel.notch().contains(120.0, 50.0, 0.0));
    }

    #[test]
    fn rounded_cut() {
        let rows = rounded_rows(0, 400, 232, 22);
        // Верх целиком, потом 22 строки скругления.
        assert_eq!(rows[0], (0, 0, 400, 210));
        assert_eq!(rows.len(), 23);
        // Сразу под прямой частью — во всю ширину, внизу — уже всего, симметрично.
        assert_eq!(rows[1], (0, 210, 400, 1));
        let last = rows[22];
        assert_eq!(last.1, 231);
        assert!(last.0 > 10 && last.0 + last.2 == 400 - last.0);
        // Пока остров только выезжает (ниже радиуса) — без прямой части и без паники.
        let low = rounded_rows(0, 400, 5, 22);
        assert_eq!(low.len(), 5);
        assert!(
            low.iter()
                .all(|&(x, y, w, h)| x >= 0 && (0..5).contains(&y) && w >= 1 && h == 1)
        );
        // Ещё не выехал — пусто.
        assert!(rounded_rows(0, 0, 0, 22).is_empty());
        assert!(rounded_rows(80, 240, 0, 22).is_empty());
        // Уже острова — посередине: всё сдвинуто на `x`.
        let mid = rounded_rows(80, 240, 100, 22);
        assert_eq!(mid[0], (80, 0, 240, 78));
        assert!(mid.iter().all(|&(x, _, w, _)| x >= 80 && x + w <= 320));
    }

    #[test]
    fn cut_grows_from_the_notch() {
        let a = DOCK.island(16.0, 9.0);
        // Ещё не выехал: шириной с чёлку, посередине, высоты нет.
        assert_eq!(
            a.cut(0.0, 240.0),
            Cut {
                x: 80.0,
                w: 240.0,
                h: 0.0
            }
        );
        // Выехал: весь остров.
        assert_eq!(
            a.cut(1.0, 240.0),
            Cut {
                x: 0.0,
                w: 400.0,
                h: a.h
            }
        );
        // Посередине пути — посередине и по ширине, и по высоте.
        assert_eq!(
            a.cut(0.5, 240.0),
            Cut {
                x: 40.0,
                w: 320.0,
                h: a.h / 2.0
            }
        );
        // Чёлка без видео не шире острова; ширины нет — сразу во всю.
        assert_eq!(a.cut(0.0, 900.0).w, 400.0);
        assert_eq!(a.cut(0.0, 0.0).w, 400.0);
        // За пределы не выходит.
        assert_eq!(a.cut(1.3, 240.0), a.cut(1.0, 240.0));
    }

    #[test]
    fn video_size_from_page() {
        assert_eq!(video_size(r#"{"w":1280,"h":720}"#), Some((1280.0, 720.0)));
        assert_eq!(video_size("null"), None);
        assert_eq!(video_size(""), None);
        assert_eq!(video_size(r#"{"w":0,"h":720}"#), None);
    }

    #[test]
    fn bezier_matches_css() {
        // Края — всегда 0 и 1; `linear` — прямая.
        for c in [OPEN.curve, CLOSE.curve] {
            assert!(c.at(0.0).abs() < 1e-6);
            assert!((c.at(1.0) - 1.0).abs() < 1e-6);
        }
        let linear = Bezier(0.0, 0.0, 1.0, 1.0);
        assert!((linear.at(0.3) - 0.3).abs() < 1e-6);
        // `ease` в CSS на середине ≈ 0.8024.
        assert!((Bezier(0.25, 0.1, 0.25, 1.0).at(0.5) - 0.8024).abs() < 1e-3);
    }

    #[test]
    fn open_is_smooth() {
        let ms = |v: u64| Duration::from_millis(v);
        let t0 = Instant::now();
        let a = Anim::new(true, 0.0, 1.0, t0);
        assert_eq!(a.k(t0), 0.0);
        // Мягко трогается: первый кадр (60 Гц) — малая доля, а не четверть острова разом,
        // и за первые 100 мс — меньше половины пути.
        let first = a.k(t0 + ms(17));
        assert!(first > 0.02 && first < 0.08, "{first}");
        let early = a.k(t0 + ms(100));
        assert!(early > 0.3 && early < 0.6, "{early}");
        // Без перелёта и всё время вперёд.
        let mut last = 0.0;
        for i in 0..=50 {
            let k = a.k(t0 + ms(i * 10));
            assert!(k >= last - 1e-9 && k <= 1.0 + 1e-9, "{i}: {k}");
            last = k;
        }
        assert!(a.done(t0 + ms(460)) && !a.done(t0 + ms(400)));
        assert_eq!(a.k(t0 + ms(1000)), 1.0);
        // Уезд с середины выезда — с той же доли, без скачка, и до нуля.
        let mid = a.k(t0 + ms(100));
        let b = Anim::new(false, mid, 1.0, t0 + ms(100));
        assert!((b.k(t0 + ms(100)) - mid).abs() < 1e-9);
        assert_eq!(b.k(t0 + ms(400)), 0.0);
        // …и короче настолько, насколько ближе цель (как обращённый переход в CSS).
        assert!((b.ms - CLOSE.ms * mid).abs() < 1e-6);
        let back = Anim::new(true, 0.25, 1.0, t0);
        assert!((back.ms - OPEN.ms * 0.75).abs() < 1e-6);
        // Скорость анимаций интерфейса растягивает и остров.
        let slow = Anim::new(true, 0.0, 2.0, t0);
        assert!(!slow.done(t0 + ms(900)) && slow.done(t0 + ms(920)));
        // Анимации выключены — сразу на месте.
        let c = Anim::new(true, 0.0, 0.0, t0);
        assert!(c.done(t0));
        assert_eq!(c.k(t0), 1.0);
    }
}
