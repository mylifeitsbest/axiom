//! Запросы сайтов на микрофон, камеру, место, уведомления и прочее.
//!
//! Движок спрашивает — мы откладываем ответ (WebView2: deferral, WebKitGTK:
//! держим запрос), а решает `app::permissions`: по памяти или окошком под
//! адресной строкой. Ответ возвращается сюда через [`resolve`].

use std::{
    cell::RefCell,
    collections::HashMap,
    sync::atomic::{AtomicU64, Ordering},
};

use tao::event_loop::EventLoopProxy;
use url::Url;
use wry::WebView;

use super::TabId;
use crate::{events::UserEvent, storage::permissions::Perm};

/// Запрос страницы, который ждёт решения.
#[derive(Debug, Clone)]
pub struct Request {
    pub id: u64,
    pub tab: TabId,
    /// Origin того, кто просит; `None` — у страницы его нет (`file:`, `data:`):
    /// такое решение не запоминается.
    pub site: Option<String>,
    pub kinds: Vec<Perm>,
    /// WebKitGTK: страница хочет знать названия микрофонов и камер. Отвечаем сами,
    /// без окошка: можно, если сайту уже разрешён микрофон или камера.
    pub device_info: bool,
}

/// «Вкладка» панели ИИ: её запросы показываются, пока панель открыта.
pub const AI: TabId = TabId::MAX;

static NEXT: AtomicU64 = AtomicU64::new(1);

fn next_id() -> u64 {
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// `https://example.com:8443` — origin веб-страницы; остальное запомнить нельзя.
pub fn origin_of(url: &str) -> Option<String> {
    let url = Url::parse(url).ok()?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return None;
    }
    Some(url.origin().ascii_serialization())
}

fn send(events: &EventLoopProxy<UserEvent>, request: Request) {
    let _ = events.send_event(UserEvent::Permission(request));
}

// ---------- WebView2 ----------

#[cfg(windows)]
mod native {
    use super::*;
    use crate::storage::permissions::Decision;
    use webview2_com::Microsoft::Web::WebView2::Win32::*;

    pub struct Pending {
        pub args: ICoreWebView2PermissionRequestedEventArgs,
        pub deferral: ICoreWebView2Deferral,
    }

    thread_local! {
        pub static PENDING: RefCell<HashMap<u64, Pending>> = RefCell::new(HashMap::new());
    }

    pub fn perm_of(kind: COREWEBVIEW2_PERMISSION_KIND) -> Option<Perm> {
        Some(match kind {
            COREWEBVIEW2_PERMISSION_KIND_MICROPHONE => Perm::Microphone,
            COREWEBVIEW2_PERMISSION_KIND_CAMERA => Perm::Camera,
            COREWEBVIEW2_PERMISSION_KIND_GEOLOCATION => Perm::Geolocation,
            COREWEBVIEW2_PERMISSION_KIND_NOTIFICATIONS => Perm::Notifications,
            COREWEBVIEW2_PERMISSION_KIND_CLIPBOARD_READ => Perm::Clipboard,
            COREWEBVIEW2_PERMISSION_KIND_MIDI_SYSTEM_EXCLUSIVE_MESSAGES => Perm::Midi,
            COREWEBVIEW2_PERMISSION_KIND_LOCAL_FONTS => Perm::Fonts,
            COREWEBVIEW2_PERMISSION_KIND_WINDOW_MANAGEMENT => Perm::Windows,
            COREWEBVIEW2_PERMISSION_KIND_MULTIPLE_AUTOMATIC_DOWNLOADS => Perm::Downloads,
            COREWEBVIEW2_PERMISSION_KIND_FILE_READ_WRITE => Perm::Files,
            _ => return None,
        })
    }

