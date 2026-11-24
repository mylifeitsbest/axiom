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

#[cfg(windows)]
thread_local! {
    /// Флаги запросов в порядке поступления: (адрес, по клику ли).
    static GESTURES: std::cell::RefCell<std::collections::VecDeque<(String, bool)>> =
        const { std::cell::RefCell::new(std::collections::VecDeque::new()) };
}

#[cfg(windows)]
const MAX_PENDING: usize = 32;

#[cfg(windows)]
fn user_initiated(url: &str) -> bool {
    GESTURES.with(|q| {
        let mut q = q.borrow_mut();
        match q.iter().position(|(u, _)| u == url) {
            // Более старые записи — запросы, до которых колбэк wry уже не дойдёт.
            Some(i) => q.drain(..=i).next_back().is_some_and(|(_, ok)| ok),
            // Наш обработчик не встал: не ломаем обычные ссылки с target=_blank.
            None => true,
        }
    })
}

#[cfg(not(windows))]
fn user_initiated(_url: &str) -> bool {
    true
}

/// Запоминает, по клику ли сайт просит новое окно. Вызывать после сборки webview.
#[cfg(windows)]
pub fn install(webview: &WebView) {
    use webview2_com::{NewWindowRequestedEventHandler, take_pwstr};
    use windows_core::{BOOL, PWSTR};
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
        GESTURES.with(|q| {
            let mut q = q.borrow_mut();
            if q.len() >= MAX_PENDING {
                q.pop_front();
            }
            q.push_back((url, user.as_bool()));
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
    if let Err(err) = result {
        eprintln!("проверка всплывающих окон недоступна: {err}");
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

    #[cfg(windows)]
    #[test]
    fn gesture_queue() {
        GESTURES.with(|q| {
            let mut q = q.borrow_mut();
            q.clear();
            q.push_back(("https://a/".into(), false));
            q.push_back(("https://b/".into(), true));
            q.push_back(("https://c/".into(), false));
        });
        // Совпадение по адресу, старые записи выбрасываются.
        assert!(allowed("https://b/"));
        assert!(!allowed("https://c/"));
        // Неизвестный адрес (обработчик не сработал) — не блокируем.
        assert!(allowed("https://d/"));
        GESTURES.with(|q| assert!(q.borrow().is_empty()));
    }
}
