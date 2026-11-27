//! Скрипты плагинов во вкладке и канал между ними и браузером.
//!
//! Каждый плагин живёт в своём изолированном мире (`Script::world`): свои
//! глобальные переменные, общий с сайтом DOM. Сайт не видит ни кода плагина, ни
//! функции-моста, поэтому подделать запрос от имени плагина не может.
//!
//! Windows (WebView2, через DevTools-протокол):
//! - `Page.addScriptToEvaluateOnNewDocument` с `worldName` ставит скрипт плагина в
//!   его мир каждого нового документа (до скриптов сайта). Для этого нужен
//!   `Page.enable` — его включаем только во вкладках, где есть включённые плагины;
//! - мост — `Runtime.addBinding` в контекст мира. Имя контекста (`executionContextName`)
//!   без `Runtime.enable` в новых документах не работает, а `Runtime.enable` сайты
//!   умеют замечать (на этом ловят ботов — пошли бы капчи). Поэтому после каждого
//!   перехода главной страницы (`Page.frameNavigated`) берём контекст мира
//!   (`Page.createIsolatedWorld` отдаёт уже созданный) и ставим мост в него;
//! - вызов моста приходит событием `Runtime.bindingCalled` с номером контекста:
//!   ответ уходит туда же (`Runtime.evaluate` с `contextId`).
//!
//! Linux (WebKitGTK): скрипт — `UserScript` своего мира, мост — обработчик
//! сообщений, зарегистрированный только в этом мире; ответы — `evaluate_javascript`
//! в этом мире главного фрейма.
//!
//! Вкладка без включённых плагинов ничего этого не получает и ничем не платит.

use std::rc::Rc;

use tao::event_loop::EventLoopProxy;
use wry::WebView;

use super::Script;
use crate::{events::UserEvent, tabs::TabId};

/// Запрос скрипта плагина из вкладки.
#[derive(Debug)]
pub struct Message {
    pub tab: TabId,
    pub plugin: String,
    /// Контекст мира, откуда пришёл запрос (Windows); на Linux — всегда главный фрейм.
    pub context: Option<i64>,
    pub raw: String,
}

/// Ставит во вкладку скрипты включённых плагинов (вместо прежних). Действует
/// с следующего документа: уже открытой странице нужна перезагрузка.
pub fn apply(
    webview: &WebView,
    tab: TabId,
    events: &EventLoopProxy<UserEvent>,
    scripts: Rc<[Script]>,
) {
    platform::apply(webview, tab, events, scripts);
}

/// Отдаёт сообщение скрипту плагина (`__axiomRecv` в `runtime.js`). `done(true)` —
/// скрипт есть и сообщение кому-то отдал (на событие подписались, запрос ждали).
pub fn deliver(
    webview: &WebView,
    script: &Script,
    context: Option<i64>,
    message: &str,
    done: impl FnOnce(bool) + 'static,
) {
    let js = format!("globalThis.__axiomRecv?__axiomRecv({message}):false");
    platform::deliver(webview, &script.world, context, &js, done);
}

/// Забывает вкладки, у которых больше нет webview (закрыты или уснули).
pub fn retain(live: impl Fn(TabId) -> bool) {
    platform::retain(live);
}

#[cfg(windows)]
mod platform {
    use std::{
        cell::{Cell, RefCell},
        collections::HashMap,
        rc::Rc,
    };

    use serde::Deserialize;
    use serde_json::{Value, json};
    use tao::event_loop::EventLoopProxy;
    use webview2_com::{
        CallDevToolsProtocolMethodCompletedHandler, DevToolsProtocolEventReceivedEventHandler,
        Microsoft::Web::WebView2::Win32::{
            ICoreWebView2, ICoreWebView2DevToolsProtocolEventReceiver,
        },
        take_pwstr,
    };
    use windows_core::{HSTRING, Interface, PWSTR};
    use wry::{WebView, WebViewExtWindows};

