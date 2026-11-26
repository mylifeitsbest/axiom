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
//!
//! WebKitGTK 2.46 и новее без DMA-BUF не рисует страницы видеокартой вовсе: другого
//! способа передать кадр в окно у него нет (`AcceleratedBackingStore::checkRequirements`),
//! так что обе тяжёлые ступени там — одно и то же. Видео тогда идёт не через GL, а
//! запасным выводом (`webkitvideosink`): каждый кадр — в обычной памяти. Аппаратные
//! декодеры NVIDIA (nvcodec) и VA-API (на NVIDIA — через nvidia-vaapi-driver) в песочнице
//! WebKit без GL — частая причина вылетов на видео, а выигрыша от них здесь почти нет:
//! кадр всё равно копируется в память. Поэтому в тяжёлых режимах на NVIDIA видео
//! декодирует процессор: ранг этих декодеров в GStreamer — NONE (`GST_PLUGIN_FEATURE_RANK`),
//! и WebKit их не выбирает. Сайт видит, какие кодеки остались, и берёт их.
//!
//! Тяжёлый режим, с которым Axiom [`CRASH_LIMIT`] раз подряд закрылся аварийно,
//! сам возвращается в «Авто» (см. [`guard`]).

use std::sync::atomic::{AtomicBool, Ordering};

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

impl Gpu {
    /// Без DMA-BUF или без ускорения: страницы рисует процессор.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn heavy(self) -> bool {
        matches!(self, Self::NoDmabuf | Self::NoCompositing)
    }
}

/// Столько аварийных закрытий подряд в тяжёлом режиме — и следующий запуск идёт в «Авто».
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub const CRASH_LIMIT: u32 = 3;

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const EXPLICIT_SYNC: &str = "__NV_DISABLE_EXPLICIT_SYNC";
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const DMABUF: &str = "WEBKIT_DISABLE_DMABUF_RENDERER";
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const COMPOSITING: &str = "WEBKIT_DISABLE_COMPOSITING_MODE";
/// Ранги элементов GStreamer: `имя:ранг` через запятую (GStreamer 1.18+). Имени,
/// которого нет в системе, GStreamer просто не находит.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const RANKS: &str = "GST_PLUGIN_FEATURE_RANK";
/// Аппаратные декодеры видео: NVDEC из nvcodec и VA-API из va (gst-plugins-bad 1.26).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const NO_HW_DECODERS: &str = "nvh264dec:NONE,nvh265dec:NONE,nvvp8dec:NONE,nvvp9dec:NONE,\
nvav1dec:NONE,vah264dec:NONE,vah265dec:NONE,vavp8dec:NONE,vavp9dec:NONE,vaav1dec:NONE,\
vavp8alphadecodebin:NONE,vavp9alphadecodebin:NONE";

/// Какие переменные поставить и в какие значения. Явную синхронизацию отключаем только
/// там, где она есть: NVIDIA под Wayland. Ступени тяжелее человек выбрал сам — для любой
/// карты; декодирование процессором к ним — только на NVIDIA.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn plan(mode: Gpu, nvidia: bool, wayland: bool) -> Vec<(&'static str, &'static str)> {
    let mut vars = Vec::new();
    if mode == Gpu::Off {
        return vars;
    }
    if nvidia && wayland {
        vars.push((EXPLICIT_SYNC, "1"));
    }
    if mode.heavy() {
        vars.push((DMABUF, "1"));
    }
    if mode == Gpu::NoCompositing {
        vars.push((COMPOSITING, "1"));
    }
    if mode.heavy() && nvidia {
        vars.push((RANKS, NO_HW_DECODERS));
    }
    vars
}

/// Режим для этого запуска: тяжёлый режим, с которым Axiom `crashes` раз подряд
/// закрылся аварийно, уступает «Авто».
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn guard(mode: Gpu, crashes: u32) -> Gpu {
    if mode.heavy() && crashes >= CRASH_LIMIT {
        Gpu::Auto
    } else {
        mode
    }
}

