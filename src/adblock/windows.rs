//! Блокировка рекламы в WebView2.
//!
//! Запросы: фильтр `WebResourceRequested` на нужные виды запросов (документы фреймов,
//! скрипты, картинки, стили, медиа, XHR/fetch, ping) — и из воркеров сайта. Каждый
//! запрос проверяет движок; рекламный получает ответ 403 без тела и в сеть не уходит.
//! Шрифты не проверяем: рекламы в них нет, а запросов много. Документ самой
//! вкладки не трогаем: его адрес помним из `NavigationStarting`.
//!
//! Косметика: `cosmetic.js` во всех фреймах просит правила сообщением
//! (`chrome.webview.postMessage`), ответ уходит тому же фрейму. Сообщения фреймов
//! приходят в их `ICoreWebView2Frame`, поэтому за созданием фреймов следим
//! (`FrameCreated`, у вложенных — `ICoreWebView2Frame7`).

use std::{cell::RefCell, collections::HashMap, sync::Arc};

use webview2_com::{
    ContentLoadingEventHandler, FrameChildFrameCreatedEventHandler, FrameCreatedEventHandler,
    FrameWebMessageReceivedEventHandler,
    Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT,
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_EVENT_SOURCE, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_FETCH,
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_IMAGE, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_MEDIA,
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_OTHER, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_PING,
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_SCRIPT, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_STYLESHEET,
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_XML_HTTP_REQUEST,
        COREWEBVIEW2_WEB_RESOURCE_REQUEST_SOURCE_KINDS_ALL, ICoreWebView2, ICoreWebView2_4,
        ICoreWebView2_22, ICoreWebView2Environment, ICoreWebView2Frame, ICoreWebView2Frame2,
        ICoreWebView2Frame7, ICoreWebView2WebMessageReceivedEventArgs,
        ICoreWebView2WebResourceRequestedEventArgs,
    },
    NavigationStartingEventHandler, WebMessageReceivedEventHandler,
    WebResourceRequestedEventHandler, take_pwstr,
};
use windows_core::{HSTRING, Interface, PWSTR, w};
use wry::{WebView, WebViewExtWindows};

use super::{Cosmetic, Kind};
use crate::tabs::TabId;

/// Виды запросов, которые проверяем.
const CONTEXTS: [COREWEBVIEW2_WEB_RESOURCE_CONTEXT; 10] = [
    COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT,
    COREWEBVIEW2_WEB_RESOURCE_CONTEXT_SCRIPT,
    COREWEBVIEW2_WEB_RESOURCE_CONTEXT_IMAGE,
    COREWEBVIEW2_WEB_RESOURCE_CONTEXT_STYLESHEET,
    COREWEBVIEW2_WEB_RESOURCE_CONTEXT_MEDIA,
    COREWEBVIEW2_WEB_RESOURCE_CONTEXT_XML_HTTP_REQUEST,
    COREWEBVIEW2_WEB_RESOURCE_CONTEXT_FETCH,
    COREWEBVIEW2_WEB_RESOURCE_CONTEXT_EVENT_SOURCE,
    COREWEBVIEW2_WEB_RESOURCE_CONTEXT_PING,
    COREWEBVIEW2_WEB_RESOURCE_CONTEXT_OTHER,
];

/// Сообщения страницы больше этого — не наши.
const MAX_MESSAGE: usize = 256 << 10;

/// Что знаем о странице вкладки.
#[derive(Default)]
struct Page {
    /// Адрес последнего перехода самой вкладки: этот документ не блокируем.
    navigating: String,
    /// Адрес страницы вкладки и решение «не блокировать на этом сайте» для него.
    source: String,
    off: bool,
}

thread_local! {
    static ENGINE: RefCell<Option<Arc<adblock::Engine>>> = const { RefCell::new(None) };
    static PAGES: RefCell<HashMap<TabId, Page>> = RefCell::new(HashMap::new());
    static COSMETIC: RefCell<Cosmetic> = RefCell::new(Cosmetic::default());
}

pub fn set_engine(engine: Arc<adblock::Engine>) {
    ENGINE.with(|e| *e.borrow_mut() = Some(engine));
    // Ответы прежнего движка больше не годятся.
    COSMETIC.with(|c| *c.borrow_mut() = Cosmetic::default());
}

/// Блокировку выключили: движок и ответы косметики больше не держим.
pub fn drop_engine() {
    ENGINE.with(|e| *e.borrow_mut() = None);
    COSMETIC.with(|c| *c.borrow_mut() = Cosmetic::default());
}

pub fn forget(tab: TabId) {
    PAGES.with(|p| p.borrow_mut().remove(&tab));
}

