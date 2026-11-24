//! Снимок страницы под окнами интерфейса.
//!
//! Подсказки адресной строки, загрузки, настройки и другие окна — разметка
//! webview интерфейса, а страница и панель ИИ — нативные webview поверх него.
//! Нарисовать окно поверх нативного webview нельзя, поэтому на время окна они
//! прячутся, и раньше на их месте оставался пустой фон. Теперь webview сначала
//! снимается (WebView2 `CapturePreview`), интерфейс кладёт снимок ровно на его
//! место, и только потом webview прячется. При закрытии наоборот: webview снова
//! на экране, и лишь после этого интерфейс убирает снимок.
//!
//! Картинка идёт в интерфейс не через `evaluate_script` (мегабайты base64), а по
//! его же протоколу: `snapshot/<n>-page.png`. Обработчик этого протокола есть
//! только у webview интерфейса, сайты во вкладках адрес не получат.
//!
//! Не успели снять за [`CAPTURE_LIMIT`] или снять не вышло — webview прячется
//! сразу, как раньше. На Linux снимков нет: WebKitGTK прячет webview без них.

use std::{
    sync::Mutex,
    thread,
    time::{Duration, Instant},
};

use serde::Serialize;
use tao::event_loop::EventLoopProxy;
use wry::WebView;

use super::TabId;
use crate::events::UserEvent;

/// Дольше снимка не ждём: окно должно открываться сразу.
const CAPTURE_LIMIT: Duration = Duration::from_millis(150);
/// Сколько ждать, пока интерфейс положит снимок на место.
const SHOW_LIMIT: Duration = Duration::from_millis(250);
/// Снимок больше этого — не страница, а ошибка: не храним.
#[cfg_attr(not(windows), allow(dead_code))]
const MAX_BYTES: u64 = 64 * 1024 * 1024;
/// PNG — пиксель в пиксель, вместе с субпиксельным сглаживанием текста, но сжимается
/// он медленнее JPEG (~20 мс на мегапиксель). Огромный webview (4K) снимаем в JPEG,
/// иначе не уложимся в [`CAPTURE_LIMIT`]; на такой плотности его огрехи не видны.
const PNG_MAX_PIXELS: i64 = 4_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    Png,
    Jpeg,
}

impl Format {
    fn for_rect(rect: PxRect) -> Self {
        if i64::from(rect.w) * i64::from(rect.h) <= PNG_MAX_PIXELS {
            Self::Png
        } else {
            Self::Jpeg
        }
    }

    fn ext(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
        }
    }

    fn mime(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
        }
    }
}

/// Что снимаем: страницу активной вкладки или панель ИИ.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    Page,
    Ai,
}

impl Part {
    fn name(self) -> &'static str {
        match self {
            Self::Page => "page",
            Self::Ai => "ai",
        }
    }
}

/// Место webview в физических пикселях окна — там же снимок и ляжет.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(not(windows), allow(dead_code))]
pub struct PxRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

#[derive(Debug)]
pub enum Event {
    /// WebView2 закончил снимок: `ok` — картинка уже лежит в хранилище.
    #[cfg_attr(not(windows), allow(dead_code))]
    Captured { seq: u64, part: Part, ok: bool },
    /// Снимок не успел за [`CAPTURE_LIMIT`].
    CaptureTimeout(u64),
    /// Интерфейс не подтвердил снимок за [`SHOW_LIMIT`].
    ShowTimeout(u64),
}

/// Сообщение интерфейсу. Части без снимка интерфейс не трогает,
/// `clear` — убрать всё: webview снова на экране.
#[derive(Debug, Serialize)]
pub struct View {
    seq: u64,
    clear: bool,
    page: Option<Shot>,
    ai: Option<Shot>,
}

#[derive(Debug, Serialize)]
struct Shot {
    src: String,
    #[serde(flatten)]
    rect: PxRect,
    /// Чья это страница: после переключения вкладки интерфейс снимок уберёт.
    tab: Option<TabId>,
}

/// Что делать приложению после шага.
pub enum Step {
    Wait,
    /// Отдать снимок интерфейсу; webview пока на экране.
    Show(View),
    /// Спрятать webview (снимок уже на месте или его не будет) и,
    /// если есть, отправить интерфейсу сообщение.
    Hide(Option<View>),
}

