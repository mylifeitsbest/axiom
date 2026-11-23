//! Сколько физической памяти осталось у системы.
//!
//! Когда памяти мало, Windows начинает сжимать и выгружать страницы на диск,
//! и тормозит всё подряд — даже видео в активной вкладке. В такой момент
//! фоновые вкладки лучше усыпить сразу, не дожидаясь обычного таймера.

const GB: u64 = 1024 * 1024 * 1024;

/// Памяти не хватает: свободно меньше 15% (порог не ниже 1 ГБ и не выше 3 ГБ).
pub fn is_low() -> bool {
    physical().is_some_and(|(available, total)| available < threshold(total))
}

fn threshold(total: u64) -> u64 {
    (total * 15 / 100).clamp(GB, 3 * GB)
}

/// Доступная и общая физическая память в байтах.
#[cfg(windows)]
fn physical() -> Option<(u64, u64)> {
    #[repr(C)]
    struct MemoryStatusEx {
        length: u32,
        memory_load: u32,
        total_phys: u64,
        avail_phys: u64,
        total_page_file: u64,
        avail_page_file: u64,
        total_virtual: u64,
        avail_virtual: u64,
        avail_extended_virtual: u64,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GlobalMemoryStatusEx(buffer: *mut MemoryStatusEx) -> i32;
    }

    let mut status = MemoryStatusEx {
        length: size_of::<MemoryStatusEx>() as u32,
        memory_load: 0,
        total_phys: 0,
        avail_phys: 0,
        total_page_file: 0,
        avail_page_file: 0,
        total_virtual: 0,
        avail_virtual: 0,
        avail_extended_virtual: 0,
    };
    // SAFETY: структура совпадает с MEMORYSTATUSEX, размер записан в `length`.
    let ok = unsafe { GlobalMemoryStatusEx(&mut status) } != 0;
    (ok && status.total_phys > 0).then_some((status.avail_phys, status.total_phys))
}

#[cfg(target_os = "linux")]
fn physical() -> Option<(u64, u64)> {
    parse_meminfo(&std::fs::read_to_string("/proc/meminfo").ok()?)
}

#[cfg(not(any(windows, target_os = "linux")))]
fn physical() -> Option<(u64, u64)> {
    None
}

/// `MemAvailable` и `MemTotal` из `/proc/meminfo` (там они в КиБ).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parse_meminfo(text: &str) -> Option<(u64, u64)> {
    let field = |name: &str| -> Option<u64> {
        let line = text.lines().find(|l| l.starts_with(name))?;
        let kib: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
        Some(kib * 1024)
    };
    let total = field("MemTotal:").filter(|&t| t > 0)?;
    Some((field("MemAvailable:")?, total))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn threshold_is_fifteen_percent_within_bounds() {
        assert_eq!(threshold(4 * GB), GB);
        assert_eq!(threshold(16 * GB), 16 * GB * 15 / 100);
        assert_eq!(threshold(64 * GB), 3 * GB);
    }

    #[test]
    fn meminfo_parsing() {
        let text = "MemTotal:       16303456 kB\nMemFree:          812344 kB\n\
                    MemAvailable:    2516780 kB\nBuffers:          123 kB\n";
        assert_eq!(
            parse_meminfo(text),
            Some((2_516_780 * 1024, 16_303_456 * 1024))
        );
        assert_eq!(parse_meminfo("MemTotal: 100 kB\n"), None);
        assert_eq!(parse_meminfo("garbage"), None);
    }

    #[test]
    fn reads_this_machine() {
        if cfg!(any(windows, target_os = "linux")) {
            let (available, total) = physical().unwrap_or_default();
            assert!(total > 0 && available <= total);
        }
    }
}
