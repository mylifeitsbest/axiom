//! Инструменты страницы во вкладке: звук, масштаб, печать, исходный код,
//! инструменты разработчика и перезагрузка без кэша.
//!
//! WebView2 умеет всё это сам, но только по своим сочетаниям и только когда фокус
//! в странице. Здесь те же действия по команде браузера: из интерфейса, из меню
//! вкладки и по клавишам, которые мы перехватываем сами (см. `ui::hotkeys`).

use tao::event_loop::EventLoopProxy;
use url::Url;
use wry::WebView;

use super::TabId;
use crate::events::UserEvent;

/// Ступени масштаба, как у Chrome: по ним же ходит Ctrl+колёсико в WebView2.
const ZOOM_STEPS: [f64; 17] = [
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
const MIN_ZOOM: f64 = 0.25;
const MAX_ZOOM: f64 = 5.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Zoom {
    In,
    Out,
    Reset,
}

/// Следующая ступень масштаба. Масштаб между ступенями (колёсико тачпада)
/// округляется к ближайшей ступени в нужную сторону.
pub fn next_zoom(current: f64, zoom: Zoom) -> f64 {
    const EPS: f64 = 0.001;
    match zoom {
        Zoom::Reset => 1.0,
        Zoom::In => ZOOM_STEPS
            .into_iter()
            .find(|&s| s > current + EPS)
            .unwrap_or(MAX_ZOOM),
        Zoom::Out => ZOOM_STEPS
            .into_iter()
            .rev()
            .find(|&s| s < current - EPS)
            .unwrap_or(MIN_ZOOM),
    }
}

const SOURCE: &str = "view-source:";

/// `view-source:` открывает только WebView2. WebKitGTK такой схемы не знает.
pub const CAN_VIEW_SOURCE: bool = cfg!(windows);

/// Адрес страницы с её исходным кодом (Ctrl+U). Только для сайтов и локальных файлов.
pub fn source_url(url: &str) -> Option<String> {
    let u = Url::parse(url).ok()?;
    matches!(u.scheme(), "http" | "https" | "file").then(|| format!("{SOURCE}{u}"))
}

/// Это исходный код страницы (`view-source:` сайта или файла).
///
/// Такой адрес открывает только сам браузер: вкладку для него создаёт Ctrl+U,
/// и только в ней навигация на `view-source:` разрешена (см. `webview::build_tab`).
pub fn is_source(url: &str) -> bool {
    url.get(..SOURCE.len())
        .is_some_and(|p| p.eq_ignore_ascii_case(SOURCE))
        && url.get(SOURCE.len()..).and_then(source_url).is_some()
}

/// Новый адрес вкладки по событию движка. WebView2 называет страницу с исходным
/// кодом адресом самой страницы — вкладка исходного кода остаётся ею (в том числе
/// после перехода по ссылке внутри исходного кода), пока в ней не откроют другой адрес.
pub fn keep_source(current: &str, reported: String) -> String {
    if is_source(current) && !is_source(&reported) {
        return source_url(&reported).unwrap_or(reported);
    }
    reported
}

// ---------- перезагрузка без кэша ----------

/// WebView2 не умеет этого напрямую — через DevTools-протокол, как Ctrl+Shift+R в Chrome.
#[cfg(windows)]
pub fn reload_ignoring_cache(webview: &WebView) {
    use webview2_com::CallDevToolsProtocolMethodCompletedHandler;
    use windows_core::HSTRING;
    use wry::WebViewExtWindows;

    let handler = CallDevToolsProtocolMethodCompletedHandler::create(Box::new(|_, _| Ok(())));
    // SAFETY: ядро webview живое, строки живут до конца вызова.
    let sent = unsafe {
        webview.webview().CallDevToolsProtocolMethod(
            &HSTRING::from("Page.reload"),
            &HSTRING::from(r#"{"ignoreCache":true}"#),
            &handler,
        )
    };
    if sent.is_err() {
        let _ = webview.reload();
    }
}

/// WebKitGTK: `webkit_web_view_reload_bypass_cache`. В wry этого нет, а библиотеку
/// WebKitGTK wry уже подключает, поэтому функцию достаточно объявить.
#[cfg(target_os = "linux")]
pub fn reload_ignoring_cache(webview: &WebView) {
    use gtk::glib::prelude::*;
    use wry::WebViewExtUnix;

    unsafe extern "C" {
        fn webkit_web_view_reload_bypass_cache(view: *mut std::ffi::c_void);
    }
    let view = webview.webview();
    // SAFETY: view — живой WebKitWebView, ссылку на него держим до конца вызова.
    unsafe { webkit_web_view_reload_bypass_cache(view.as_ptr().cast()) };
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn reload_ignoring_cache(webview: &WebView) {
    let _ = webview.reload();
}

// ---------- печать ----------

/// Окно печати с предпросмотром, как по Ctrl+P в самой странице.
#[cfg(windows)]
pub fn print(webview: &WebView) {
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_PRINT_DIALOG_KIND_BROWSER, ICoreWebView2_16,
    };
    use windows_core::Interface;
    use wry::WebViewExtWindows;

    let shown = webview
        .webview()
        .cast::<ICoreWebView2_16>()
        // SAFETY: ядро webview живое, вызов без указателей.
        .and_then(|core| unsafe { core.ShowPrintUI(COREWEBVIEW2_PRINT_DIALOG_KIND_BROWSER) });
    if shown.is_err() {
        // Старый WebView2 без ShowPrintUI: печать изнутри страницы. Это `window.print()`
        // через `ExecuteScript`, то есть с жестом — единственный служебный скрипт с ним
        // (см. `script`), и он к месту: печать попросил сам человек (Ctrl+P, меню),
        // а повторные `print()` без жеста Chromium придерживает.
        let _ = webview.print();
    }
}

/// WebKitGTK: окно печати без скриптов в странице (`WebKitPrintOperation` в wry).
#[cfg(not(windows))]
pub fn print(webview: &WebView) {
    let _ = webview.print();
}

// ---------- масштаб ----------

#[cfg(windows)]
pub fn zoom(webview: &WebView, step: Zoom) {
    use wry::WebViewExtWindows;
    let mut current = 1.0;
    // SAFETY: контроллер принадлежит живому webview, указатель на локальную переменную.
    if unsafe { webview.controller().ZoomFactor(&mut current) }.is_err() {
        current = 1.0;
    }
    let _ = webview.zoom(next_zoom(current, step));
}

#[cfg(target_os = "linux")]
pub fn zoom(webview: &WebView, step: Zoom) {
    use gtk::glib::prelude::*;
    use wry::WebViewExtUnix;
    let current = webview.webview().property::<f64>("zoom-level");
    let _ = webview.zoom(next_zoom(current, step));
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn zoom(_webview: &WebView, _step: Zoom) {}

// ---------- звук ----------

/// Выключает или включает звук страницы. Состояние хранит вкладка (`Tab::muted`):
/// после пробуждения новый webview получает его снова.
#[cfg(windows)]
pub fn set_muted(webview: &WebView, muted: bool) {
    use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2_8;
    use windows_core::Interface;
    use wry::WebViewExtWindows;
    if let Ok(core) = webview.webview().cast::<ICoreWebView2_8>() {
        // SAFETY: ядро webview живое, вызов без указателей.
        let _ = unsafe { core.SetIsMuted(muted) };
    }
}

/// WebKitGTK: свойство `is-muted` (с версии 2.30; на более старых звук не выключается).
#[cfg(target_os = "linux")]
pub fn set_muted(webview: &WebView, muted: bool) {
    use gtk::glib::prelude::*;
    use wry::WebViewExtUnix;
    let view = webview.webview();
    if view.find_property("is-muted").is_some() {
        view.set_property("is-muted", muted);
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn set_muted(_webview: &WebView, _muted: bool) {}

/// Сообщает, когда страница начинает и перестаёт играть звук (значок динамика на вкладке).
/// Выключенный звук страница всё равно «играет»: значок остаётся, только перечёркнутый.
#[cfg(windows)]
pub fn watch_audio(webview: &WebView, id: TabId, events: EventLoopProxy<UserEvent>) {
    use webview2_com::{
        IsDocumentPlayingAudioChangedEventHandler, Microsoft::Web::WebView2::Win32::ICoreWebView2_8,
    };
    use windows_core::{BOOL, Interface};
    use wry::WebViewExtWindows;

    let Ok(core) = webview.webview().cast::<ICoreWebView2_8>() else {
        return;
    };
    let handler = IsDocumentPlayingAudioChangedEventHandler::create(Box::new(move |sender, _| {
        let Some(sender) = sender else { return Ok(()) };
        let core = sender.cast::<ICoreWebView2_8>()?;
        let mut playing = BOOL::default();
        // SAFETY: core — живое ядро webview, указатель на локальную переменную.
        unsafe { core.IsDocumentPlayingAudio(&mut playing)? };
        let _ = events.send_event(UserEvent::TabAudio {
            id,
            playing: playing.as_bool(),
        });
        Ok(())
    }));
    let mut token = 0i64;
    // SAFETY: ядро webview живёт столько же, сколько сам webview.
    if let Err(err) = unsafe { core.add_IsDocumentPlayingAudioChanged(&handler, &mut token) } {
        eprintln!("значок звука на вкладке недоступен: {err}");
    }
}

/// WebKitGTK: уведомление о свойстве `is-playing-audio`.
#[cfg(target_os = "linux")]
pub fn watch_audio(webview: &WebView, id: TabId, events: EventLoopProxy<UserEvent>) {
    use gtk::glib::prelude::*;
    use wry::WebViewExtUnix;
    let view = webview.webview();
    if view.find_property("is-playing-audio").is_none() {
        return;
    }
    view.connect_notify_local(Some("is-playing-audio"), move |view, _| {
        let playing = view.property::<bool>("is-playing-audio");
        let _ = events.send_event(UserEvent::TabAudio { id, playing });
    });
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn watch_audio(_webview: &WebView, _id: TabId, _events: EventLoopProxy<UserEvent>) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoom_steps_like_chrome() {
        assert_eq!(next_zoom(1.0, Zoom::In), 1.1);
        assert_eq!(next_zoom(1.0, Zoom::Out), 0.9);
        assert_eq!(next_zoom(1.75, Zoom::Reset), 1.0);
        // Между ступенями (тачпад) — к ближайшей ступени в нужную сторону.
        assert_eq!(next_zoom(1.17, Zoom::In), 1.25);
        assert_eq!(next_zoom(1.17, Zoom::Out), 1.1);
        // Почти ступень (погрешность double) — считается ступенью.
        assert_eq!(next_zoom(1.0999999, Zoom::In), 1.25);
        // За краями дальше не идём.
        assert_eq!(next_zoom(5.0, Zoom::In), 5.0);
        assert_eq!(next_zoom(0.25, Zoom::Out), 0.25);
        assert_eq!(next_zoom(0.3, Zoom::Out), 0.25);
    }

    #[test]
    fn source_only_for_pages() {
        assert_eq!(
            source_url("https://example.com/a?b=1").as_deref(),
            Some("view-source:https://example.com/a?b=1")
        );
        assert_eq!(
            source_url("file:///C:/x.html").as_deref(),
            Some("view-source:file:///C:/x.html")
        );
        assert_eq!(source_url("axiom://newtab"), None);
        assert_eq!(source_url("data:text/html,<b>x</b>"), None);
        assert_eq!(source_url("javascript:alert(1)"), None);
        assert!(is_source("view-source:https://example.com/"));
        assert!(is_source("VIEW-SOURCE:http://example.com/"));
        // Внутри — только сайт или файл.
        assert!(!is_source("view-source:javascript:alert(1)"));
        assert!(!is_source("view-source:data:text/html,x"));
        assert!(!is_source("view-source:view-source:https://example.com/"));
        assert!(!is_source("https://example.com/"));
        assert!(!is_source("view-sour"));
    }

    #[test]
    fn source_tab_stays_source() {
        let src = "view-source:https://e.example/a";
        // WebView2 сообщает адрес самой страницы — вкладка остаётся исходным кодом.
        assert_eq!(keep_source(src, "https://e.example/a".into()), src);
        // Ссылка внутри исходного кода ведёт на исходный код другой страницы.
        assert_eq!(
            keep_source(src, "https://e.example/b".into()),
            "view-source:https://e.example/b"
        );
        // Обычная вкладка не превращается в исходный код.
        assert_eq!(
            keep_source("https://e.example/a", "https://e.example/b".into()),
            "https://e.example/b"
        );
        assert_eq!(keep_source(src, "about:blank".into()), "about:blank");
    }
}