fn kind_of(context: COREWEBVIEW2_WEB_RESOURCE_CONTEXT) -> Option<Kind> {
    Some(match context {
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT => Kind::Frame,
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_SCRIPT => Kind::Script,
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_IMAGE => Kind::Image,
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_STYLESHEET => Kind::Stylesheet,
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_MEDIA => Kind::Media,
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_XML_HTTP_REQUEST
        | COREWEBVIEW2_WEB_RESOURCE_CONTEXT_FETCH
        | COREWEBVIEW2_WEB_RESOURCE_CONTEXT_EVENT_SOURCE => Kind::Xhr,
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_PING => Kind::Ping,
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_OTHER => Kind::Other,
        _ => return None,
    })
}

fn is_web(uri: &str) -> bool {
    uri.starts_with("https://") || uri.starts_with("http://")
}

pub fn install(webview: &WebView, tab: TabId) {
    let core = webview.webview();
    let env = webview.environment();
    PAGES.with(|p| p.borrow_mut().insert(tab, Page::default()));
    if let Err(err) = subscribe(&core, env, tab) {
        eprintln!("блокировка рекламы недоступна во вкладке: {err}");
    }
}

fn subscribe(
    core: &ICoreWebView2,
    env: ICoreWebView2Environment,
    tab: TabId,
) -> windows_core::Result<()> {
    let mut token = 0i64;
    let navigating = NavigationStartingEventHandler::create(Box::new(move |_, args| {
        if let Some(args) = args {
            let mut raw = PWSTR::null();
            // SAFETY: args — живой COM-объект события, указатель на локальную переменную.
            unsafe { args.Uri(&mut raw)? };
            // Запрос документа уходит без `#фрагмента`: сравнивать будем с ним.
            let mut uri = take_pwstr(raw);
            if let Some(at) = uri.find('#') {
                uri.truncate(at);
            }
            PAGES.with(|p| p.borrow_mut().entry(tab).or_default().navigating = uri);
        }
        Ok(())
    }));
    // Новый документ вкладки: счёт заблокированного на странице — с нуля, и решение
    // «не блокировать на этом сайте» — для его адреса. Скрипты документа ещё не
    // начались: косметика спросит уже про него.
    let loading = ContentLoadingEventHandler::create(Box::new(move |sender, _| {
        super::reset(tab);
        if let Some(core) = sender {
            let mut raw = PWSTR::null();
            // SAFETY: core — отправитель события, указатель на локальную переменную.
            unsafe { core.Source(&mut raw)? };
            let source = take_pwstr(raw);
            PAGES.with(|p| {
                let mut pages = p.borrow_mut();
                let page = pages.entry(tab).or_default();
                page.off = super::site_allowed(&source);
                page.source = source;
            });
        }
        Ok(())
    }));
    let requested =
        WebResourceRequestedEventHandler::create(Box::new(move |sender, args| {
            match (sender, args) {
                (Some(core), Some(args)) => on_request(&core, &env, tab, &args),
                _ => Ok(()),
            }
        }));
    let message = WebMessageReceivedEventHandler::create(Box::new(move |sender, args| {
        let (Some(core), Some(args)) = (sender, args) else {
            return Ok(());
        };
        if let Some(reply) = on_message(tab, &args)? {
            // SAFETY: core — отправитель события, живой COM-объект.
            unsafe { core.PostWebMessageAsJson(&HSTRING::from(reply))? };
        }
        Ok(())
    }));
    // SAFETY: ядро webview живёт столько же, сколько сам webview; строки — до конца вызовов.
    unsafe {
        core.add_NavigationStarting(&navigating, &mut token)?;
        core.add_ContentLoading(&loading, &mut token)?;
        core.add_WebResourceRequested(&requested, &mut token)?;
        core.add_WebMessageReceived(&message, &mut token)?;
        // С видами источников — и запросы воркеров сайта, без — только документов.
        let with_kinds = core.cast::<ICoreWebView2_22>().ok();
        for context in CONTEXTS {
            let added = match &with_kinds {
                Some(core) => core.AddWebResourceRequestedFilterWithRequestSourceKinds(
                    w!("*"),
                    context,
                    COREWEBVIEW2_WEB_RESOURCE_REQUEST_SOURCE_KINDS_ALL,
                ),
                None => core.AddWebResourceRequestedFilter(w!("*"), context),
            };
            if let Err(err) = added {
                eprintln!("блокировка рекламы: фильтр {} не встал: {err}", context.0);
            }
        }
        if let Ok(core4) = core.cast::<ICoreWebView2_4>() {
            let created = FrameCreatedEventHandler::create(Box::new(move |_, args| {
                if let Some(args) = args {
                    // Внутри общего unsafe-блока: args — живой COM-объект события.
                    watch_frame(&args.Frame()?, tab);
                }
                Ok(())
            }));
            core4.add_FrameCreated(&created, &mut token)?;
        }
    }
    Ok(())
}