    use super::{super::api, Message, Script};
    use crate::{events::UserEvent, tabs::TabId};

    struct Tab {
        core: ICoreWebView2,
        scripts: Rc<[Script]>,
        /// Номера поставленных скриптов: ими их снимают. Приходят не сразу.
        ids: Rc<RefCell<Vec<String>>>,
        /// Номер установки: скрипт, поставленный прежней, снимается, как только придёт его номер.
        generation: Rc<Cell<u64>>,
        _receivers: Vec<ICoreWebView2DevToolsProtocolEventReceiver>,
    }

    thread_local! {
        static TABS: RefCell<HashMap<TabId, Tab>> = RefCell::new(HashMap::new());
    }

    fn key(core: &ICoreWebView2) -> usize {
        core.as_raw() as usize
    }

    pub fn apply(
        webview: &WebView,
        tab: TabId,
        events: &EventLoopProxy<UserEvent>,
        scripts: Rc<[Script]>,
    ) {
        let core = webview.webview();
        TABS.with(|t| {
            let mut tabs = t.borrow_mut();
            // Вкладка проснулась с новым webview — прежняя запись не о нём.
            if !tabs.get(&tab).is_some_and(|e| key(&e.core) == key(&core)) {
                tabs.remove(&tab);
                if scripts.is_empty() {
                    return;
                }
                let receivers = subscribe(&core, tab, events);
                cdp(&core, "Page.enable", json!({}), |_| {});
                tabs.insert(
                    tab,
                    Tab {
                        core: core.clone(),
                        scripts: Rc::from(Vec::new()),
                        ids: Rc::default(),
                        generation: Rc::default(),
                        _receivers: receivers,
                    },
                );
            }
            let Some(entry) = tabs.get_mut(&tab) else {
                return;
            };
            for id in entry.ids.borrow_mut().drain(..) {
                cdp(
                    &core,
                    "Page.removeScriptToEvaluateOnNewDocument",
                    json!({ "identifier": id }),
                    |_| {},
                );
            }
            let generation = entry.generation.get() + 1;
            entry.generation.set(generation);
            entry.scripts = scripts.clone();
            for script in scripts.iter() {
                let (ids, current, target) =
                    (entry.ids.clone(), entry.generation.clone(), core.clone());
                let params = json!({ "source": script.source, "worldName": script.world });
                cdp(
                    &core,
                    "Page.addScriptToEvaluateOnNewDocument",
                    params,
                    move |reply| {
                        let Some(id) = reply.as_deref().and_then(identifier) else {
                            eprintln!("плагин не встал во вкладку");
                            return;
                        };
                        if current.get() == generation {
                            ids.borrow_mut().push(id);
                        } else {
                            cdp(
                                &target,
                                "Page.removeScriptToEvaluateOnNewDocument",
                                json!({ "identifier": id }),
                                |_| {},
                            );
                        }
                    },
                );
            }
        });
    }

    pub fn deliver(
        webview: &WebView,
        _world: &str,
        context: Option<i64>,
        js: &str,
        done: impl FnOnce(bool) + 'static,
    ) {
        let Some(context) = context else {
            done(false);
            return;
        };
        cdp(
            &webview.webview(),
            "Runtime.evaluate",
            evaluate(js, context),
            move |reply| {
                done(reply.as_deref().is_some_and(is_true));
            },
        );
    }

    pub fn retain(live: impl Fn(TabId) -> bool) {
        TABS.with(|t| t.borrow_mut().retain(|id, _| live(*id)));
    }

    fn subscribe(
        core: &ICoreWebView2,
        tab: TabId,
        events: &EventLoopProxy<UserEvent>,
    ) -> Vec<ICoreWebView2DevToolsProtocolEventReceiver> {
        let events = events.clone();
        let called = receive(core, "Runtime.bindingCalled", move |core, json| {
            on_binding(core, tab, &json, &events);
        });
        let navigated = receive(core, "Page.frameNavigated", move |core, json| {
            on_navigated(core, tab, &json);
        });
        called.into_iter().chain(navigated).collect()
    }

