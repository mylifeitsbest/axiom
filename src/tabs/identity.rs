//! Как вкладки представляются сайтам: подмена User-Agent и Referer.
//!
//! User-Agent меняется штатно: WebView2 — `ICoreWebView2Settings2::UserAgent`,
//! WebKitGTK — настройка `user-agent` webview. Это и заголовок, и
//! `navigator.userAgent`. Пресеты собираются от настоящей версии движка
//! (а где её нет — от сегодняшней даты), так что «Chrome» не стареет вместе с Axiom.
//!
//! Только WebView2: Referer переписываем в `WebResourceRequested` у всех запросов
//! вкладки, там же убираем подсказки `Sec-CH-UA*` — иначе они выдали бы WebView2
//! при любом подменённом User-Agent. `document.referrer` и `navigator.userAgentData`
//! приводит в лад скрипт, который выполняется до скриптов страницы. Перехват всех
//! запросов не бесплатен, поэтому фильтр стоит, только пока подмена включена.
//! WebKitGTK заголовки запросов наружу не отдаёт — там меняется только User-Agent.
//!
//! Интерфейс и панель ИИ подмена не трогает: только вкладки.

use std::{
    sync::{OnceLock, PoisonError, RwLock},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Deserializer, Serialize, de::DeserializeOwned};
use url::Url;

pub const MAX_AGENT: usize = 512;
pub const MAX_REFERER: usize = 2048;

/// Каким браузером вкладки представляются сайтам. В настройках — строкой.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Agent {
    /// Как есть: строка самого движка.
    #[default]
    Default,
    /// Google Chrome на Windows.
    Chrome,
    /// Firefox на Windows.
    Firefox,
    /// Safari на Mac.
    Safari,
    /// Chrome на Android: сайты покажут мобильную версию.
    Android,
    /// Safari на iPhone: сайты покажут мобильную версию.
    Iphone,
    /// Своя строка из настроек.
    Custom,
}

/// Что вкладки шлют в заголовке Referer. В настройках — строкой.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Referer {
    /// Как решит движок: другим сайтам — только адрес сайта, без страницы.
    #[default]
    Default,
    /// Не отправлять вовсе.
    Off,
    /// Каждый запрос — будто со страницы того же сайта, куда он идёт.
    Site,
    /// Свой адрес для всех запросов.
    Custom,
}

/// Неизвестное значение (или мусор в файле настроек) — умолчание,
/// а не отказ читать все настройки целиком.
pub fn lenient<'de, D: Deserializer<'de>, T: DeserializeOwned + Default>(
    d: D,
) -> Result<T, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    Ok(T::deserialize(v).unwrap_or_default())
}

/// Выбор из настроек.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Prefs {
    pub agent: Agent,
    /// Своя строка (для `Agent::Custom`), уже проверенная [`valid_agent`].
    pub agent_custom: String,
    pub referer: Referer,
    /// Свой адрес (для `Referer::Custom`), уже очищенный [`clean_referer`].
    pub referer_custom: String,
}

impl Prefs {
    /// Нужен ли перехват запросов и скрипт на странице (только WebView2).
    fn hooks(&self) -> bool {
        cfg!(windows) && (self.agent != Agent::Default || self.referer != Referer::Default)
    }
}

/// Текущий выбор. Обработчики запросов живут в webview и про настройки
/// не знают, поэтому выбор общий.
static PREFS: RwLock<Prefs> = RwLock::new(Prefs {
    agent: Agent::Default,
    agent_custom: String::new(),
    referer: Referer::Default,
    referer_custom: String::new(),
});

/// User-Agent движка без подмены: с него берём версию Chrome и к нему возвращаемся.
static ENGINE_AGENT: OnceLock<String> = OnceLock::new();

fn prefs() -> Prefs {
    PREFS.read().unwrap_or_else(PoisonError::into_inner).clone()
}

#[cfg_attr(not(windows), allow(dead_code))]
fn prefs_agent() -> Agent {
    PREFS.read().unwrap_or_else(PoisonError::into_inner).agent
}

