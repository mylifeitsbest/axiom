//! Служебные скрипты браузера в страницах вкладок и панели ИИ: проверки перед
//! усыплением и при пробуждении, перевод, ссылки в фоне, поиск по странице.
//!
//! `ExecuteScript` WebView2 (в wry — `evaluate_script`) выполняет скрипт как
//! действие пользователя, и ещё секунд пять после него страница считается
//! «кликнутой»: может открыть окно мимо блокировщика (`IsUserInitiated` в
//! `popups`), включить видео со звуком, развернуться на весь экран, писать в буфер
//! обмена. А спрашиваем мы страницу постоянно: просыпающуюся — каждые 250 мс,
//! про ссылку в фоне — на каждый запрос нового окна (и так сайт открывал бы окна
//! цепочкой после одного клика). Поэтому на Windows скрипт идёт через
//! DevTools-протокол: `Runtime.evaluate` с `userGesture: false`.
//!
//! WebKitGTK (`webkit_web_view_evaluate_javascript`, через него и `evaluate_script`
//! wry) тоже выполняет скрипт с жестом (`ForceUserGesture::Yes`), но с 2.42 снимает
//! его, как только скрипт закончился (`RemoveTransientActivation::Yes`); в 2.40 и
//! раньше он держится ещё несколько секунд. Выполнить скрипт без жеста открытый API
//! WebKitGTK не даёт, поэтому там — обычный `evaluate_script`.
//!
//! Интерфейс (axiom://) этим не пользуется: там наш код, жест ему ничего не даёт.

use wry::WebView;

/// Выполняет скрипт, ответ не нужен. `false` — до страницы не дошло.
#[cfg(windows)]
pub fn run(webview: &WebView, js: &str) -> bool {
    evaluate(webview, js, false, false, |_| ())
}

/// Выполняет скрипт как действие пользователя. Только в ответ на клик человека в
/// интерфейсе браузера по действию, которое страница без жеста не выполнит: окошко
/// «картинка в картинке» (`requestPictureInPicture`) открывается только по клику.
/// Страница после этого несколько секунд считается «кликнутой» (см. описание модуля).
#[cfg(windows)]
pub fn run_as_click(webview: &WebView, js: &str) -> bool {
    evaluate(webview, js, true, false, |_| ())
}

/// Выполняет скрипт и отдаёт его значение в JSON, как `evaluate_script_with_callback`:
/// `undefined` — `null`, исключение в скрипте — пустая строка. Обещания не ждём.
/// `false` — до страницы не дошло, ответа не будет.
#[cfg(windows)]
pub fn ask(webview: &WebView, js: &str, reply: impl Fn(String) + Send + 'static) -> bool {
    evaluate(webview, js, false, false, reply)
}

/// Как [`ask`], но если скрипт вернул обещание — ответ, когда оно исполнится (островок
/// ждёт, пока страница нарисует видео). Обещание должно исполниться само, со своим
/// пределом по времени.
#[cfg(windows)]
pub fn ask_after(webview: &WebView, js: &str, reply: impl Fn(String) + Send + 'static) -> bool {
    evaluate(webview, js, false, true, reply)
}

#[cfg(not(windows))]
pub fn run(webview: &WebView, js: &str) -> bool {
    webview.evaluate_script(js).is_ok()
}

/// WebKitGTK и так выполняет скрипт с жестом (см. описание модуля).
#[cfg(not(windows))]
pub fn run_as_click(webview: &WebView, js: &str) -> bool {
    webview.evaluate_script(js).is_ok()
}

#[cfg(not(windows))]
pub fn ask(webview: &WebView, js: &str, reply: impl Fn(String) + Send + 'static) -> bool {
    webview.evaluate_script_with_callback(js, reply).is_ok()
}

/// WebKitGTK обещаний здесь не ждёт: ответ сразу, как у [`ask`].
#[cfg(not(windows))]
pub fn ask_after(webview: &WebView, js: &str, reply: impl Fn(String) + Send + 'static) -> bool {
    ask(webview, js, reply)
}

#[cfg(windows)]
fn evaluate(
    webview: &WebView,
    js: &str,
    gesture: bool,
    wait: bool,
    reply: impl FnOnce(String) + 'static,
) -> bool {
    use webview2_com::CallDevToolsProtocolMethodCompletedHandler;
    use windows_core::HSTRING;
    use wry::WebViewExtWindows;

    let handler =
        CallDevToolsProtocolMethodCompletedHandler::create(Box::new(move |done, json| {
            // Ошибка протокола (страница уходит, контекста ещё нет) — как исключение.
            reply(if done.is_ok() {
                answer(&json)
            } else {
                String::new()
            });
            Ok(())
        }));
    // SAFETY: ядро webview живое, строки живут до конца вызова.
    unsafe {
        webview.webview().CallDevToolsProtocolMethod(
            &HSTRING::from("Runtime.evaluate"),
            &HSTRING::from(params(js, gesture, wait)),
            &handler,
        )
    }
    .is_ok()
}

