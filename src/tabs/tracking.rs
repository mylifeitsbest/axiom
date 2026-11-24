//! Метки слежки в адресах: `utm_*`, `fbclid`, `gclid` и подобные.
//!
//! Убираем их из адресов, которые открываются во вкладках: ввод в адресной
//! строке, ссылки из других программ, новые вкладки от сайтов и переходы
//! по ссылкам. Только из запроса верхнего уровня: фрагмент `#…` и адреса
//! внутри значений не трогаем, порядок и кодировку остальных параметров
//! сохраняем. Если меток нет, адрес не переписываем вовсе.
//!
//! Переход по ссылке внутри вкладки переписываем, только когда это безопасно:
//! переход начал человек (клик), это новый документ (не «Назад» и не F5),
//! не перенаправление сервера и не отправка формы. Иначе адрес остаётся как есть.
//! Чистый адрес второй раз не чистится, а повтор того же адреса за пару секунд
//! пропускаем — циклов нет.
//!
//! Как переписываем: `NavigationStarting` запрос не держит — WebView2 шлёт его
//! на сервер параллельно с событием, так что отмена и повторная загрузка
//! всё равно отдали бы метки серверу. Поэтому в `NavigationStarting` только
//! запоминаем решение, а сам запрос документа перехватываем в
//! `WebResourceRequested` и отвечаем перенаправлением 307 на чистый адрес:
//! на сервер уходит только он, в истории — только он. Если перехват
//! не встал (старый WebView2), отменяем переход и грузим чистый адрес.
//!
//! WebKitGTK: wry отдаёт в обработчик навигации только адрес, без метода,
//! жеста и признака перенаправления, поэтому там переходы по ссылкам
//! не переписываем — только ввод, ссылки извне и новые вкладки.

use std::sync::atomic::{AtomicBool, Ordering};

use url::Url;

/// Метки рекламных систем и рассылок (имя без учёта регистра).
/// Плюс всё, что начинается с `utm_` (Google Analytics и почти все остальные).
const PARAMS: &[&str] = &[
    "fbclid",      // Facebook
    "gclid",       // Google Ads
    "dclid",       // Google Display & Video 360
    "gbraid",      // Google Ads, iOS
    "wbraid",      // Google Ads, iOS
    "yclid",       // Яндекс Директ
    "ysclid",      // Яндекс
    "msclkid",     // Microsoft Ads
    "igshid",      // Instagram
    "mc_cid",      // Mailchimp
    "mc_eid",      // Mailchimp
    "_hsenc",      // HubSpot
    "_hsmi",       // HubSpot
    "mkt_tok",     // Marketo
    "twclid",      // X (Twitter)
    "li_fat_id",   // LinkedIn
    "oly_enc_id",  // Omeda
    "oly_anon_id", // Omeda
    "vero_id",     // Vero
    "_openstat",   // Openstat
];

/// Включено ли (настройка `strip_tracking`). Обработчик навигации живёт
/// в webview и про настройки не знает, поэтому флаг общий.
static ENABLED: AtomicBool = AtomicBool::new(true);

pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

#[cfg_attr(not(windows), allow(dead_code))]
fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Адрес без меток слежки. `None` — убирать нечего (или это не http/https).
pub fn clean(url: &str) -> Option<String> {
    let mut parsed = Url::parse(url).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return None;
    }
    let mut removed = false;
    let kept: Vec<&str> = parsed
        .query()?
        .split('&')
        .filter(|pair| !pair.is_empty())
        .filter(|pair| {
            let tracking = is_tracking(pair);
            removed |= tracking;
            !tracking
        })
        .collect();
    if !removed {
        return None;
    }
    // Остальные параметры — те же куски исходной строки: кодировка не меняется.
    let query = kept.join("&");
    parsed.set_query((!query.is_empty()).then_some(query.as_str()));
    Some(parsed.into())
}

fn is_tracking(pair: &str) -> bool {
    let raw = pair.split('=').next().unwrap_or(pair);
    // Имя бывает закодировано (`utm%5Fsource`), а `+` в запросе — пробел.
    let Some((name, _)) = url::form_urlencoded::parse(raw.as_bytes()).next() else {
        return false;
    };
    let name = name.to_ascii_lowercase();
    name.starts_with("utm_") || PARAMS.contains(&name.as_str())
}

