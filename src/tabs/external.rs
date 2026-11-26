//! Ссылки на приложения: `mailto:`, `tg:`, `steam:`, `zoommtg:`, `magnet:` и любые
//! другие не-веб схемы, для которых в системе есть программа.
//!
//! Сайт открывает приложение только по клику и только после «Открыть» в окошке
//! Axiom (или сразу, если для этого сайта и схемы выбрали «Всегда разрешать»
//! и на странице только что был настоящий клик, см. [`INPUT_PROBE`]).
//! «Всегда разрешать» — только если движок сам назвал, кто просит (`InitiatingOrigin`,
//! фрейм нового окна): иначе просить мог чужой фрейм на странице, и на WebKitGTK
//! и старом WebView2 остаётся только окошко.
//! Без клика — молча ничего. Опасные схемы ([`NEVER`]) не открываются никогда,
//! окна для них нет. Ссылку, набранную в адресной строке, открываем сразу:
//! человек сам её написал и нажал Enter, второй вопрос ничего бы не добавил,
//! а опасные схемы закрыты и там.
//!
//! WebView2: о ссылке на зарегистрированную в системе схему сообщает событие
//! `LaunchingExternalUriScheme` (ICoreWebView2_18) — с любого фрейма, с адресом
//! сайта и флагом жеста. Своё окно WebView2 там всегда отменяем и решаем сами.
//! Если события нет (старый WebView2), переход отменяет обработчик навигации,
//! а о клике судим только по [`INPUT_PROBE`].
//!
//! WebKitGTK сам программы не запускает, а wry отдаёт в обработчик навигации
//! только адрес, без жеста. Там о клике судим только по [`INPUT_PROBE`],
//! а открываем через `xdg-open`.
//!
//! Запуск — `ShellExecuteW` / `xdg-open` с одним аргументом: самой ссылкой,
//! разобранной и собранной заново, где всё, кроме безопасных символов URL,
//! закодировано: `"`, пробел или `|` не выйдут за пределы аргумента.

use std::sync::atomic::{AtomicBool, Ordering};

use tao::event_loop::EventLoopProxy;
use url::Url;
use wry::WebView;

use super::TabId;
use crate::events::UserEvent;

/// Схемы, которые не открываем никогда: ни по клику, ни из адресной строки.
/// Окна нет — молча ничего.
const NEVER: &[&str] = &[
    // Follina (CVE-2022-30190): средство диагностики выполняло код из ссылки.
    "ms-msdt",
    // Поиск Windows по чужой сетевой папке: файлы злоумышленника выглядят своими.
    "search-ms",
    "search",
    // Запуск приложений Office с параметрами из ссылки.
    "ms-officecmd",
    // Системные окна входа и первой настройки Windows (Cloud Experience Host).
    "ms-cxh",
    "ms-cxh-full",
    // Установка приложения прямо по ссылке (CVE-2021-43890, Emotet).
    "ms-appinstaller",
    // Старая справка Windows и CHM: выполняли скрипты из ссылки (CVE-2010-1885).
    "hcp",
    "its",
    "ms-its",
    "mk",
    "mhtml",
    "res",
    // Системные папки и программы (`shell:startup`).
    "shell",
    // Локальные файлы и сетевые папки с веб-страниц: запуск чужих файлов
    // и утечка NTLM-хэша при обращении к чужому серверу.
    "file",
    "smb",
    "nfs",
    "afp",
    // Скрипт в адресе. Со страниц такие ссылки сюда не доходят (их выполняет
    // сама страница), а из адресной строки и других программ уходят в поиск.
    "javascript",
    "vbscript",
    // Веб-адреса и внутренние страницы браузеров — это не приложения.
    "http",
    "https",
    "ws",
    "wss",
    "about",
    "blob",
    "data",
    "view-source",
    "axiom",
    "chrome",
    "chrome-extension",
    "chrome-untrusted",
    "devtools",
    "edge",
];

/// Приложения Office: `ms-word:ofe|u|https://…` открывает документ по ссылке
/// (в том числе сразу для правки) — так подсовывают документы с макросами
/// и уводят NTLM-хэш. Команды (`ofe|u|`, `ofv|u|`, `nft|u|`) не пропускаем,
/// само приложение без команды открыть можно.
const OFFICE: &[&str] = &[
    "ms-word",
    "ms-excel",
    "ms-powerpoint",
    "ms-visio",
    "ms-access",
    "ms-project",
    "ms-publisher",
    "ms-spd",
    "ms-infopath",
];