/// Webview, который можно снять: его место и ссылка на ядро WebView2.
#[cfg_attr(not(windows), allow(dead_code))]
pub struct Target {
    part: Part,
    tab: Option<TabId>,
    rect: PxRect,
    #[cfg(windows)]
    core: webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2,
}

/// Готовит снимок видимого webview. `None` — снимать нечего (или нечем).
#[cfg(windows)]
pub fn target(webview: &WebView, part: Part, tab: Option<TabId>) -> Option<Target> {
    use wry::WebViewExtWindows;
    // На Windows wry отдаёт место в физических пикселях: масштаб ни на что не влияет.
    let bounds = webview.bounds().ok()?;
    let pos = bounds.position.to_physical::<i32>(1.0);
    let size = bounds.size.to_physical::<i32>(1.0);
    let rect = PxRect {
        x: pos.x,
        y: pos.y,
        w: size.width,
        h: size.height,
    };
    (rect.w > 0 && rect.h > 0).then(|| Target {
        part,
        tab,
        rect,
        core: webview.webview(),
    })
}

#[cfg(not(windows))]
pub fn target(_webview: &WebView, _part: Part, _tab: Option<TabId>) -> Option<Target> {
    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Waiting,
    Ready,
    Failed,
}

struct Taken {
    part: Part,
    tab: Option<TabId>,
    rect: PxRect,
    format: Format,
    state: State,
    #[cfg_attr(not(debug_assertions), allow(dead_code))]
    started: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Phase {
    /// Окна поверх нет.
    #[default]
    Off,
    /// Снимаем: webview ещё на экране.
    Capturing,
    /// Снимок у интерфейса, ждём, пока он ляжет на место.
    Showing,
    /// Webview спрятаны, на их месте снимок (или пустой фон, если снять не вышло).
    Covered,
}

#[derive(Default)]
pub struct Snapshots {
    /// Номер снимка: ответы на прежние снимки отбрасываются.
    seq: u64,
    phase: Phase,
    taken: Vec<Taken>,
}

impl Snapshots {
    /// Webview должны остаться на экране: снимок ещё не на месте.
    pub fn holding(&self) -> bool {
        matches!(self.phase, Phase::Capturing | Phase::Showing)
    }

    /// Окно открылось поверх страницы.
    pub fn begin(&mut self, targets: Vec<Target>, events: &EventLoopProxy<UserEvent>) -> Step {
        self.next_seq();
        self.taken = start(targets, self.seq, events);
        if self.taken.is_empty() {
            // Снимать нечего: прячем сразу, интерфейс не ждёт.
            self.phase = Phase::Covered;
            return Step::Hide(Some(self.view(false)));
        }
        self.phase = Phase::Capturing;
        after(CAPTURE_LIMIT, events, Event::CaptureTimeout(self.seq));
        Step::Wait
    }

    /// Активная вкладка сменилась, пока окно открыто.
    pub fn retake(&mut self, targets: Vec<Target>, events: &EventLoopProxy<UserEvent>) -> Step {
        match self.phase {
            Phase::Off => Step::Wait,
            Phase::Capturing | Phase::Showing => self.begin(targets, events),
            Phase::Covered => {
                // Webview уже спрятаны: снимаем новую страницу как есть (спрятанной)
                // и подменяем картинку. Не вышло — останется пустой фон, как раньше.
                self.next_seq();
                let page = targets.into_iter().filter(|t| t.part == Part::Page);
                self.taken = start(page.collect(), self.seq, events);
                Step::Wait
            }
        }
    }

    pub fn on_event(&mut self, event: Event, events: &EventLoopProxy<UserEvent>) -> Step {
        match event {
            Event::Captured { seq, part, ok } if seq == self.seq => {
                let Some(taken) = self.taken.iter_mut().find(|t| t.part == part) else {
                    return Step::Wait;
                };
                taken.state = if ok { State::Ready } else { State::Failed };
                #[cfg(debug_assertions)]
                eprintln!(
                    "снимок {}: {} за {} мс",
                    part.name(),
                    if ok { "готов" } else { "не вышел" },
                    taken.started.elapsed().as_millis()
                );
                if self.taken.iter().any(|t| t.state == State::Waiting) {
                    return Step::Wait;
                }
                let any = self.taken.iter().any(|t| t.state == State::Ready);
                match self.phase {
                    Phase::Capturing if any => {
                        self.phase = Phase::Showing;
                        after(SHOW_LIMIT, events, Event::ShowTimeout(self.seq));
                        Step::Show(self.view(false))
                    }
                    Phase::Capturing => {
                        self.phase = Phase::Covered;
                        Step::Hide(Some(self.view(false)))
                    }
                    // Пересъёмка под уже открытым окном: webview и так спрятаны.
                    Phase::Covered if any => Step::Show(self.view(false)),
                    _ => Step::Wait,
                }
            }
            Event::CaptureTimeout(seq) if seq == self.seq && self.phase == Phase::Capturing => {
                // Опоздавший снимок не покажем: после пустого фона он был бы скачком.
                self.next_seq();
                self.taken.clear();
                self.phase = Phase::Covered;
                Step::Hide(Some(self.view(false)))
            }
            Event::ShowTimeout(seq) if seq == self.seq && self.phase == Phase::Showing => {
                self.phase = Phase::Covered;
                Step::Hide(None)
            }
            _ => Step::Wait,
        }
    }