/// Новый выбор. `true` — он изменился и живые вкладки надо обновить ([`apply`]).
pub fn set(next: Prefs) -> bool {
    let mut prefs = PREFS.write().unwrap_or_else(PoisonError::into_inner);
    if *prefs == next {
        return false;
    }
    *prefs = next;
    true
}

/// Запоминает User-Agent движка. Звать с webview без подмены (интерфейс).
pub fn remember_engine(webview: &wry::WebView) {
    if ENGINE_AGENT.get().is_none()
        && let Some(ua) = platform::user_agent(webview)
        && !ua.is_empty()
    {
        let _ = ENGINE_AGENT.set(ua);
    }
}

/// Строка User-Agent для новых вкладок. `None` — как есть.
pub fn agent() -> Option<String> {
    let prefs = prefs();
    agent_string(prefs.agent, &prefs.agent_custom, engine(), today())
}

/// Грузить ли страницу новой вкладки только после [`install`]: иначе первый
/// документ ушёл бы без подмены Referer и без скрипта.
pub fn defer_load() -> bool {
    prefs().hooks()
}

/// Строки всех пресетов — показать в настройках, что именно увидят сайты.
pub fn presets() -> Vec<(Agent, String)> {
    let (engine, day) = (engine(), today());
    [
        Agent::Default,
        Agent::Chrome,
        Agent::Firefox,
        Agent::Safari,
        Agent::Android,
        Agent::Iphone,
    ]
    .into_iter()
    .filter_map(|a| {
        let ua = match a {
            Agent::Default => engine?.to_string(),
            _ => agent_string(a, "", engine, day)?,
        };
        Some((a, ua))
    })
    .collect()
}

fn engine() -> Option<&'static str> {
    ENGINE_AGENT.get().map(String::as_str)
}

/// Своя строка годится: непустая, до 512 знаков, печатные символы ASCII
/// (в заголовке HTTP другим не место).
pub fn valid_agent(ua: &str) -> bool {
    !ua.is_empty() && ua.len() <= MAX_AGENT && ua.bytes().all(|b| (0x20..0x7f).contains(&b))
}

/// Свой Referer: адрес http(s) без фрагмента и без логина с паролем.
/// Без схемы — `https://`. `None` — не адрес.
pub fn clean_referer(input: &str) -> Option<String> {
    let input = input.trim();
    if input.is_empty() || input.len() > MAX_REFERER {
        return None;
    }
    let mut url = if input.contains("://") {
        Url::parse(input)
    } else {
        Url::parse(&format!("https://{input}"))
    }
    .ok()?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none_or(str::is_empty) {
        return None;
    }
    url.set_fragment(None);
    let _ = url.set_username("");
    let _ = url.set_password(None);
    let url: String = url.into();
    (url.len() <= MAX_REFERER).then_some(url)
}

/// Выход новой версии: номер, день (от 1970-01-01) и средний шаг между версиями
/// в десятых долях дня — для оценки сегодняшней версии по дате.
struct Release {
    version: u64,
    day: u64,
    step_tenths: u64,
}

/// Chrome 158 — уже есть к 25 ноября 2026 (WebView2), новые — раз в 4 недели.
const CHROME: Release = Release {
    version: 158,
    day: 20782,
    step_tenths: 280,
};
/// Firefox 145 — 11 ноября 2025, новые — раз в 4 недели.
const FIREFOX: Release = Release {
    version: 145,
    day: 20403,
    step_tenths: 280,
};
/// Safari 26 — 15 сентября 2025, раз в год.
const SAFARI: Release = Release {
    version: 26,
    day: 20346,
    step_tenths: 3653,
};

fn estimate(release: &Release, day: u64) -> u64 {
    release.version + day.saturating_sub(release.day) * 10 / release.step_tenths
}

fn today() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() / 86_400)
}