/// Только из адресной строки: сайту незачем открывать параметры Windows
/// (например, чтобы подсунуть «отключите защиту» в настоящем окне системы),
/// а человек набирает `ms-settings:` сам.
const TYPED_ONLY: &[&str] = &["ms-settings"];

/// Как в Chromium: слишком длинные ссылки ломали обработчики.
const MAX_LAUNCH: usize = 2048;

/// Откуда ссылка.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Со страницы: клик, скрипт, фрейм.
    Site,
    /// Набрана в адресной строке.
    Typed,
}

/// Был ли клик (или другое действие человека) перед запросом.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gesture {
    Yes,
    No,
    /// Движок не сообщает (WebKitGTK, старый WebView2): решит [`INPUT_PROBE`].
    Unknown,
}

/// Страница просит открыть приложение.
#[derive(Debug, Clone)]
pub struct Request {
    pub tab: TabId,
    pub url: String,
    /// Origin страницы (`https://example.com`) или адрес фрейма, если движок их сообщил.
    /// Только по нему можно «Всегда разрешать»: адрес вкладки — не обязательно тот,
    /// кто просит (реклама или виджет во фрейме).
    pub origin: Option<String>,
    pub gesture: Gesture,
}

/// Сколько миллисекунд назад человек кликал или нажимал клавишу; `-1` — неизвестно.
/// Клики отмечает общий скрипт вкладки `links::TRACKER` (только `isTrusted`).
///
/// Флагу жеста от движка одному не верим: его нет на WebKitGTK и старом WebView2,
/// а у WebView2 он держится секунд пять после любого действия пользователя.
/// Скрипты самого Axiom жеста больше не дают (см. `script`), но «Всегда разрешать»
/// открывает программу без вопроса — только после настоящего клика в самой странице.
pub const INPUT_PROBE: &str =
    "typeof window.__axiomInputAgo === 'function' ? window.__axiomInputAgo() : -1";

/// Клик по ссылке и запрос приходят почти сразу: от нажатия до ответа страницы
/// на [`INPUT_PROBE`] — обычно десятки, на тяжёлой странице сотни миллисекунд.
/// Секунда — с запасом на это и ровно столько, сколько блокировщик всплывающих
/// окон Firefox считал окно «открытым по клику» (`dom.disable_open_click_delay`).
/// Дольше — и сайт с «Всегда разрешать» мог бы открыть программу сам, через
/// пару секунд после любого клика по странице. Не успел — просто покажем окошко.
pub const INPUT_FRESH_MS: f64 = 1000.0;

#[derive(Debug)]
pub enum Event {
    Request(Request),
    /// Страница ответила, был ли настоящий клик (ответ на [`INPUT_PROBE`]).
    Checked {
        request: Request,
        raw: String,
    },
    /// В системе нет программы для этой схемы.
    NoApp {
        tab: TabId,
        scheme: String,
    },
    /// Система не смогла запустить программу.
    Failed {
        scheme: String,
    },
}

/// Можно ли открыть ссылку приложением, и какой строкой. `None` — нельзя никогда.
pub fn classify(url: &str, source: Source) -> Option<String> {
    let parsed = Url::parse(url).ok()?;
    let scheme = parsed.scheme();
    // Одна буква — диск Windows (`C:\…`), а не схема.
    if scheme.len() < 2 || NEVER.contains(&scheme) {
        return None;
    }
    if source == Source::Site && TYPED_ONLY.contains(&scheme) {
        return None;
    }
    if OFFICE.contains(&scheme) && office_command(&parsed) {
        return None;
    }
    launch_string(parsed.as_str())
}

/// Сколько слоёв `%XX` снимаем, ища `|` в ссылке Office. Глубже — уже не ссылка,
/// а попытка обойти проверку: такую считаем командой.
const MAX_DECODE: usize = 8;

/// `ofe|u|…`: после схемы идёт команда с разделителем `|` — в том числе
/// закодированным (`%7C`) и закодированным повторно (`%257C`, `%25257C`…):
/// обработчик Office может снимать кодирование не один раз.
fn office_command(url: &Url) -> bool {
    let mut rest = url.as_str().as_bytes()[url.scheme().len() + 1..].to_vec();
    for _ in 0..MAX_DECODE {
        if rest.contains(&b'|') {
            return true;
        }
        let decoded = percent_decode(&rest);
        if decoded == rest {
            return false;
        }
        rest = decoded;
    }
    true
}