    /// Интерфейс положил снимок `seq` на место. `true` — теперь можно прятать webview.
    pub fn shown(&mut self, seq: u64) -> bool {
        if seq != self.seq {
            return false;
        }
        // Картинка у интерфейса уже разжата — байты больше не нужны.
        store_reset(self.seq);
        let hide = self.phase == Phase::Showing;
        if hide {
            self.phase = Phase::Covered;
        }
        hide
    }

    /// Окно закрылось. Сообщение отправлять, когда webview уже снова на экране.
    pub fn end(&mut self) -> View {
        self.next_seq();
        self.taken.clear();
        self.phase = Phase::Off;
        self.view(true)
    }

    fn next_seq(&mut self) {
        self.seq += 1;
        store_reset(self.seq);
    }

    fn view(&self, clear: bool) -> View {
        let shot = |part: Part| {
            self.taken
                .iter()
                .find(|t| t.part == part && t.state == State::Ready)
                .map(|t| Shot {
                    src: format!("snapshot/{}-{}.{}", self.seq, part.name(), t.format.ext()),
                    rect: t.rect,
                    tab: t.tab,
                })
        };
        View {
            seq: self.seq,
            clear,
            page: shot(Part::Page),
            ai: shot(Part::Ai),
        }
    }
}

/// Запускает снимки; в ответ придут `Event::Captured`.
fn start(targets: Vec<Target>, seq: u64, events: &EventLoopProxy<UserEvent>) -> Vec<Taken> {
    targets
        .into_iter()
        .filter_map(|t| {
            let format = Format::for_rect(t.rect);
            capture(&t, format, seq, events).then(|| Taken {
                part: t.part,
                tab: t.tab,
                rect: t.rect,
                format,
                state: State::Waiting,
                started: Instant::now(),
            })
        })
        .collect()
}

fn after(delay: Duration, events: &EventLoopProxy<UserEvent>, event: Event) {
    let events = events.clone();
    thread::spawn(move || {
        thread::sleep(delay);
        let _ = events.send_event(UserEvent::Snapshot(event));
    });
}

#[cfg(windows)]
fn capture(target: &Target, format: Format, seq: u64, events: &EventLoopProxy<UserEvent>) -> bool {
    use webview2_com::{
        CapturePreviewCompletedHandler,
        Microsoft::Web::WebView2::Win32::{
            COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_JPEG,
            COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_PNG,
        },
    };
    use windows::Win32::{
        Foundation::HGLOBAL, System::Com::StructuredStorage::CreateStreamOnHGlobal,
    };

    // SAFETY: пустой поток в памяти, освобождается вместе с последней ссылкой на него.
    let Ok(stream) = (unsafe { CreateStreamOnHGlobal(HGLOBAL::default(), true) }) else {
        return false;
    };
    let output = stream.clone();
    let part = target.part;
    let events = events.clone();
    let handler = CapturePreviewCompletedHandler::create(Box::new(move |result| {
        let ok = result.is_ok()
            && read_all(&output).is_some_and(|bytes| store_put(seq, part, format, bytes));
        let _ = events.send_event(UserEvent::Snapshot(Event::Captured { seq, part, ok }));
        Ok(())
    }));
    let kind = match format {
        Format::Png => COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_PNG,
        Format::Jpeg => COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_JPEG,
    };
    // SAFETY: ядро живого webview, поток интерфейса; поток и обработчик WebView2 держит сам.
    let started = unsafe { target.core.CapturePreview(kind, &stream, &handler) };
    started.is_ok()
}

#[cfg(not(windows))]
fn capture(
    _target: &Target,
    _format: Format,
    _seq: u64,
    _events: &EventLoopProxy<UserEvent>,
) -> bool {
    false
}

#[cfg(windows)]
fn read_all(stream: &windows::Win32::System::Com::IStream) -> Option<Vec<u8>> {
    use windows::Win32::System::Com::{STREAM_SEEK_END, STREAM_SEEK_SET};
    let mut size = 0u64;
    // SAFETY: живой поток, указатели на локальные переменные и на буфер нужной длины.
    unsafe {
        stream.Seek(0, STREAM_SEEK_END, Some(&mut size)).ok()?;
        if size == 0 || size > MAX_BYTES {
            return None;
        }
        stream.Seek(0, STREAM_SEEK_SET, None).ok()?;
        let len = u32::try_from(size).ok()?;
        let mut bytes = vec![0u8; len as usize];
        let mut read = 0u32;
        stream
            .Read(bytes.as_mut_ptr().cast(), len, Some(&mut read))
            .ok()
            .ok()?;
        bytes.truncate(read as usize);
        Some(bytes)
    }
}

// ---------- хранилище для протокола интерфейса ----------

/// Картинки текущего снимка. Протокол интерфейса может звать `serve` из любого
/// потока, поэтому не thread-local.
struct Store {
    seq: u64,
    images: Vec<(Part, Format, Vec<u8>)>,
}

static STORE: Mutex<Store> = Mutex::new(Store {
    seq: 0,
    images: Vec::new(),
});

/// Новый снимок: прежние картинки больше не нужны.
fn store_reset(seq: u64) {
    if let Ok(mut store) = STORE.lock() {
        store.seq = seq;
        store.images = Vec::new();
    }
}

/// Кладёт картинку, если её снимок ещё актуален.
#[cfg_attr(not(windows), allow(dead_code))]
fn store_put(seq: u64, part: Part, format: Format, bytes: Vec<u8>) -> bool {
    let Ok(mut store) = STORE.lock() else {
        return false;
    };
    if store.seq != seq {
        return false;
    }
    store.images.retain(|(p, _, _)| *p != part);
    store.images.push((part, format, bytes));
    true
}

/// Отдаёт снимок по пути вида `/snapshot/3-page.png`.
pub fn serve(path: &str) -> Option<(&'static str, Vec<u8>)> {
    let (name, ext) = path.strip_prefix("/snapshot/")?.split_once('.')?;
    let (seq, part) = name.split_once('-')?;
    let seq: u64 = seq.parse().ok()?;
    let store = STORE.lock().ok()?;
    if store.seq != seq {
        return None;
    }
    store
        .images
        .iter()
        .find(|(p, f, _)| p.name() == part && f.ext() == ext)
        .map(|(_, f, bytes)| (f.mime(), bytes.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serves_only_current_snapshot() {
        store_reset(41);
        assert!(store_put(41, Part::Page, Format::Png, vec![1, 2, 3]));
        // Опоздавший снимок прежнего окна не кладётся.
        assert!(!store_put(40, Part::Ai, Format::Png, vec![9]));
        assert_eq!(
            serve("/snapshot/41-page.png"),
            Some(("image/png", vec![1, 2, 3]))
        );
        assert!(serve("/snapshot/41-page.jpg").is_none());
        assert!(serve("/snapshot/41-ai.png").is_none());
        assert!(serve("/snapshot/40-page.png").is_none());
        assert!(serve("/snapshot/41-page.png/../x").is_none());
        assert!(serve("/snapshot/../index.html").is_none());
        store_reset(42);
        assert!(serve("/snapshot/41-page.png").is_none());
    }

    #[test]
    fn huge_webview_goes_jpeg() {
        let rect = |w, h| PxRect { x: 0, y: 0, w, h };
        // Развёрнутое окно 1920×1080 — ещё PNG, 4K в полный экран — уже JPEG.
        assert_eq!(Format::for_rect(rect(1908, 984)), Format::Png);
        assert_eq!(Format::for_rect(rect(3828, 2010)), Format::Jpeg);
    }

    #[test]
    fn view_shape() {
        let s = Snapshots::default();
        let json = serde_json::to_value(s.view(true)).unwrap_or_default();
        assert_eq!(json["clear"], true);
        assert!(json["page"].is_null() && json["ai"].is_null());
    }
}
