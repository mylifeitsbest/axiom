//! Linux: обходы WebKitGTK для видеокарт NVIDIA.
//!
//! На NVIDIA под Wayland (Hyprland, GNOME, KDE) WebKitGTK мигает чёрным, когда окно
//! двигают или меняют его размер, а иногда окно молча закрывается (ошибка протокола
//! Wayland 71): драйвер с явной синхронизацией кадров (explicit sync) не ладит с выводом
//! WebKit через DMA-BUF. Переменные окружения, которые это обходят, читают драйвер и
//! процессы WebKit при старте, поэтому ставим их первым делом, до GTK и до потоков.
//!
//! Ступени — от лёгкой к тяжёлой (так же их советует документация Tauri, у которого
//! тот же движок): без явной синхронизации (скорость не страдает) → без DMA-BUF
//! (мигание уходит почти везде, но видео и прокрутка тяжелее) → без аппаратного
//! композитинга страниц (крайний случай). Выбирают в настройках: «Система» →
//! «Отрисовка страниц». Переменную, заданную человеком самим, не трогаем.

use serde::{Deserialize, Serialize};

/// Режим отрисовки страниц на Linux. В настройках — строкой.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Gpu {
    /// Ещё и без DMA-BUF.
    NoDmabuf,
    /// Ещё и без аппаратного композитинга страниц.
    NoCompositing,
    /// Ничего не менять.
    Off,
    /// На NVIDIA под Wayland — без явной синхронизации, остальное как есть.
    /// Неизвестное значение из файла настроек — тоже «авто» (serde требует его последним).
    #[default]
    #[serde(other)]
    Auto,
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const EXPLICIT_SYNC: &str = "__NV_DISABLE_EXPLICIT_SYNC";
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const DMABUF: &str = "WEBKIT_DISABLE_DMABUF_RENDERER";
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const COMPOSITING: &str = "WEBKIT_DISABLE_COMPOSITING_MODE";

/// Какие переменные поставить в `1`. Явную синхронизацию отключаем только там, где
/// она есть: NVIDIA под Wayland. Ступени тяжелее человек выбрал сам — для любой карты.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn plan(mode: Gpu, nvidia: bool, wayland: bool) -> Vec<&'static str> {
    let mut vars = Vec::new();
    if mode == Gpu::Off {
        return vars;
    }
    if nvidia && wayland {
        vars.push(EXPLICIT_SYNC);
    }
    if matches!(mode, Gpu::NoDmabuf | Gpu::NoCompositing) {
        vars.push(DMABUF);
    }
    if mode == Gpu::NoCompositing {
        vars.push(COMPOSITING);
    }
    vars
}

/// Ставит переменные для выбранного режима. Вызывается из `main` до GTK и до
/// первых потоков: окружение в этот момент никто не читает. Возвращает строку для
/// журнала: его ещё нет, `main` запишет её, когда журнал откроется.
#[cfg(target_os = "linux")]
pub fn apply(mode: Gpu) -> String {
    use std::path::Path;
    let nvidia = Path::new("/proc/driver/nvidia/version").exists()
        || Path::new("/sys/module/nvidia").exists();
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty())
        || std::env::var("XDG_SESSION_TYPE").is_ok_and(|v| v == "wayland");
    let mut set = Vec::new();
    for var in plan(mode, nvidia, wayland) {
        if std::env::var_os(var).is_none() {
            // SAFETY: начало `main`, других потоков ещё нет — окружение никто не читает.
            unsafe { std::env::set_var(var, "1") };
            set.push(var);
        }
    }
    // По отчёту видно, с какими обходами работала копия.
    format!("отрисовка: {mode:?}, nvidia={nvidia}, wayland={wayland}, заданы: {set:?}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_from_light_to_heavy() {
        assert_eq!(plan(Gpu::Auto, true, true), [EXPLICIT_SYNC]);
        assert!(plan(Gpu::Auto, true, false).is_empty());
        assert!(plan(Gpu::Auto, false, true).is_empty());
        assert_eq!(plan(Gpu::NoDmabuf, true, true), [EXPLICIT_SYNC, DMABUF]);
        assert_eq!(plan(Gpu::NoDmabuf, false, false), [DMABUF]);
        assert_eq!(
            plan(Gpu::NoCompositing, true, true),
            [EXPLICIT_SYNC, DMABUF, COMPOSITING]
        );
        assert!(plan(Gpu::Off, true, true).is_empty());
    }

    #[test]
    fn unknown_mode_is_auto() {
        let mode: Gpu = serde_json::from_str("\"no_dmabuf\"").unwrap_or_default();
        assert_eq!(mode, Gpu::NoDmabuf);
        let mode: Gpu = serde_json::from_str("\"turbo\"").unwrap_or(Gpu::Off);
        assert_eq!(mode, Gpu::Auto);
        assert_eq!(
            serde_json::to_string(&Gpu::NoCompositing).unwrap_or_default(),
            "\"no_compositing\""
        );
    }
}