/// Один слой `%XX` → байт. Неполное `%` (`%7`, `%zz`) остаётся как есть.
fn percent_decode(text: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len());
    let mut i = 0;
    while let Some(&b) = text.get(i) {
        if let Some(byte) = escape_at(text, i) {
            out.push(byte);
            i += 3;
        } else {
            out.push(b);
            i += 1;
        }
    }
    out
}

/// Байт, закодированный как `%XX` с позиции `i`.
fn escape_at(text: &[u8], i: usize) -> Option<u8> {
    if text.get(i) != Some(&b'%') {
        return None;
    }
    let hex = |at: usize| char::from(*text.get(at)?).to_digit(16);
    u8::try_from((hex(i + 1)? << 4) | hex(i + 2)?).ok()
}

/// Ссылка для запуска: всё, кроме безопасных символов URL (RFC 3986), — `%XX`.
/// После `Url` в адресе только ASCII, но в непрозрачной части (`tg:x y"z`)
/// остаются пробелы и кавычки — они и разорвали бы командную строку программы.
fn launch_string(url: &str) -> Option<String> {
    const SAFE: &[u8] = b"-._~:/?#[]@!$&'()*+,;=%";
    let mut out = String::with_capacity(url.len());
    for b in url.bytes() {
        if b.is_ascii_alphanumeric() || SAFE.contains(&b) {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    (out.len() <= MAX_LAUNCH).then_some(out)
}

pub fn scheme_of(url: &str) -> &str {
    url.split(':').next().unwrap_or("")
}

/// Ссылка, набранная в адресной строке: `Some(Ok(ссылка))` — открыть сразу,
/// `Some(Err(схема))` — такие не открываем, `None` — это не ссылка на приложение
/// (адрес сайта, `localhost:3000` или поиск).
pub fn typed(input: &str) -> Option<Result<String, String>> {
    let s = input.trim();
    if s == super::omnibox::NEWTAB
        || s.chars().any(char::is_whitespace)
        || super::omnibox::is_page(s)
    {
        return None;
    }
    let parsed = Url::parse(s).ok()?;
    // `localhost:3000` и `example.com:8080` для `Url` — тоже схемы; их отличает то,
    // что программы для них в системе нет.
    handler(parsed.scheme())?;
    Some(classify(s, Source::Typed).ok_or_else(|| parsed.scheme().to_string()))
}

/// Событие `LaunchingExternalUriScheme` встало (WebView2 достаточно новый).
#[cfg_attr(not(windows), allow(dead_code))]
static HOOKED: AtomicBool = AtomicBool::new(false);

/// Обработчик навигации вкладки для не-веб адреса. `true` — пустить переход
/// дальше, к `LaunchingExternalUriScheme`: там решим по жесту и сайту.
pub fn on_navigation(tab: TabId, url: &str, events: &EventLoopProxy<UserEvent>) -> bool {
    let Some(link) = classify(url, Source::Site) else {
        eprintln!("ссылка заблокирована: {}:", scheme_of(url));
        return false;
    };
    let scheme = scheme_of(&link).to_string();
    if handler(&scheme).is_none() {
        send(events, Event::NoApp { tab, scheme });
        return false;
    }
    if HOOKED.load(Ordering::Relaxed) {
        return true;
    }
    send(
        events,
        Event::Request(Request {
            tab,
            url: link,
            origin: None,
            gesture: Gesture::Unknown,
        }),
    );
    false
}

/// `<a target=_blank href="tg://…">` и `window.open("tg://…")`: окно не открываем,
/// а спрашиваем, как о ссылке в самой вкладке. `gesture` — по клику ли, `frame` —
/// адрес фрейма, который просит окно (если движок его сообщил).
pub fn on_new_window(
    tab: TabId,
    url: String,
    gesture: bool,
    frame: Option<String>,
    events: &EventLoopProxy<UserEvent>,
) {
    if classify(&url, Source::Site).is_none() {
        return;
    }
    let gesture = if gesture { Gesture::Yes } else { Gesture::No };
    send(
        events,
        Event::Request(Request {
            tab,
            url,
            origin: frame,
            gesture,
        }),
    );
}

fn send(events: &EventLoopProxy<UserEvent>, event: Event) {
    let _ = events.send_event(UserEvent::AppLink(event));
}

/// Своё решение вместо окна WebView2. Вызывать после сборки webview.
#[cfg(windows)]
pub fn install(webview: &WebView, tab: TabId, events: EventLoopProxy<UserEvent>) {
    use webview2_com::{
        LaunchingExternalUriSchemeEventHandler, Microsoft::Web::WebView2::Win32::ICoreWebView2_18,
        take_pwstr,
    };
    use windows_core::{BOOL, Interface, PWSTR};
    use wry::WebViewExtWindows;

    let Ok(core) = webview.webview().cast::<ICoreWebView2_18>() else {
        return;
    };
    let handler = LaunchingExternalUriSchemeEventHandler::create(Box::new(move |_, args| {
        let Some(args) = args else { return Ok(()) };
        let (mut uri, mut origin, mut user) = (PWSTR::null(), PWSTR::null(), BOOL::default());
        // SAFETY: args — живой COM-объект события, указатели на локальные переменные.
        unsafe {
            // Окно и запуск WebView2 — никогда: решает Axiom.
            args.SetCancel(true)?;
            args.Uri(&mut uri)?;
            args.InitiatingOrigin(&mut origin)?;
            args.IsUserInitiated(&mut user)?;
        }
        let origin = take_pwstr(origin);
        let request = Request {
            tab,
            url: take_pwstr(uri),
            origin: (!origin.is_empty()).then_some(origin),
            gesture: if user.as_bool() {
                Gesture::Yes
            } else {
                Gesture::No
            },
        };
        send(&events, Event::Request(request));
        Ok(())
    }));
    let mut token = 0i64;
    // SAFETY: ядро webview живёт столько же, сколько сам webview.
    match unsafe { core.add_LaunchingExternalUriScheme(&handler, &mut token) } {
        Ok(()) => HOOKED.store(true, Ordering::Relaxed),
        Err(err) => eprintln!("ссылки на приложения: {err}"),
    }
}

#[cfg(not(windows))]
pub fn install(_webview: &WebView, _tab: TabId, _events: EventLoopProxy<UserEvent>) {}

/// Программа, назначенная в системе для схемы.
pub struct Handler {
    /// Название для окошка; пустое — не нашли (покажем схему).
    pub name: String,
}

/// Зарегистрированная схема — ключ `HKCR\<схема>` со значением `URL Protocol`
/// (так регистрируются и обычные программы, и приложения из Store).
/// Название — как в «Открыть с помощью» (`AssocQueryString`, учитывает выбор
/// человека), иначе из описания схемы в реестре.
#[cfg(windows)]
pub fn handler(scheme: &str) -> Option<Handler> {
    use windows_registry::CLASSES_ROOT;
    // Схема после `Url` — только [a-z0-9+.-]: обратной косой в путь ключа не попадёт.
    if scheme.is_empty()
        || !scheme
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"+.-".contains(&b))
    {
        return None;
    }
    const ASSOCSTR_EXECUTABLE: u32 = 2;
    const ASSOCSTR_FRIENDLYAPPNAME: u32 = 4;
    let key = CLASSES_ROOT.open(scheme).ok()?;
    key.get_type("URL Protocol").ok()?;
    // Программу по умолчанию не выбрали: Windows спросит сама («Выбор приложения»),
    // а название этого окна — не название программы.
    let picker = assoc_query(scheme, ASSOCSTR_EXECUTABLE).is_some_and(|exe| {
        exe.rsplit(['\\', '/'])
            .next()
            .is_some_and(|f| f.eq_ignore_ascii_case("OpenWith.exe"))
    });
    let name = if picker {
        String::new()
    } else {
        assoc_query(scheme, ASSOCSTR_FRIENDLYAPPNAME)
            .or_else(|| key.get_string("").ok().and_then(|d| friendly(&d, scheme)))
            .unwrap_or_default()
    };
    Some(Handler { name })
}

/// Что Windows сама ответит про схему (`kind` — `ASSOCSTR_*`): её ответ учитывает
/// и `UserChoice`, и `UserChoiceLatest`, и проверку их хешей.
#[cfg(windows)]
pub(crate) fn assoc_query(scheme: &str, kind: u32) -> Option<String> {
    #[link(name = "shlwapi")]
    unsafe extern "system" {
        fn AssocQueryStringW(
            flags: u32,
            kind: u32,
            assoc: *const u16,
            extra: *const u16,
            out: *mut u16,
            len: *mut u32,
        ) -> i32;
    }
    const ASSOCF_NOTRUNCATE: u32 = 0x20;
    const ASSOCF_INIT_IGNOREUNKNOWN: u32 = 0x400;
    const ASSOCF_IS_PROTOCOL: u32 = 0x1000;

    let wide: Vec<u16> = scheme.encode_utf16().chain(Some(0)).collect();
    let mut buf = [0u16; 512];
    let mut len = buf.len() as u32;
    // SAFETY: строка с нулём на конце, буфер и длина — локальные переменные.
    let hr = unsafe {
        AssocQueryStringW(
            ASSOCF_IS_PROTOCOL | ASSOCF_INIT_IGNOREUNKNOWN | ASSOCF_NOTRUNCATE,
            kind,
            wide.as_ptr(),
            std::ptr::null(),
            buf.as_mut_ptr(),
            &mut len,
        )
    };
    if hr != 0 {
        return None;
    }
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    let name = String::from_utf16_lossy(&buf[..end]).trim().to_string();
    (!name.is_empty()).then_some(name)
}

/// `URL:Telegram Link` → `Telegram`. Описание, которое повторяет схему, не нужно.
#[cfg_attr(not(windows), allow(dead_code))]
fn friendly(description: &str, scheme: &str) -> Option<String> {
    let mut name = description.trim();
    if let Some((head, rest)) = name.split_at_checked(4)
        && head.eq_ignore_ascii_case("url:")
    {
        name = rest.trim();
    }
    for suffix in [" protocol", " link", " uri", " url"] {
        let cut = name.len().checked_sub(suffix.len());
        if let Some(cut) = cut
            && name.is_char_boundary(cut)
            && name[cut..].eq_ignore_ascii_case(suffix)
        {
            name = name[..cut].trim();
        }
    }
    let name: String = name.chars().take(60).collect();
    // `URL:steam protocol` — та же схема строчными: окошко и так её покажет.
    (!name.is_empty() && name != scheme).then_some(name)
}

/// Открывает ссылку программой из системы (в фоновом потоке: `ShellExecute`
/// может ждать, пока программа ответит). Ссылка — уже из [`classify`].
#[cfg(windows)]
pub fn launch(link: String, events: EventLoopProxy<UserEvent>) {
    std::thread::spawn(move || {
        use windows::Win32::System::Com::{
            COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoUninitialize,
        };
        #[link(name = "shell32")]
        unsafe extern "system" {
            fn ShellExecuteW(
                hwnd: isize,
                operation: *const u16,
                file: *const u16,
                parameters: *const u16,
                directory: *const u16,
                show: i32,
            ) -> isize;
        }
        const SW_SHOWNORMAL: i32 = 1;
        let wide = |s: &str| -> Vec<u16> { s.encode_utf16().chain(Some(0)).collect() };
        let (verb, file) = (wide("open"), wide(&link));
        // SAFETY: строки с нулём на конце живут до конца вызова; COM — для этого потока.
        let code = unsafe {
            // Обработчики ссылок бывают расширениями оболочки: им нужен COM.
            let com = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
            let code = ShellExecuteW(
                0,
                verb.as_ptr(),
                file.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                SW_SHOWNORMAL,
            );
            if com.is_ok() {
                CoUninitialize();
            }
            code
        };
        // Меньше 33 — код ошибки (так устроен ShellExecute).
        if code <= 32 {
            let scheme = scheme_of(&link).to_string();
            send(&events, Event::Failed { scheme });
        }
    });
}

/// Freedesktop: программа для `x-scheme-handler/<схема>` по `xdg-mime`,
/// название — из её `.desktop`.
#[cfg(target_os = "linux")]
pub fn handler(scheme: &str) -> Option<Handler> {
    use std::process::Command;
    if scheme.is_empty()
        || !scheme
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"+.-".contains(&b))
    {
        return None;
    }
    let out = Command::new("xdg-mime")
        .args(["query", "default", &format!("x-scheme-handler/{scheme}")])
        .output()
        .ok()?;
    let id = String::from_utf8(out.stdout).ok()?.trim().to_string();
    if !id.ends_with(".desktop") || id.contains('/') {
        return None;
    }
    let name = desktop_dirs()
        .into_iter()
        .find_map(|dir| std::fs::read_to_string(dir.join("applications").join(&id)).ok())
        .and_then(|text| desktop_name(&text))
        .unwrap_or_default();
    Some(Handler { name })
}

