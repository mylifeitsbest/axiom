//! Страница не открылась: повтор при сбое сети и своя страница ошибки (WebKitGTK).
//!
//! Сам WebKitGTK на неудачную загрузку показывает голый текст ошибки
//! (`<html><body>…</body></html>`) без цветов: в тёмной теме это чёрные буквы на
//! тёмном фоне webview. Поэтому сигнал `load-failed` вкладок и панели ИИ разбираем сами:
//!
//! - отмену, превращение страницы в загрузку файла и прочие «не ошибки» пропускаем,
//!   как и сам движок;
//! - сбой, который часто проходит сам (оборвалось рукопожатие TLS или соединение,
//!   временная ошибка DNS), повторяем до двух раз — через 300 мс и через секунду, и
//!   только если с тех пор во вкладке не начиналась другая загрузка;
//! - иначе — своя страница (`webkit_web_view_load_alternate_html`): что случилось,
//!   адрес и «Повторить», в цветах Axiom для светлой и тёмной темы. Адресом страницы
//!   остаётся адрес сайта, поэтому F5, «Назад» и «Повторить» работают как обычно.
//!
//! Ошибку сертификата не повторяем и не обходим (`load-failed-with-tls-errors` никто
//! не перехватывает, движок сам присылает её в `load-failed`): только объясняем.
//!
//! WebView2 показывает свою страницу ошибки, у неё свои цвета для обеих тем, —
//! там ничего не нужно. Разбор и страница — обычный код: проверяются тестами везде.
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::time::Duration;

use url::Url;

/// Откуда ошибка: домен `GError`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Domain {
    /// `WebKitNetworkError`.
    Network,
    /// `WebKitPolicyError`.
    Policy,
    /// `WebKitPluginError`.
    Plugin,
    /// `GIOErrorEnum`: сокеты, соединение, прокси.
    Io,
    /// `GTlsError`: шифрование и сертификаты (glib-networking).
    Tls,
    /// `GResolverError`: DNS.
    Resolver,
    Other,
}

/// Что случилось — для заголовка и пояснения на странице.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Нет соединения с сайтом: сеть недоступна, сайт не принял или оборвал соединение.
    Offline,
    /// Не удалось договориться о шифровании (рукопожатие TLS).
    Tls,
    /// Не нашёлся адрес сервера.
    Dns,
    /// Сайт не ответил вовремя.
    Timeout,
    /// Сертификат сайта не прошёл проверку.
    Certificate,
    /// Прокси не отвечает или не пускает.
    Proxy,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Failure {
    pub kind: Kind,
    /// Сбой часто проходит сам — стоит повторить загрузку.
    pub transient: bool,
}

/// Коды ошибок из заголовков GIO и WebKitGTK (на Linux их сверяет тест с `ffi`).
mod code {
    pub const NET_TRANSPORT: i32 = 300;
    pub const NET_CANCELLED: i32 = 302;
    pub const POLICY_INTERRUPTED: i32 = 102;
    pub const PLUGIN_WILL_HANDLE_LOAD: i32 = 204;

    pub const IO_FAILED: i32 = 0;
    pub const IO_CANCELLED: i32 = 19;
    pub const IO_TIMED_OUT: i32 = 24;
    pub const IO_HOST_NOT_FOUND: i32 = 28;
    pub const IO_PARTIAL_INPUT: i32 = 34;
    pub const IO_HOST_UNREACHABLE: i32 = 37;
    pub const IO_NETWORK_UNREACHABLE: i32 = 38;
    pub const IO_CONNECTION_REFUSED: i32 = 39;
    pub const IO_PROXY_FAILED: i32 = 40;
    pub const IO_PROXY_NOT_ALLOWED: i32 = 43;
    pub const IO_CONNECTION_CLOSED: i32 = 44;
    pub const IO_NOT_CONNECTED: i32 = 45;

    pub const TLS_MISC: i32 = 1;
    pub const TLS_BAD_CERTIFICATE: i32 = 2;
    pub const TLS_NOT_TLS: i32 = 3;
    pub const TLS_HANDSHAKE: i32 = 4;
    pub const TLS_CERTIFICATE_REQUIRED: i32 = 5;
    pub const TLS_EOF: i32 = 6;
    pub const TLS_INAPPROPRIATE_FALLBACK: i32 = 7;
    pub const TLS_BAD_CERTIFICATE_PASSWORD: i32 = 8;