/// Параметры `Runtime.evaluate`.
#[cfg_attr(not(windows), allow(dead_code))]
fn params(js: &str, gesture: bool, wait: bool) -> String {
    serde_json::json!({
        "expression": js,
        // Главное: скрипт браузера — не действие человека (кроме `run_as_click`).
        "userGesture": gesture,
        // Ответ — сразу значением: в странице не остаются ссылки на объекты.
        "returnByValue": true,
        // Как `ExecuteScript`: обещание отдаётся как есть, его не ждём (кроме `ask_after`).
        "awaitPromise": wait,
        // Ошибка в скрипте не останавливает отладчик и не пишется в консоль сайта.
        "silent": true,
    })
    .to_string()
}

/// Ответ `Runtime.evaluate` (`{"result": {"type": …, "value": …}, "exceptionDetails"?}`)
/// → JSON значения, как отдаёт `ExecuteScript`. Значения нет (`undefined`, `NaN`,
/// `Infinity`, `1n`) — `null`; исключение или непонятный ответ — пустая строка.
#[cfg_attr(not(windows), allow(dead_code))]
fn answer(raw: &str) -> String {
    use serde::Deserialize;

    #[derive(Deserialize)]
    struct Reply {
        result: Remote,
        #[serde(rename = "exceptionDetails")]
        exception: Option<serde::de::IgnoredAny>,
    }
    #[derive(Deserialize)]
    struct Remote {
        #[serde(default)]
        value: serde_json::Value,
    }

    match serde_json::from_str::<Reply>(raw) {
        Ok(Reply {
            result,
            exception: None,
        }) => result.value.to_string(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn params_without_gesture() {
        let js = r#"window.find("a\"b", false)"#;
        let p: serde_json::Value =
            serde_json::from_str(&params(js, false, false)).unwrap_or_default();
        assert_eq!(p["expression"], js);
        assert_eq!(p["userGesture"], false);
        assert_eq!(p["returnByValue"], true);
        assert_eq!(p["awaitPromise"], false);
        assert_eq!(p["silent"], true);
        let click: serde_json::Value =
            serde_json::from_str(&params(js, true, false)).unwrap_or_default();
        assert_eq!(click["userGesture"], true);
        let wait: serde_json::Value =
            serde_json::from_str(&params(js, false, true)).unwrap_or_default();
        assert_eq!(wait["awaitPromise"], true);
        assert_eq!(wait["userGesture"], false);
    }

    #[test]
    fn values_as_execute_script() {
        let ok = |raw: &str| answer(raw);
        assert_eq!(ok(r#"{"result":{"type":"boolean","value":true}}"#), "true");
        assert_eq!(
            ok(r#"{"result":{"type":"number","value":120.5,"description":"120.5"}}"#),
            "120.5"
        );
        assert_eq!(
            ok(r#"{"result":{"type":"number","value":0,"description":"0"}}"#),
            "0"
        );
        assert_eq!(
            ok(r#"{"result":{"type":"number","value":-1,"description":"-1"}}"#),
            "-1"
        );
        assert_eq!(
            ok(r#"{"result":{"type":"string","value":"a\"б\n"}}"#),
            r#""a\"б\n""#
        );
        // Объект — тем же JSON, его разбирают вызывающие (`PageState::parse`).
        let state = ok(r#"{"result":{"type":"object","value":{"y":120.5,"keep":true}}}"#);
        assert_eq!(
            crate::tabs::sleep::PageState::parse(&state),
            crate::tabs::sleep::PageState {
                y: 120.5,
                keep: true
            }
        );
        let batch = ok(
            r#"{"result":{"type":"object","value":{"b":[[[1,"Hello"],[2,"world"]]],"more":false}}}"#,
        );
        assert_eq!(batch, r#"{"b":[[[1,"Hello"],[2,"world"]]],"more":false}"#);
    }

    #[test]
    fn missing_values_are_null() {
        assert_eq!(answer(r#"{"result":{"type":"undefined"}}"#), "null");
        assert_eq!(
            answer(r#"{"result":{"type":"object","subtype":"null","value":null}}"#),
            "null"
        );
        assert_eq!(
            answer(
                r#"{"result":{"type":"number","unserializableValue":"NaN","description":"NaN"}}"#
            ),
            "null"
        );
        assert_eq!(
            answer(r#"{"result":{"type":"bigint","unserializableValue":"1n","description":"1n"}}"#),
            "null"
        );
    }

    #[test]
    fn errors_are_empty() {
        // Исключение в скрипте: результат — сама ошибка, но ответа нет.
        let thrown = r#"{"result":{"type":"object","subtype":"error","className":"TypeError",
            "description":"TypeError: x is not a function"},
            "exceptionDetails":{"exceptionId":1,"text":"Uncaught","lineNumber":0,"columnNumber":0}}"#;
        assert_eq!(answer(thrown), "");
        // Ответ протокола об ошибке и мусор.
        assert_eq!(
            answer(r#"{"code":-32000,"message":"Cannot find default execution context"}"#),
            ""
        );
        assert_eq!(answer(""), "");
        assert_eq!(answer("null"), "");
        assert_eq!(answer("{}"), "");
        // Вызывающие понимают пустой ответ как «не знаю».
        assert!(!crate::tabs::waking::parse(""));
        assert!(!crate::tabs::links::answer(""));
        assert_eq!(
            crate::tabs::sleep::PageState::parse(""),
            crate::tabs::sleep::PageState::default()
        );
    }
}
