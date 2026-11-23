//! Создание webview для вкладок и панели ИИ.
//!
//! У этих webview нет IPC: сайты не могут отправлять команды браузеру.
//! Заголовок, адрес и новые окна приходят через нативные колбэки WebView2.

use anyhow::{Context, Result};
use tao::{event_loop::EventLoopProxy, window::Window};
use wry::{
    NewWindowResponse, PageLoadEvent, ProxyConfig, RGBA, Rect, Theme, WebContext, WebView,
    WebViewBuilder,
};

use super::{TabId, omnibox};
use crate::{events::UserEvent, ui::hotkeys};

/// Внешний вид webview: фон до загрузки страницы и тема для сайтов.
#[derive(Debug, Clone, Copy)]
pub struct Look {
    pub background: RGBA,
    /// Что сайты видят в `prefers-color-scheme`.
    pub theme: Theme,
    /// Принудительное затемнение сайтов (фиксируется при запуске процесса). Только WebView2.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub force_dark: bool,
}

/// Общие настройки для всех webview: фон, тема и аргументы запуска WebView2.
///
/// Аргументы обязаны совпадать у всех webview с одной папкой данных, поэтому
/// задаём их явно и одинаково для интерфейса, вкладок и панели ИИ.
pub fn styled<'a>(
    builder: WebViewBuilder<'a>,
    look: Look,
    proxy: Option<&ProxyConfig>,
) -> WebViewBuilder<'a> {
    let builder = builder.with_background_color(look.background);
    #[cfg(windows)]
    let builder = {
        use wry::WebViewBuilderExtWindows;
        builder
            .with_theme(look.theme)
            .with_additional_browser_args(browser_args(look.force_dark, proxy))
    };
    #[cfg(not(windows))]
    let builder = match proxy {
        Some(proxy) => builder.with_proxy_config(proxy.clone()),
        None => builder,
    };
    builder
}

/// Аргументы WebView2: умолчания wry, плюс тёмные сайты и прокси при необходимости.
///
/// Ради памяти выключены кэш «назад-вперёд» (после перехода он держит прежнюю
/// страницу целиком, вместе с её процессом) и запасной процесс отрисовки,
/// который Chromium заранее запускает впрок для следующего перехода.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn browser_args(force_dark: bool, proxy: Option<&ProxyConfig>) -> String {
    let mut args = String::from(
        "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection,\
         BackForwardCache,SpareRendererForSitePerProcess",
    );
    if force_dark {
        args.push_str(" --enable-features=WebContentsForceDark");
    }
    if let Some(proxy) = proxy {
        let (scheme, endpoint) = match proxy {
            ProxyConfig::Http(e) => ("http", e),
            ProxyConfig::Socks5(e) => ("socks5", e),
        };
        args.push_str(&format!(
            " --proxy-server={scheme}://{}:{}",
            endpoint.host, endpoint.port
        ));
    }
    args
}

/// Меняет тему сайтов на лету (WebView2 хранит её в профиле, поэтому
/// достаточно вызвать для любого webview профиля).
#[cfg(windows)]
pub fn set_theme(webview: &WebView, theme: Theme) {
    use wry::WebViewExtWindows;
    let _ = webview.set_theme(theme);
}

/// WebKitGTK берёт `prefers-color-scheme` из настройки GTK.
#[cfg(target_os = "linux")]
pub fn apply_theme_globally(theme: Theme) {
    use gtk::prelude::*;
    if let Some(settings) = gtk::Settings::default() {
        settings.set_gtk_application_prefer_dark_theme(matches!(theme, Theme::Dark));
    }
}

pub struct TabSpec<'a> {
    pub id: TabId,
    pub url: &'a str,
    pub bounds: Rect,
    pub visible: bool,
    pub look: Look,
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

    let builder = WebViewBuilder::new_with_web_context(context)
        .with_url(spec.url)
        .with_bounds(spec.bounds)
        .with_visible(spec.visible)
        .with_focused(spec.visible)
        .with_devtools(true)
        .with_hotkeys_zoom(true)
        .with_initialization_script(super::sleep::DRAFT_TRACKER)
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
    let builder = styled(builder, spec.look, spec.proxy.as_ref());

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
#[cfg(not(target_os = "linux"))]
pub fn build_child(builder: WebViewBuilder<'_>, window: &Window) -> wry::Result<WebView> {
    builder.build_as_child(window)
}

/// На Linux все webview живут в одном `gtk::Fixed`: так работают и X11, и Wayland.
/// Порядок добавления задаёт порядок слоёв: интерфейс снизу, вкладки и ИИ поверх.
#[cfg(target_os = "linux")]
pub fn build_child(builder: WebViewBuilder<'_>, window: &Window) -> wry::Result<WebView> {
    use wry::WebViewBuilderExtUnix;
    let fixed = gtk_container(window);
    builder.build_gtk(&fixed)
}

#[cfg(target_os = "linux")]
fn gtk_container(window: &Window) -> gtk::Fixed {
    use gtk::prelude::*;
    use std::cell::RefCell;
    use tao::platform::unix::WindowExtUnix;

    thread_local! {
        static FIXED: RefCell<Option<gtk::Fixed>> = const { RefCell::new(None) };
    }
    FIXED.with(|cell| {
        cell.borrow_mut()
            .get_or_insert_with(|| {
                let fixed = gtk::Fixed::new();
                if let Some(vbox) = window.default_vbox() {
                    vbox.pack_start(&fixed, true, true, 0);
                }
                fixed.show_all();
                fixed
            })
            .clone()
    })
}

/// Замораживает спрятанный webview: скрипты и таймеры страницы встают,
/// а WebView2 сам переводит её память в экономный режим. Страницу со звуком
/// не трогаем. Показ webview размораживает его, см. [`resume`].
#[cfg(windows)]
pub fn suspend(webview: &WebView) {
    use webview2_com::TrySuspendCompletedHandler;
    use windows_core::BOOL;
    let Some(core) = core3(webview) else { return };
    let mut suspended = BOOL::default();
    // SAFETY: core — живой COM-объект, указатель на локальную переменную.
    if unsafe { core.IsSuspended(&mut suspended) }.is_ok() && suspended.as_bool() {
        return;
    }
    if is_playing_audio(webview) {
        return;
    }
    let handler = TrySuspendCompletedHandler::create(Box::new(|_, _| Ok(())));
    // SAFETY: обработчик — COM-объект, который WebView2 держит до вызова.
    let _ = unsafe { core.TrySuspend(&handler) };
}

/// Размораживает webview перед показом или перед тем, как спросить страницу.
#[cfg(windows)]
pub fn resume(webview: &WebView) {
    if let Some(core) = core3(webview) {
        // SAFETY: core — живой COM-объект; для незамороженного webview вызов ничего не делает.
        let _ = unsafe { core.Resume() };
    }
}

#[cfg(windows)]
type Core3 = webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2_3;

#[cfg(windows)]
fn core3(webview: &WebView) -> Option<Core3> {
    use windows_core::Interface;
    use wry::WebViewExtWindows;
    webview.webview().cast().ok()
}

#[cfg(not(windows))]
pub fn suspend(_webview: &WebView) {}

#[cfg(not(windows))]
pub fn resume(_webview: &WebView) {}

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