/// Чистит переходы по ссылкам во вкладке. Ставить после сборки webview:
/// наш `NavigationStarting` идёт после обработчика wry и видит его решение.
#[cfg(windows)]
pub fn install(
    webview: &wry::WebView,
    tab: super::TabId,
    events: tao::event_loop::EventLoopProxy<crate::events::UserEvent>,
) {
    use webview2_com::{NavigationStartingEventHandler, take_pwstr};
    use windows_core::{BOOL, PWSTR};
    use wry::WebViewExtWindows;

    let redirect = match redirect::install(webview) {
        Ok(()) => true,
        Err(err) => {
            eprintln!("перехват запросов для чистых адресов недоступен: {err}");
            false
        }
    };
    let handler = NavigationStartingEventHandler::create(Box::new(move |_, args| {
        let Some(args) = args else { return Ok(()) };
        if !enabled() {
            return Ok(());
        }
        let mut cancelled = BOOL::default();
        let mut raw = PWSTR::null();
        // SAFETY: args — живой COM-объект события, указатели на локальные переменные.
        unsafe {
            args.Cancel(&mut cancelled)?;
            args.Uri(&mut raw)?;
        }
        let url = take_pwstr(raw);
        // Переход уже отменён (не веб-адрес) — решать нечего.
        if cancelled.as_bool() {
            return Ok(());
        }
        let Some(clean) = clean(&url) else {
            return Ok(());
        };
        if !safe_to_rewrite(&args)? || !guard::take(tab, &clean) {
            return Ok(());
        }
        if redirect {
            redirect::expect(&url, clean);
        } else {
            // SAFETY: как выше.
            unsafe { args.SetCancel(true)? };
            let _ = events.send_event(crate::events::UserEvent::Untracked {
                id: tab,
                url: clean,
            });
        }
        Ok(())
    }));
    let mut token = 0i64;
    // SAFETY: ядро webview живёт столько же, сколько сам webview.
    let result = unsafe {
        webview
            .webview()
            .add_NavigationStarting(&handler, &mut token)
    };
    if let Err(err) = result {
        eprintln!("очистка адресов от меток недоступна: {err}");
    }
}

/// Перехват запроса документа: вместо адреса с метками — перенаправление 307
/// на чистый. Отвечаем только на запросы, которые `NavigationStarting` только что
/// отметил как безопасные (верхний уровень, клик, GET): остальные документы,
/// в том числе фреймы, идут как шли.
#[cfg(windows)]
mod redirect {
    use std::{
        cell::RefCell,
        collections::VecDeque,
        time::{Duration, Instant},
    };

    use webview2_com::{
        Microsoft::Web::WebView2::Win32::COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT,
        WebResourceRequestedEventHandler, take_pwstr,
    };
    use windows_core::{HSTRING, PWSTR, w};
    use wry::WebViewExtWindows;

    /// Запрос приходит сразу после `NavigationStarting`; старые записи — переходы,
    /// которые до сети не дошли (кэш, отмена).
    const TTL: Duration = Duration::from_secs(5);
    const MAX: usize = 16;

    thread_local! {
        /// (адрес запроса без `#`, куда перенаправить, когда решили).
        static EXPECTED: RefCell<VecDeque<(String, String, Instant)>> =
            const { RefCell::new(VecDeque::new()) };
    }

    pub fn expect(url: &str, clean: String) {
        EXPECTED.with(|q| {
            let mut q = q.borrow_mut();
            q.retain(|(_, _, at)| at.elapsed() < TTL);
            if q.len() >= MAX {
                q.pop_front();
            }
            q.push_back((without_fragment(url).to_string(), clean, Instant::now()));
        });
    }

    fn take(url: &str) -> Option<String> {
        EXPECTED.with(|q| {
            let mut q = q.borrow_mut();
            if q.is_empty() {
                return None;
            }
            let i = q
                .iter()
                .position(|(u, _, at)| u == url && at.elapsed() < TTL)?;
            q.remove(i).map(|(_, clean, _)| clean)
        })
    }

    fn without_fragment(url: &str) -> &str {
        url.split('#').next().unwrap_or(url)
    }

