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
        }
    }

    /// Эти действия выполняет интерфейс (фокус, панели), остальные — Rust.
    pub fn is_ui(self) -> bool {
        matches!(
            self,
            Self::FocusOmnibox | Self::ToggleAi | Self::Find | Self::Settings
        )
    }

    /// Виртуальный код клавиши Windows + модификаторы → действие.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn from_vk(vk: u32, ctrl: bool, shift: bool, alt: bool) -> Option<Self> {
        const VK_TAB: u32 = 0x09;
        const VK_LEFT: u32 = 0x25;
        const VK_RIGHT: u32 = 0x27;
        const VK_F5: u32 = 0x74;
        const VK_COMMA: u32 = 0xBC;
        let key = |c: u8| u32::from(c);
        Some(match (vk, ctrl, shift, alt) {
            (v, true, true, false) if v == key(b'T') => Self::RestoreTab,
            (v, true, false, false) if v == key(b'T') => Self::NewTab,
            (v, true, _, false) if v == key(b'W') => Self::CloseTab,
            (VK_TAB, true, false, false) => Self::NextTab,
            (VK_TAB, true, true, false) => Self::PrevTab,
            (v, true, false, false) if v == key(b'L') => Self::FocusOmnibox,
            (v, true, false, false) if v == key(b'K') => Self::ToggleAi,
            (v, true, false, false) if v == key(b'F') => Self::Find,
            (v, true, false, false) if v == key(b'R') => Self::Reload,
            (VK_F5, false, false, false) => Self::Reload,
            (VK_COMMA, true, false, false) => Self::Settings,
            (v, true, false, false) if v == key(b'D') => Self::Bookmark,
            (VK_LEFT, false, false, true) => Self::Back,
            (VK_RIGHT, false, false, true) => Self::Forward,
            (v, true, false, false) if (key(b'1')..=key(b'9')).contains(&v) => {
                Self::SelectTab((v - key(b'0')) as u8)
            }
            _ => return None,
        })
    }
}

/// Перехват горячих клавиш внутри страницы (Windows).
#[cfg(windows)]
pub fn install(webview: &WebView, events: EventLoopProxy<UserEvent>) {
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
        if let Some(hotkey) = Hotkey::from_vk(vk, down(VK_CONTROL), down(VK_SHIFT), down(VK_MENU)) {
            // SAFETY: см. выше.
            unsafe { args.SetHandled(true)? };
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

/// На других платформах клавиши в странице пока ловит только меню приложения (TODO).
#[cfg(not(windows))]
pub fn install(_webview: &WebView, _events: EventLoopProxy<UserEvent>) {}

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
        assert_eq!(Hotkey::from_vk(u32::from(b'C'), true, false, false), None);
    }
}