    /// Обратное к `perm_of`: каждый запоминаемый вид есть и в WebView2 (тест ниже).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn kind_of(perm: Perm) -> Option<COREWEBVIEW2_PERMISSION_KIND> {
        Some(match perm {
            Perm::Microphone => COREWEBVIEW2_PERMISSION_KIND_MICROPHONE,
            Perm::Camera => COREWEBVIEW2_PERMISSION_KIND_CAMERA,
            Perm::Geolocation => COREWEBVIEW2_PERMISSION_KIND_GEOLOCATION,
            Perm::Notifications => COREWEBVIEW2_PERMISSION_KIND_NOTIFICATIONS,
            Perm::Clipboard => COREWEBVIEW2_PERMISSION_KIND_CLIPBOARD_READ,
            Perm::Midi => COREWEBVIEW2_PERMISSION_KIND_MIDI_SYSTEM_EXCLUSIVE_MESSAGES,
            Perm::Fonts => COREWEBVIEW2_PERMISSION_KIND_LOCAL_FONTS,
            Perm::Windows => COREWEBVIEW2_PERMISSION_KIND_WINDOW_MANAGEMENT,
            Perm::Downloads => COREWEBVIEW2_PERMISSION_KIND_MULTIPLE_AUTOMATIC_DOWNLOADS,
            Perm::Files => COREWEBVIEW2_PERMISSION_KIND_FILE_READ_WRITE,
            Perm::Screen | Perm::Unknown => return None,
        })
    }

    pub fn decision_of(state: COREWEBVIEW2_PERMISSION_STATE) -> Option<Decision> {
        match state {
            COREWEBVIEW2_PERMISSION_STATE_ALLOW => Some(Decision::Allow),
            COREWEBVIEW2_PERMISSION_STATE_DENY => Some(Decision::Block),
            _ => None,
        }
    }
}

/// Своё окошко вместо окна WebView2. Вызывать после сборки webview вкладки.
#[cfg(windows)]
pub fn install(webview: &WebView, tab: TabId, events: EventLoopProxy<UserEvent>) {
    use native::*;
    use webview2_com::{
        Microsoft::Web::WebView2::Win32::*, PermissionRequestedEventHandler, take_pwstr,
    };
    use windows_core::{Interface, PWSTR};
    use wry::WebViewExtWindows;

    let handler = PermissionRequestedEventHandler::create(Box::new(move |_, args| {
        let Some(args) = args else { return Ok(()) };
        let mut kind = COREWEBVIEW2_PERMISSION_KIND::default();
        let mut uri = PWSTR::null();
        // SAFETY: args — живой COM-объект события, указатели на локальные переменные.
        unsafe {
            args.PermissionKind(&mut kind)?;
            args.Uri(&mut uri)?;
        }
        let uri = take_pwstr(uri);
        if kind == COREWEBVIEW2_PERMISSION_KIND_OTHER_SENSORS {
            // Датчики (наклон, освещённость) Chrome даёт без вопроса — и мы.
            // SAFETY: то же.
            unsafe { args.SetState(COREWEBVIEW2_PERMISSION_STATE_ALLOW)? };
            return Ok(());
        }
        // Автовоспроизведение и неизвестное решает сам WebView2.
        let Some(perm) = perm_of(kind) else {
            return Ok(());
        };
        // Помнит решения Axiom, а не профиль WebView2: иначе тот отвечал бы сам,
        // и в Настройках → Безопасность разрешение было бы не отменить.
        if let Ok(args3) = args.cast::<ICoreWebView2PermissionRequestedEventArgs3>() {
            // SAFETY: то же.
            let _ = unsafe { args3.SetSavesInProfile(false) };
        }
        // SAFETY: то же; ответ дадим позже, на главном потоке.
        let deferral = unsafe { args.GetDeferral()? };
        let id = next_id();
        PENDING.with(|p| {
            p.borrow_mut().insert(
                id,
                Pending {
                    args: args.clone(),
                    deferral,
                },
            )
        });
        send(
            &events,
            Request {
                id,
                tab,
                site: origin_of(&uri),
                kinds: vec![perm],
                device_info: false,
            },
        );
        Ok(())
    }));
    let mut token = 0i64;
    // SAFETY: ядро webview живёт столько же, сколько сам webview.
    if let Err(err) = unsafe {
        webview
            .webview()
            .add_PermissionRequested(&handler, &mut token)
    } {
        eprintln!("разрешения сайтов: {err}");
    }
}