    pub fn install(webview: &wry::WebView) -> windows_core::Result<()> {
        let core = webview.webview();
        let env = webview.environment();
        // SAFETY: ядро webview живёт столько же, сколько сам webview.
        unsafe {
            core.AddWebResourceRequestedFilter(w!("*"), COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT)?
        };
        let handler = WebResourceRequestedEventHandler::create(Box::new(move |_, args| {
            let Some(args) = args else { return Ok(()) };
            let (mut uri, mut method) = (PWSTR::null(), PWSTR::null());
            // SAFETY: args — живой COM-объект события, указатели на локальные переменные.
            let request = unsafe { args.Request()? };
            unsafe {
                request.Uri(&mut uri)?;
                request.Method(&mut method)?;
            }
            let (uri, method) = (take_pwstr(uri), take_pwstr(method));
            let Some(clean) = take(&uri) else {
                return Ok(());
            };
            if !method.eq_ignore_ascii_case("GET") {
                return Ok(());
            }
            // Чистый адрес — из `Url`: переводов строк в нём нет, заголовок не разорвать.
            let headers = HSTRING::from(format!("Location: {clean}\nCache-Control: no-store"));
            // SAFETY: окружение живёт столько же, сколько webview; поток интерфейса.
            unsafe {
                let response =
                    env.CreateWebResourceResponse(None, 307, w!("Temporary Redirect"), &headers)?;
                args.SetResponse(&response)?;
            }
            Ok(())
        }));
        let mut token = 0i64;
        // SAFETY: как выше.
        unsafe { core.add_WebResourceRequested(&handler, &mut token) }
    }
}

#[cfg(not(windows))]
pub fn install(
    _webview: &wry::WebView,
    _tab: super::TabId,
    _events: tao::event_loop::EventLoopProxy<crate::events::UserEvent>,
) {
}

/// Можно ли отменить переход и начать заново с другим адресом, ничего не сломав.
#[cfg(windows)]
fn safe_to_rewrite(
    args: &webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2NavigationStartingEventArgs,
) -> windows_core::Result<bool> {
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_NAVIGATION_KIND, COREWEBVIEW2_NAVIGATION_KIND_NEW_DOCUMENT,
        ICoreWebView2NavigationStartingEventArgs3,
    };
    use windows_core::{BOOL, Interface, w};

    let (mut user, mut redirected, mut body) = (BOOL::default(), BOOL::default(), BOOL::default());
    // SAFETY: args — живой COM-объект события, указатели на локальные переменные.
    unsafe {
        args.IsUserInitiated(&mut user)?;
        args.IsRedirected(&mut redirected)?;
    }
    // Скрипт без клика может перейти на адрес с меткой снова и снова, а ответ
    // сервера (перенаправление) уже учёл всё, что хотел: такие не трогаем.
    if !user.as_bool() || redirected.as_bool() {
        return Ok(false);
    }
    // «Назад», «Вперёд» и F5 идут по записям истории: новый переход их бы испортил.
    // Старый WebView2 вида перехода не сообщает — тогда тоже не трогаем.
    let Ok(args3) = args.cast::<ICoreWebView2NavigationStartingEventArgs3>() else {
        return Ok(false);
    };
    let mut kind = COREWEBVIEW2_NAVIGATION_KIND::default();
    // SAFETY: как выше.
    unsafe { args3.NavigationKind(&mut kind)? };
    if kind != COREWEBVIEW2_NAVIGATION_KIND_NEW_DOCUMENT {
        return Ok(false);
    }
    // Отправка формы (POST): у запроса есть тело, а `load_url` отправил бы GET без него.
    // SAFETY: как выше.
    unsafe {
        let headers = args.RequestHeaders()?;
        headers.Contains(w!("Content-Type"), &mut body)?;
    }
    Ok(!body.as_bool())
}

/// Страховка от циклов: один и тот же чистый адрес в той же вкладке
/// не переписываем чаще раза в несколько секунд.
#[cfg(windows)]
mod guard {
    use std::{
        cell::RefCell,
        time::{Duration, Instant},
    };

    use crate::tabs::TabId;

    const WINDOW: Duration = Duration::from_secs(3);

    thread_local! {
        static LAST: RefCell<Option<(TabId, String, Instant)>> = const { RefCell::new(None) };
    }

