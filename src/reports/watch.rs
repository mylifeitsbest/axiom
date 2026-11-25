//! Падения процессов движка: WebView2 `ProcessFailed`, WebKitGTK `web-process-terminated`.
//!
//! Адрес страницы в отчёт не попадает — только чей это webview и что случилось.

use wry::WebView;

/// Чей webview.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Ui,
    Tab,
    Ai,
}

impl Source {
    fn name(self) -> &'static str {
        match self {
            Self::Ui => "интерфейс",
            Self::Tab => "вкладка",
            Self::Ai => "панель ИИ",
        }
    }
}

/// Подписывает webview на сообщения о падении его процессов.
#[cfg(windows)]
pub fn watch(webview: &WebView, source: Source) {
    use webview2_com::{
        Microsoft::Web::WebView2::Win32::{
            COREWEBVIEW2_PROCESS_FAILED_KIND, COREWEBVIEW2_PROCESS_FAILED_REASON,
            ICoreWebView2ProcessFailedEventArgs2,
        },
        ProcessFailedEventHandler,
    };
    use windows::core::PWSTR;
    use windows_core::Interface;
    use wry::WebViewExtWindows;

    let handler = ProcessFailedEventHandler::create(Box::new(move |_, args| {
        let Some(args) = args else { return Ok(()) };
        let mut kind = COREWEBVIEW2_PROCESS_FAILED_KIND::default();
        // SAFETY: args — живой COM-объект события, указатель на локальную переменную.
        unsafe { args.ProcessFailedKind(&mut kind)? };
        let mut reason = COREWEBVIEW2_PROCESS_FAILED_REASON::default();
        let mut code = 0i32;
        let mut about = String::new();
        if let Ok(args) = args.cast::<ICoreWebView2ProcessFailedEventArgs2>() {
            let mut text = PWSTR::null();
            // SAFETY: то же; строку освобождает take_pwstr.
            unsafe {
                let _ = args.Reason(&mut reason);
                let _ = args.ExitCode(&mut code);
                if args.ProcessDescription(&mut text).is_ok() && !text.is_null() {
                    about = webview2_com::take_pwstr(text);
                }
            }
        }
        let (kind, important) = kind_name(kind.0);
        let reason = reason_name(reason.0);
        let mut what = format!(
            "WebView2: {kind}, причина: {reason}, код выхода {:#x}",
            code as u32
        );
        if !about.is_empty() {
            what.push_str(&format!(" ({about})"));
        }
        // Служебные процессы (звук, сеть) перезапускаются сами; о них — только если упали.
        let report = important || matches!(reason, "вылет" | "нехватка памяти" | "не запустился");
        super::process_failed(source.name(), &what, report);
        Ok(())
    }));
    let mut token = 0i64;
    // SAFETY: ядро webview живёт столько же, сколько сам webview.
    if let Err(err) = unsafe { webview.webview().add_ProcessFailed(&handler, &mut token) } {
        eprintln!("сообщения о падении процессов WebView2 недоступны: {err}");
    }
}

/// Название вида процесса и важно ли его падение само по себе.
#[cfg(windows)]
fn kind_name(kind: i32) -> (&'static str, bool) {
    match kind {
        0 => ("упал браузерный процесс", true),
        1 => ("упал процесс страницы", true),
        2 => ("страница не отвечает", true),
        3 => ("упал процесс фрейма", true),
        4 => ("упал служебный процесс", false),
        5 => ("упал процесс песочницы", false),
        6 => ("упал процесс GPU", true),
        7 | 8 => ("упал процесс плагина", false),
        _ => ("упал неизвестный процесс", false),
    }
}

#[cfg(windows)]
fn reason_name(reason: i32) -> &'static str {
    match reason {
        1 => "завис",
        2 => "завершён",
        3 => "вылет",
        4 => "не запустился",
        5 => "нехватка памяти",
        6 => "профиль удалён",
        _ => "неизвестно",
    }
}

/// WebKitGTK: крейта webkit2gtk среди зависимостей нет, поэтому сигнал — по имени,
/// а причину (`WebKitWebProcessTerminationReason`) читаем как перечисление GObject.
#[cfg(target_os = "linux")]
pub fn watch(webview: &WebView, source: Source) {
    use gtk::glib::{self, prelude::*};
    use wry::WebViewExtUnix;

    let view = webview.webview();
    view.connect_local("web-process-terminated", false, move |values| {
        let reason = values
            .get(1)
            .and_then(glib::EnumValue::from_value)
            .map(|(_, v)| v.nick().to_string())
            .unwrap_or_default();
        // terminated-by-api — процесс завершили мы сами, это не сбой.
        if reason != "terminated-by-api" {
            let what = match reason.as_str() {
                "crashed" => "WebKitGTK: упал процесс страницы (вылет)".to_string(),
                "exceeded-memory-limit" => {
                    "WebKitGTK: процесс страницы превысил лимит памяти".to_string()
                }
                other => format!("WebKitGTK: процесс страницы завершился ({other})"),
            };
            super::process_failed(source.name(), &what, true);
        }
        None
    });
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn watch(_webview: &WebView, _source: Source) {}