    /// Главная страница вкладки сменилась: мост — в миры плагинов, которым она подходит.
    fn on_navigated(core: &ICoreWebView2, tab: TabId, json: &str) {
        #[derive(Deserialize)]
        struct Navigated {
            frame: Frame,
        }
        #[derive(Deserialize)]
        struct Frame {
            id: String,
            #[serde(rename = "parentId")]
            parent: Option<String>,
            url: String,
        }
        let Ok(Navigated { frame }) = serde_json::from_str::<Navigated>(json) else {
            return;
        };
        if frame.parent.is_some() {
            return;
        }
        let wanted: Vec<(String, String)> = TABS.with(|t| {
            t.borrow()
                .get(&tab)
                .filter(|e| key(&e.core) == key(core))
                .map(|e| {
                    e.scripts
                        .iter()
                        .filter(|s| s.wants(&frame.url))
                        .map(|s| (s.world.clone(), s.binding.clone()))
                        .collect()
                })
                .unwrap_or_default()
        });
        for (world, binding) in wanted {
            connect(core, &frame.id, world, binding);
        }
    }

    /// Контекст мира в документе фрейма → мост в него → «мост готов» скрипту.
    fn connect(core: &ICoreWebView2, frame: &str, world: String, binding: String) {
        let target = core.clone();
        let params = json!({ "frameId": frame, "worldName": world });
        cdp(core, "Page.createIsolatedWorld", params, move |reply| {
            let Some(context) = reply.as_deref().and_then(context_id) else {
                return;
            };
            let again = target.clone();
            let params = json!({ "name": binding, "executionContextId": context });
            cdp(&target, "Runtime.addBinding", params, move |reply| {
                if reply.is_some() {
                    cdp(
                        &again,
                        "Runtime.evaluate",
                        evaluate("globalThis.__axiomUp&&__axiomUp()", context),
                        |_| {},
                    );
                }
            });
        });
    }

    fn on_binding(
        core: &ICoreWebView2,
        tab: TabId,
        json: &str,
        events: &EventLoopProxy<UserEvent>,
    ) {
        #[derive(Deserialize)]
        struct Called {
            name: String,
            payload: String,
            #[serde(rename = "executionContextId")]
            context: i64,
        }
        let Ok(called) = serde_json::from_str::<Called>(json) else {
            return;
        };
        if called.payload.len() > api::MAX_MESSAGE {
            return;
        }
        let plugin = TABS.with(|t| {
            let tabs = t.borrow();
            let entry = tabs.get(&tab).filter(|e| key(&e.core) == key(core))?;
            entry
                .scripts
                .iter()
                .find(|s| s.binding == called.name)
                .map(|s| s.plugin.clone())
        });
        // Плагин уже выключили, а страница с ним ещё открыта: молчим.
        if let Some(plugin) = plugin {
            let _ = events.send_event(UserEvent::Plugin(Message {
                tab,
                plugin,
                context: Some(called.context),
                raw: called.payload,
            }));
        }
    }

    fn evaluate(js: &str, context: i64) -> Value {
        json!({
            "expression": js,
            "contextId": context,
            "returnByValue": true,
            "awaitPromise": false,
            "silent": true,
            // Ответ браузера — не клик человека: сайт не получит права «по клику».
            "userGesture": false,
        })
    }

    fn identifier(raw: &str) -> Option<String> {
        let v: Value = serde_json::from_str(raw).ok()?;
        v.get("identifier")?.as_str().map(str::to_owned)
    }

    fn context_id(raw: &str) -> Option<i64> {
        let v: Value = serde_json::from_str(raw).ok()?;
        v.get("executionContextId")?.as_i64()
    }

