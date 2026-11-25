//! Горячие клавиши как в Chrome.
//!
//! Когда фокус в интерфейсе, клавиши ловит JS и присылает их через IPC.
//! Когда фокус на странице, на Windows клавиши перехватываются нативно
//! (событие WebView2 `AcceleratorKeyPressed`): сайт не может их ни подделать, ни заблокировать.

use tao::event_loop::EventLoopProxy;
use wry::WebView;

use crate::events::UserEvent;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hotkey {
    NewTab,
    CloseTab,
    RestoreTab,
    NextTab,
    PrevTab,
    SelectTab(u8),
    Reload,
    Back,
    Forward,
    FocusOmnibox,
    ToggleAi,
    Find,
    Settings,
    Bookmark,
    Downloads,
    /// F11: страница на весь экран.
    Fullscreen,
    /// Esc: только подсматриваем (см. [`Hotkey::consumes`]).
    Escape,
    /// Ctrl+Shift+R, Ctrl+F5, Shift+F5: перезагрузка без кэша.
    HardReload,
    /// F12, Ctrl+Shift+I: инструменты разработчика для страницы.
    DevTools,
    /// Ctrl+U: исходный код страницы в новой вкладке.
    ViewSource,
    /// Ctrl+P: печать страницы.
    Print,
    /// Ctrl+M: выключить или включить звук вкладки.
    Mute,
    /// Ctrl+= (Ctrl++), Ctrl+− и Ctrl+0: масштаб страницы.
    ZoomIn,
    ZoomOut,
    ZoomReset,
}

