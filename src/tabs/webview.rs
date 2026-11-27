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
    // Отладка: тестовая копия стоит внизу стопки окон, под другими, и для Chromium она
    // «не видна» — страницы не рисуют кадров и не грузят видео. Пусть живёт как видимая
    // (только debug-сборка с DevTools-протоколом).
    #[cfg(debug_assertions)]
    if std::env::var("AXIOM_CDP_PORT").is_ok_and(|p| p.parse::<u16>().is_ok()) {
        args.push_str(",CalculateNativeWinOcclusion --disable-backgrounding-occluded-windows");
    }
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
    // Отладка: поддельные микрофон и камера, чтобы автотест разрешений не включал настоящие.
    #[cfg(debug_assertions)]
    if std::env::var_os("AXIOM_FAKE_MEDIA").is_some() {
        args.push_str(" --use-fake-device-for-media-stream");
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
    /// Масштаб страницы (1.0 — 100 %).
    pub zoom: f64,
    /// Скрипты включённых плагинов.
    pub plugins: std::rc::Rc<[crate::plugins::Script]>,
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
    // С подменой Referer страница грузится после `identity::install`: иначе первый
    // документ ушёл бы без неё. User-Agent wry ставит ещё до первого запроса.
    // Плагинам тоже: их скрипты должны встать до первого документа. На Windows —
    // всегда: запрет на локальную сеть (`local_net`) ставится на готовый webview.
    let plugins = spec.plugins.clone();
    let defer = cfg!(windows) || super::identity::defer_load() || !plugins.is_empty();

    let builder = WebViewBuilder::new_with_web_context(context);
    let builder = if defer {
        builder
    } else {
        builder.with_url(spec.url)
    };
    let builder = match super::identity::agent() {
        Some(ua) => builder.with_user_agent(ua),
        None => builder,
    };
    // Linux: Referer меняет расширение веб-процесса — процесс вкладки грузит его сразу.
    let builder = super::identity::with_extension(builder);
    let builder = builder
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
        // Что играет и кнопки для островка — только главный фрейм (см. `media`).
        .with_initialization_script_for_main_only(super::media::PAGE_SCRIPT, true)
        // Скрытие рекламы — во всех фреймах (Windows; на Linux скрипт пустой).
        .with_initialization_script_for_main_only(crate::adblock::PAGE_SCRIPT, false)
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
    super::local_net::install(&webview);
    crate::adblock::install(&webview, id);
    hotkeys::install_tab(&webview, events.clone());
    super::popups::install(&webview);
    super::external::install(&webview, id, events.clone());
    super::permissions::install(&webview, id, events.clone());
    super::tracking::install(&webview, id, events.clone());
    super::fullscreen::install(&webview, super::fullscreen::Owner::Tab(id), events.clone());
    downloads::install(&webview, Some(id), events.clone());
    super::tools::watch_audio(&webview, id, events.clone());
    super::zoom::watch(&webview, id, events.clone());
    crate::reports::watch(&webview, crate::reports::Source::Tab);
    super::error_page::install(&webview);
    if spec.muted {
        super::tools::set_muted(&webview, true);
    }
    if !super::zoom::same(spec.zoom, 1.0) {
        super::zoom::set(&webview, spec.zoom);
    }
    enable_autofill(&webview);
    super::identity::install(&webview);
    crate::plugins::bridge::apply(&webview, id, events, plugins);
    if defer {
        webview
            .load_url(spec.url)
            .context("не удалось открыть страницу вкладки")?;
    }
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
thread_local! {
    static FIXED: std::cell::RefCell<Option<gtk::Fixed>> = const { std::cell::RefCell::new(None) };
}

#[cfg(target_os = "linux")]
fn gtk_container(window: &Window) -> gtk::Fixed {
    use gtk::prelude::*;
    use tao::platform::unix::WindowExtUnix;

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

/// Как [`set_bounds`], но webview может свисать за правый и нижний край окна (там его
/// просто не видно): островок с видео ставит вкладку её же размера левым верхним углом
/// под чёлку, и сайт не перестраивает страницу. На Linux размер всё равно не больше окна.
pub fn set_bounds_hanging(webview: &WebView, rect: Rect) {
    #[cfg(target_os = "linux")]
    {
        use gtk::prelude::*;
        use wry::WebViewExtUnix;
        let widget = webview.webview().upcast::<gtk::Widget>();
        let scale = f64::from(widget.scale_factor());
        let (x, y) = rect.position.to_logical::<i32>(scale).into();
        let (w, h) = rect.size.to_logical::<i32>(scale).into();
        gtk_place::remember(&widget, gtk_place::Place::Hang(x, y, w, h));
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = webview.set_bounds(rect);
    }
}

/// Для журнала тестов: где webview на деле — место GTK, окно GDK и видим ли он.
#[cfg(target_os = "linux")]
pub fn geometry(webview: &WebView) -> String {
    use gtk::prelude::*;
    use wry::WebViewExtUnix;
    let widget = webview.webview().upcast::<gtk::Widget>();
    let a = widget.allocation();
    let window = widget
        .has_window()
        .then(|| widget.window())
        .flatten()
        .map(|w| {
            let (x, y) = w.position();
            format!(
                "gdk {}x{} at {x},{y} native={}",
                w.width(),
                w.height(),
                w.has_native()
            )
        })
        .unwrap_or_else(|| "gdk none".into());
    format!(
        "alloc {}x{} at {},{} visible={} mapped={} {window}",
        a.width(),
        a.height(),
        a.x(),
        a.y(),
        widget.is_visible(),
        widget.is_mapped()
    )
}

/// Поднимает webview над остальными (остров с видео — над страницей и интерфейсом).
pub fn raise(webview: &WebView) {
    #[cfg(target_os = "linux")]
    {
        use gtk::prelude::*;
        use wry::WebViewExtUnix;
        // У webview WebKitGTK своё окно GDK: его место в стопке — порядок рисования и
        // того, кто ловит мышь.
        let widget = webview.webview().upcast::<gtk::Widget>();
        if widget.has_window()
            && let Some(window) = widget.window()
        {
            window.raise();
        }
    }
    #[cfg(not(target_os = "linux"))]
    set_behind(webview, false);
}

/// Где курсор: логические px от угла окна, как у интерфейса; `None` — курсор не над
/// окном браузера. На Wayland координат экрана нет, но где курсор над своим окном, GDK
/// знает всегда.
#[cfg(target_os = "linux")]
pub fn pointer() -> Option<(f64, f64)> {
    use gtk::prelude::*;
    let fixed = FIXED.with(|c| c.borrow().clone())?;
    let window = fixed.window()?;
    let pointer = window.display().default_seat()?.pointer()?;
    // Курсор ушёл с окна: на Wayland у указателя больше нет нашего окна, на X11 под
    // курсором чужое.
    pointer.window_at_position().0?;
    let (_, x, y, _) = window.device_position_double(&pointer);
    // У GtkFixed нет своего окна: его угол — в окне родителя.
    let a = fixed.allocation();
    Some((x - f64::from(a.x()), y - f64::from(a.y())))
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

/// Где держать webview (x, y, ширина, высота), пока окно меняет размер, а интерфейс
/// ещё не прислал новую раскладку: `base` — размер контейнера, для которого место
/// посчитано, `now` — нынешний. Отступы справа и снизу сохраняются, так что страница
/// тянется вместе с окном и не оставляет тёмной полосы; узкая панель у правого края
/// (ИИ) едет за краем. Точную раскладку чуть позже пришлёт интерфейс.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn follow(rect: (i32, i32, i32, i32), base: (i32, i32), now: (i32, i32)) -> (i32, i32, i32, i32) {
    let (x, y, w, h) = rect;
    let ((bw, bh), (nw, nh)) = (base, now);
    if base == now || bw <= 0 || bh <= 0 {
        return rect;
    }
    let right = bw - (x + w);
    let bottom = bh - (y + h);
    let (x, w) = if right == 0 && x > 0 && w * 2 < bw {
        (nw - w, w)
    } else if right >= 0 {
        (x, nw - x - right)
    } else {
        (x, w)
    };
    let h = if bottom >= 0 { nh - y - bottom } else { h };
    (x.max(0), y, w.max(1), h.max(1))
}

/// Место webview внутри контейнера `size` (x, y, ширина, высота): угол — в контейнере,
/// размер — не меньше 1×1 и не дальше его края. Webview за краем окна не виден, а
/// отрицательный угол или размер больше окна GTK отдал бы окну GDK, а WebKitGTK —
/// буферу кадра (огромный буфер видеокарта может и не выделить).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn fit(rect: (i32, i32, i32, i32), size: (i32, i32)) -> (i32, i32, i32, i32) {
    let (x, y, w, h) = rect;
    let (cw, ch) = (size.0.max(1), size.1.max(1));
    let (x, y) = (x.clamp(0, cw - 1), y.clamp(0, ch - 1));
    (x, y, w.clamp(1, cw - x), h.clamp(1, ch - y))
}

/// Как [`fit`], но размер ограничен только размером контейнера: webview может свисать за
/// его правый и нижний край (остров с видео, см. [`set_bounds_hanging`]). Буфер кадра
/// WebKitGTK от этого не больше окна.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn hang(rect: (i32, i32, i32, i32), size: (i32, i32)) -> (i32, i32, i32, i32) {
    let (x, y, w, h) = rect;
    let (cw, ch) = (size.0.max(1), size.1.max(1));
    let (x, y) = (x.clamp(0, cw - 1), y.clamp(0, ch - 1));
    (x, y, w.clamp(1, cw), h.clamp(1, ch))
}

