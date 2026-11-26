//! Какие страницы открываются в самой панели ИИ.
//!
//! Панель — только для Google AI Mode, а не второй браузер. В ней остаются
//! страницы AI Mode (`google.<зона>/search?udm=50`) и то, без чего он не работает:
//! вход в аккаунт Google, согласие на cookies и проверка «я не робот». Остальное —
//! ссылка из ответа, обычная выдача Google («Все», «Картинки»), главная Google,
//! другие сайты — в панели не открывается: если человек сам по этому щёлкнул,
//! адрес уходит в обычную вкладку, а панель остаётся как была. Переадресацию без
//! клика (Google увёл бы AI Mode на другую страницу) панель просто не пускает.
//!
//! WebView2 спрашивает обработчик навигации только о переходах всей страницы: там
//! чужой переход отменяется сразу. WebKitGTK спрашивает и о фреймах и не говорит,
//! фрейм ли это, поэтому там обработчик пропускает их, как во вкладках, а переход
//! всей панели ловит [`guard`] по `load-changed` (он только о главном фрейме) и
//! останавливает, пока новая страница не открылась.

use tao::event_loop::EventLoopProxy;
use url::Url;
use wry::WebView;

use crate::{events::UserEvent, tabs::omnibox};

/// Куда идёт переход всей панели.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Nav {
    /// AI Mode или то, без чего он не работает: остаётся в панели.
    Stay,
    /// Страница не для панели: во вкладку, если человек сам по ней щёлкнул.
    Tab,
    /// Не страница сайта (`javascript:`, `data:`, `file:`, ссылка на приложение): никуда.
    Block,
}

/// Решение для перехода всей панели на `url`.
pub fn decide(url: &str) -> Nav {
    let Ok(u) = Url::parse(url) else {
        return Nav::Block;
    };
    match u.scheme() {
        "http" | "https" if stays(&u) => Nav::Stay,
        "http" | "https" => Nav::Tab,
        // Пустая страница (и `about:blank#blocked`) никуда не ведёт.
        "about" if u.path() == "blank" => Nav::Stay,
        // Содержимое, которое собрала сама страница Google: `blob:https://www.google.com/…`.
        "blob"
            if Url::parse(u.path()).is_ok_and(|inner| {
                inner.scheme() == "https" && inner.host_str().and_then(google_sub).is_some()
            }) =>
        {
            Nav::Stay
        }
        _ => Nav::Block,
    }
}

/// Страница, которая остаётся в панели.
fn stays(u: &Url) -> bool {
    let Some(host) = u.host_str() else {
        return false;
    };
    let path = u.path();
    // Вход в аккаунт Google по пути ставит cookies и YouTube.
    if host == "accounts.youtube.com" {
        return under(path, "/accounts");
    }
    match google_sub(host) {
        // AI Mode; «я не робот» (`/sorry/index`); вход (`/accounts/SetSID`).
        Some("" | "www") => ai_mode(u) || under(path, "/sorry") || under(path, "/accounts"),
        Some("ipv4" | "ipv6") => under(path, "/sorry"),
        // Вход, согласие на cookies и просьбы после входа («способы восстановления»).
        Some("accounts" | "consent" | "gds") => true,
        _ => false,
    }
}

/// Страница AI Mode: `/search` с `udm=50` (по первому `udm` в адресе).
fn ai_mode(u: &Url) -> bool {
    u.path().trim_end_matches('/') == "/search"
        && u.query_pairs()
            .find(|(k, _)| k == "udm")
            .is_some_and(|(_, v)| v == "50")
}