impl Hotkey {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "new_tab" => Self::NewTab,
            "close_tab" => Self::CloseTab,
            "restore_tab" => Self::RestoreTab,
            "next_tab" => Self::NextTab,
            "prev_tab" => Self::PrevTab,
            "reload" => Self::Reload,
            "back" => Self::Back,
            "forward" => Self::Forward,
            "focus_omnibox" => Self::FocusOmnibox,
            "toggle_ai" => Self::ToggleAi,
            "find" => Self::Find,
            "settings" => Self::Settings,
            "bookmark" => Self::Bookmark,
            "downloads" => Self::Downloads,
            "fullscreen" => Self::Fullscreen,
            "escape" => Self::Escape,
            "hard_reload" => Self::HardReload,
            "devtools" => Self::DevTools,
            "view_source" => Self::ViewSource,
            "print" => Self::Print,
            "mute" => Self::Mute,
            "zoom_in" => Self::ZoomIn,
            "zoom_out" => Self::ZoomOut,
            "zoom_reset" => Self::ZoomReset,
            other => {
                let n: u8 = other.strip_prefix("tab_")?.parse().ok()?;
                if (1..=9).contains(&n) {
                    Self::SelectTab(n)
                } else {
                    return None;
                }
            }
        })
    }

    pub fn name(self) -> String {
        match self {
            Self::NewTab => "new_tab".into(),
            Self::CloseTab => "close_tab".into(),
            Self::RestoreTab => "restore_tab".into(),
            Self::NextTab => "next_tab".into(),
            Self::PrevTab => "prev_tab".into(),
            Self::SelectTab(n) => format!("tab_{n}"),
            Self::Reload => "reload".into(),
            Self::Back => "back".into(),
            Self::Forward => "forward".into(),
            Self::FocusOmnibox => "focus_omnibox".into(),
            Self::ToggleAi => "toggle_ai".into(),
            Self::Find => "find".into(),
            Self::Settings => "settings".into(),
            Self::Bookmark => "bookmark".into(),
            Self::Downloads => "downloads".into(),
            Self::Fullscreen => "fullscreen".into(),
            Self::Escape => "escape".into(),
            Self::HardReload => "hard_reload".into(),
            Self::DevTools => "devtools".into(),
            Self::ViewSource => "view_source".into(),
            Self::Print => "print".into(),
            Self::Mute => "mute".into(),
            Self::ZoomIn => "zoom_in".into(),
            Self::ZoomOut => "zoom_out".into(),
            Self::ZoomReset => "zoom_reset".into(),
        }
    }

    /// Эти действия выполняет интерфейс (фокус, панели), остальные — Rust.
    pub fn is_ui(self) -> bool {
        matches!(
            self,
            Self::FocusOmnibox | Self::ToggleAi | Self::Find | Self::Settings | Self::Downloads
        )
    }

    /// Забрать ли клавишу у страницы. Esc странице тоже нужен (закрыть окно,
    /// выйти из видео), поэтому его только подсматриваем.
    pub fn consumes(self) -> bool {
        self != Self::Escape
    }

    /// Действия со страницей вкладки: Ctrl+P, Ctrl+U, Ctrl+M, F12, масштаб, перезагрузка без кэша.
    pub fn is_page_tool(self) -> bool {
        matches!(
            self,
            Self::HardReload
                | Self::DevTools
                | Self::ViewSource
                | Self::Print
                | Self::Mute
                | Self::ZoomIn
                | Self::ZoomOut
                | Self::ZoomReset
        )
    }

    /// Не сворачивать из-за этого сочетания видео на весь экран: звук и масштаб
    /// меняют прямо во время просмотра.
    pub fn keeps_fullscreen(self) -> bool {
        matches!(
            self,
            Self::Fullscreen
                | Self::Escape
                | Self::Mute
                | Self::ZoomIn
                | Self::ZoomOut
                | Self::ZoomReset
        )
    }

    /// Виртуальный код клавиши Windows + модификаторы → действие.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn from_vk(vk: u32, ctrl: bool, shift: bool, alt: bool) -> Option<Self> {
        const VK_TAB: u32 = 0x09;
        const VK_ESCAPE: u32 = 0x1B;
        const VK_LEFT: u32 = 0x25;
        const VK_RIGHT: u32 = 0x27;
        const VK_F5: u32 = 0x74;
        const VK_F11: u32 = 0x7A;
        const VK_F12: u32 = 0x7B;
        const VK_NUMPAD0: u32 = 0x60;
        const VK_ADD: u32 = 0x6B;
        const VK_SUBTRACT: u32 = 0x6D;
        const VK_COMMA: u32 = 0xBC;
        // Клавиши «=/+» и «−» основной клавиатуры в любой раскладке.
        const VK_OEM_PLUS: u32 = 0xBB;
        const VK_OEM_MINUS: u32 = 0xBD;
        let key = |c: u8| u32::from(c);
        Some(match (vk, ctrl, shift, alt) {
            (v, true, true, false) if v == key(b'T') => Self::RestoreTab,
            (v, true, true, false) if v == key(b'R') => Self::HardReload,
            (VK_F5, true, false, false) | (VK_F5, false, true, false) => Self::HardReload,
            (VK_F12, false, false, false) => Self::DevTools,
            (v, true, true, false) if v == key(b'I') => Self::DevTools,
            (v, true, false, false) if v == key(b'U') => Self::ViewSource,
            (v, true, false, false) if v == key(b'P') => Self::Print,
            (v, true, false, false) if v == key(b'M') => Self::Mute,
            // Ctrl++ набирают и с Shift: на многих раскладках «+» — это Shift+«=».
            (VK_OEM_PLUS | VK_ADD, true, _, false) => Self::ZoomIn,
            (VK_OEM_MINUS | VK_SUBTRACT, true, _, false) => Self::ZoomOut,
            (v, true, false, false) if v == key(b'0') || v == VK_NUMPAD0 => Self::ZoomReset,
            (v, true, false, false) if v == key(b'T') => Self::NewTab,
            (v, true, _, false) if v == key(b'W') => Self::CloseTab,
            (VK_TAB, true, false, false) => Self::NextTab,
            (VK_TAB, true, true, false) => Self::PrevTab,
            (v, true, false, false) if v == key(b'L') => Self::FocusOmnibox,
            (v, true, false, false) if v == key(b'K') => Self::ToggleAi,
            (v, true, false, false) if v == key(b'F') => Self::Find,
            (v, true, false, false) if v == key(b'R') => Self::Reload,
            (VK_F5, false, false, false) => Self::Reload,
            (VK_F11, false, false, false) => Self::Fullscreen,
            (VK_ESCAPE, false, false, false) => Self::Escape,
            (VK_COMMA, true, false, false) => Self::Settings,
            (v, true, false, false) if v == key(b'D') => Self::Bookmark,
            (v, true, false, false) if v == key(b'J') => Self::Downloads,
            (VK_LEFT, false, false, true) => Self::Back,
            (VK_RIGHT, false, false, true) => Self::Forward,
            (v, true, false, false) if (key(b'1')..=key(b'9')).contains(&v) => {
                Self::SelectTab((v - key(b'0')) as u8)
            }
            _ => return None,
        })
    }
}

