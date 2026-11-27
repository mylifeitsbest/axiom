//! Процессы движка WebView2 и чьи в них страницы.
//!
//! У каждого окружения WebView2 свой главный процесс, видеокарта, службы и процессы
//! страниц. Окружение говорит, фреймы каких страниц живут в каждом процессе
//! страниц; поднявшись от фрейма к главному фрейму страницы, находим её webview.
//! WebKitGTK такого не сообщает: на Linux процессы делятся только по имени.

use wry::WebView;

// На Linux процессы движка не приходят: типы есть, но не создаются.
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Главный процесс движка.
    Browser,
    /// Страницы и их фреймы.
    Renderer,
    /// Видеокарта: отрисовка и видео.
    Gpu,
    /// Сеть, звук, хранилище и прочие службы.
    Utility,
}

/// Процесс движка: номер, что он делает и главные фреймы страниц, чьи фреймы в нём.
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineProc {
    pub pid: u32,
    pub kind: Kind,
    pub roots: Vec<u32>,
}

/// Главный процесс окружения, в котором живёт webview (у панели ИИ с прокси — свой).
#[cfg(windows)]
pub fn browser_pid(webview: &WebView) -> Option<u32> {
    use wry::WebViewExtWindows;
    let mut pid = 0u32;
    // SAFETY: ядро webview живое, вызов в потоке интерфейса.
    unsafe { webview.webview().BrowserProcessId(&mut pid) }.ok()?;
    (pid != 0).then_some(pid)
}

/// Номер главного фрейма страницы webview (0 — ещё ничего не грузили).
#[cfg(windows)]
pub fn main_frame(webview: &WebView) -> Option<u32> {
    use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2_20;
    use windows_core::Interface;
    use wry::WebViewExtWindows;
    let mut id = 0u32;
    // SAFETY: то же; старый WebView2 без ICoreWebView2_20 вернёт ошибку приведения.
    unsafe {
        webview
            .webview()
            .cast::<ICoreWebView2_20>()
            .and_then(|core| core.FrameId(&mut id))
    }
    .ok()?;
    (id != 0).then_some(id)
}

/// Спрашивает окружение webview о его процессах; ответ придёт в `done` в потоке
/// интерфейса (`None` — движок не ответил). `false` — спросить не удалось
/// (старый WebView2 без этой возможности), `done` не вызовется.
#[cfg(windows)]
pub fn query(webview: &WebView, done: impl FnOnce(Option<Vec<EngineProc>>) + 'static) -> bool {
    use webview2_com::{
        GetProcessExtendedInfosCompletedHandler,
        Microsoft::Web::WebView2::Win32::{ICoreWebView2_2, ICoreWebView2Environment13},
    };
    use windows_core::Interface;
    use wry::WebViewExtWindows;

    // SAFETY: ядро webview живое, вызов в потоке интерфейса.
    let env = unsafe {
        webview
            .webview()
            .cast::<ICoreWebView2_2>()
            .and_then(|core| core.Environment())
            .and_then(|env| env.cast::<ICoreWebView2Environment13>())
    };
    let Ok(env) = env else {
        return false;
    };
    let handler = GetProcessExtendedInfosCompletedHandler::create(Box::new(move |result, list| {
        let procs = match (result, list) {
            // SAFETY: список — живой COM-объект ответа.
            (Ok(()), Some(list)) => Some(unsafe { read(&list) }),
            _ => None,
        };
        done(procs);
        Ok(())
    }));
    // SAFETY: обработчик COM держит сам, пока не ответит.
    unsafe { env.GetProcessExtendedInfos(&handler) }.is_ok()
}

/// Разбор ответа: процессы, их роль и главные фреймы страниц в процессах страниц.
#[cfg(windows)]
unsafe fn read(
    list: &webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2ProcessExtendedInfoCollection,
) -> Vec<EngineProc> {
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_PROCESS_KIND, COREWEBVIEW2_PROCESS_KIND_BROWSER,
        COREWEBVIEW2_PROCESS_KIND_GPU, COREWEBVIEW2_PROCESS_KIND_RENDERER,
    };

    let mut count = 0u32;
    // SAFETY: список живой, указатель на локальную переменную.
    if unsafe { list.Count(&mut count) }.is_err() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for i in 0..count.min(1024) {
        // SAFETY: индекс в пределах Count.
        let Ok(item) = (unsafe { list.GetValueAtIndex(i) }) else {
            continue;
        };
        let mut pid = 0i32;
        let mut kind = COREWEBVIEW2_PROCESS_KIND::default();
        // SAFETY: элемент живой, указатели на локальные переменные.
        let ok = unsafe {
            item.ProcessInfo()
                .and_then(|info| info.ProcessId(&mut pid).and_then(|()| info.Kind(&mut kind)))
        };
        let Ok(pid) = u32::try_from(pid) else {
            continue;
        };
        if ok.is_err() || pid == 0 {
            continue;
        }
        let kind = match kind {
            COREWEBVIEW2_PROCESS_KIND_BROWSER => Kind::Browser,
            COREWEBVIEW2_PROCESS_KIND_RENDERER => Kind::Renderer,
            COREWEBVIEW2_PROCESS_KIND_GPU => Kind::Gpu,
            _ => Kind::Utility,
        };
        let roots = if kind == Kind::Renderer {
            // SAFETY: элемент живой.
            unsafe { roots(&item) }
        } else {
            Vec::new()
        };
        out.push(EngineProc { pid, kind, roots });
    }
    out
}

/// Главные фреймы страниц, чьи фреймы работают в этом процессе.
#[cfg(windows)]
unsafe fn roots(
    item: &webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2ProcessExtendedInfo,
) -> Vec<u32> {
    use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2FrameInfo2;
    use windows_core::{BOOL, Interface};

    let mut out = Vec::new();
    // SAFETY: элемент живой; итератор и фреймы — живые COM-объекты ответа.
    unsafe {
        // Коллекцию держим, пока идём по ней: итератор WebView2 сам её не держит,
        // и без неё первый же GetCurrent обращается к освобождённой памяти.
        let Ok(frames) = item.AssociatedFrameInfos() else {
            return out;
        };
        let Ok(iter) = frames.GetIterator() else {
            return out;
        };
        let mut has = BOOL(0);
        let mut steps = 0;
        while iter.HasCurrent(&mut has).is_ok() && has.as_bool() && steps < 4096 {
            steps += 1;
            if let Ok(frame) = iter
                .GetCurrent()
                .and_then(|f| f.cast::<ICoreWebView2FrameInfo2>())
            {
                // Вверх до главного фрейма: у него родителя нет.
                let mut top = frame;
                for _ in 0..64 {
                    match top
                        .ParentFrameInfo()
                        .and_then(|p| p.cast::<ICoreWebView2FrameInfo2>())
                    {
                        Ok(parent) => top = parent,
                        Err(_) => break,
                    }
                }
                let mut id = 0u32;
                if top.FrameId(&mut id).is_ok() && id != 0 && !out.contains(&id) {
                    out.push(id);
                }
            }
            if iter.MoveNext(&mut has).is_err() {
                break;
            }
        }
    }
    out
}

#[cfg(not(windows))]
pub fn browser_pid(_webview: &WebView) -> Option<u32> {
    None
}

#[cfg(not(windows))]
pub fn main_frame(_webview: &WebView) -> Option<u32> {
    None
}

#[cfg(not(windows))]
pub fn query(_webview: &WebView, _done: impl FnOnce(Option<Vec<EngineProc>>) + 'static) -> bool {
    false
}