    fn is_true(raw: &str) -> bool {
        serde_json::from_str::<Value>(raw)
            .is_ok_and(|v| v.pointer("/result/value") == Some(&Value::Bool(true)))
    }

    /// Метод DevTools-протокола; `done` получает ответ (`None` — ошибка).
    fn cdp(
        core: &ICoreWebView2,
        method: &str,
        params: Value,
        done: impl FnOnce(Option<String>) + 'static,
    ) {
        let handler =
            CallDevToolsProtocolMethodCompletedHandler::create(Box::new(move |result, json| {
                done(result.is_ok().then_some(json));
                Ok(())
            }));
        // SAFETY: ядро живого webview, строки живут до конца вызова.
        let sent = unsafe {
            core.CallDevToolsProtocolMethod(
                &HSTRING::from(method),
                &HSTRING::from(params.to_string()),
                &handler,
            )
        };
        if let Err(err) = sent {
            eprintln!("плагины: {method}: {err}");
        }
    }

    /// Подписка на событие DevTools-протокола. Ядро webview — из `sender`: держать
    /// его в обработчике значило бы держать webview живым через него самого.
    fn receive(
        core: &ICoreWebView2,
        event: &str,
        on: impl Fn(&ICoreWebView2, String) + 'static,
    ) -> Option<ICoreWebView2DevToolsProtocolEventReceiver> {
        // SAFETY: ядро живого webview, строка живёт до конца вызова.
        let receiver =
            unsafe { core.GetDevToolsProtocolEventReceiver(&HSTRING::from(event)) }.ok()?;
        let handler =
            DevToolsProtocolEventReceivedEventHandler::create(Box::new(move |sender, args| {
                let (Some(sender), Some(args)) = (sender, args) else {
                    return Ok(());
                };
                let mut raw = PWSTR::null();
                // SAFETY: args — живой объект события, строку освобождает take_pwstr.
                unsafe { args.ParameterObjectAsJson(&mut raw)? };
                on(&sender, take_pwstr(raw));
                Ok(())
            }));
        let mut token = 0i64;
        // SAFETY: получатель событий живого webview.
        if let Err(err) =
            unsafe { receiver.add_DevToolsProtocolEventReceived(&handler, &mut token) }
        {
            eprintln!("плагины: подписка на {event}: {err}");
            return None;
        }
        Some(receiver)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn parses_replies() {
            assert_eq!(identifier(r#"{"identifier":"7"}"#).as_deref(), Some("7"));
            assert_eq!(context_id(r#"{"executionContextId":12}"#), Some(12));
            assert!(is_true(r#"{"result":{"type":"boolean","value":true}}"#));
            assert!(!is_true(r#"{"result":{"type":"boolean","value":false}}"#));
            assert!(!is_true(r#"{"result":{"type":"undefined"}}"#));
            assert!(!is_true(""));
        }
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use std::{
        cell::RefCell,
        collections::{HashMap, HashSet},
        rc::Rc,
    };

    use tao::event_loop::EventLoopProxy;
    use webkit2gtk::{
        UserContentInjectedFrames, UserContentManagerExt, UserScript, UserScriptInjectionTime,
        WebViewExt,
    };
    use wry::{WebView, WebViewExtUnix};

    use super::{super::api, Message, Script};
    use crate::{events::UserEvent, tabs::TabId};

    struct Tab {
        /// Страница webview: вкладка могла проснуться с новым.
        page: u64,
        current: Rc<[Script]>,
        scripts: Vec<UserScript>,
        /// Обработчики сообщений уже зарегистрированы (снять их нельзя — и не нужно).
        handlers: HashSet<String>,
    }

    thread_local! {
        static TABS: RefCell<HashMap<TabId, Tab>> = RefCell::new(HashMap::new());
    }

    pub fn apply(
        webview: &WebView,
        tab: TabId,
        events: &EventLoopProxy<UserEvent>,
        scripts: Rc<[Script]>,
    ) {
        let view = webview.webview();
        let Some(manager) = view.user_content_manager() else {
            return;
        };
        let page = view.page_id();
        TABS.with(|t| {
            let mut tabs = t.borrow_mut();
            if !tabs.get(&tab).is_some_and(|e| e.page == page) {
                tabs.remove(&tab);
                if scripts.is_empty() {
                    return;
                }
                tabs.insert(
                    tab,
                    Tab {
                        page,
                        current: Rc::from(Vec::new()),
                        scripts: Vec::new(),
                        handlers: HashSet::new(),
                    },
                );
            }
            let Some(entry) = tabs.get_mut(&tab) else {
                return;
            };
            for old in entry.scripts.drain(..) {
                manager.remove_script(&old);
            }
            entry.current = scripts.clone();
            for script in scripts.iter() {
                if entry.handlers.insert(script.binding.clone()) {
                    let events = events.clone();
                    let binding = script.binding.clone();
                    // Сначала подписка, потом регистрация — как советует WebKit.
                    manager.connect_script_message_received(
                        Some(&script.binding),
                        move |_, result| {
                            let Some(value) = result.js_value() else {
                                return;
                            };
                            let raw = value.to_string();
                            if raw.len() > api::MAX_MESSAGE {
                                return;
                            }
                            let plugin = TABS.with(|t| {
                                let tabs = t.borrow();
                                let entry = tabs.get(&tab).filter(|e| e.page == page)?;
                                entry
                                    .current
                                    .iter()
                                    .find(|s| s.binding == binding)
                                    .map(|s| s.plugin.clone())
                            });
                            if let Some(plugin) = plugin {
                                let _ = events.send_event(UserEvent::Plugin(Message {
                                    tab,
                                    plugin,
                                    context: None,
                                    raw,
                                }));
                            }
                        },
                    );
                    manager
                        .register_script_message_handler_in_world(&script.binding, &script.world);
                }
                let frames = if script.all_frames {
                    UserContentInjectedFrames::AllFrames
                } else {
                    UserContentInjectedFrames::TopFrame
                };
                let user = UserScript::for_world(
                    &script.source,
                    frames,
                    UserScriptInjectionTime::Start,
                    &script.world,
                    &[],
                    &[],
                );
                manager.add_script(&user);
                entry.scripts.push(user);
            }
        });
    }

    /// `evaluate_javascript` есть только с WebKitGTK 2.40 (wry требует 2.38):
    /// `run_javascript_in_world` делает то же и есть с 2.22.
    #[allow(deprecated)]
    pub fn deliver(
        webview: &WebView,
        world: &str,
        _context: Option<i64>,
        js: &str,
        done: impl FnOnce(bool) + 'static,
    ) {
        webview.webview().run_javascript_in_world(
            js,
            world,
            None::<&webkit2gtk::gio::Cancellable>,
            move |result| {
                let value = result.ok().and_then(|r| r.js_value());
                done(value.is_some_and(|v| v.to_string() == "true"));
            },
        );
    }

    pub fn retain(live: impl Fn(TabId) -> bool) {
        TABS.with(|t| t.borrow_mut().retain(|id, _| live(*id)));
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
mod platform {
    use std::rc::Rc;

    use tao::event_loop::EventLoopProxy;
    use wry::WebView;

    use super::Script;
    use crate::{events::UserEvent, tabs::TabId};

    pub fn apply(
        _webview: &WebView,
        _tab: TabId,
        _events: &EventLoopProxy<UserEvent>,
        _scripts: Rc<[Script]>,
    ) {
    }

    pub fn deliver(
        _webview: &WebView,
        _world: &str,
        _context: Option<i64>,
        _js: &str,
        done: impl FnOnce(bool) + 'static,
    ) {
        done(false);
    }

    pub fn retain(_live: impl Fn(TabId) -> bool) {}
}