/// Основная версия Chrome из строки движка (у WebView2 она есть, у WebKitGTK нет).
fn chrome_major(engine: &str) -> Option<u64> {
    let rest = &engine[engine.find("Chrome/")? + "Chrome/".len()..];
    let digits = rest.split(|c: char| !c.is_ascii_digit()).next()?;
    digits.parse().ok().filter(|v| *v >= 100)
}

/// Строка User-Agent для выбора. `None` — как есть.
fn agent_string(agent: Agent, custom: &str, engine: Option<&str>, day: u64) -> Option<String> {
    let chrome = engine
        .and_then(chrome_major)
        .unwrap_or_else(|| estimate(&CHROME, day));
    let firefox = estimate(&FIREFOX, day);
    let safari = estimate(&SAFARI, day);
    // Как пишут сами браузеры: Chrome — только основную версию, система Mac и iOS
    // в строке заморожена (10_15_7 и 18_6).
    Some(match agent {
        Agent::Default => return None,
        Agent::Custom => return valid_agent(custom).then(|| custom.to_string()),
        Agent::Chrome => format!(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{chrome}.0.0.0 Safari/537.36"
        ),
        Agent::Firefox => format!(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:{firefox}.0) Gecko/20100101 Firefox/{firefox}.0"
        ),
        Agent::Safari => format!(
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/{safari}.0 Safari/605.1.15"
        ),
        Agent::Android => format!(
            "Mozilla/5.0 (Linux; Android 10; K) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{chrome}.0.0.0 Mobile Safari/537.36"
        ),
        Agent::Iphone => format!(
            "Mozilla/5.0 (iPhone; CPU iPhone OS 18_6 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/{safari}.0 Mobile/15E148 Safari/604.1"
        ),
    })
}

/// Что сделать с заголовком Referer запроса.
#[derive(Debug, PartialEq, Eq)]
#[cfg_attr(not(windows), allow(dead_code))]
enum Rewrite {
    Keep,
    Remove,
    Set(String),
}

#[cfg_attr(not(windows), allow(dead_code))]
fn referer_for(prefs: &Prefs, request: &str) -> Rewrite {
    match prefs.referer {
        Referer::Default => Rewrite::Keep,
        Referer::Off => Rewrite::Remove,
        Referer::Custom if prefs.referer_custom.is_empty() => Rewrite::Keep,
        Referer::Custom => Rewrite::Set(prefs.referer_custom.clone()),
        Referer::Site => match Url::parse(request) {
            Ok(url) if matches!(url.scheme(), "http" | "https") => {
                Rewrite::Set(format!("{}/", url.origin().ascii_serialization()))
            }
            _ => Rewrite::Keep,
        },
    }
}

/// `navigator.platform` и `navigator.vendor`, как у браузера из пресета.
#[cfg_attr(not(windows), allow(dead_code))]
fn platform_of(agent: Agent) -> Option<(&'static str, &'static str)> {
    match agent {
        Agent::Firefox => Some(("Win32", "")),
        Agent::Safari => Some(("MacIntel", "Apple Computer, Inc.")),
        Agent::Android => Some(("Linux armv81", "Google Inc.")),
        Agent::Iphone => Some(("iPhone", "Apple Computer, Inc.")),
        Agent::Default | Agent::Chrome | Agent::Custom => None,
    }
}

/// Скрипт до скриптов страницы: `document.referrer` и `navigator` в лад
/// с заголовками. `None` — подменять нечего.
#[cfg_attr(not(windows), allow(dead_code))]
fn page_script(prefs: &Prefs) -> Option<String> {
    let referrer = match prefs.referer {
        Referer::Default => None,
        Referer::Off => Some("''".to_string()),
        Referer::Site => Some("/^https?:$/.test(location.protocol)?location.origin+'/':''".into()),
        Referer::Custom if prefs.referer_custom.is_empty() => None,
        // Адрес после `Url` — только ASCII без кавычек и переводов строк, JSON-строка — готовый JS.
        Referer::Custom => serde_json::to_string(&prefs.referer_custom).ok(),
    };
    let agent = prefs.agent != Agent::Default;
    if referrer.is_none() && !agent {
        return None;
    }
    let mut js = String::from("(()=>{try{");
    let mut define = |proto: &str, name: &str, expr: &str| {
        js.push_str(&format!(
            "Object.defineProperty({proto}.prototype,'{name}',{{configurable:true,enumerable:true,get(){{return {expr}}}}});"
        ));
    };
    if let Some(expr) = &referrer {
        define("Document", "referrer", expr);
    }
    if let Some((platform, vendor)) = platform_of(prefs.agent) {
        define("Navigator", "platform", &format!("'{platform}'"));
        define("Navigator", "vendor", &format!("'{vendor}'"));
    }
    // Подсказки Client Hints выдали бы WebView2: без них сайты смотрят на User-Agent.
    if agent {
        js.push_str("delete Navigator.prototype.userAgentData;");
    }
    js.push_str("}catch(_){}})();");
    Some(js)
}