/// `dir` и всё под ним: `/sorry`, `/sorry/index`, но не `/sorryx`.
fn under(path: &str, dir: &str) -> bool {
    path.strip_prefix(dir)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

/// Сайт Google: `google.com`, `www.google.ru`, `accounts.google.co.uk`,
/// `google.com.br`, `google.cat`. Возвращает поддомен (у самого `google.<зона>` — пустой).
fn google_sub(host: &str) -> Option<&str> {
    let (sub, zone) = match host.strip_prefix("google.") {
        Some(zone) => ("", zone),
        None => {
            let at = host.find(".google.")?;
            (&host[..at], &host[at + ".google.".len()..])
        }
    };
    google_zone(zone).then_some(sub)
}

/// Зоны Google: `com`, `cat`, код страны (`ru`) или `co.`/`com.` с кодом страны.
fn google_zone(zone: &str) -> bool {
    let country = |s: &str| s.len() == 2 && s.bytes().all(|b| b.is_ascii_lowercase());
    match zone.split_once('.') {
        None => matches!(zone, "com" | "cat") || country(zone),
        Some((second, cc)) => matches!(second, "co" | "com") && country(cc),
    }
}

/// Обработчик навигации панели (`with_navigation_handler`): `true` — переход идёт.
pub fn on_navigation(url: &str, events: &EventLoopProxy<UserEvent>) -> bool {
    if cfg!(windows) {
        enter(url, events)
    } else {
        // Фреймы — как во вкладках; переход всей панели проверит `guard`.
        omnibox::is_navigable(url)
    }
}

/// Переход всей панели на `url`: `true` — он остаётся в панели. Адрес для вкладки
/// уходит приложению: оно спросит панель, был ли клик (см. [`CLICK_PROBE`]).
fn enter(url: &str, events: &EventLoopProxy<UserEvent>) -> bool {
    match decide(url) {
        Nav::Stay => true,
        Nav::Tab => {
            let _ = events.send_event(UserEvent::AiLeft(url.to_string()));
            false
        }
        Nav::Block => false,
    }
}

/// WebKitGTK: переход всей панели (и его переадресация — тогда адрес уже новый)
/// останавливается, пока новая страница не открылась, и прежняя остаётся на месте.
#[cfg(target_os = "linux")]
pub fn guard(webview: &WebView, events: EventLoopProxy<UserEvent>) {
    use webkit2gtk::{LoadEvent, WebViewExt};
    use wry::WebViewExtUnix;

    webview.webview().connect_load_changed(move |view, event| {
        if matches!(event, LoadEvent::Started | LoadEvent::Redirected)
            && let Some(url) = view.uri()
            && !enter(&url, &events)
        {
            view.stop_loading();
        }
    });
}

/// WebView2 отменяет чужие переходы ещё в обработчике навигации.
#[cfg(not(target_os = "linux"))]
pub fn guard(_webview: &WebView, _events: EventLoopProxy<UserEvent>) {}

/// Вопрос панели: сколько миллисекунд назад человек кликал или нажимал клавишу
/// (`links::TRACKER`, `-1` — неизвестно) и не во фрейме ли страницы фокус —
/// клики во фреймах скрипт главной страницы не видит. Ответ разбирает [`clicked`].
pub const CLICK_PROBE: &str = "[typeof window.__axiomInputAgo === 'function' ? window.__axiomInputAgo() : -1, document.activeElement instanceof HTMLIFrameElement]";

/// Сколько после клика переход ещё считается «по клику». Дольше, чем у ссылок на
/// приложения (`external::INPUT_FRESH_MS`): скрипт Google уходит со страницы не
/// всегда сразу, а лишняя вкладка — не программа, запущенная без спроса.
const CLICK_FRESH_MS: f64 = 3000.0;

/// Ответ на [`CLICK_PROBE`]: переход начал человек — открыть его во вкладке.
/// Невнятный ответ — нет.
pub fn clicked(raw: &str) -> bool {
    serde_json::from_str::<(f64, bool)>(raw)
        .is_ok_and(|(ago, in_frame)| in_frame || (0.0..=CLICK_FRESH_MS).contains(&ago))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ai_mode_stays() {
        assert_eq!(decide(omnibox::AI_HOME), Nav::Stay);
        assert_eq!(decide(&omnibox::ai_url("как дела?")), Nav::Stay);
        for url in [
            "https://www.google.com/search?udm=50&q=rust&mstk=AUtExfB&csuir=1",
            "https://www.google.com/search?q=rust&udm=50",
            "https://google.com/search?udm=50",
            "https://www.google.ru/search?udm=50&q=x",
            "https://www.google.co.uk/search/?udm=50",
            "https://www.google.com.br/search?udm=50",
            "https://www.google.cat/search?udm=50",
            "http://www.google.com/search?udm=50",
        ] {
            assert_eq!(decide(url), Nav::Stay, "{url}");
        }
    }

    #[test]
    fn ordinary_google_goes_to_tab() {
        for url in [
            // Обычная выдача: «Все», «Картинки», «Видео», «Новости».
            "https://www.google.com/search?q=rust",
            "https://www.google.com/search?q=rust&udm=2",
            "https://www.google.com/search?q=rust&udm=39",
            "https://www.google.com/search?q=rust&tbm=nws",
            "https://www.google.com/search?udm=2&udm=50&q=rust",
            "https://www.google.com/search?q=udm%3D50",
            // Главная, переадресация ссылок, карты, другие сервисы.
            "https://www.google.com/",
            "https://www.google.com/webhp",
            "https://www.google.com/url?q=https://example.com/",
            "https://www.google.com/maps/place/x",
            "https://www.google.com/imgres?imgurl=x",
            "https://www.google.com/searchx?udm=50",
            "https://images.google.com/search?udm=50",
            "https://maps.google.com/",
            "https://myaccount.google.com/",
            "https://support.google.com/websearch",
            "https://policies.google.com/privacy",
            "https://www.youtube.com/watch?v=1",
            "https://accounts.youtube.com/",
        ] {
            assert_eq!(decide(url), Nav::Tab, "{url}");
        }
    }

    #[test]
    fn other_sites_go_to_tab() {
        for url in [
            "https://example.com/",
            "https://en.wikipedia.org/wiki/Rust",
            "http://192.168.0.1/",
            // Похожие на Google имена — не Google.
            "https://google.com.evil.io/search?udm=50",
            "https://www.google.evil.com/search?udm=50",
            "https://evilgoogle.com/search?udm=50",
            "https://google.example/search?udm=50",
            "https://www.google.com.evil.io/sorry/index",
            "https://accounts.google.com.evil.io/",
            "https://www.google.co.example/search?udm=50",
            "https://www.google.org/search?udm=50",
        ] {
            assert_eq!(decide(url), Nav::Tab, "{url}");
        }
    }

    #[test]
    fn sign_in_consent_and_captcha_stay() {
        for url in [
            "https://accounts.google.com/ServiceLogin?continue=https://www.google.com/search?udm%3D50",
            "https://accounts.google.com/v3/signin/identifier?flowName=GlifWebSignIn",
            "https://accounts.google.com/CheckCookie?continue=x",
            "https://accounts.google.ru/accounts/SetSID?ssdc=1",
            "https://www.google.com/accounts/SetSID?ssdc=1",
            "https://accounts.youtube.com/accounts/SetSID?ssdc=1",
            "https://gds.google.com/web/recoveryoptions?continue=x",
            "https://consent.google.com/ml?continue=https://www.google.com/search?udm%3D50",
            "https://consent.google.de/ml",
            "https://www.google.com/sorry/index?continue=x",
            "https://ipv4.google.com/sorry/index?continue=x",
            "https://ipv6.google.com/sorry/index",
        ] {
            assert_eq!(decide(url), Nav::Stay, "{url}");
        }
        // Похожие пути — уже не они.
        assert_eq!(decide("https://www.google.com/sorrynot"), Nav::Tab);
        assert_eq!(decide("https://www.google.com/accountsx"), Nav::Tab);
        assert_eq!(decide("https://ipv4.google.com/search?udm=50"), Nav::Tab);
    }

    #[test]
    fn not_pages_are_blocked() {
        for url in [
            "javascript:alert(1)",
            "JavaScript:location='https://example.com'",
            "data:text/html,<b>x</b>",
            "file:///C:/Windows/win.ini",
            "mailto:a@example.com",
            "tg://resolve?domain=x",
            "about:srcdoc",
            "about:settings",
            "blob:https://example.com/550e8400-e29b-41d4-a716-446655440000",
            "blob:null/550e8400",
            "chrome-error://chromewebdata/",
            "",
            "не адрес",
        ] {
            assert_eq!(decide(url), Nav::Block, "{url}");
        }
        assert_eq!(decide("about:blank"), Nav::Stay);
        assert_eq!(decide("about:blank#blocked"), Nav::Stay);
        assert_eq!(
            decide("blob:https://www.google.com/550e8400-e29b-41d4-a716-446655440000"),
            Nav::Stay
        );
    }

    #[test]
    fn google_zones() {
        assert_eq!(google_sub("google.com"), Some(""));
        assert_eq!(google_sub("www.google.com"), Some("www"));
        assert_eq!(google_sub("accounts.google.co.jp"), Some("accounts"));
        assert_eq!(google_sub("www.google.com.au"), Some("www"));
        assert_eq!(google_sub("www.google.co"), Some("www"));
        for host in [
            "google.evil.com",
            "google.co.evil",
            "google.c0.uk",
            "google.co.uk.",
            "google.",
            "google",
            "notgoogle.com",
            "google.com.",
        ] {
            assert_eq!(google_sub(host), None, "{host}");
        }
    }

    #[test]
    fn click_answer() {
        assert!(clicked("[120.5,false]"));
        assert!(clicked("[0,false]"));
        assert!(clicked("[2999,false]"));
        // Клик во фрейме страницы: фокус на нём, а время клика неизвестно.
        assert!(clicked("[-1,true]"));
        assert!(!clicked("[3500,false]"));
        assert!(!clicked("[-1,false]"));
        assert!(!clicked("[1e15,false]"));
        // Страница не ответила или ответила не то.
        for raw in ["", "null", "true", "120", "[\"120\",false]", "[120]"] {
            assert!(!clicked(raw), "{raw}");
        }
        assert!(CLICK_PROBE.contains("window.__axiomInputAgo()"));
        assert!(crate::tabs::links::TRACKER.contains("'__axiomInputAgo'"));
    }
}
