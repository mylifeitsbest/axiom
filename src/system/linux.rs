//! Браузер по умолчанию на Linux через `xdg-settings` (пакет xdg-utils).

use std::process::Command;

use anyhow::{Context, Result, bail};

/// Имя файла, который кладут пакеты: `/usr/share/applications/axiom.desktop`.
const DESKTOP: &str = "axiom.desktop";

pub fn is_default_browser() -> bool {
    Command::new("xdg-settings")
        .args(["get", "default-web-browser"])
        .output()
        .is_ok_and(|out| String::from_utf8_lossy(&out.stdout).trim() == DESKTOP)
}

pub fn make_default() -> Result<bool> {
    let status = Command::new("xdg-settings")
        .args(["set", "default-web-browser", DESKTOP])
        .status()
        .context("не найден xdg-settings — установите пакет xdg-utils")?;
    if !status.success() {
        bail!("xdg-settings не смог сменить браузер по умолчанию");
    }
    Ok(true)
}