    pub const DNS_TEMPORARY_FAILURE: i32 = 1;
}

/// Разбор ошибки загрузки. `None` — это не ошибка для человека (загрузку остановили,
/// страница стала загрузкой файла): ничего не показываем, как и сам движок.
pub fn classify(domain: Domain, code: i32, message: &str) -> Option<Failure> {
    use code::*;
    let failure = |kind, transient| Some(Failure { kind, transient });
    match (domain, code) {
        (Domain::Network, NET_CANCELLED)
        | (Domain::Io, IO_CANCELLED)
        | (Domain::Policy, POLICY_INTERRUPTED)
        | (Domain::Plugin, PLUGIN_WILL_HANDLE_LOAD) => None,
        (
            Domain::Tls,
            TLS_BAD_CERTIFICATE | TLS_CERTIFICATE_REQUIRED | TLS_BAD_CERTIFICATE_PASSWORD,
        ) => failure(Kind::Certificate, false),
        (
            Domain::Tls,
            TLS_MISC | TLS_NOT_TLS | TLS_HANDSHAKE | TLS_EOF | TLS_INAPPROPRIATE_FALLBACK,
        ) => failure(Kind::Tls, true),
        (Domain::Resolver, DNS_TEMPORARY_FAILURE) => failure(Kind::Dns, true),
        (Domain::Resolver, _) | (Domain::Io, IO_HOST_NOT_FOUND) => failure(Kind::Dns, false),
        (Domain::Io, IO_TIMED_OUT) => failure(Kind::Timeout, false),
        (Domain::Io, IO_PROXY_FAILED..=IO_PROXY_NOT_ALLOWED) => failure(Kind::Proxy, false),
        // Обрыв во время рукопожатия glib-networking отдаёт и ошибкой сокета.
        (Domain::Io, IO_FAILED) if mentions_tls(message) => failure(Kind::Tls, true),
        (
            Domain::Io,
            IO_PARTIAL_INPUT
            | IO_HOST_UNREACHABLE
            | IO_NETWORK_UNREACHABLE
            | IO_CONNECTION_REFUSED
            | IO_CONNECTION_CLOSED
            | IO_NOT_CONNECTED,
        )
        | (Domain::Network, NET_TRANSPORT) => {
            let kind = if mentions_tls(message) {
                Kind::Tls
            } else {
                Kind::Offline
            };
            failure(kind, true)
        }
        _ => failure(Kind::Other, false),
    }
}

/// Сообщения glib-networking о шифровании пишут «TLS» (или «SSL») и в переводах.
fn mentions_tls(message: &str) -> bool {
    message.contains("TLS") || message.contains("SSL")
}

/// Через сколько повторять: первый повтор, второй. Дальше — страница ошибки.
const RETRY_AFTER: [Duration; 2] = [Duration::from_millis(300), Duration::from_secs(1)];

/// Повторы одной вкладки.
///
/// Счёт идёт от загрузки, которую начал человек или сайт, а не от адреса: повтор,
/// который сервер перенаправил на другой адрес, не начинает счёт заново, так что
/// повторов всегда не больше двух.
#[derive(Debug, Default)]
struct Retries {
    /// Сколько раз уже повторили.
    done: usize,
    /// Номер загрузки: растёт с каждым её началом. Отложенный повтор, заставший
    /// другой номер, опоздал — во вкладке уже идёт что-то другое.
    load: u64,
    /// Следующая начатая загрузка — наш повтор.
    ours: bool,
}

impl Retries {
    /// Во вкладке началась загрузка. Не наш повтор — новая попытка, счёт с нуля.
    fn started(&mut self) {
        self.load += 1;
        if !std::mem::take(&mut self.ours) {
            self.done = 0;
        }
    }

    /// Загрузка не удалась. `Some((через сколько, номер загрузки))` — повторить,
    /// `None` — показать страницу ошибки.
    fn failed(&mut self, transient: bool) -> Option<(Duration, u64)> {
        match RETRY_AFTER.get(self.done).filter(|_| transient) {
            Some(&delay) => {
                self.done += 1;
                Some((delay, self.load))
            }
            None => {
                self.done = 0;
                None
            }
        }
    }

