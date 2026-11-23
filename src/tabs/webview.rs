//! Создание webview для вкладок и панели ИИ.
//!
//! У этих webview нет IPC: сайты не могут отправлять команды браузеру.
//! Заголовок, адрес и новые окна приходят через нативные колбэки WebView2.

use anyhow::{Context, Result};
use tao::{event_loop::EventLoopProxy, window::Window};
use wry::{
    NewWindowResponse, PageLoadEvent, ProxyConfig, RGBA, Rect, WebContext, WebView, WebViewBuilder,
};

use super::{TabId, omnibox};
use crate::{events::UserEvent, ui::hotkeys};

pub struct TabSpec<'a> {
    pub id: TabId,
    pub url: &'a str,
    pub bounds: Rect,
    pub visible: bool,
    pub background: RGBA,
    /// Для вкладок, открытых из панели ИИ: тот же прокси, что у панели.
    pub proxy: Option<ProxyConfig>,
    pub ai: bool,
}

pub fn build_tab(
    window: &Window,
    context: &mut WebContext,
    events: &EventLoopProxy<UserEvent>,
    spec: TabSpec<'_>,
) -> Result<WebView> {
    let id = spec.id;
    let ai = spec.ai;
    let (title_tx, load_tx, open_tx) = (events.clone(), events.clone(), events.clone());

    let mut builder = WebViewBuilder::new_with_web_context(context)
        .with_url(spec.url)
        .with_bounds(spec.bounds)
        .with_visible(spec.visible)
        .with_focused(spec.visible)
        .with_background_color(spec.background)
        .with_devtools(true)
        .with_hotkeys_zoom(true)
        .with_navigation_handler(|url| omnibox::is_navigable(&url))
        .with_document_title_changed_handler(move |title| {
            let _ = title_tx.send_event(UserEvent::TabTitle(id, title));
        })
        .with_on_page_load_handler(move |event, url| {
            let started = matches!(event, PageLoadEvent::Started);
            let _ = load_tx.send_event(UserEvent::TabLoad { id, started, url });
        })
        .with_new_window_req_handler(move |url, _features| {
            // Вместо всплывающего окна — новая вкладка рядом.
            if omnibox::is_navigable(&url) && !url.starts_with("about:") {
                let _ = open_tx.send_event(UserEvent::OpenTab { url, ai });
            }
            NewWindowResponse::Deny
        });
    if let Some(proxy) = spec.proxy {
        builder = builder.with_proxy_config(proxy);
    }

    let webview = build_child(builder, window).context("не удалось создать webview вкладки")?;
    hotkeys::install(&webview, events.clone());
    enable_autofill(&webview);
    Ok(webview)
}

/// Сохранение паролей и автозаполнение форм (хранятся зашифрованными в профиле WebView2).
#[cfg(windows)]
pub fn enable_autofill(webview: &WebView) {
    use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Settings4;
    use windows_core::Interface;
    use wry::WebViewExtWindows;
    // SAFETY: настройки принадлежат живому webview, вызовы не передают указателей.
    unsafe {
        if let Ok(settings) = webview.webview().Settings()
            && let Ok(settings) = settings.cast::<ICoreWebView2Settings4>()
        {
            let _ = settings.SetIsPasswordAutosaveEnabled(true);
            let _ = settings.SetIsGeneralAutofillEnabled(true);
        }
    }
}

#[cfg(not(windows))]
pub fn enable_autofill(_webview: &WebView) {}

/// Дочерний webview поверх окна. На Windows и macOS — нативный child.
pub fn build_child(builder: WebViewBuilder<'_>, window: &Window) -> wry::Result<WebView> {
    builder.build_as_child(window)
}

/// Фоновые вкладки просим WebView2 держать в «экономном» режиме памяти.
#[cfg(windows)]
pub fn set_background(webview: &WebView, background: bool) {
    use wry::{MemoryUsageLevel, WebViewExtWindows};
    let level = if background {
        MemoryUsageLevel::Low
    } else {
        MemoryUsageLevel::Normal
    };
    let _ = webview.set_memory_usage_level(level);
}

#[cfg(not(windows))]
pub fn set_background(_webview: &WebView, _background: bool) {}

/// Вкладку, которая играет звук, не усыпляем (музыка, звонки).
#[cfg(windows)]
pub fn is_playing_audio(webview: &WebView) -> bool {
    use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2_8;
    use windows_core::{BOOL, Interface};
    use wry::WebViewExtWindows;
    let Ok(core) = webview.webview().cast::<ICoreWebView2_8>() else {
        return false;
    };
    let mut playing = BOOL::default();
    // SAFETY: core — живой COM-объект, указатель на локальную переменную.
    unsafe { core.IsDocumentPlayingAudio(&mut playing) }.is_ok() && playing.as_bool()
}

#[cfg(not(windows))]
pub fn is_playing_audio(_webview: &WebView) -> bool {
    false
}