#[cfg(target_os = "linux")]
fn desktop_dirs() -> Vec<std::path::PathBuf> {
    use std::path::PathBuf;
    let mut dirs = Vec::new();
    match std::env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()) {
        Some(home) => dirs.push(PathBuf::from(home)),
        None => {
            if let Some(home) = std::env::var_os("HOME") {
                dirs.push(PathBuf::from(home).join(".local/share"));
            }
        }
    }
    let system = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    dirs.extend(
        system
            .split(':')
            .filter(|d| !d.is_empty())
            .map(PathBuf::from),
    );
    dirs
}

/// `Name=` из раздела `[Desktop Entry]`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn desktop_name(text: &str) -> Option<String> {
    let mut in_entry = false;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
        } else if in_entry && let Some(name) = line.strip_prefix("Name=") {
            let name: String = name.trim().chars().take(60).collect();
            return (!name.is_empty()).then_some(name);
        }
    }
    None
}

#[cfg(target_os = "linux")]
pub fn launch(link: String, events: EventLoopProxy<UserEvent>) {
    std::thread::spawn(move || {
        // Ссылка начинается со схемы (буквы), так что за ключ xdg-open её не примет.
        let ok = std::process::Command::new("xdg-open")
            .arg(&link)
            .status()
            .is_ok_and(|s| s.success());
        if !ok {
            let scheme = scheme_of(&link).to_string();
            send(&events, Event::Failed { scheme });
        }
    });
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn handler(_scheme: &str) -> Option<Handler> {
    None
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn launch(link: String, events: EventLoopProxy<UserEvent>) {
    let scheme = scheme_of(&link).to_string();
    send(&events, Event::Failed { scheme });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site(url: &str) -> Option<String> {
        classify(url, Source::Site)
    }

    #[test]
    fn app_links_pass() {
        for url in [
            "mailto:a@b.c",
            "tg://resolve?domain=telegram",
            "tel:+79990001122",
            "steam://run/440",
            "discord://-/channels/1/2",
            "zoommtg://zoom.us/join?confno=123&pwd=abc",
            "magnet:?xt=urn:btih:abc&dn=Some+Name",
            "vscode://file/c:/src/main.rs:10",
            "obsidian://open?vault=My%20Vault",
            "spotify:track:6rqhFgbbKwnb9MLmUQDhG6",
            "sgnl://signal.me/#p/+15550001122",
            "calculator:",
            // Само приложение Office без команды открыть можно.
            "ms-word:",
        ] {
            assert_eq!(site(url).as_deref(), Some(url), "{url}");
        }
        // Схема — в нижнем регистре.
        assert_eq!(
            site("TG://resolve?domain=x").as_deref(),
            Some("tg://resolve?domain=x")
        );
    }

    #[test]
    fn dangerous_are_blocked() {
        for url in [
            "ms-msdt:/id PCWDiagnostic /skip force /param \"IT_RebrowseForFile=?\"",
            "search-ms:query=invoice&crumb=location:\\\\evil.example\\share",
            "search:query=x",
            "ms-officecmd:{\"id\":3}",
            "ms-word:ofe|u|https://evil.example/doc.docx",
            "ms-excel:ofv|u|https://evil.example/a.xlsx",
            "ms-powerpoint:nft%7Cu%7Chttps://evil.example/p.pptx",
            // Закодированный повторно `|`: обработчик может снять кодирование дважды.
            "ms-word:ofe%257Cu%257Chttps://evil.example/doc.docx",
            "ms-excel:ofv%25257cu%25257chttps://evil.example/a.xlsx",
            "ms-visio:ofe%%7C7Cu",
            "ms-cxh://setaddlocalonly",
            "ms-cxh-full://0",
            "ms-appinstaller:?source=https://evil.example/a.appinstaller",
            "hcp://services/search?query=x",
            "shell:startup",
            "file://evil.example/share/a.exe",
            "smb://evil.example/share",
            "javascript:alert(1)",
            "vbscript:msgbox(1)",
            "http://example.com/",
            "axiom://newtab",
            "C:\\Windows\\System32\\calc.exe",
            "не ссылка",
        ] {
            assert_eq!(site(url), None, "{url}");
            if !url.starts_with("ms-settings") {
                assert_eq!(classify(url, Source::Typed), None, "{url}");
            }
        }
        // Параметры Windows: сайту нельзя, человеку из адресной строки можно.
        assert_eq!(site("ms-settings:windowsdefender"), None);
        assert_eq!(
            classify("ms-settings:windowsdefender", Source::Typed).as_deref(),
            Some("ms-settings:windowsdefender")
        );
    }

    #[test]
    fn office_commands_are_decoded() {
        assert_eq!(percent_decode(b"a%7Cb%7cc"), b"a|b|c");
        assert_eq!(percent_decode(b"%257C"), b"%7C");
        // Неполные и не шестнадцатеричные `%` — как есть.
        assert_eq!(percent_decode(b"%7 %zz %"), b"%7 %zz %");
        assert_eq!(percent_decode(b"100%"), b"100%");
        // Сколько угодно слоёв: глубже MAX_DECODE — тоже команда.
        for layers in [1, 2, 5, MAX_DECODE - 1, MAX_DECODE, MAX_DECODE + 3] {
            let url = format!("ms-word:ofe%{}7Cu", "25".repeat(layers));
            assert_eq!(site(&url), None, "{url}");
        }
        // Без `|` даже после раскодирования — просто приложение.
        assert_eq!(site("ms-word:%41").as_deref(), Some("ms-word:%41"));
    }

    #[test]
    fn pages_are_not_app_links() {
        // Адреса страниц в адресной строке (и из других программ) — не ссылки
        // на приложения, даже если вкладка сама по ним не переходит.
        for input in [
            "data:text/html,x",
            "about:settings",
            "https://a.example/",
            "rust webview",
        ] {
            assert!(typed(input).is_none(), "{input}");
        }
    }

    #[test]
    fn launch_string_cannot_break_out() {
        // Кавычка, пробел и `|` в непрозрачной части не выходят за пределы аргумента.
        assert_eq!(
            site("steam://run/440\" --evil \"y").as_deref(),
            Some("steam://run/440%22%20--evil%20%22y")
        );
        assert_eq!(
            site("tg:a\"b c|d^e`f{g}<h>\\i").as_deref(),
            Some("tg:a%22b%20c%7Cd%5Ee%60f%7Bg%7D%3Ch%3E%5Ci")
        );
        // Перевод строки `Url` выбрасывает сам, не ASCII кодирует.
        assert_eq!(site("tg:a\nb").as_deref(), Some("tg:ab"));
        assert_eq!(
            site("mailto:a@b.c?subject=Привет").as_deref(),
            Some("mailto:a@b.c?subject=%D0%9F%D1%80%D0%B8%D0%B2%D0%B5%D1%82")
        );
        let long = format!("tg://x?{}", "a".repeat(MAX_LAUNCH));
        assert_eq!(site(&long), None);
    }

    #[test]
    fn friendly_names() {
        assert_eq!(
            friendly("URL:Telegram Link", "tg").as_deref(),
            Some("Telegram")
        );
        assert_eq!(
            friendly("URL:Discord Protocol", "discord").as_deref(),
            Some("Discord")
        );
        assert_eq!(friendly("URL:steam protocol", "steam"), None);
        assert_eq!(friendly("URL:mailto", "mailto"), None);
        assert_eq!(
            friendly("URL:Magnet URI", "magnet").as_deref(),
            Some("Magnet")
        );
        assert_eq!(friendly("", "x"), None);
        assert_eq!(
            friendly("Протокол ссылок", "x").as_deref(),
            Some("Протокол ссылок")
        );
    }

    #[test]
    fn desktop_entry_name() {
        let text = "[Desktop Action new]\nName=New window\n\n[Desktop Entry]\nType=Application\nName[ru]=Телеграм\nName=Telegram Desktop\nExec=telegram %u\n";
        assert_eq!(desktop_name(text).as_deref(), Some("Telegram Desktop"));
        assert_eq!(desktop_name("[Desktop Entry]\nExec=x\n"), None);
    }
}