/// Подключает подмену к новой вкладке. Ставить сразу после сборки webview.
pub fn install(webview: &wry::WebView) {
    platform::install(webview);
    let prefs = prefs();
    if prefs.hooks() {
        platform::apply(webview, &prefs, None);
    }
}

/// Значки вкладок интерфейс берёт с самих сайтов (`/favicon.ico`): с подменой
/// User-Agent и эти запросы уходят с ним, а не с настоящим. Звать один раз
/// после сборки интерфейса, дальше — [`apply_ui`] при каждом новом выборе.
pub fn install_ui(webview: &wry::WebView) {
    platform::install_ui(webview);
}

pub fn apply_ui(webview: &wry::WebView) {
    let prefs = prefs();
    let ua = agent_string(prefs.agent, &prefs.agent_custom, engine(), today());
    platform::apply_ui(webview, &prefs, ua.as_deref());
}

/// Новый выбор — живой вкладке: следующие запросы и документы уже с ним.
pub fn apply(webview: &wry::WebView) {
    let prefs = prefs();
    let ua = agent_string(prefs.agent, &prefs.agent_custom, engine(), today());
    platform::apply(webview, &prefs, Some(ua.as_deref()));
}

#[cfg(windows)]
mod platform {
    use std::{cell::RefCell, collections::HashMap, rc::Rc};

    use webview2_com::{
        AddScriptToExecuteOnDocumentCreatedCompletedHandler,
        Microsoft::Web::WebView2::Win32::{
            COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_IMAGE,
            ICoreWebView2, ICoreWebView2Settings2, ICoreWebView2WebResourceRequestedEventArgs,
        },
        WebResourceRequestedEventHandler, take_pwstr,
    };
    use windows_core::{HSTRING, Interface, PCWSTR, PWSTR, w};
    use wry::{WebView, WebViewExtWindows};

    use super::{Agent, Prefs, Referer, Rewrite};

    /// Подсказки Client Hints о браузере: и те, что идут всегда, и те,
    /// что сайт может запросить через `Accept-CH`.
    const HINTS: [PCWSTR; 12] = [
        w!("Sec-CH-UA"),
        w!("Sec-CH-UA-Mobile"),
        w!("Sec-CH-UA-Platform"),
        w!("Sec-CH-UA-Platform-Version"),
        w!("Sec-CH-UA-Arch"),
        w!("Sec-CH-UA-Bitness"),
        w!("Sec-CH-UA-Model"),
        w!("Sec-CH-UA-Full-Version"),
        w!("Sec-CH-UA-Full-Version-List"),
        w!("Sec-CH-UA-WoW64"),
        w!("Sec-CH-UA-Form-Factors"),
        w!("Sec-CH-UA-Form-Factor"),
    ];

    thread_local! {
        /// Id нашего скрипта в каждом webview (ключ — указатель на ядро).
        static SCRIPTS: RefCell<HashMap<usize, HSTRING>> = RefCell::new(HashMap::new());
    }

    fn key(core: &ICoreWebView2) -> usize {
        core.as_raw() as usize
    }

    fn settings(webview: &WebView) -> Option<ICoreWebView2Settings2> {
        // SAFETY: настройки принадлежат живому webview.
        unsafe { webview.webview().Settings().ok()?.cast().ok() }
    }

