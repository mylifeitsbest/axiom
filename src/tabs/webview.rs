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
use crate::{downloads, events::UserEvent, ui::hotkeys};

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
/// Кэш «назад-вперёд» и запасной процесс отрисовки не выключаем: без них каждый
/// переход на другой сайт ждёт запуска нового процесса, а «Назад» грузит страницу
/// заново — заметно медленнее, чем в Chrome.
///
/// `msWebOOUI`, в отличие от wry, не выключаем: без него WebView2 отказывает сайтам
/// в полном экране (`requestFullscreen` → «not granted»), и видео не разворачивается.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn browser_args(force_dark: bool, proxy: Option<&ProxyConfig>) -> String {
    let mut args = String::from("--disable-features=msPdfOOUI,msSmartScreenProtection");
    if force_dark {
        args.push_str(" --enable-features=WebContentsForceDark");
    }
    // Отладка: DevTools-протокол для автотестов интерфейса (только debug-сборка).
    #[cfg(debug_assertions)]
    if let Ok(port) = std::env::var("AXIOM_CDP_PORT")
        && port.parse::<u16>().is_ok()
    {
        args.push_str(&format!(" --remote-debugging-port={port}"));
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

/// Режим автотестов (`AXIOM_CDP_PORT` в debug-сборке): открыт DevTools-порт, а окно
/// никогда не забирает фокус, чтобы тестовая копия не перехватывала ввод пользователя.
pub fn test_mode() -> bool {
    cfg!(debug_assertions) && std::env::var_os("AXIOM_CDP_PORT").is_some()
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
    /// Звук вкладки выключен (после пробуждения — снова): кнопкой или громкостью 0 %.
    pub muted: bool,
}

pub fn build_tab(
    window: &Window,
    context: &mut WebContext,
    events: &EventLoopProxy<UserEvent>,
    spec: TabSpec<'_>,
) -> Result<WebView> {
    let id = spec.id;
    let ai = spec.ai;
    // Исходный код (Ctrl+U) открывается только во вкладке, созданной для него браузером:
    // сайты сами на `view-source:` перейти не могут.
    let source = super::tools::is_source(spec.url);
    let (title_tx, load_tx, open_tx) = (events.clone(), events.clone(), events.clone());
    let nav_tx = events.clone();

    let builder = WebViewBuilder::new_with_web_context(context)
        .with_url(spec.url)
        .with_bounds(spec.bounds)
        .with_visible(spec.visible)
        .with_focused(spec.visible)
        .with_devtools(true)
        .with_hotkeys_zoom(true)
        .with_initialization_script(super::sleep::DRAFT_TRACKER)
        .with_initialization_script(super::links::TRACKER)
        // Громкость вкладки — во всех фреймах: звук часто играет во встроенном плеере.
        // (WebView2 и так ставит скрипты во фреймы.) На Linux скрипт пустой, см. `PAGE_SCRIPT`.
        .with_initialization_script_for_main_only(super::volume::PAGE_SCRIPT, false)
        .with_navigation_handler(move |url| {
            // `data:` — только набранный человеком; не страница вовсе — ссылка
            // на приложение (`mailto:`, `tg:`…): решает external.
            omnibox::is_navigable(&url)
                || omnibox::take_typed(id, &url)
                || (source && super::tools::is_source(&url))
                || (!omnibox::is_page(&url) && super::external::on_navigation(id, &url, &nav_tx))
        })
        .with_document_title_changed_handler(move |title| {
            let _ = title_tx.send_event(UserEvent::TabTitle(id, title));
        })
        .with_on_page_load_handler(move |event, url| {
            let started = matches!(event, PageLoadEvent::Started);
            let _ = load_tx.send_event(UserEvent::TabLoad { id, started, url });
        })
        .with_new_window_req_handler(move |url, _features| {
            // Вместо всплывающего окна — новая вкладка рядом, если сайт просил по клику.
            if super::popups::allowed(&url) {
                let _ = open_tx.send_event(UserEvent::OpenTab {
                    url,
                    ai,
                    from: Some(id),
                });
            } else if !omnibox::is_page(&url) {
                let (gesture, frame) = super::popups::request(&url);
                super::external::on_new_window(id, url, gesture, frame, &open_tx);
            }
            NewWindowResponse::Deny
        });
    let builder = styled(builder, spec.look, spec.proxy.as_ref());
    let builder = downloads::attach(builder, events);

    let webview = build_child(builder, window).context("не удалось создать webview вкладки")?;
    hotkeys::install_tab(&webview, events.clone());
    super::popups::install(&webview);
    super::external::install(&webview, id, events.clone());
    super::tracking::install(&webview, id, events.clone());
    super::fullscreen::install(&webview, super::fullscreen::Owner::Tab(id), events.clone());
    downloads::install(&webview, Some(id), events.clone());
    super::tools::watch_audio(&webview, id, events.clone());
    crate::reports::watch(&webview, crate::reports::Source::Tab);
    if spec.muted {
        super::tools::set_muted(&webview, true);
    }
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
                // GtkFixed только что разложил детей по их стартовым размерам — возвращаем
                // каждому webview его место (см. `gtk_place`).
                fixed.connect_size_allocate(|_, alloc| gtk_place::relayout(alloc));
                fixed.show_all();
                fixed
            })
            .clone()
    })
}

/// Ставит webview на место: прямоугольник в логических пикселях окна.
pub fn set_bounds(webview: &WebView, rect: Rect) {
    #[cfg(target_os = "linux")]
    {
        use gtk::prelude::*;
        use wry::WebViewExtUnix;
        let widget = webview.webview().upcast::<gtk::Widget>();
        let scale = f64::from(widget.scale_factor());
        let (x, y) = rect.position.to_logical::<i32>(scale).into();
        let (w, h) = rect.size.to_logical::<i32>(scale).into();
        gtk_place::remember(&widget, gtk_place::Place::At(x, y, w, h));
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = webview.set_bounds(rect);
    }
}

