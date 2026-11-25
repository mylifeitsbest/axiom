//! Всплывающие окна сайтов открываются вкладками — но только по действию пользователя.
//!
//! WebView2 сам не блокирует `window.open` без клика: решать должно приложение
//! по флагу `IsUserInitiated`. wry этот флаг в свой колбэк не передаёт, поэтому
//! рядом с его обработчиком `NewWindowRequested` ставим свой. Наш вызывается
//! сразу после обработчика wry, а тот откладывает колбэк приложения до следующего
//! витка цикла сообщений, так что к моменту решения флаг уже записан.
//!
//! WebKitGTK окна без жеста блокирует сам (`javascript-can-open-windows-automatically`
//! выключен по умолчанию), там проверять нечего.

use wry::WebView;

/// Можно ли открыть вкладку по запросу сайта.
///
/// Только http(s) и blob: `data:` в новой вкладке — готовая фишинговая страница
/// под чужим адресом, а `file:` открывал бы сайту локальные файлы.
pub fn allowed(url: &str) -> bool {
    let scheme = url.split(':').next().unwrap_or("").to_ascii_lowercase();
    matches!(scheme.as_str(), "http" | "https" | "blob") && user_initiated(url)
}

/// Запрос сайта на новое окно, как его увидел наш обработчик.
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Debug, Clone, PartialEq)]
struct Asked {
    url: String,
    /// По клику ли (`IsUserInitiated`).
    user: bool,
    /// Адрес фрейма, который просит окно, если движок его сообщил.
    frame: Option<String>,
}

#[cfg(windows)]
thread_local! {
    /// Запросы в порядке поступления.
    static GESTURES: std::cell::RefCell<std::collections::VecDeque<Asked>> =
        const { std::cell::RefCell::new(std::collections::VecDeque::new()) };
}

#[cfg(windows)]
const MAX_PENDING: usize = 32;

/// Наш обработчик `NewWindowRequested` встал.
#[cfg(windows)]
static HOOKED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Запрос с этим адресом из очереди. Более старые записи — запросы, до которых
/// колбэк wry уже не дойдёт: их выбрасываем.
#[cfg_attr(not(windows), allow(dead_code))]
fn take(queue: &mut std::collections::VecDeque<Asked>, url: &str) -> Option<Asked> {
    let i = queue.iter().position(|a| a.url == url)?;
    queue.drain(..=i).next_back()
}

/// По клику ли. Записи нет: если наш обработчик встал, запрос он пропустил
/// (очередь переполнена окнами без клика) — не открываем. Не встал вовсе —
/// судить не по чему, и обычные ссылки с `target=_blank` не ломаем.
#[cfg_attr(not(windows), allow(dead_code))]
fn by_click(found: Option<&Asked>, hooked: bool) -> bool {
    found.map_or(!hooked, |a| a.user)
}

/// Просил ли сайт окно по клику и из какого фрейма (ссылки на приложения
/// с `target=_blank` тоже идут сюда).
#[cfg(windows)]
pub fn request(url: &str) -> (bool, Option<String>) {
    let found = GESTURES.with(|q| take(&mut q.borrow_mut(), url));
    let hooked = HOOKED.load(std::sync::atomic::Ordering::Relaxed);
    (
        by_click(found.as_ref(), hooked),
        found.and_then(|a| a.frame),
    )
}

#[cfg(not(windows))]
pub fn request(_url: &str) -> (bool, Option<String>) {
    (true, None)
}

/// Просил ли сайт окно по клику.
pub fn user_initiated(url: &str) -> bool {
    request(url).0
}

/// Запоминает, по клику ли сайт просит новое окно. Вызывать после сборки webview.
#[cfg(windows)]
pub fn install(webview: &WebView) {
    use webview2_com::{
        Microsoft::Web::WebView2::Win32::ICoreWebView2NewWindowRequestedEventArgs3,
        NewWindowRequestedEventHandler, take_pwstr,
    };
    use windows_core::{BOOL, Interface, PWSTR};
    use wry::WebViewExtWindows;

    let handler = NewWindowRequestedEventHandler::create(Box::new(|_, args| {
        let Some(args) = args else { return Ok(()) };
        let mut raw = PWSTR::null();
        let mut user = BOOL::default();
        // SAFETY: args — живой COM-объект события, указатели на локальные переменные.
        unsafe {
            args.Uri(&mut raw)?;
            args.IsUserInitiated(&mut user)?;
        }
        let url = take_pwstr(raw);
        // Фрейм, который просит окно (WebView2 1.0.2088+).
        let frame = args
            .cast::<ICoreWebView2NewWindowRequestedEventArgs3>()
            .ok()
            // SAFETY: живой COM-объект события, поток интерфейса.
            .and_then(|args| unsafe { args.OriginalSourceFrameInfo() }.ok())
            .and_then(|info| {
                let mut raw = PWSTR::null();
                // SAFETY: живой объект фрейма, указатель на локальную переменную.
                unsafe { info.Source(&mut raw) }.ok()?;
                Some(take_pwstr(raw))
            })
            .filter(|source| !source.is_empty());
        GESTURES.with(|q| {
            let mut q = q.borrow_mut();
            if q.len() >= MAX_PENDING {
                q.pop_front();
            }
            q.push_back(Asked {
                url,
                user: user.as_bool(),
                frame,
            });
        });
        Ok(())
    }));
    let mut token = 0i64;
    // SAFETY: ядро webview живёт столько же, сколько сам webview.
    let result = unsafe {
        webview
            .webview()
            .add_NewWindowRequested(&handler, &mut token)
    };
    match result {
        Ok(()) => HOOKED.store(true, std::sync::atomic::Ordering::Relaxed),
        Err(err) => eprintln!("проверка всплывающих окон недоступна: {err}"),
    }
}

#[cfg(not(windows))]
pub fn install(_webview: &WebView) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_web_schemes() {
        assert!(allowed("https://example.com/"));
        assert!(allowed("HTTP://example.com/"));
        assert!(allowed("blob:https://example.com/1"));
        assert!(!allowed("data:text/html,<b>x</b>"));
        assert!(!allowed("file:///C:/Users/"));
        assert!(!allowed("about:blank"));
        assert!(!allowed("javascript:alert(1)"));
    }

    fn asked(url: &str, user: bool) -> Asked {
        Asked {
            url: url.into(),
            user,
            frame: (url == "https://b/").then(|| "https://site.example/page".into()),
        }
    }

    #[test]
    fn gesture_queue() {
        let mut q: std::collections::VecDeque<Asked> = [
            asked("https://a/", false),
            asked("https://b/", true),
            asked("https://c/", false),
        ]
        .into();
        // Совпадение по адресу, старые записи выбрасываются.
        let b = take(&mut q, "https://b/");
        assert_eq!(b, Some(asked("https://b/", true)));
        assert_eq!(
            b.and_then(|a| a.frame).as_deref(),
            Some("https://site.example/page")
        );
        assert_eq!(q.len(), 1);
        let c = take(&mut q, "https://c/");
        assert!(!by_click(c.as_ref(), true));
        assert!(take(&mut q, "https://d/").is_none());
        assert!(q.is_empty());
    }

    #[test]
    fn missing_record_fails_closed() {
        let clicked = asked("https://a/", true);
        assert!(by_click(Some(&clicked), true));
        assert!(by_click(Some(&clicked), false));
        // Обработчик встал, а записи нет — без клика.
        assert!(!by_click(None, true));
        // Обработчик не встал — судить не по чему, обычные ссылки не ломаем.
        assert!(by_click(None, false));
    }
}
