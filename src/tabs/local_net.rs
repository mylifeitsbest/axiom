//! Сайты из интернета не обращаются к устройствам локальной сети и к этому компьютеру.
//!
//! Без запрета страница без единого клика шлёт запросы роутеру, принтеру, dev-серверу
//! на 127.0.0.1: подделывает формы (CSRF) и перебирает порты. Правило — как Local
//! Network Access в Chromium: адреса делятся на «этот компьютер», «локальная сеть»
//! и «интернет», и страница не обращается к пространству ближе своего. Сайт — по адресу
//! вкладки, так что и фрейм внутри неё запрет не обойдёт. Переходы самой вкладки
//! (набрали адрес роутера, сеть отеля перенаправила на 10.0.0.1) не запрещаются.
//!
//! Linux: правило в расширении веб-процесса (`webext.c`, те же случаи в его тесте).
//! Windows: WebView2 сам такие запросы не запрещает — его политика Local Network Access
//! «Allow» даже с включённой функцией Chromium (проверено на WebView2 158), поэтому
//! запросы к локальным адресам ловим фильтром `WebResourceRequested` и отвечаем 403.
//! Фильтр — только на локальные адреса: остальные запросы в Axiom не заходят и не ждут
//! главного потока. Имя без точки (`http://router/`) шаблоном не описать — на Windows
//! его не ловим. Имя, которое DNS превратит в локальный адрес, не видно нигде: до
//! соединения его ещё никто не разрешил.

// Linux правило держит в C, здесь ему нужны только тесты.
#![cfg_attr(not(windows), allow(dead_code))]

use std::net::IpAddr;

/// Пространство адресов: чем меньше, тем «ближе».
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Space {
    Loopback,
    Private,
    Public,
}

/// Хост из адреса http(s)/ws(s) — как его привёл движок: «схема://[кто@]хост[:порт][/?#…]»,
/// у `blob:` — его создатель. Скобки IPv6 снимаем. Не такой адрес — `None`.
fn host_of(uri: &str) -> Option<&str> {
    let uri = strip_prefix_ci(uri, "blob:").unwrap_or(uri);
    let rest = ["http://", "https://", "ws://", "wss://"]
        .iter()
        .find_map(|scheme| strip_prefix_ci(uri, scheme))?;
    let end = rest.find(['/', '?', '#', '\\']).unwrap_or(rest.len());
    let authority = &rest[..end];
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    if let Some(v6) = host.strip_prefix('[') {
        let close = v6.find(']')?;
        return (close > 0).then(|| &v6[..close]);
    }
    let host = host.split_once(':').map_or(host, |(host, _)| host);
    (!host.is_empty()).then_some(host)
}

fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let head = s.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| &s[prefix.len()..])
}

fn space_of_ip(ip: IpAddr) -> Space {
    // ::ffff:a.b.c.d — тот же IPv4.
    let ip = match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(ip, IpAddr::V4),
        v4 => v4,
    };
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            // 0.0.0.0 — тоже этот компьютер.
            if a == 127 || a == 0 {
                Space::Loopback
            } else if a == 10
                || (a == 172 && b & 0xf0 == 16)
                || (a == 192 && b == 168)
                // link-local и CGNAT (100.64/10).
                || (a == 169 && b == 254)
                || (a == 100 && b & 0xc0 == 64)
            {
                Space::Private
            } else {
                Space::Public
            }
        }
        IpAddr::V6(v6) => {
            let first = v6.segments()[0];
            if v6.is_loopback() || v6.is_unspecified() {
                Space::Loopback
            // fc00::/7 (ULA), fe80::/10 (link-local), fec0::/10 (старый site-local).
            } else if first & 0xfe00 == 0xfc00 || first & 0xff80 == 0xfe80 {
                Space::Private
            } else {
                Space::Public
            }
        }
    }
}

fn space_of_host(raw: &str) -> Option<Space> {
    let host = raw.to_ascii_lowercase();
    let host = host.trim_end_matches('.');
    if host.is_empty() {
        return None;
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Some(space_of_ip(ip));
    }
    Some(if host == "localhost" || host.ends_with(".localhost") {
        Space::Loopback
    } else if host.ends_with(".local") || !host.contains('.') {
        // mDNS (`printer.local`) и имена без точки (`http://router/`) — это сеть рядом.
        Space::Private
    } else {
        Space::Public
    })
}

/// Пространство адреса; не сайт (`data:`, `about:`, кривой адрес) — `None`.
pub fn space_of(uri: &str) -> Option<Space> {
    space_of_host(host_of(uri)?)
}

/// Страница с адресом `page` не может запросить `target`.
pub fn crosses_into_local(page: &str, target: &str) -> bool {
    let Some(from) = space_of(page) else {
        return false;
    };
    from != Space::Loopback && space_of(target).is_some_and(|to| to < from)
}

/// Подключить запрет к вкладке или панели ИИ. Звать сразу после сборки webview,
/// до первой загрузки страницы.
pub fn install(webview: &wry::WebView) {
    platform::install(webview);
}

#[cfg(windows)]
mod platform {
    use std::{
        cell::{Cell, RefCell},
        collections::HashMap,
    };