    pub fn user_agent(webview: &WebView) -> Option<String> {
        let settings = settings(webview)?;
        let mut raw = PWSTR::null();
        // SAFETY: указатель на локальную переменную, строку освобождает take_pwstr.
        unsafe { settings.UserAgent(&mut raw).ok()? };
        Some(take_pwstr(raw))
    }

    pub fn install(webview: &WebView) {
        let core = webview.webview();
        // Указатель мог достаться от закрытой вкладки: её скрипта здесь нет.
        SCRIPTS.with(|s| s.borrow_mut().remove(&key(&core)));
        let handler = WebResourceRequestedEventHandler::create(Box::new(|_, args| match args {
            Some(args) => on_request(&args),
            None => Ok(()),
        }));
        let mut token = 0i64;
        // SAFETY: ядро webview живёт столько же, сколько сам webview.
        if let Err(err) = unsafe { core.add_WebResourceRequested(&handler, &mut token) } {
            eprintln!("подмена Referer недоступна: {err}");
        }
    }

    /// `ua`: `None` — User-Agent не трогать (новая вкладка получила его при сборке),
    /// `Some(None)` — вернуть строку движка.
    pub fn apply(webview: &WebView, prefs: &Prefs, ua: Option<Option<&str>>) {
        if let Some(ua) = ua
            && let Some(ua) = ua.or(super::engine())
        {
            set_user_agent(webview, ua);
        }
        let core = webview.webview();
        // SAFETY: ядро живого webview. Снять фильтр, которого нет, — безвредная ошибка;
        // снимаем всегда, чтобы второй `apply` не поставил его дважды.
        unsafe {
            let _ = core
                .RemoveWebResourceRequestedFilter(w!("*"), COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL);
            if prefs.hooks() {
                let _ = core
                    .AddWebResourceRequestedFilter(w!("*"), COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL);
            }
        }
        set_script(&core, super::page_script(prefs));
    }

    fn set_user_agent(webview: &WebView, ua: &str) {
        if let Some(settings) = settings(webview) {
            // SAFETY: настройки живого webview, строка живёт до конца вызова.
            let _ = unsafe { settings.SetUserAgent(&HSTRING::from(ua)) };
        }
    }

    pub fn install_ui(webview: &WebView) {
        let handler = WebResourceRequestedEventHandler::create(Box::new(|_, args| match args {
            Some(args) => on_ui_request(&args),
            None => Ok(()),
        }));
        let mut token = 0i64;
        // SAFETY: ядро webview живёт столько же, сколько сам webview.
        if let Err(err) = unsafe {
            webview
                .webview()
                .add_WebResourceRequested(&handler, &mut token)
        } {
            eprintln!("подмена для значков сайтов недоступна: {err}");
        }
    }

    /// Интерфейс: тот же User-Agent, а у картинок с сайтов — без подсказок Client Hints.
    /// Свои страницы интерфейса wry отдаёт сам, им всё равно.
    pub fn apply_ui(webview: &WebView, prefs: &Prefs, ua: Option<&str>) {
        if let Some(ua) = ua.or(super::engine()) {
            set_user_agent(webview, ua);
        }
        let core = webview.webview();
        // SAFETY: как в `apply`.
        unsafe {
            let _ = core
                .RemoveWebResourceRequestedFilter(w!("*"), COREWEBVIEW2_WEB_RESOURCE_CONTEXT_IMAGE);
            if prefs.agent != Agent::Default {
                let _ = core.AddWebResourceRequestedFilter(
                    w!("*"),
                    COREWEBVIEW2_WEB_RESOURCE_CONTEXT_IMAGE,
                );
            }
        }
    }