/// Переменные, которые человек задал сам и которые влияют на отрисовку: для журнала.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn own_vars(vars: impl Iterator<Item = (String, String)>, ours: &[&str]) -> Vec<String> {
    const PREFIXES: [&str; 11] = [
        "WEBKIT_", "GST_", "__NV_", "__GL_", "__GLX_", "__EGL_", "GDK_", "GSK_", "LIBGL_", "MESA_",
        "GBM_",
    ];
    let mut own: Vec<String> = vars
        .filter(|(name, _)| PREFIXES.iter().any(|p| name.starts_with(p)))
        .filter(|(name, _)| !ours.contains(&name.as_str()))
        .map(|(name, value)| format!("{name}={value}"))
        .collect();
    own.sort();
    own
}

/// Этот запуск сбросил тяжёлый режим: интерфейс скажет об этом один раз.
static RESET: AtomicBool = AtomicBool::new(false);

/// Текст всплывающей подсказки, если этот запуск сбросил тяжёлый режим (один раз).
pub fn reset_notice() -> Option<&'static str> {
    RESET.swap(false, Ordering::Relaxed).then_some(
        "Axiom несколько раз подряд закрылся аварийно, и отрисовка страниц вернулась \
         в «Авто». Сменить её можно в настройках: «Система» → «Отрисовка страниц»",
    )
}

/// Чем закончился выбор режима при запуске.
#[cfg(target_os = "linux")]
pub struct Start {
    /// С каким режимом идёт этот запуск.
    pub mode: Gpu,
    /// Что выбрано в настройках: тяжёлый режим после вылетов подряд уступает «Авто».
    chosen: Gpu,
    /// Строка для журнала: его ещё нет, запишем, когда откроется.
    line: String,
}

/// Ставит переменные для режима из настроек (или «Авто», если с ним Axiom `crashes`
/// раз подряд упал). Вызывается из `main` до GTK и до первых потоков: окружение в этот
/// момент никто не читает.
#[cfg(target_os = "linux")]
pub fn apply(chosen: Gpu, crashes: u32) -> Start {
    use std::path::Path;
    let mode = guard(chosen, crashes);
    let nvidia = Path::new("/proc/driver/nvidia/version").exists()
        || Path::new("/sys/module/nvidia").exists();
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty())
        || std::env::var("XDG_SESSION_TYPE").is_ok_and(|v| v == "wayland");
    let mut set = Vec::new();
    for (var, value) in plan(mode, nvidia, wayland) {
        if std::env::var_os(var).is_none() {
            // SAFETY: начало `main`, других потоков ещё нет — окружение никто не читает.
            unsafe { std::env::set_var(var, value) };
            set.push(var);
        }
    }
    // `vars_os`: `vars()` паникует на значении не в UTF-8.
    let env = std::env::vars_os().map(|(name, value)| {
        let name = name.to_string_lossy().into_owned();
        (name, value.to_string_lossy().into_owned())
    });
    let own = own_vars(env, &set);
    // По отчёту видно, с какими обходами работала копия.
    let mut line =
        format!("отрисовка: {mode:?}, nvidia={nvidia}, wayland={wayland}, заданы: {set:?}");
    if !own.is_empty() {
        line.push_str(&format!(", свои: {own:?}"));
    }
    Start { mode, chosen, line }
}

#[cfg(target_os = "linux")]
impl Start {
    /// Журнал открыт: записать строку о режиме, а если режим сброшен — запомнить
    /// «Авто» в настройках и сказать человеку, когда интерфейс будет готов.
    pub fn announce(&self, paths: &crate::storage::Paths) {
        eprintln!("{}", self.line);
        if self.mode == self.chosen {
            return;
        }
        eprintln!(
            "отрисовка: Axiom {CRASH_LIMIT} раза подряд закрылся аварийно в режиме {:?} — \
             режим сброшен в «Авто»",
            self.chosen
        );
        let mut settings = crate::settings::Settings::load(paths);
        settings.gpu = Gpu::Auto;
        if let Err(err) = settings.save(paths) {
            eprintln!("отрисовка: «Авто» не записано в настройки: {err:#}");
        }
        RESET.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(vars: Vec<(&'static str, &'static str)>) -> Vec<&'static str> {
        vars.into_iter().map(|(name, _)| name).collect()
    }

