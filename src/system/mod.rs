//! Установка в систему и браузер по умолчанию.
//!
//! Windows: копия exe в профиль, ярлык в «Пуске» и регистрация в HKCU — права
//! администратора не нужны. Сделать браузер основным Windows разрешает только
//! пользователю, поэтому открываем нужную страницу параметров.
//!
//! Linux: Axiom ставится пакетом (.deb, pacman), а основным браузером
//! становится через `xdg-settings`.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

use std::path::PathBuf;

use serde::Serialize;

/// Что интерфейс показывает в разделе «Система» и на экране приветствия.
#[derive(Debug, Clone, Serialize)]
pub struct Status {
    pub platform: &'static str,
    pub installed: bool,
    pub default_browser: bool,
    pub chrome_found: bool,
    /// Можно ли установить из самого приложения (на Linux — через пакетный менеджер).
    pub can_install: bool,
    pub version: &'static str,
}

pub fn status() -> Status {
    Status {
        platform: std::env::consts::OS,
        installed: is_installed(),
        default_browser: is_default_browser(),
        chrome_found: crate::storage::chrome::is_available(),
        can_install: cfg!(windows),
        version: env!("CARGO_PKG_VERSION"),
    }
}

/// `%LOCALAPPDATA%\Programs\Axiom\axiom.exe`
#[cfg_attr(not(windows), allow(dead_code))]
pub fn install_path() -> Option<PathBuf> {
    let base = directories::BaseDirs::new()?;
    Some(
        base.data_local_dir()
            .join("Programs")
            .join("Axiom")
            .join("axiom.exe"),
    )
}

#[cfg(windows)]
pub fn is_installed() -> bool {
    let (Ok(current), Some(target)) = (std::env::current_exe(), install_path()) else {
        return false;
    };
    same_file(&current, &target)
}

/// На Linux «установлен» — значит лежит там, куда его кладёт пакет.
#[cfg(not(windows))]
pub fn is_installed() -> bool {
    std::env::current_exe().is_ok_and(|p| p.starts_with("/usr") || p.starts_with("/opt"))
}

#[cfg_attr(not(windows), allow(dead_code))]
fn same_file(a: &std::path::Path, b: &std::path::Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

#[cfg(windows)]
pub use self::windows::{install, is_default_browser, uninstall};

/// Делает Axiom браузером по умолчанию. `Ok(true)` — готово сразу,
/// `Ok(false)` — открыты системные параметры, выбор за пользователем.
#[cfg(windows)]
pub fn make_default() -> anyhow::Result<bool> {
    self::windows::open_default_apps()?;
    Ok(false)
}

#[cfg(target_os = "linux")]
pub use self::linux::{is_default_browser, make_default};

#[cfg(not(windows))]
pub fn install() -> anyhow::Result<PathBuf> {
    anyhow::bail!("на этой системе Axiom ставится через пакетный менеджер")
}

#[cfg(not(windows))]
pub fn uninstall() -> anyhow::Result<()> {
    anyhow::bail!("на этой системе Axiom удаляется через пакетный менеджер")
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn is_default_browser() -> bool {
    false
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn make_default() -> anyhow::Result<bool> {
    anyhow::bail!("выберите Axiom браузером по умолчанию в настройках системы")
}