    fn on_ui_request(
        args: &ICoreWebView2WebResourceRequestedEventArgs,
    ) -> windows_core::Result<()> {
        if super::prefs_agent() == Agent::Default {
            return Ok(());
        }
        // SAFETY: args — живой COM-объект события, указатели на локальные переменные.
        let request = unsafe { args.Request()? };
        let mut raw = PWSTR::null();
        unsafe { request.Uri(&mut raw)? };
        if crate::ui::is_ui_origin(&take_pwstr(raw)) {
            return Ok(());
        }
        let headers = unsafe { request.Headers()? };
        for name in HINTS {
            // SAFETY: как выше; убрать заголовок, которого нет, — не ошибка.
            let _ = unsafe { headers.RemoveHeader(name) };
        }
        Ok(())
    }

    fn set_script(core: &ICoreWebView2, js: Option<String>) {
        let k = key(core);
        if let Some(old) = SCRIPTS.with(|s| s.borrow_mut().remove(&k)) {
            // SAFETY: ядро живого webview, id — от него же.
            let _ = unsafe { core.RemoveScriptToExecuteOnDocumentCreated(&old) };
        }
        let Some(js) = js else { return };
        let id: Rc<RefCell<Option<String>>> = Rc::default();
        let out = id.clone();
        let target = core.clone();
        // Ждём id, как wry со своими скриптами: без него скрипт потом не снять.
        let result = AddScriptToExecuteOnDocumentCreatedCompletedHandler::wait_for_async_operation(
            Box::new(move |handler| {
                let js = HSTRING::from(js);
                // SAFETY: ядро живого webview, строка живёт до конца вызова.
                unsafe { target.AddScriptToExecuteOnDocumentCreated(&js, &handler) }
                    .map_err(Into::into)
            }),
            Box::new(move |result, script| {
                result?;
                *out.borrow_mut() = Some(script);
                Ok(())
            }),
        );
        match (result, id.take()) {
            (Ok(()), Some(id)) => SCRIPTS.with(|s| {
                s.borrow_mut().insert(k, HSTRING::from(id));
            }),
            (Err(err), _) => eprintln!("скрипт подмены не встал: {err}"),
            _ => {}
        }
    }