    /// Пора повторять: с отказа во вкладке ничего не начиналось.
    fn due(&mut self, load: u64) -> bool {
        let due = self.load == load;
        if due {
            self.ours = true;
        }
        due
    }
}

/// Длиннее адрес и текст движка на странице не показываем.
const MAX_URL: usize = 600;
const MAX_MESSAGE: usize = 400;

/// Цвета Axiom (`ui/styles.css`): светлые и тёмные, по `prefers-color-scheme`.
/// Цвета заданы явно для обеих тем — страница читается при любом фоне webview.
const STYLE: &str = r#":root{color-scheme:light dark;--bg:#fcfdff;--fg:#181b20;--muted:#52555c;--faint:#83868e;--box:#ecf0f9;--line:#dfe3eb;--acc:#8b9cff;--on-acc:#0b0b10}
@media (prefers-color-scheme:dark){:root{--bg:#12151a;--fg:#eceef3;--muted:#a1a5ac;--faint:#6b6f76;--box:#0c0e14;--line:#22252b}}
html{background:var(--bg);color:var(--fg)}
body{margin:0;min-height:100vh;display:flex;align-items:center;justify-content:center;padding:32px 20px;box-sizing:border-box;font:15px/1.5 system-ui,-apple-system,"Segoe UI",Ubuntu,Cantarell,"Noto Sans",sans-serif}
main{width:100%;max-width:560px}
h1{margin:0 0 8px;font-size:22px;font-weight:600;line-height:1.3}
p{margin:0 0 16px;color:var(--muted)}
.url{margin:0 0 18px;padding:8px 12px;border:1px solid var(--line);border-radius:10px;background:var(--box);color:var(--fg);font:13px/1.45 ui-monospace,"Cascadia Mono","DejaVu Sans Mono",monospace;overflow-wrap:anywhere}
.why{margin:18px 0 0;font-size:12.5px;color:var(--faint);overflow-wrap:anywhere}
button{font:inherit;font-weight:600;padding:8px 20px;border:0;border-radius:10px;background:var(--acc);color:var(--on-acc);cursor:pointer}
button:hover{filter:brightness(1.08)}
button:focus-visible{outline:2px solid var(--fg);outline-offset:2px}"#;

/// Заголовок и пояснение для человека.
fn words(kind: Kind) -> (&'static str, &'static str) {
    match kind {
        Kind::Offline => (
            "Не удалось подключиться к сайту",
            "Сайт не отвечает, или пропало подключение к интернету. Проверьте сеть, VPN и прокси и попробуйте ещё раз.",
        ),
        Kind::Tls => (
            "Не удалось установить защищённое соединение",
            "Соединение оборвалось, пока браузер и сайт договаривались о шифровании (TLS). Обычно это сбой сети, VPN или фильтрации трафика — попробуйте ещё раз.",
        ),
        Kind::Dns => (
            "Сайт не найден",
            "Не удалось узнать адрес сервера. Проверьте, правильно ли написан адрес, и подключение к интернету.",
        ),
        Kind::Timeout => (
            "Сайт долго не отвечает",
            "Сервер не ответил вовремя: он перегружен, или соединение слишком медленное. Попробуйте ещё раз чуть позже.",
        ),
        Kind::Certificate => (
            "Сертификат сайта не прошёл проверку",
            "Axiom не стал открывать страницу: соединение может быть небезопасным, кто-то может выдавать себя за этот сайт. Проверьте адрес, а также дату и время на компьютере.",
        ),
        Kind::Proxy => (
            "Прокси-сервер не отвечает",
            "Не удалось подключиться через прокси. Проверьте, что прокси запущен и его адрес указан верно.",
        ),
        Kind::Other => (
            "Не удалось открыть страницу",
            "Движок не смог загрузить эту страницу. Попробуйте ещё раз.",
        ),
    }
}

/// Страница ошибки. Адрес и текст движка — только экранированные: из них
/// не получится ни тега, ни атрибута.
pub fn page(kind: Kind, url: &str, message: &str) -> String {
    let (heading, text) = words(kind);
    // В заголовке вкладки (и в истории) — сайт, как у Chrome, а не текст ошибки.
    let title = Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .filter(|host| !host.is_empty())
        .unwrap_or_else(|| shorten(url, 80));
    let message = message.trim();
    let details = if message.is_empty() {
        String::new()
    } else {
        format!(
            "<p class=\"why\">Подробности: {}</p>",
            escape(&shorten(message, MAX_MESSAGE))
        )
    };
    format!(
        "<!doctype html><html lang=\"ru\"><head><meta charset=\"utf-8\">\
         <meta name=\"color-scheme\" content=\"light dark\">\
         <title>{title}</title><style>{STYLE}</style></head>\
         <body><main><h1>{heading}</h1><p>{text}</p><div class=\"url\">{url}</div>\
         <button type=\"button\" onclick=\"location.reload()\">Повторить</button>{details}\
         </main></body></html>",
        title = escape(&title),
        url = escape(&shorten(url, MAX_URL)),
    )
}

/// Экранирование для текста и значений атрибутов HTML.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// Не длиннее `max` символов: лишнее — многоточием.
fn shorten(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text.to_string(),
    }
}

/// Разбор `load-failed` у webview вкладки или панели ИИ. Ставить сразу после сборки.
#[cfg(target_os = "linux")]
pub fn install(webview: &wry::WebView) {
    platform::install(webview);
}

/// WebView2 показывает свою страницу ошибки.
#[cfg(not(target_os = "linux"))]
pub fn install(_webview: &wry::WebView) {}

#[cfg(target_os = "linux")]
mod platform {
    use std::{cell::RefCell, rc::Rc};

    use webkit2gtk::{
        LoadEvent, NetworkError, PluginError, PolicyError, WebViewExt, gio,
        glib::{self, error::ErrorDomain, prelude::*},
    };
    use wry::WebViewExtUnix;

    use super::{Domain, Retries, classify, page};

    /// Домен и код ошибки.
    fn split(error: &glib::Error) -> (Domain, i32) {
        fn code_in<T: ErrorDomain>(error: &glib::Error) -> Option<i32> {
            error.kind::<T>().map(T::code)
        }
        [
            (Domain::Network, code_in::<NetworkError>(error)),
            (Domain::Policy, code_in::<PolicyError>(error)),
            (Domain::Plugin, code_in::<PluginError>(error)),
            (Domain::Io, code_in::<gio::IOErrorEnum>(error)),
            (Domain::Tls, code_in::<gio::TlsError>(error)),
            (Domain::Resolver, code_in::<gio::ResolverError>(error)),
        ]
        .into_iter()
        .find_map(|(domain, found)| found.map(|found| (domain, found)))
        .unwrap_or((Domain::Other, 0))
    }

    pub fn install(webview: &wry::WebView) {
        let view = webview.webview();
        let retries = Rc::new(RefCell::new(Retries::default()));
        let state = retries.clone();
        // Только главный фрейм: и `load-changed`, и `load-failed` — про страницу целиком.
        view.connect_load_changed(move |_, event| {
            if event == LoadEvent::Started {
                state.borrow_mut().started();
            }
        });
        view.connect_load_failed(move |view, event, uri, error| {
            let (domain, code) = split(error);
            let Some(failure) = classify(domain, code, error.message()) else {
                // Отмена, загрузка файла — дальше как у движка (он тоже молчит).
                return false;
            };
            // Страница уже начала показываться — повторять поздно, только объяснить.
            let transient = failure.transient && event == LoadEvent::Started;
            let retry = retries.borrow_mut().failed(transient);
            // В журнал — без адреса: только что за ошибка.
            match retry {
                Some((delay, load)) => {
                    eprintln!(
                        "страница не открылась: {:?} ({domain:?} {code}), повтор через {} мс",
                        failure.kind,
                        delay.as_millis()
                    );
                    let (weak, state, url) = (view.downgrade(), retries.clone(), uri.to_string());
                    glib::timeout_add_local_once(delay, move || {
                        let Some(view) = weak.upgrade() else { return };
                        if view.is_loading() {
                            return;
                        }
                        let due = state.borrow_mut().due(load);
                        if due {
                            view.load_uri(&url);
                        }
                    });
                }
                None => {
                    eprintln!(
                        "страница не открылась: {:?} ({domain:?} {code})",
                        failure.kind
                    );
                    // Адрес документа — сам сайт (`base_uri`): так `location.reload()`
                    // у «Повторить» грузит его, а не `about:blank`.
                    let html = page(failure.kind, uri, error.message());
                    view.load_alternate_html(&html, uri, Some(uri));
                }
            }
            true
        });
    }

    #[cfg(test)]
    mod tests {
        use webkit2gtk::{ffi as w, gio::ffi as g};

        use crate::tabs::error_page::code;

        /// Коды в `code` — те же, что в заголовках GIO и WebKitGTK.
        #[test]
        fn codes_match_headers() {
            assert_eq!(code::NET_TRANSPORT, w::WEBKIT_NETWORK_ERROR_TRANSPORT);
            assert_eq!(code::NET_CANCELLED, w::WEBKIT_NETWORK_ERROR_CANCELLED);
            assert_eq!(
                code::POLICY_INTERRUPTED,
                w::WEBKIT_POLICY_ERROR_FRAME_LOAD_INTERRUPTED_BY_POLICY_CHANGE
            );
            assert_eq!(
                code::PLUGIN_WILL_HANDLE_LOAD,
                w::WEBKIT_PLUGIN_ERROR_WILL_HANDLE_LOAD
            );
            assert_eq!(code::IO_FAILED, g::G_IO_ERROR_FAILED);
            assert_eq!(code::IO_CANCELLED, g::G_IO_ERROR_CANCELLED);
            assert_eq!(code::IO_TIMED_OUT, g::G_IO_ERROR_TIMED_OUT);
            assert_eq!(code::IO_HOST_NOT_FOUND, g::G_IO_ERROR_HOST_NOT_FOUND);
            assert_eq!(code::IO_PARTIAL_INPUT, g::G_IO_ERROR_PARTIAL_INPUT);
            assert_eq!(code::IO_HOST_UNREACHABLE, g::G_IO_ERROR_HOST_UNREACHABLE);
            assert_eq!(
                code::IO_NETWORK_UNREACHABLE,
                g::G_IO_ERROR_NETWORK_UNREACHABLE
            );
            assert_eq!(
                code::IO_CONNECTION_REFUSED,
                g::G_IO_ERROR_CONNECTION_REFUSED
            );
            assert_eq!(code::IO_PROXY_FAILED, g::G_IO_ERROR_PROXY_FAILED);
            assert_eq!(code::IO_PROXY_NOT_ALLOWED, g::G_IO_ERROR_PROXY_NOT_ALLOWED);
            assert_eq!(code::IO_CONNECTION_CLOSED, g::G_IO_ERROR_CONNECTION_CLOSED);
            assert_eq!(code::IO_NOT_CONNECTED, g::G_IO_ERROR_NOT_CONNECTED);
            assert_eq!(code::TLS_MISC, g::G_TLS_ERROR_MISC);
            assert_eq!(code::TLS_BAD_CERTIFICATE, g::G_TLS_ERROR_BAD_CERTIFICATE);
            assert_eq!(code::TLS_NOT_TLS, g::G_TLS_ERROR_NOT_TLS);
            assert_eq!(code::TLS_HANDSHAKE, g::G_TLS_ERROR_HANDSHAKE);
            assert_eq!(
                code::TLS_CERTIFICATE_REQUIRED,
                g::G_TLS_ERROR_CERTIFICATE_REQUIRED
            );
            assert_eq!(code::TLS_EOF, g::G_TLS_ERROR_EOF);
            assert_eq!(
                code::TLS_INAPPROPRIATE_FALLBACK,
                g::G_TLS_ERROR_INAPPROPRIATE_FALLBACK
            );
            assert_eq!(
                code::TLS_BAD_CERTIFICATE_PASSWORD,
                g::G_TLS_ERROR_BAD_CERTIFICATE_PASSWORD
            );
            assert_eq!(
                code::DNS_TEMPORARY_FAILURE,
                g::G_RESOLVER_ERROR_TEMPORARY_FAILURE
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kind(domain: Domain, code: i32, message: &str) -> Option<(Kind, bool)> {
        classify(domain, code, message).map(|f| (f.kind, f.transient))
    }

    #[test]
    fn skips_what_is_not_an_error() {
        assert_eq!(kind(Domain::Network, code::NET_CANCELLED, ""), None);
        assert_eq!(kind(Domain::Io, code::IO_CANCELLED, ""), None);
        // Страница стала загрузкой файла.
        assert_eq!(
            kind(
                Domain::Policy,
                code::POLICY_INTERRUPTED,
                "Frame load was interrupted"
            ),
            None
        );
        assert_eq!(
            kind(Domain::Plugin, code::PLUGIN_WILL_HANDLE_LOAD, ""),
            None
        );
        // Те же числа в другом домене — уже ошибка.
        assert!(kind(Domain::Io, code::NET_CANCELLED, "").is_some());
        assert!(kind(Domain::Policy, 103, "restricted port").is_some());
    }

    #[test]
    fn handshake_is_retried_certificate_never() {
        for c in [
            code::TLS_MISC,
            code::TLS_NOT_TLS,
            code::TLS_HANDSHAKE,
            code::TLS_EOF,
            code::TLS_INAPPROPRIATE_FALLBACK,
        ] {
            assert_eq!(
                kind(Domain::Tls, c, "Error performing TLS handshake"),
                Some((Kind::Tls, true)),
                "{c}"
            );
        }
        for c in [
            code::TLS_BAD_CERTIFICATE,
            code::TLS_CERTIFICATE_REQUIRED,
            code::TLS_BAD_CERTIFICATE_PASSWORD,
        ] {
            assert_eq!(
                kind(Domain::Tls, c, "Unacceptable TLS certificate"),
                Some((Kind::Certificate, false)),
                "{c}"
            );
        }
        // Нет glib-networking — повтор не поможет.
        assert_eq!(
            kind(Domain::Tls, 0, "TLS/SSL support not available"),
            Some((Kind::Other, false))
        );
        // Обрыв рукопожатия, пришедший ошибкой сокета.
        assert_eq!(
            kind(
                Domain::Io,
                code::IO_FAILED,
                "Error performing TLS handshake: Error in the pull function"
            ),
            Some((Kind::Tls, true))
        );
        assert_eq!(
            kind(
                Domain::Io,
                code::IO_CONNECTION_CLOSED,
                "TLS connection closed"
            ),
            Some((Kind::Tls, true))
        );
    }

    #[test]
    fn network_kinds() {
        for c in [
            code::IO_PARTIAL_INPUT,
            code::IO_HOST_UNREACHABLE,
            code::IO_NETWORK_UNREACHABLE,
            code::IO_CONNECTION_REFUSED,
            code::IO_CONNECTION_CLOSED,
            code::IO_NOT_CONNECTED,
        ] {
            assert_eq!(
                kind(Domain::Io, c, "Connection reset by peer"),
                Some((Kind::Offline, true)),
                "{c}"
            );
        }
        assert_eq!(
            kind(Domain::Network, code::NET_TRANSPORT, ""),
            Some((Kind::Offline, true))
        );
        assert_eq!(
            kind(Domain::Resolver, code::DNS_TEMPORARY_FAILURE, ""),
            Some((Kind::Dns, true))
        );
        assert_eq!(kind(Domain::Resolver, 0, ""), Some((Kind::Dns, false)));
        assert_eq!(
            kind(Domain::Io, code::IO_HOST_NOT_FOUND, ""),
            Some((Kind::Dns, false))
        );
        assert_eq!(
            kind(Domain::Io, code::IO_TIMED_OUT, ""),
            Some((Kind::Timeout, false))
        );
        for c in code::IO_PROXY_FAILED..=code::IO_PROXY_NOT_ALLOWED {
            assert_eq!(kind(Domain::Io, c, ""), Some((Kind::Proxy, false)), "{c}");
        }
        assert_eq!(
            kind(Domain::Io, code::IO_FAILED, "HTTP/2 Error: PROTOCOL_ERROR"),
            Some((Kind::Other, false))
        );
        assert_eq!(kind(Domain::Other, 7, "?"), Some((Kind::Other, false)));
        assert_eq!(kind(Domain::Network, 399, ""), Some((Kind::Other, false)));
    }

    #[test]
    fn retries_twice_then_gives_up() {
        let mut r = Retries::default();
        r.started();
        assert_eq!(r.failed(true), Some((RETRY_AFTER[0], 1)));
        assert!(r.due(1));
        // Наш повтор начался — счёт не сбрасывается.
        r.started();
        assert_eq!(r.failed(true), Some((RETRY_AFTER[1], 2)));
        assert!(r.due(2));
        r.started();
        assert_eq!(r.failed(true), None);
        // Человек нажал «Повторить» — снова два повтора.
        r.started();
        assert_eq!(r.failed(true), Some((RETRY_AFTER[0], 4)));
    }

    #[test]
    fn no_retry_after_navigation_or_for_lasting_errors() {
        let mut r = Retries::default();
        r.started();
        let first = r.failed(true);
        assert!(first.is_some());
        let load = first.map_or(0, |(_, load)| load);
        // Пока ждали, во вкладке начался другой переход — повтор опоздал.
        r.started();
        assert!(!r.due(load));
        // Тот переход тоже не удался: счёт с нуля, он ведь новый.
        assert_eq!(r.failed(true).map(|(d, _)| d), Some(RETRY_AFTER[0]));
        // Сертификат, «сайт не найден» — сразу страница ошибки.
        let mut r = Retries::default();
        r.started();
        assert_eq!(r.failed(false), None);
        assert_eq!(r.done, 0);
    }

    #[test]
    fn escapes_everything_from_outside() {
        assert_eq!(
            escape(r#"<script>alert("x&y")</script>'"#),
            "&lt;script&gt;alert(&quot;x&amp;y&quot;)&lt;/script&gt;&#39;"
        );
        let html = page(
            Kind::Tls,
            "https://evil.example/\"><script>alert(1)</script>?a=1&b='2'",
            "Error <b>performing</b> TLS handshake: \"x\"",
        );
        assert!(!html.contains("<script>"), "{html}");
        assert!(!html.contains("<b>"), "{html}");
        assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;?a=1&amp;b=&#39;2&#39;"));
        assert!(html.contains("Подробности: Error &lt;b&gt;performing&lt;/b&gt;"));
        // Один скрипт — наша кнопка.
        assert_eq!(html.matches("onclick=").count(), 1);
    }

    #[test]
    fn page_reads_in_both_themes() {
        let html = page(Kind::Tls, "https://www.youtube.com/watch?v=1", "TLS");
        assert!(html.contains("<title>www.youtube.com</title>"), "{html}");
        assert!(html.contains("Не удалось установить защищённое соединение"));
        assert!(html.contains("https://www.youtube.com/watch?v=1"));
        assert!(html.contains("Повторить") && html.contains("location.reload()"));
        assert!(html.contains("<meta charset=\"utf-8\">"));
        assert!(html.contains("color-scheme:light dark"));
        // Цвет текста и фона задан для обеих тем.
        let dark = html.find("prefers-color-scheme:dark").unwrap_or(0);
        assert!(dark > 0);
        for var in ["--bg:", "--fg:"] {
            assert_eq!(html.matches(var).count(), 2, "{var}");
            assert!(
                html[..dark].contains(var) && html[dark..].contains(var),
                "{var}"
            );
        }
        // Ничего внешнего.
        assert!(
            !html.contains("http://") && !html.contains("src="),
            "{html}"
        );
        assert!(!html.contains("url("), "{html}");
        // Без текста движка — без строки подробностей.
        assert!(!page(Kind::Dns, "https://a.example/", " ").contains("Подробности"));
    }

    #[test]
    fn every_kind_has_words() {
        for k in [
            Kind::Offline,
            Kind::Tls,
            Kind::Dns,
            Kind::Timeout,
            Kind::Certificate,
            Kind::Proxy,
            Kind::Other,
        ] {
            let (heading, text) = words(k);
            assert!(!heading.is_empty() && text.len() > 20, "{k:?}");
            assert!(!heading.contains('<') && !text.contains('<'), "{k:?}");
        }
    }

    #[test]
    fn long_text_is_shortened() {
        assert_eq!(shorten("абвгд", 3), "абв…");
        assert_eq!(shorten("абв", 3), "абв");
        let long = format!("https://a.example/{}", "я".repeat(2000));
        let html = page(Kind::Other, &long, &"x".repeat(5000));
        assert!(html.chars().count() < 6000, "{}", html.len());
        // Адрес без сайта — заголовок из самого адреса.
        assert!(page(Kind::Other, "about:blank", "").contains("<title>about:blank</title>"));
    }
}