/// Фрейм страницы: его сообщения и его собственные фреймы.
fn watch_frame(frame: &ICoreWebView2Frame, tab: TabId) {
    let mut token = 0i64;
    if let Ok(frame2) = frame.cast::<ICoreWebView2Frame2>() {
        let message = FrameWebMessageReceivedEventHandler::create(Box::new(move |sender, args| {
            let (Some(frame), Some(args)) = (sender, args) else {
                return Ok(());
            };
            if let Some(reply) = on_message(tab, &args)? {
                // SAFETY: frame — отправитель события, живой COM-объект.
                unsafe {
                    frame
                        .cast::<ICoreWebView2Frame2>()?
                        .PostWebMessageAsJson(&HSTRING::from(reply))?;
                }
            }
            Ok(())
        }));
        // SAFETY: фрейм живой: событие о нём только что пришло.
        if let Err(err) = unsafe { frame2.add_WebMessageReceived(&message, &mut token) } {
            eprintln!("блокировка рекламы: фрейм без косметики: {err}");
        }
    }
    if let Ok(frame7) = frame.cast::<ICoreWebView2Frame7>() {
        let created = FrameChildFrameCreatedEventHandler::create(Box::new(move |_, args| {
            if let Some(args) = args {
                // SAFETY: args — живой COM-объект события.
                watch_frame(&unsafe { args.Frame()? }, tab);
            }
            Ok(())
        }));
        // SAFETY: как выше.
        let _ = unsafe { frame7.add_FrameCreated(&created, &mut token) };
    }
}

fn on_request(
    core: &ICoreWebView2,
    env: &ICoreWebView2Environment,
    tab: TabId,
    args: &ICoreWebView2WebResourceRequestedEventArgs,
) -> windows_core::Result<()> {
    if !super::active() {
        return Ok(());
    }
    let mut context = COREWEBVIEW2_WEB_RESOURCE_CONTEXT::default();
    // SAFETY: args и core — живые COM-объекты события, указатели на локальные переменные.
    unsafe { args.ResourceContext(&mut context)? };
    let Some(kind) = kind_of(context) else {
        return Ok(());
    };
    let request = unsafe { args.Request()? };
    let mut raw = PWSTR::null();
    unsafe { request.Uri(&mut raw)? };
    let uri = take_pwstr(raw);
    if !is_web(&uri) {
        return Ok(());
    }
    let mut raw = PWSTR::null();
    unsafe { core.Source(&mut raw)? };
    let source = take_pwstr(raw);
    let skip = PAGES.with(|p| {
        let mut pages = p.borrow_mut();
        let page = pages.entry(tab).or_default();
        if kind == Kind::Frame && page.navigating == uri {
            return true;
        }
        if page.source != source {
            page.off = super::site_allowed(&source);
            page.source = source.clone();
        }
        page.off || !is_web(&source)
    });
    if skip {
        return Ok(());
    }
    let block = ENGINE.with(|e| {
        e.borrow()
            .as_deref()
            .is_some_and(|engine| super::should_block(engine, &uri, &source, kind))
    });
    if !block {
        return Ok(());
    }
    // SAFETY: окружение живёт столько же, сколько webview; поток интерфейса.
    unsafe {
        let response = env.CreateWebResourceResponse(None, 403, w!("Blocked"), w!(""))?;
        args.SetResponse(&response)?;
    }
    super::count(tab);
    // Пустая рамка и значок битой картинки на месте рекламы — тоже реклама:
    // странице — какие картинки и фреймы схлопнуть (cosmetic.js).
    if matches!(kind, Kind::Image | Kind::Frame) {
        let reply = serde_json::json!({ "axab": 1, "x": [uri] }).to_string();
        // SAFETY: core — отправитель события, живой COM-объект.
        let _ = unsafe { core.PostWebMessageAsJson(&HSTRING::from(reply)) };
    }
    Ok(())
}

/// Сообщение скрипта косметики из документа вкладки или её фрейма; ответ — JSON.
fn on_message(
    tab: TabId,
    args: &ICoreWebView2WebMessageReceivedEventArgs,
) -> windows_core::Result<Option<String>> {
    let mut raw = PWSTR::null();
    // SAFETY: args — живой COM-объект события, указатель на локальную переменную.
    unsafe { args.WebMessageAsJson(&mut raw)? };
    let json = take_pwstr(raw);
    if json.len() > MAX_MESSAGE || !json.contains("\"axab\"") {
        return Ok(None);
    }
    let mut raw = PWSTR::null();
    unsafe { args.Source(&mut raw)? };
    let url = take_pwstr(raw);
    if !is_web(&url) {
        return Ok(None);
    }
    // Сайт вкладки, а не фрейма: «не блокировать на этом сайте» — и для его фреймов.
    let off = !super::active()
        || PAGES.with(|p| {
            p.borrow()
                .get(&tab)
                .map_or_else(|| super::site_allowed(&url), |page| page.off)
        });
    let engine = ENGINE.with(|e| e.borrow().clone());
    let Some(engine) = engine else {
        return Ok(Some(r#"{"axab":1,"off":true}"#.into()));
    };
    Ok(COSMETIC.with(|c| c.borrow_mut().answer(&engine, &url, off, &json)))
}