/// Linux: места webview в общем `gtk::Fixed`.
///
/// `set_bounds` у wry для webview в GtkFixed только разово вызывает `size_allocate`,
/// а запрошенный размер и позиция остаются стартовыми. При любой перекладке GTK
/// (оконный менеджер подогнал окно под плитку, развернул его, показалась вкладка)
/// GtkFixed возвращает детей к стартовым размерам: интерфейс застревал в углу окна.
/// Здесь каждый webview помнит своё место и получает его после каждой перекладки,
/// а запрошенный размер сброшен до 1×1, чтобы не мешать окну уменьшаться. Если окно
/// с тех пор поменяло размер, место подстраивается под него сразу ([`follow`]), не
/// дожидаясь интерфейса: иначе при перетаскивании окна в плиточном менеджере
/// (Hyprland) страница отставала и мигала тёмной полосой.
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
        /// То же, но может свисать за правый и нижний край (остров с видео); за окном
        /// не тянется — его место пришлёт заново `redock_mini`.
        Hang(i32, i32, i32, i32),
    }

    /// Место webview и размер контейнера, для которого оно посчитано.
    type Entry = (glib::WeakRef<gtk::Widget>, Place, Option<(i32, i32)>);
    /// То же с живым виджетом — на время одной перекладки.
    type Placed = (gtk::Widget, Place, Option<(i32, i32)>);

    thread_local! {
        static PLACES: RefCell<Vec<Entry>> = const { RefCell::new(Vec::new()) };
    }

    pub fn remember(widget: &gtk::Widget, place: Place) {
        let parent = widget.parent().map(|p| p.allocation());
        let base = parent.as_ref().map(|a| (a.width(), a.height()));
        PLACES.with(|p| {
            let mut p = p.borrow_mut();
            p.retain(|(w, _, _)| w.upgrade().is_some_and(|w| &w != widget));
            p.push((widget.downgrade(), place, base));
        });
        widget.set_size_request(1, 1);
        if let Some(parent) = parent {
            allocate(widget, place, base, &parent);
        }
    }

    pub fn relayout(parent: &gtk::Allocation) {
        // Сначала список, потом размещение: size_allocate не должен застать занятый RefCell.
        let items: Vec<Placed> = PLACES.with(|p| {
            let mut p = p.borrow_mut();
            p.retain(|(w, _, _)| w.upgrade().is_some());
            p.iter()
                .filter_map(|(w, place, base)| w.upgrade().map(|w| (w, *place, *base)))
                .collect()
        });
        for (widget, place, base) in items {
            allocate(&widget, place, base, parent);
        }
    }

    fn allocate(
        widget: &gtk::Widget,
        place: Place,
        base: Option<(i32, i32)>,
        parent: &gtk::Allocation,
    ) {
        if !widget.is_visible() {
            return;
        }
        let now = (parent.width(), parent.height());
        let (x, y, w, h) = match place {
            Place::Fill => (0, 0, now.0, now.1),
            Place::At(x, y, w, h) => super::fit(
                base.map_or((x, y, w, h), |base| super::follow((x, y, w, h), base, now)),
                now,
            ),
            Place::Hang(x, y, w, h) => super::hang((x, y, w, h), now),
        };
        // У GtkFixed нет своего окна: координаты детей — от угла родительского окна.
        let rect = gtk::Allocation::new(parent.x() + x, parent.y() + y, w, h);
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

#[cfg(test)]
mod follow_tests {
    use super::{fit, follow, hang};

    #[test]
    fn island_tab_hangs_past_the_edge_but_not_bigger_than_the_window() {
        // Вкладка своего размера, угол под чёлкой — свисает за правый край как есть.
        assert_eq!(
            hang((600, 30, 1000, 680), (1000, 720)),
            (600, 30, 1000, 680)
        );
        // Больше окна не бывает (буфер кадра), угол — в окне.
        assert_eq!(hang((-20, -5, 9000, 90000), (1000, 720)), (0, 0, 1000, 720));
        assert_eq!(hang((10, 10, 0, -1), (0, 0)), (0, 0, 1, 1));
    }

    #[test]
    fn place_stays_inside_the_container() {
        // Внутри — как есть.
        assert_eq!(fit((240, 80, 760, 640), (1000, 720)), (240, 80, 760, 640));
        // Шире и ниже окна — до его края.
        assert_eq!(
            fit((240, 80, 9000, 90000), (1000, 720)),
            (240, 80, 760, 640)
        );
        // Отрицательный угол — в угол окна, нулевой и отрицательный размер — 1×1.
        assert_eq!(fit((-50, -5, 0, -10), (1000, 720)), (0, 0, 1, 1));
        // Угол за краем окна — у края, 1×1.
        assert_eq!(fit((5000, 900, 300, 300), (1000, 720)), (999, 719, 1, 1));
        // Контейнер ещё не разложен (0×0) — 1×1 в углу, без паники.
        assert_eq!(fit((10, 10, 100, 100), (0, 0)), (0, 0, 1, 1));
        assert_eq!(fit((10, 10, 100, 100), (-3, -3)), (0, 0, 1, 1));
    }

    #[test]
    fn page_stretches_and_panel_rides_the_edge() {
        // Страница от сайдбара до правого края, окно стало шире.
        assert_eq!(
            follow((240, 80, 760, 640), (1000, 720), (1200, 800)),
            (240, 80, 960, 720)
        );
        // Страница рядом с панелью ИИ: отступ справа (ширина панели) сохраняется.
        assert_eq!(
            follow((240, 80, 400, 640), (1000, 720), (1200, 720)),
            (240, 80, 600, 640)
        );
        // Узкая панель у правого края едет за краем, ширина та же.
        assert_eq!(
            follow((640, 80, 360, 640), (1000, 720), (1200, 720)),
            (840, 80, 360, 640)
        );
        // Размер не менялся — как есть.
        assert_eq!(follow((1, 2, 3, 4), (10, 10), (10, 10)), (1, 2, 3, 4));
        // Окно сжалось почти в ноль — не меньше 1×1.
        assert_eq!(
            follow((240, 80, 760, 640), (1000, 720), (100, 50)),
            (240, 80, 1, 1)
        );
    }
}