    pub fn take(tab: TabId, clean: &str) -> bool {
        LAST.with(|last| {
            let mut last = last.borrow_mut();
            let repeat = last
                .as_ref()
                .is_some_and(|(t, u, at)| *t == tab && u == clean && at.elapsed() < WINDOW);
            if !repeat {
                *last = Some((tab, clean.to_string(), Instant::now()));
            }
            !repeat
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(url: &str) -> Option<String> {
        clean(url)
    }

    #[test]
    fn removes_tracking_params() {
        assert_eq!(
            c("https://example.com/page?utm_source=x&id=1"),
            Some("https://example.com/page?id=1".into())
        );
        assert_eq!(
            c("https://example.com/?id=1&fbclid=AbC&utm_medium=mail&gclid=1"),
            Some("https://example.com/?id=1".into())
        );
        // Все параметры — метки: знака вопроса не остаётся.
        assert_eq!(
            c("https://example.com/a?utm_source=x&utm_campaign=y"),
            Some("https://example.com/a".into())
        );
        for name in PARAMS {
            let url = format!("https://example.com/?{name}=1&q=2");
            assert_eq!(c(&url), Some("https://example.com/?q=2".into()), "{name}");
        }
        // Имя без учёта регистра и закодированное.
        assert_eq!(
            c("https://example.com/?UTM_Source=x&FBCLID=y&q=1"),
            Some("https://example.com/?q=1".into())
        );
        assert_eq!(
            c("https://example.com/?utm%5Fsource=x&q=1"),
            Some("https://example.com/?q=1".into())
        );
    }

    #[test]
    fn leaves_clean_urls_alone() {
        assert_eq!(c("https://example.com/"), None);
        assert_eq!(c("https://example.com/?"), None);
        assert_eq!(c("https://example.com/?q=1&id=2"), None);
        // Другой порядок параметров — не повод переписывать адрес.
        assert_eq!(c("https://example.com/?b=2&a=1"), None);
        // Похожие, но чужие имена.
        assert_eq!(
            c("https://example.com/?utm=1&source_utm_x=2&gclid2=3"),
            None
        );
        // Не веб-адреса не трогаем.
        assert_eq!(c("file:///C:/a.html?utm_source=x"), None);
        assert_eq!(c("mailto:a@b.c?utm_source=x"), None);
        assert_eq!(c("не адрес"), None);
    }

    #[test]
    fn empty_values_and_pieces() {
        assert_eq!(
            c("https://example.com/?utm_source=&id="),
            Some("https://example.com/?id=".into())
        );
        assert_eq!(
            c("https://example.com/?id=&utm_medium"),
            Some("https://example.com/?id=".into())
        );
        // Пустые куски (`&&`) убираем, только если и так переписываем адрес.
        assert_eq!(
            c("https://example.com/?a=1&&utm_source=x&&b"),
            Some("https://example.com/?a=1&b".into())
        );
        assert_eq!(c("https://example.com/?a=1&&b=2"), None);
    }

    #[test]
    fn keeps_encoding_and_fragment() {
        assert_eq!(
            c(
                "https://example.com/?q=%D0%BF%D1%80%D0%B8%D0%B2%D0%B5%D1%82+%D0%BC%D0%B8%D1%80&utm_source=x"
            ),
            Some(
                "https://example.com/?q=%D0%BF%D1%80%D0%B8%D0%B2%D0%B5%D1%82+%D0%BC%D0%B8%D1%80"
                    .into()
            )
        );
        assert_eq!(
            c("https://example.com/?a=%26b%3Dc&yclid=1&x=%2B"),
            Some("https://example.com/?a=%26b%3Dc&x=%2B".into())
        );
        // Фрагмент остаётся как был, даже если в нём похожие слова.
        assert_eq!(
            c("https://example.com/p?utm_source=x&id=1#utm_source=y&s"),
            Some("https://example.com/p?id=1#utm_source=y&s".into())
        );
        assert_eq!(c("https://example.com/p#?utm_source=y"), None);
        // Адрес внутри значения — не запрос верхнего уровня.
        assert_eq!(
            c("https://r.example/go?to=https%3A%2F%2Fa.b%2F%3Futm_source%3Dx&utm_term=t"),
            Some("https://r.example/go?to=https%3A%2F%2Fa.b%2F%3Futm_source%3Dx".into())
        );
    }

    #[test]
    fn idempotent() {
        for url in [
            "https://example.com/page?utm_source=x&id=1",
            "https://example.com/?utm_source=&id=&q=%20#f",
            "http://127.0.0.1:8765/t.html?utm_source=x&id=1",
            "https://пример.рф/?fbclid=1&q=да",
        ] {
            let once = c(url);
            assert!(once.is_some(), "{url}");
            assert_eq!(once.as_deref().and_then(c), None, "{url}");
        }
    }
}