    use webview2_com::{
        Microsoft::Web::WebView2::Win32::{
            COREWEBVIEW2_WEB_RESOURCE_CONTEXT, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
            COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT,
            COREWEBVIEW2_WEB_RESOURCE_REQUEST_SOURCE_KINDS_ALL, ICoreWebView2, ICoreWebView2_2,
            ICoreWebView2_22, ICoreWebView2WebResourceRequestedEventArgs,
        },
        NavigationStartingEventHandler, WebResourceRequestedEventHandler, take_pwstr,
    };
    use windows_core::{HSTRING, Interface, PWSTR, w};
    use wry::{WebView, WebViewExtWindows};

    /// Адреса, которые могут вести в локальную сеть. Шаблон грубее правила (`172.*`,
    /// `100.*`, `localhost*`) — точно решает [`super::space_of`]. `*@*` — адрес с именем
    /// перед хостом (`http://a@127.0.0.1/`): по началу его не узнать.
    const PATTERNS: [&str; 14] = [
        "*://localhost*",
        "*://*.localhost*",
        "*://127.*",
        "*://0.*",
        "*://10.*",
        "*://172.*",
        "*://192.168.*",
        "*://169.254.*",
        "*://100.*",
        "*://[*",
        "*://*.local",
        "*://*.local/*",
        "*://*.local:*",
        "*://*@*",
    ];

    thread_local! {
        /// Последний переход самой вкладки (ключ — указатель на ядро): его документ
        /// пропускаем, даже если старая страница из интернета, а новый адрес — роутер.
        static NAVIGATING: RefCell<HashMap<usize, String>> = RefCell::new(HashMap::new());
        /// Сколько запретов уже записано в журнал: перебор портов дал бы тысячи строк.
        static TOLD: Cell<u32> = const { Cell::new(0) };
    }

    fn key(core: &ICoreWebView2) -> usize {
        core.as_raw() as usize
    }

    pub fn install(webview: &WebView) {
        let core = webview.webview();
        // Указатель мог достаться от закрытой вкладки: её переход здесь ни при чём.
        NAVIGATING.with(|n| n.borrow_mut().remove(&key(&core)));
        let navigating = NavigationStartingEventHandler::create(Box::new(|sender, args| {
            if let (Some(core), Some(args)) = (sender, args) {
                let mut raw = PWSTR::null();
                // SAFETY: args — живой COM-объект события, указатель на локальную переменную.
                unsafe { args.Uri(&mut raw)? };
                let uri = take_pwstr(raw);
                NAVIGATING.with(|n| n.borrow_mut().insert(key(&core), uri));
            }
            Ok(())
        }));
        let requested = WebResourceRequestedEventHandler::create(Box::new(|sender, args| {
            match (sender, args) {
                (Some(core), Some(args)) => on_request(&core, &args),
                _ => Ok(()),
            }
        }));
        let mut token = 0i64;
        // SAFETY: ядро webview живёт столько же, сколько сам webview; строки шаблонов
        // живут до конца каждого вызова.
        unsafe {
            if let Err(err) = core.add_NavigationStarting(&navigating, &mut token) {
                eprintln!("запрет на локальную сеть недоступен: {err}");
                return;
            }
            if let Err(err) = core.add_WebResourceRequested(&requested, &mut token) {
                eprintln!("запрет на локальную сеть недоступен: {err}");
                return;
            }
            // С видами источников — и запросы воркеров сайта, без — только документов.
            let with_kinds = core.cast::<ICoreWebView2_22>().ok();
            for pattern in PATTERNS {
                let pattern = HSTRING::from(pattern);
                let added = match &with_kinds {
                    Some(core) => core.AddWebResourceRequestedFilterWithRequestSourceKinds(
                        &pattern,
                        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
                        COREWEBVIEW2_WEB_RESOURCE_REQUEST_SOURCE_KINDS_ALL,
                    ),
                    None => core.AddWebResourceRequestedFilter(
                        &pattern,
                        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
                    ),
                };
                if let Err(err) = added {
                    eprintln!("запрет на локальную сеть: шаблон {pattern} не встал: {err}");
                }
            }
        }
    }

    fn on_request(
        core: &ICoreWebView2,
        args: &ICoreWebView2WebResourceRequestedEventArgs,
    ) -> windows_core::Result<()> {
        // SAFETY: args и core — живые COM-объекты события, указатели на локальные переменные.
        let request = unsafe { args.Request()? };
        let mut raw = PWSTR::null();
        unsafe { request.Uri(&mut raw)? };
        let target = take_pwstr(raw);
        // Шаблоны грубее правила: сначала дёшево отсеять не локальное.
        if super::space_of(&target).is_none_or(|space| space == super::Space::Public) {
            return Ok(());
        }
        let mut context = COREWEBVIEW2_WEB_RESOURCE_CONTEXT::default();
        unsafe { args.ResourceContext(&mut context)? };
        if context == COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT
            && NAVIGATING.with(|n| n.borrow().get(&key(core)) == Some(&target))
        {
            return Ok(());
        }
        let mut raw = PWSTR::null();
        unsafe { core.Source(&mut raw)? };
        let page = take_pwstr(raw);
        if !super::crosses_into_local(&page, &target) {
            return Ok(());
        }
        let environment = unsafe { core.cast::<ICoreWebView2_2>()?.Environment()? };
        let response =
            unsafe { environment.CreateWebResourceResponse(None, 403, w!("Forbidden"), w!(""))? };
        unsafe { args.SetResponse(&response)? };
        TOLD.with(|told| {
            if told.get() < 20 {
                told.set(told.get() + 1);
                eprintln!("запрос к локальной сети запрещён: {target} (со страницы {page})");
            }
        });
        Ok(())
    }
}