/// Ответ на запрос. Вкладку могли закрыть — тогда ответ просто никому не нужен.
#[cfg(windows)]
pub fn resolve(id: u64, allow: bool) {
    use webview2_com::Microsoft::Web::WebView2::Win32::*;
    let Some(pending) = native::PENDING.with(|p| p.borrow_mut().remove(&id)) else {
        return;
    };
    let state = if allow {
        COREWEBVIEW2_PERMISSION_STATE_ALLOW
    } else {
        COREWEBVIEW2_PERMISSION_STATE_DENY
    };
    // SAFETY: COM-объекты события живы, пока мы держим ссылки на них.
    unsafe {
        let _ = pending.args.SetState(state);
        let _ = pending.deferral.Complete();
    }
}

/// Что WebView2 запомнил сам, пока Axiom не отвечал на запросы (его собственное
/// окно «Разрешить / Блокировать»): переносим к себе и стираем у него, иначе
/// такие сайты получали бы доступ без нас. `done` получает список решений.
#[cfg(windows)]
pub fn take_saved(webview: &WebView, done: impl FnOnce(Vec<Saved>) + 'static) {
    use native::*;
    use webview2_com::{
        GetNonDefaultPermissionSettingsCompletedHandler, Microsoft::Web::WebView2::Win32::*,
        SetPermissionStateCompletedHandler, take_pwstr,
    };
    use windows_core::{HSTRING, Interface, PWSTR};
    use wry::WebViewExtWindows;

    // SAFETY: ядро webview живо, пока жив webview.
    let profile = unsafe {
        webview
            .webview()
            .cast::<ICoreWebView2_13>()
            .and_then(|core| core.Profile())
            .and_then(|profile| profile.cast::<ICoreWebView2Profile4>())
    };
    let Ok(profile) = profile else {
        return;
    };
    let keep = profile.clone();
    let handler =
        GetNonDefaultPermissionSettingsCompletedHandler::create(Box::new(move |result, list| {
            result?;
            let Some(list) = list else { return Ok(()) };
            let mut count = 0u32;
            // SAFETY: список — живой COM-объект, указатель на локальную переменную.
            unsafe { list.Count(&mut count)? };
            let mut saved = Vec::new();
            for i in 0..count {
                // SAFETY: индекс в пределах Count.
                let Ok(item) = (unsafe { list.GetValueAtIndex(i) }) else {
                    continue;
                };
                let mut kind = COREWEBVIEW2_PERMISSION_KIND::default();
                let mut state = COREWEBVIEW2_PERMISSION_STATE::default();
                let mut origin = PWSTR::null();
                // SAFETY: то же.
                let read = unsafe {
                    item.PermissionKind(&mut kind)
                        .and_then(|()| item.PermissionState(&mut state))
                        .and_then(|()| item.PermissionOrigin(&mut origin))
                };
                if read.is_err() {
                    continue;
                }
                let origin = take_pwstr(origin);
                let (Some(perm), Some(decision)) = (perm_of(kind), decision_of(state)) else {
                    continue;
                };
                // Стираем у WebView2: дальше он будет спрашивать нас.
                let reset = SetPermissionStateCompletedHandler::create(Box::new(|_| Ok(())));
                // SAFETY: профиль жив (держим ссылку), строка живёт до конца вызова.
                let _ = unsafe {
                    keep.SetPermissionState(
                        kind,
                        &HSTRING::from(origin.as_str()),
                        COREWEBVIEW2_PERMISSION_STATE_DEFAULT,
                        &reset,
                    )
                };
                if let Some(site) = origin_of(&origin) {
                    saved.push(Saved {
                        site,
                        perm,
                        decision,
                    });
                }
            }
            done(saved);
            Ok(())
        }));
    // SAFETY: профиль — живой COM-объект.
    if let Err(err) = unsafe { profile.GetNonDefaultPermissionSettings(&handler) } {
        eprintln!("разрешения WebView2 не прочитаны: {err}");
    }
}

/// Решение, которое запомнил сам движок.
#[derive(Debug, Clone)]
#[cfg_attr(not(windows), allow(dead_code))]
pub struct Saved {
    pub site: String,
    pub perm: Perm,
    pub decision: crate::storage::permissions::Decision,
}

// ---------- WebKitGTK ----------

#[cfg(target_os = "linux")]
mod native {
    use super::*;

    thread_local! {
        pub static PENDING: RefCell<HashMap<u64, webkit2gtk::PermissionRequest>> =
            RefCell::new(HashMap::new());
    }
}