/// Перехват горячих клавиш внутри страницы вкладки (Windows): все сочетания.
pub fn install_tab(webview: &WebView, events: EventLoopProxy<UserEvent>) {
    intercept(webview, events, true);
}

/// Перехват в панели ИИ. Инструменты страницы (масштаб, печать, F12…) там относятся
/// к самой панели, а не к вкладке, — их оставляем WebView2.
pub fn install(webview: &WebView, events: EventLoopProxy<UserEvent>) {
    intercept(webview, events, false);
}

#[cfg(windows)]
fn intercept(webview: &WebView, events: EventLoopProxy<UserEvent>, page_tools: bool) {
    use webview2_com::{
        AcceleratorKeyPressedEventHandler,
        Microsoft::Web::WebView2::Win32::{
            COREWEBVIEW2_KEY_EVENT_KIND, COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN,
            COREWEBVIEW2_KEY_EVENT_KIND_SYSTEM_KEY_DOWN,
        },
    };
    use wry::WebViewExtWindows;

    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetKeyState(virtual_key: i32) -> i16;
    }
    const VK_SHIFT: i32 = 0x10;
    const VK_CONTROL: i32 = 0x11;
    const VK_MENU: i32 = 0x12;

    let handler = AcceleratorKeyPressedEventHandler::create(Box::new(move |_, args| {
        let Some(args) = args else { return Ok(()) };
        let mut kind = COREWEBVIEW2_KEY_EVENT_KIND::default();
        let mut vk = 0u32;
        // SAFETY: args — живой COM-объект события, указатели на локальные переменные.
        unsafe {
            args.KeyEventKind(&mut kind)?;
            if kind != COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN
                && kind != COREWEBVIEW2_KEY_EVENT_KIND_SYSTEM_KEY_DOWN
            {
                return Ok(());
            }
            args.VirtualKey(&mut vk)?;
        }
        // SAFETY: GetKeyState — чтение состояния клавиатуры текущего потока.
        let down = |k: i32| unsafe { GetKeyState(k) } < 0;
        let hotkey = Hotkey::from_vk(vk, down(VK_CONTROL), down(VK_SHIFT), down(VK_MENU))
            .filter(|h| page_tools || !h.is_page_tool());
        if let Some(hotkey) = hotkey {
            if hotkey.consumes() {
                // SAFETY: см. выше.
                unsafe { args.SetHandled(true)? };
            }
            let _ = events.send_event(UserEvent::Hotkey(hotkey));
        }
        Ok(())
    }));

    let mut token = 0i64;
    // SAFETY: контроллер принадлежит webview и живёт столько же, сколько он.
    let result = unsafe {
        webview
            .controller()
            .add_AcceleratorKeyPressed(&handler, &mut token)
    };
    if let Err(err) = result {
        eprintln!("горячие клавиши в странице недоступны: {err}");
    }
}

/// На других платформах клавиши ловятся на уровне окна (см. `install_window`).
#[cfg(not(windows))]
fn intercept(_webview: &WebView, _events: EventLoopProxy<UserEvent>, _page_tools: bool) {}

/// Linux: обработчик `key-press-event` окна GTK срабатывает раньше, чем клавиша
/// доходит до WebKitGTK, поэтому сочетания работают и внутри страниц.
#[cfg(target_os = "linux")]
pub fn install_window(window: &tao::window::Window, events: EventLoopProxy<UserEvent>) {
    use gtk::{gdk, glib, prelude::*};
    use tao::platform::unix::WindowExtUnix;

    let gtk_window = window.gtk_window();
    let keymap = gdk::Keymap::for_display(&gtk_window.display());
    gtk_window.connect_key_press_event(move |_, event| {
        let state = event.state();
        let ctrl = state.contains(gdk::ModifierType::CONTROL_MASK);
        let shift = state.contains(gdk::ModifierType::SHIFT_MASK);
        let alt = state.contains(gdk::ModifierType::MOD1_MASK);
        // Клавиша в текущей раскладке, а если она не латинская — в первой латинской:
        // Ctrl+T работает и на русской, даже когда русская раскладка в списке первая.
        let code = u32::from(event.hardware_keycode());
        let in_group = |group: i32| {
            keymap
                .as_ref()?
                .translate_keyboard_state(code, state, group)
                .map(|(keyval, ..)| keyval)
        };
        let vk = std::iter::once(*event.keyval())
            .chain((0..4).filter_map(in_group))
            .find_map(vk_from_keyval);
        match vk.and_then(|vk| Hotkey::from_vk(vk, ctrl, shift, alt)) {
            Some(hotkey) => {
                let _ = events.send_event(UserEvent::Hotkey(hotkey));
                if hotkey.consumes() {
                    glib::Propagation::Stop
                } else {
                    glib::Propagation::Proceed
                }
            }
            None => glib::Propagation::Proceed,
        }
    });
}