/// Linux: правило — в расширении веб-процесса (`webext.c`).
#[cfg(not(windows))]
mod platform {
    pub fn install(_webview: &wry::WebView) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Те же случаи, что в тесте `webext.c`: правила на Windows и Linux одни.
    #[test]
    fn same_rules_as_webext() {
        let cases: &[(&str, &str, bool)] = &[
            // Интернет → компьютер и локальная сеть.
            (
                "https://evil.example/",
                "http://127.0.0.1:18906/secret",
                true,
            ),
            ("https://evil.example/", "http://localhost:8080/", true),
            ("https://evil.example/", "http://api.localhost./", true),
            ("https://evil.example/", "http://[::1]:631/", true),
            ("https://evil.example/", "http://[::ffff:127.0.0.1]/", true),
            ("https://evil.example/", "http://0.0.0.0:3000/", true),
            (
                "https://evil.example/",
                "http://192.168.0.1/apply.cgi",
                true,
            ),
            ("https://evil.example/", "http://10.1.2.3/", true),
            ("https://evil.example/", "http://172.31.0.1/", true),
            (
                "https://evil.example/",
                "http://169.254.169.254/latest/",
                true,
            ),
            ("https://evil.example/", "http://100.100.1.1/", true),
            ("https://evil.example/", "http://[fd00::1]/", true),
            ("https://evil.example/", "http://[fe80::1]/", true),
            ("https://evil.example/", "http://printer.local/", true),
            ("https://evil.example/", "http://router/", true),
            (
                "https://evil.example/",
                "ws://127.0.0.1:9222/devtools",
                true,
            ),
            (
                "https://evil.example/",
                "http://user@192.168.0.1:80/%zz",
                true,
            ),
            ("https://evil.example/", "http://a@b@192.168.0.1/", true),
            ("blob:https://evil.example/1-2", "http://192.168.0.1/", true),
            ("https://EVIL.example/", "HTTP://LOCALHOST/", true),
            // Локальная сеть → компьютер.
            (
                "http://192.168.0.113:18905/attack.html",
                "http://127.0.0.1:18906/",
                true,
            ),
            // Разрешено: в своё пространство и дальше от себя.
            ("https://evil.example/", "https://cdn.example/x.js", false),
            ("https://evil.example/", "http://172.32.0.1/", false),
            ("https://evil.example/", "http://11.0.0.1/", false),
            ("https://evil.example/", "http://100.128.0.1/", false),
            ("https://evil.example/", "http://[2001:db8::1]/", false),
            ("http://192.168.0.1/", "http://192.168.0.1/style.css", false),
            ("http://192.168.0.1/", "http://10.0.0.5/", false),
            ("http://192.168.0.1/", "https://cdn.example/x.js", false),
            ("http://localhost:3000/", "http://127.0.0.1:8080/api", false),
            ("http://localhost:3000/", "http://192.168.0.1/", false),
            ("http://127.0.0.1:18905/", "http://127.0.0.1:18906/", false),
            // Не сайт — не трогаем: интерфейс, файлы, пустая вкладка, кривой адрес.
            (
                "axiom://localhost/",
                "http://192.168.0.1/favicon.ico",
                false,
            ),
            ("file:///home/u/a.html", "http://127.0.0.1/", false),
            ("about:blank", "http://127.0.0.1/", false),
            ("", "http://127.0.0.1/", false),
            ("https://evil.example/", "data:text/plain,1", false),
            ("https://evil.example/", "http://[::1/", false),
            ("https:///", "http://127.0.0.1/", false),
        ];
        let wrong: Vec<_> = cases
            .iter()
            .filter(|(page, target, blocked)| crosses_into_local(page, target) != *blocked)
            .collect();
        assert!(wrong.is_empty(), "неверно: {wrong:#?}");
    }

    /// Интерфейс на Windows — `https://axiom.localhost/`: этот компьютер, ему можно всё.
    #[test]
    fn ui_and_lookalikes() {
        assert!(!crosses_into_local(
            "https://axiom.localhost/index.html",
            "http://192.168.0.1/favicon.ico"
        ));
        // Шаблон `localhost*` ловит и такие адреса — правило их пропускает.
        assert_eq!(
            space_of("https://localhost.example.com/"),
            Some(Space::Public)
        );
        assert!(!crosses_into_local(
            "https://evil.example/",
            "https://localhostfoo.com/"
        ));
        assert_eq!(space_of("https://evil.example/@user"), Some(Space::Public));
    }
}
