//! Установка в систему: копия exe в профиль пользователя, ярлык в «Пуске»
//! и регистрация как браузера. Всё в HKCU — права администратора не нужны.
//!
//! Сделать Axiom браузером по умолчанию программа сама не может: Windows
//! разрешает это только пользователю, поэтому мы открываем нужную страницу параметров.

#[cfg(windows)]
mod windows;

use std::path::PathBuf;

use serde::Serialize;

/// Что интерфейс показывает на экране установки.
#[derive(Debug, Clone, Serialize)]
pub struct Status {
    pub installed: bool,
    pub default_browser: bool,
    pub chrome_found: bool,
    pub can_install: bool,
    pub version: &'static str,
}

pub fn status() -> Status {
    Status {
        installed: is_installed(),
        default_browser: is_default_browser(),
        chrome_found: crate::storage::chrome::is_available(),
        can_install: cfg!(windows),
        version: env!("CARGO_PKG_VERSION"),
    }
}

/// `%LOCALAPPDATA%\Programs\Axiom\axiom.exe`
pub fn install_path() -> Option<PathBuf> {
    let base = directories::BaseDirs::new()?;
    Some(
        base.data_local_dir()
            .join("Programs")
            .join("Axiom")
            .join("axiom.exe"),
    )
}

pub fn is_installed() -> bool {
    let (Ok(current), Some(target)) = (std::env::current_exe(), install_path()) else {
        return false;
    };
    same_file(&current, &target)
}

fn same_file(a: &std::path::Path, b: &std::path::Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

#[cfg(windows)]
pub use self::windows::{install, is_default_browser, open_default_apps, uninstall};

#[cfg(not(windows))]
pub fn install() -> anyhow::Result<PathBuf> {
    anyhow::bail!("установка пока поддерживается только на Windows")
}

#[cfg(not(windows))]
pub fn uninstall() -> anyhow::Result<()> {
    Ok(())
}

#[cfg(not(windows))]
pub fn is_default_browser() -> bool {
    false
}

#[cfg(not(windows))]
pub fn open_default_apps() -> anyhow::Result<()> {
    anyhow::bail!("откройте системные настройки браузера по умолчанию вручную")
}