/// WebKitGTK: без нас движок молча отказывает во всём, а микрофона и камеры
/// у страниц нет вовсе — их включают настройки `enable-media-stream` и `enable-webrtc`.
#[cfg(target_os = "linux")]
pub fn install(webview: &WebView, tab: TabId, events: EventLoopProxy<UserEvent>) {
    use gtk::glib::prelude::*;
    use webkit2gtk::{
        DeviceInfoPermissionRequest, GeolocationPermissionRequest, MediaKeySystemPermissionRequest,
        NotificationPermissionRequest, PermissionRequestExt, PointerLockPermissionRequest,
        SettingsExt, UserMediaPermissionRequest, UserMediaPermissionRequestExt, WebViewExt,
    };
    use wry::WebViewExtUnix;

    let view = webview.webview();
    if let Some(settings) = WebViewExt::settings(&view) {
        settings.set_enable_media_stream(true);
        settings.set_enable_webrtc(true);
    }
    view.connect_permission_request(move |view, request| {
        let mut kinds = Vec::new();
        let mut device_info = false;
        if let Some(media) = request.downcast_ref::<UserMediaPermissionRequest>() {
            if media.is_for_audio_device() {
                kinds.push(Perm::Microphone);
            }
            if media.is_for_video_device() {
                kinds.push(Perm::Camera);
            }
            // Ни звука, ни видео — значит, экран (getDisplayMedia).
            if kinds.is_empty() {
                kinds.push(Perm::Screen);
            }
        } else if request.is::<GeolocationPermissionRequest>() {
            kinds.push(Perm::Geolocation);
        } else if request.is::<NotificationPermissionRequest>() {
            kinds.push(Perm::Notifications);
        } else if request.is::<DeviceInfoPermissionRequest>() {
            device_info = true;
        } else if request.is::<PointerLockPermissionRequest>()
            || request.is::<MediaKeySystemPermissionRequest>()
        {
            // Захват мыши (игры) и защищённое видео — как в Chrome, без вопроса.
            request.allow();
            return true;
        } else {
            // Остальное движок решает сам (обычно — отказ).
            return false;
        }
        let id = next_id();
        native::PENDING.with(|p| p.borrow_mut().insert(id, request.clone()));
        send(
            &events,
            Request {
                id,
                tab,
                site: view.uri().and_then(|u| origin_of(&u)),
                kinds,
                device_info,
            },
        );
        true
    });
}

#[cfg(target_os = "linux")]
pub fn resolve(id: u64, allow: bool) {
    use webkit2gtk::PermissionRequestExt;
    let Some(request) = native::PENDING.with(|p| p.borrow_mut().remove(&id)) else {
        return;
    };
    if allow {
        request.allow();
    } else {
        request.deny();
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn install(_webview: &WebView, _tab: TabId, _events: EventLoopProxy<UserEvent>) {}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn resolve(_id: u64, _allow: bool) {}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn origins() {
        assert_eq!(
            origin_of("https://Meet.Example.com/abc-defg?x#y").as_deref(),
            Some("https://meet.example.com")
        );
        assert_eq!(
            origin_of("http://127.0.0.1:8765/t.html").as_deref(),
            Some("http://127.0.0.1:8765")
        );
        assert_eq!(
            origin_of("https://a.example:443/").as_deref(),
            Some("https://a.example")
        );
        assert_eq!(origin_of("file:///C:/a.html"), None);
        assert_eq!(origin_of("data:text/html,hi"), None);
        assert_eq!(origin_of("about:blank"), None);
        // Что сводится к origin, то и запоминается.
        for url in [
            "https://meet.example.com/x",
            "http://[::1]:3000/",
            "http://localhost:5173/",
        ] {
            let site = origin_of(url).expect("origin");
            assert!(crate::storage::permissions::valid_site(&site), "{site}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn webview2_kinds_round_trip() {
        use crate::storage::permissions::Perm::*;
        for perm in [
            Microphone,
            Camera,
            Geolocation,
            Notifications,
            Clipboard,
            Midi,
            Fonts,
            Windows,
            Downloads,
            Files,
        ] {
            let kind = native::kind_of(perm).expect("вид WebView2");
            assert_eq!(native::perm_of(kind), Some(perm));
        }
    }
}