    #[test]
    fn steps_from_light_to_heavy() {
        assert_eq!(names(plan(Gpu::Auto, true, true)), [EXPLICIT_SYNC]);
        assert!(plan(Gpu::Auto, true, false).is_empty());
        assert!(plan(Gpu::Auto, false, true).is_empty());
        assert_eq!(
            names(plan(Gpu::NoDmabuf, true, true)),
            [EXPLICIT_SYNC, DMABUF, RANKS]
        );
        assert_eq!(names(plan(Gpu::NoDmabuf, false, false)), [DMABUF]);
        assert_eq!(
            names(plan(Gpu::NoCompositing, true, true)),
            [EXPLICIT_SYNC, DMABUF, COMPOSITING, RANKS]
        );
        assert_eq!(
            names(plan(Gpu::NoCompositing, false, true)),
            [DMABUF, COMPOSITING]
        );
        assert!(plan(Gpu::Off, true, true).is_empty());
    }

    #[test]
    fn heavy_modes_on_nvidia_decode_video_on_the_cpu() {
        let ranks = plan(Gpu::NoDmabuf, true, false)
            .into_iter()
            .find(|(name, _)| *name == RANKS)
            .map(|(_, value)| value)
            .unwrap_or_default();
        // GStreamer разбирает `имя:ранг` через запятую, без пробелов вокруг.
        let pairs: Vec<&str> = ranks.split(',').collect();
        assert!(pairs.contains(&"nvh264dec:NONE") && pairs.contains(&"vaav1dec:NONE"));
        assert!(pairs.iter().all(|p| {
            p.split_once(':').is_some_and(|(name, rank)| {
                !name.is_empty() && !name.contains(' ') && rank == "NONE"
            })
        }));
        // Остальные значения — просто «1».
        assert!(
            plan(Gpu::NoCompositing, true, true)
                .iter()
                .all(|(name, value)| *name == RANKS || *value == "1")
        );
        // «Авто» видео не трогает.
        assert!(
            plan(Gpu::Auto, true, true)
                .iter()
                .all(|(name, _)| *name != RANKS)
        );
    }

    #[test]
    fn crash_loop_falls_back_to_auto() {
        assert_eq!(guard(Gpu::NoDmabuf, 0), Gpu::NoDmabuf);
        assert_eq!(guard(Gpu::NoDmabuf, CRASH_LIMIT - 1), Gpu::NoDmabuf);
        assert_eq!(guard(Gpu::NoDmabuf, CRASH_LIMIT), Gpu::Auto);
        assert_eq!(guard(Gpu::NoCompositing, CRASH_LIMIT + 5), Gpu::Auto);
        // Лёгкие режимы вылетами не сбрасываются.
        assert_eq!(guard(Gpu::Off, CRASH_LIMIT), Gpu::Off);
        assert_eq!(guard(Gpu::Auto, CRASH_LIMIT), Gpu::Auto);
    }

    #[test]
    fn own_vars_are_the_graphics_ones_we_did_not_set() {
        let env = [
            ("WEBKIT_DISABLE_DMABUF_RENDERER", "1"),
            ("GST_PLUGIN_FEATURE_RANK", "nvh264dec:MAX"),
            ("__NV_DISABLE_EXPLICIT_SYNC", "1"),
            ("HOME", "/home/someone"),
            ("PATH", "/usr/bin"),
            ("GDK_BACKEND", "x11"),
        ]
        .map(|(n, v)| (n.to_string(), v.to_string()));
        assert_eq!(
            own_vars(env.into_iter(), &[EXPLICIT_SYNC]),
            [
                "GDK_BACKEND=x11",
                "GST_PLUGIN_FEATURE_RANK=nvh264dec:MAX",
                "WEBKIT_DISABLE_DMABUF_RENDERER=1"
            ]
        );
    }

    #[test]
    fn reset_notice_is_shown_once() {
        RESET.store(true, Ordering::Relaxed);
        assert!(reset_notice().is_some_and(|t| t.contains("«Авто»")));
        assert!(reset_notice().is_none());
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
