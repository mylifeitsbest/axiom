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

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

/// Чёрная рамка вокруг видео, логические px (её же рисует страница, см. `tabs::media`).
pub const PAD: f64 = 8.0;
/// Скругление острова снизу, логические px.
pub const RADIUS: f64 = 22.0;
/// Сколько курсор может быть вне чёлки и острова, прежде чем видео спрячется.
pub const LEAVE: Duration = Duration::from_millis(350);
/// Кадров анимации выезда и уезда.
pub const FRAMES: u8 = 14;
pub const FRAME: Duration = Duration::from_millis(14);

/// Прямоугольник в логических px окна (или в px экрана — см. [`Shared`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Area {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Area {
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
    /// Чёлка и остров в px экрана; `None` — показа больше нет.
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

/// Linux: островок видео не показывает (см. `tabs::media::PAGE_SCRIPT`).
#[cfg(not(windows))]
pub fn watch(_shared: Handle, done: impl FnOnce() + Send + 'static) {
    done();
}

/// Только debug-сборка: `AXIOM_PEEK_HOLD=1` держит видео без мыши (тесты через DevTools
/// настоящий курсор не двигают).
#[cfg(windows)]
fn hold_for_tests() -> bool {
    cfg!(debug_assertions) && std::env::var_os("AXIOM_PEEK_HOLD").is_some()
}

/// Только debug-сборка: `AXIOM_PEEK_LOG=<файл>` — куда записать, почему видео уехало.
#[cfg(windows)]
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

/// Оставляет от webview только вырез `x, w, h` от его верха (px экрана) со скруглёнными
/// нижними углами; `None` — снова весь. Верхние углы прямые: там остров заходит под чёлку.
/// Вне выреза webview не рисуется и мышь не ловит.
#[cfg(windows)]
pub fn round_bottom(webview: &wry::WebView, cut: Option<(i32, i32, i32)>, radius: i32) {
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
    let mut hwnd = HWND::default();
    // SAFETY: контроллер принадлежит живому webview; его родитель — окно-контейнер wry
    // внутри нашего окна. После SetWindowRgn регионом владеет система.
    unsafe {
        if webview.controller().ParentWindow(&mut hwnd).is_err() || hwnd.is_invalid() {
            return;
        }
        let rgn = match cut {
            Some((x, w, h)) => {
                let d = 2 * radius;
                CreateRoundRectRgn(x, -d, x + w + 1, h + 1, d, d)
            }
            None => 0,
        };
        SetWindowRgn(hwnd, rgn, 1);
    }
}

#[cfg(not(windows))]
pub fn round_bottom(_webview: &wry::WebView, _cut: Option<(i32, i32, i32)>, _radius: i32) {}

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