/// Интерфейс на всё окно. На Linux он просто растягивается вместе с контейнером:
/// размер окна от tao в момент `Resized` бывает ещё старым.
pub fn fill_window(webview: &WebView, rect: Rect) {
    #[cfg(target_os = "linux")]
    {
        use gtk::prelude::*;
        use wry::WebViewExtUnix;
        let _ = rect;
        let widget = webview.webview().upcast::<gtk::Widget>();
        gtk_place::remember(&widget, gtk_place::Place::Fill);
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = webview.set_bounds(rect);
    }
}

/// Linux: места webview в общем `gtk::Fixed`.
///
/// `set_bounds` у wry для webview в GtkFixed только разово вызывает `size_allocate`,
/// а запрошенный размер и позиция остаются стартовыми. При любой перекладке GTK
/// (оконный менеджер подогнал окно под плитку, развернул его, показалась вкладка)
/// GtkFixed возвращает детей к стартовым размерам: интерфейс застревал в углу окна.
/// Здесь каждый webview помнит своё место и получает его после каждой перекладки,
/// а запрошенный размер сброшен до 1×1, чтобы не мешать окну уменьшаться.
#[cfg(target_os = "linux")]
mod gtk_place {
    use gtk::{glib, prelude::*};
    use std::cell::RefCell;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Place {
        /// На весь контейнер (интерфейс).
        Fill,
        /// x, y, ширина, высота в логических пикселях от угла контейнера.
        At(i32, i32, i32, i32),
    }

    thread_local! {
        static PLACES: RefCell<Vec<(glib::WeakRef<gtk::Widget>, Place)>> =
            const { RefCell::new(Vec::new()) };
    }

    pub fn remember(widget: &gtk::Widget, place: Place) {
        PLACES.with(|p| {
            let mut p = p.borrow_mut();
            p.retain(|(w, _)| w.upgrade().is_some_and(|w| &w != widget));
            p.push((widget.downgrade(), place));
        });
        widget.set_size_request(1, 1);
        if let Some(parent) = widget.parent() {
            allocate(widget, place, &parent.allocation());
        }
    }

    pub fn relayout(parent: &gtk::Allocation) {
        // Сначала список, потом размещение: size_allocate не должен застать занятый RefCell.
        let items: Vec<(gtk::Widget, Place)> = PLACES.with(|p| {
            let mut p = p.borrow_mut();
            p.retain(|(w, _)| w.upgrade().is_some());
            p.iter()
                .filter_map(|(w, place)| w.upgrade().map(|w| (w, *place)))
                .collect()
        });
        for (widget, place) in items {
            allocate(&widget, place, parent);
        }
    }

    fn allocate(widget: &gtk::Widget, place: Place, parent: &gtk::Allocation) {
        if !widget.is_visible() {
            return;
        }
        let (x, y, w, h) = match place {
            Place::Fill => (0, 0, parent.width(), parent.height()),
            Place::At(x, y, w, h) => (x, y, w, h),
        };
        // У GtkFixed нет своего окна: координаты детей — от угла родительского окна.
        let rect = gtk::Allocation::new(parent.x() + x, parent.y() + y, w.max(1), h.max(1));
        widget.size_allocate(&rect);
    }
}

/// Просыпающаяся страница на Windows не прячется, а уходит под окно интерфейса:
/// WebView2 считает её видимой, поэтому она грузится и рисуется без притормаживания,
/// а человек видит заглушку интерфейса. На Linux её просто прячем.
pub const WAKES_BEHIND: bool = cfg!(windows);

/// Уводит webview под окно интерфейса (`behind`) или возвращает поверх него.
#[cfg(windows)]
pub fn set_behind(webview: &WebView, behind: bool) {
    use windows::Win32::Foundation::HWND;
    use wry::WebViewExtWindows;
    #[link(name = "user32")]
    unsafe extern "system" {
        fn SetWindowPos(
            hwnd: HWND,
            after: isize,
            x: i32,
            y: i32,
            cx: i32,
            cy: i32,
            flags: u32,
        ) -> i32;
    }
    const HWND_TOP: isize = 0;
    const HWND_BOTTOM: isize = 1;
    // SWP_NOSIZE | SWP_NOMOVE | SWP_NOACTIVATE | SWP_NOOWNERZORDER
    const FLAGS: u32 = 0x0001 | 0x0002 | 0x0010 | 0x0200;
    let mut hwnd = HWND::default();
    // SAFETY: контроллер принадлежит живому webview; его родитель — окно-контейнер wry
    // внутри нашего окна, указатель на локальную переменную.
    unsafe {
        if webview.controller().ParentWindow(&mut hwnd).is_ok() && !hwnd.is_invalid() {
            let after = if behind { HWND_BOTTOM } else { HWND_TOP };
            SetWindowPos(hwnd, after, 0, 0, 0, 0, FLAGS);
        }
    }
}

#[cfg(not(windows))]
pub fn set_behind(_webview: &WebView, _behind: bool) {}

/// Webview закрытой вкладки, который держим только ради идущей загрузки:
/// его никто не увидит, поэтому память — в экономный режим.
#[cfg(windows)]
pub fn set_low_memory(webview: &WebView) {
    use wry::{MemoryUsageLevel, WebViewExtWindows};
    let _ = webview.set_memory_usage_level(MemoryUsageLevel::Low);
}

#[cfg(not(windows))]
pub fn set_low_memory(_webview: &WebView) {}

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