    fn on_request(args: &ICoreWebView2WebResourceRequestedEventArgs) -> windows_core::Result<()> {
        let prefs = super::PREFS
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let agent = prefs.agent != Agent::Default;
        if !agent && prefs.referer == Referer::Default {
            return Ok(());
        }
        // SAFETY: args — живой COM-объект события, указатели на локальные переменные.
        let request = unsafe { args.Request()? };
        let headers = unsafe { request.Headers()? };
        if agent {
            for name in HINTS {
                // SAFETY: как выше; убрать заголовок, которого нет, — не ошибка.
                let _ = unsafe { headers.RemoveHeader(name) };
            }
        }
        let uri = if prefs.referer == Referer::Site {
            let mut raw = PWSTR::null();
            // SAFETY: как выше.
            unsafe { request.Uri(&mut raw)? };
            take_pwstr(raw)
        } else {
            String::new()
        };
        // SAFETY: как выше; значение — адрес после `Url`, без переводов строк.
        match super::referer_for(&prefs, &uri) {
            Rewrite::Keep => {}
            Rewrite::Remove => unsafe { headers.RemoveHeader(w!("Referer"))? },
            Rewrite::Set(value) => unsafe {
                headers.SetHeader(w!("Referer"), &HSTRING::from(value))?
            },
        }
        Ok(())
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use webkit2gtk::{SettingsExt, WebViewExt};
    use wry::{WebView, WebViewExtUnix};

    use super::Prefs;

    pub fn user_agent(webview: &WebView) -> Option<String> {
        Some(webview.webview().settings()?.user_agent()?.to_string())
    }

    pub fn install(_webview: &WebView) {}

    pub fn install_ui(_webview: &WebView) {}

    pub fn apply_ui(webview: &WebView, prefs: &Prefs, ua: Option<&str>) {
        apply(webview, prefs, Some(ua));
    }

    /// WebKitGTK: только User-Agent (`None` у настройки — строка движка).
    pub fn apply(webview: &WebView, _prefs: &Prefs, ua: Option<Option<&str>>) {
        if let Some(ua) = ua
            && let Some(settings) = webview.webview().settings()
        {
            settings.set_user_agent(ua);
        }
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
mod platform {
    use wry::WebView;

    use super::Prefs;

    pub fn user_agent(_webview: &WebView) -> Option<String> {
        None
    }

    pub fn install(_webview: &WebView) {}

    pub fn install_ui(_webview: &WebView) {}

    pub fn apply_ui(_webview: &WebView, _prefs: &Prefs, _ua: Option<&str>) {}

    pub fn apply(_webview: &WebView, _prefs: &Prefs, _ua: Option<Option<&str>>) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    const WEBVIEW2: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/142.0.0.0 Safari/537.36 Edg/142.0.0.0";
    const WEBKIT: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Safari/605.1.15";
    /// 25 ноября 2026.
    const DAY: u64 = 20782;

    fn ua(agent: Agent, engine: Option<&str>) -> Option<String> {
        agent_string(agent, "", engine, DAY)
    }

    #[test]
    fn presets_follow_engine_version() {
        assert_eq!(ua(Agent::Default, Some(WEBVIEW2)), None);
        let chrome = ua(Agent::Chrome, Some(WEBVIEW2)).unwrap_or_default();
        assert!(
            chrome.contains("Chrome/142.0.0.0 Safari/537.36"),
            "{chrome}"
        );
        assert!(!chrome.contains("Edg/"), "{chrome}");
        let android = ua(Agent::Android, Some(WEBVIEW2)).unwrap_or_default();
        assert!(android.contains("Android") && android.contains("Chrome/142.0.0.0 Mobile"));
        // У WebKitGTK версии Chrome нет — оценка по дате, и не старая.
        let chrome = ua(Agent::Chrome, Some(WEBKIT)).unwrap_or_default();
        let v = chrome_major(&chrome).unwrap_or(0);
        assert!((150..=160).contains(&v), "{chrome}");
        let firefox = ua(Agent::Firefox, None).unwrap_or_default();
        assert!(firefox.contains("Gecko/20100101 Firefox/"), "{firefox}");
        assert!(!firefox.contains("Chrome"), "{firefox}");
        assert!(
            ua(Agent::Safari, None)
                .unwrap_or_default()
                .contains("Version/27.0 Safari/605.1.15")
        );
        assert!(
            ua(Agent::Iphone, None)
                .unwrap_or_default()
                .contains("iPhone OS 18_6")
        );
        for a in [
            Agent::Chrome,
            Agent::Firefox,
            Agent::Safari,
            Agent::Android,
            Agent::Iphone,
        ] {
            assert!(
                ua(a, Some(WEBVIEW2)).is_some_and(|s| valid_agent(&s)),
                "{a:?}"
            );
        }
    }

    #[test]
    fn estimates_by_date() {
        assert_eq!(estimate(&CHROME, CHROME.day), 158);
        assert_eq!(estimate(&CHROME, 0), 158);
        assert_eq!(estimate(&CHROME, CHROME.day + 27), 158);
        assert_eq!(estimate(&CHROME, CHROME.day + 28), 159);
        assert_eq!(estimate(&FIREFOX, 20782), 158);
        assert_eq!(estimate(&SAFARI, 20346 + 364), 26);
        assert_eq!(estimate(&SAFARI, 20346 + 366), 27);
        assert_eq!(chrome_major(WEBVIEW2), Some(142));
        assert_eq!(chrome_major(WEBKIT), None);
        assert_eq!(chrome_major("Chrome/"), None);
    }

    #[test]
    fn custom_agent() {
        assert_eq!(
            agent_string(Agent::Custom, "Mozilla/5.0 Test", None, DAY).as_deref(),
            Some("Mozilla/5.0 Test")
        );
        assert!(!valid_agent(""));
        assert!(!valid_agent("Bad\r\nX-Evil: 1"));
        assert!(!valid_agent("Браузер"));
        assert!(!valid_agent(&"a".repeat(MAX_AGENT + 1)));
        assert!(valid_agent(&"a".repeat(MAX_AGENT)));
        assert_eq!(agent_string(Agent::Custom, "плохо", None, DAY), None);
    }

    #[test]
    fn cleans_custom_referer() {
        assert_eq!(
            clean_referer("https://www.google.com").as_deref(),
            Some("https://www.google.com/")
        );
        assert_eq!(
            clean_referer(" google.com ").as_deref(),
            Some("https://google.com/")
        );
        assert_eq!(
            clean_referer("http://user:pw@example.com/a?b=1#frag").as_deref(),
            Some("http://example.com/a?b=1")
        );
        assert_eq!(
            clean_referer("https://пример.рф/").as_deref(),
            Some("https://xn--e1afmkfd.xn--p1ai/")
        );
        for bad in [
            "",
            "   ",
            "javascript:alert(1)",
            "ftp://example.com/",
            "file:///C:/x",
            "https://",
        ] {
            assert_eq!(clean_referer(bad), None, "{bad}");
        }
        let long = format!("https://example.com/{}", "a".repeat(MAX_REFERER));
        assert_eq!(clean_referer(&long), None);
    }

    fn prefs(referer: Referer, custom: &str) -> Prefs {
        Prefs {
            referer,
            referer_custom: custom.into(),
            ..Prefs::default()
        }
    }

    #[test]
    fn rewrites_referer() {
        let page = "https://img.example.com:8443/a/b.png?x=1";
        assert_eq!(
            referer_for(&prefs(Referer::Default, ""), page),
            Rewrite::Keep
        );
        assert_eq!(referer_for(&prefs(Referer::Off, ""), page), Rewrite::Remove);
        assert_eq!(
            referer_for(&prefs(Referer::Site, ""), page),
            Rewrite::Set("https://img.example.com:8443/".into())
        );
        assert_eq!(
            referer_for(&prefs(Referer::Site, ""), "data:text/plain,1"),
            Rewrite::Keep
        );
        assert_eq!(
            referer_for(&prefs(Referer::Custom, "https://www.google.com/"), page),
            Rewrite::Set("https://www.google.com/".into())
        );
        assert_eq!(
            referer_for(&prefs(Referer::Custom, ""), page),
            Rewrite::Keep
        );
    }

    #[test]
    fn page_script_only_when_needed() {
        assert_eq!(page_script(&Prefs::default()), None);
        let off = page_script(&prefs(Referer::Off, "")).unwrap_or_default();
        assert!(
            off.contains("'referrer'") && off.contains("return ''"),
            "{off}"
        );
        assert!(!off.contains("userAgentData"), "{off}");
        let custom =
            page_script(&prefs(Referer::Custom, "https://www.google.com/")).unwrap_or_default();
        assert!(
            custom.contains("return \"https://www.google.com/\""),
            "{custom}"
        );
        let safari = page_script(&Prefs {
            agent: Agent::Safari,
            ..Prefs::default()
        })
        .unwrap_or_default();
        assert!(
            safari.contains("delete Navigator.prototype.userAgentData"),
            "{safari}"
        );
        assert!(
            safari.contains("'MacIntel'") && !safari.contains("'referrer'"),
            "{safari}"
        );
    }

    #[test]
    fn reads_leniently() -> serde_json::Result<()> {
        #[derive(Deserialize)]
        struct S {
            #[serde(default, deserialize_with = "lenient")]
            a: Agent,
            #[serde(default, deserialize_with = "lenient")]
            r: Option<Referer>,
        }
        let s: S = serde_json::from_str(r#"{"a":"iphone","r":"site"}"#)?;
        assert_eq!((s.a, s.r), (Agent::Iphone, Some(Referer::Site)));
        let s: S = serde_json::from_str(r#"{"a":"netscape","r":42}"#)?;
        assert_eq!((s.a, s.r), (Agent::Default, None));
        let s: S = serde_json::from_str("{}")?;
        assert_eq!((s.a, s.r), (Agent::Default, None));
        assert_eq!(serde_json::to_string(&Agent::Iphone)?, r#""iphone""#);
        Ok(())
    }
}