/// Код клавиши GDK (keysym) → виртуальный код в стиле Windows для [`Hotkey::from_vk`].
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn vk_from_keyval(keyval: u32) -> Option<u32> {
    Some(match keyval {
        0x61..=0x7a => keyval - 0x20,
        0x41..=0x5a | 0x30..=0x39 => keyval,
        0xff09 | 0xfe20 => 0x09,
        0xff51 => 0x25,
        0xff53 => 0x27,
        0xffc2 => 0x74,
        0xffc8 => 0x7A,
        0xffc9 => 0x7B,
        0xff1b => 0x1B,
        0x2c => 0xBC,
        // = и + (Shift+=), − и _ (Shift+−); цифровой блок: +, −, 0.
        0x3d | 0x2b => 0xBB,
        0x2d | 0x5f => 0xBD,
        0xffab => 0x6B,
        0xffad => 0x6D,
        0xffb0 => 0x60,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_roundtrip() {
        for h in [
            Hotkey::NewTab,
            Hotkey::SelectTab(3),
            Hotkey::Settings,
            Hotkey::Back,
            Hotkey::Downloads,
            Hotkey::Fullscreen,
        ] {
            assert_eq!(Hotkey::parse(&h.name()), Some(h));
        }
        assert_eq!(Hotkey::parse("tab_0"), None);
        assert_eq!(Hotkey::parse("format_c"), None);
    }

    #[test]
    fn virtual_keys() {
        assert_eq!(
            Hotkey::from_vk(u32::from(b'T'), true, true, false),
            Some(Hotkey::RestoreTab)
        );
        assert_eq!(
            Hotkey::from_vk(u32::from(b'T'), true, false, false),
            Some(Hotkey::NewTab)
        );
        assert_eq!(
            Hotkey::from_vk(u32::from(b'4'), true, false, false),
            Some(Hotkey::SelectTab(4))
        );
        assert_eq!(
            Hotkey::from_vk(u32::from(b'J'), true, false, false),
            Some(Hotkey::Downloads)
        );
        assert_eq!(Hotkey::from_vk(u32::from(b'C'), true, false, false), None);
        assert_eq!(
            Hotkey::from_vk(0x7A, false, false, false),
            Some(Hotkey::Fullscreen)
        );
        // Esc страница получает и сама.
        assert_eq!(
            Hotkey::from_vk(0x1B, false, false, false),
            Some(Hotkey::Escape)
        );
        assert!(!Hotkey::Escape.consumes());
        assert!(Hotkey::Fullscreen.consumes());
    }

    #[test]
    fn page_tool_keys() {
        let vk = |c: u8| u32::from(c);
        let hk = Hotkey::from_vk;
        // Без кэша: Ctrl+Shift+R, Ctrl+F5, Shift+F5; обычная перезагрузка — Ctrl+R и F5.
        assert_eq!(hk(vk(b'R'), true, true, false), Some(Hotkey::HardReload));
        assert_eq!(hk(0x74, true, false, false), Some(Hotkey::HardReload));
        assert_eq!(hk(0x74, false, true, false), Some(Hotkey::HardReload));
        assert_eq!(hk(vk(b'R'), true, false, false), Some(Hotkey::Reload));
        assert_eq!(hk(0x74, false, false, false), Some(Hotkey::Reload));
        assert_eq!(hk(0x7B, false, false, false), Some(Hotkey::DevTools));
        assert_eq!(hk(vk(b'I'), true, true, false), Some(Hotkey::DevTools));
        assert_eq!(hk(vk(b'I'), true, false, false), None);
        assert_eq!(hk(vk(b'U'), true, false, false), Some(Hotkey::ViewSource));
        assert_eq!(hk(vk(b'P'), true, false, false), Some(Hotkey::Print));
        assert_eq!(hk(vk(b'M'), true, false, false), Some(Hotkey::Mute));
        // Масштаб: основная клавиатура (Ctrl+Shift+= — это Ctrl++) и цифровой блок.
        assert_eq!(hk(0xBB, true, false, false), Some(Hotkey::ZoomIn));
        assert_eq!(hk(0xBB, true, true, false), Some(Hotkey::ZoomIn));
        assert_eq!(hk(0x6B, true, false, false), Some(Hotkey::ZoomIn));
        assert_eq!(hk(0xBD, true, false, false), Some(Hotkey::ZoomOut));
        assert_eq!(hk(0x6D, true, false, false), Some(Hotkey::ZoomOut));
        assert_eq!(hk(vk(b'0'), true, false, false), Some(Hotkey::ZoomReset));
        assert_eq!(hk(0x60, true, false, false), Some(Hotkey::ZoomReset));
        // Без Ctrl это обычный ввод, с AltGr (Ctrl+Alt) — тоже.
        assert_eq!(hk(0xBB, false, false, false), None);
        assert_eq!(hk(vk(b'0'), true, false, true), None);
        assert_eq!(hk(vk(b'M'), true, false, true), None);
        // Ctrl+Shift+W по-прежнему закрывает вкладку.
        assert_eq!(hk(vk(b'W'), true, true, false), Some(Hotkey::CloseTab));
        for h in [
            Hotkey::HardReload,
            Hotkey::DevTools,
            Hotkey::ViewSource,
            Hotkey::Print,
            Hotkey::Mute,
            Hotkey::ZoomIn,
            Hotkey::ZoomOut,
            Hotkey::ZoomReset,
        ] {
            assert_eq!(Hotkey::parse(&h.name()), Some(h));
            assert!(h.consumes() && !h.is_ui() && h.is_page_tool());
        }
        assert!(!Hotkey::Reload.is_page_tool() && !Hotkey::NewTab.is_page_tool());
        assert!(Hotkey::Mute.keeps_fullscreen() && Hotkey::ZoomIn.keeps_fullscreen());
        assert!(!Hotkey::ViewSource.keeps_fullscreen());
    }

    #[test]
    fn gdk_keyvals() {
        let hk = |kv, ctrl, shift, alt| {
            vk_from_keyval(kv).and_then(|vk| Hotkey::from_vk(vk, ctrl, shift, alt))
        };
        assert_eq!(hk(0x74, true, false, false), Some(Hotkey::NewTab));
        assert_eq!(hk(0x54, true, true, false), Some(Hotkey::RestoreTab));
        assert_eq!(hk(0xfe20, true, true, false), Some(Hotkey::PrevTab));
        assert_eq!(hk(0xffc2, false, false, false), Some(Hotkey::Reload));
        assert_eq!(hk(0x2c, true, false, false), Some(Hotkey::Settings));
        assert_eq!(hk(0xffc8, false, false, false), Some(Hotkey::Fullscreen));
        assert_eq!(hk(0xff1b, false, false, false), Some(Hotkey::Escape));
        assert_eq!(hk(0x6c1, true, false, false), None);
        assert_eq!(hk(0x52, true, true, false), Some(Hotkey::HardReload));
        assert_eq!(hk(0xffc2, false, true, false), Some(Hotkey::HardReload));
        assert_eq!(hk(0xffc9, false, false, false), Some(Hotkey::DevTools));
        assert_eq!(hk(0x49, true, true, false), Some(Hotkey::DevTools));
        assert_eq!(hk(0x75, true, false, false), Some(Hotkey::ViewSource));
        assert_eq!(hk(0x70, true, false, false), Some(Hotkey::Print));
        assert_eq!(hk(0x6d, true, false, false), Some(Hotkey::Mute));
        assert_eq!(hk(0x3d, true, false, false), Some(Hotkey::ZoomIn));
        assert_eq!(hk(0x2b, true, true, false), Some(Hotkey::ZoomIn));
        assert_eq!(hk(0xffab, true, false, false), Some(Hotkey::ZoomIn));
        assert_eq!(hk(0x2d, true, false, false), Some(Hotkey::ZoomOut));
        assert_eq!(hk(0x5f, true, true, false), Some(Hotkey::ZoomOut));
        assert_eq!(hk(0x30, true, false, false), Some(Hotkey::ZoomReset));
        assert_eq!(hk(0xffb0, true, false, false), Some(Hotkey::ZoomReset));
    }
}
