//! Журнал `axiom.log` в корне профиля: всё, что Axiom пишет в stderr.
//!
//! У release-сборки на Windows консоли нет, и `eprintln!` уходил в никуда. Теперь
//! stderr процесса — файл: на Windows через `SetStdHandle` (Rust берёт дескриптор
//! заново при каждой записи), на Linux через `dup2` на дескриптор 2, так что туда
//! же попадают и предупреждения дочерних процессов WebKitGTK. Запущен из терминала
//! (Linux) — журнал идёт и на экран, а вылет по сигналу оставляет строку с его
//! именем (см. `reports::terminal`). Debug-сборка пишет в консоль, как раньше
//! (`AXIOM_LOG=1` включает файл и в ней).

use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom},
    path::Path,
};

pub const FILE: &str = "axiom.log";
const OLD: &str = "axiom.log.1";
/// Больше — при запуске журнал уходит в `axiom.log.1`, а новый начинается с нуля.
const ROTATE_AT: u64 = 1024 * 1024;

/// Писать ли stderr в файл.
pub fn to_file() -> bool {
    !cfg!(debug_assertions) || std::env::var_os("AXIOM_LOG").is_some_and(|v| v == "1")
}

/// Пока Axiom открыт, журнал может расти (поток предупреждений WebKitGTK):
/// больше этого — откладывается в `axiom.log.1` прямо на ходу.
const ROTATE_RUNNING_AT: u64 = 4 * ROTATE_AT;

/// Направляет stderr в журнал и пишет строку о запуске.
pub fn open(root: &Path) {
    let opened = !to_file() || reopen(root, ROTATE_AT);
    #[cfg(target_os = "linux")]
    super::terminal::catch_crashes();
    if !opened {
        return;
    }
    eprintln!(
        "\n=== Axiom {} · {} · {} ===",
        crate::update::current_string(),
        std::env::consts::OS,
        utc_now()
    );
    #[cfg(target_os = "linux")]
    if super::terminal::mirrored() {
        eprintln!("журнал: {}", root.join(FILE).display());
    }
}

/// Журнал слишком вырос, пока Axiom открыт: начать новый.
pub fn keep_small(root: &Path) {
    let path = root.join(FILE);
    if to_file()
        && fs::metadata(&path).is_ok_and(|m| m.len() > ROTATE_RUNNING_AT)
        && reopen(root, ROTATE_RUNNING_AT)
    {
        eprintln!("=== журнал продолжен · {} ===", utc_now());
    }
}

/// Отложить большой журнал в `axiom.log.1` и направить stderr в свежий файл.
fn reopen(root: &Path, limit: u64) -> bool {
    let path = root.join(FILE);
    if fs::metadata(&path).is_ok_and(|m| m.len() > limit) {
        // rename заменяет прежний `axiom.log.1` и на Windows; открытый файл
        // переименовать можно: Rust открывает файлы с FILE_SHARE_DELETE.
        let _ = fs::rename(&path, root.join(OLD));
    }
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .is_ok_and(redirect)
}

#[cfg(windows)]
fn redirect(file: File) -> bool {
    use std::os::windows::io::IntoRawHandle;
    // STD_ERROR_HANDLE = (DWORD)-12.
    const STD_ERROR_HANDLE: u32 = -12i32 as u32;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn SetStdHandle(which: u32, handle: *mut std::ffi::c_void) -> i32;
    }
    // Дескриптор остаётся открытым до конца процесса: им теперь владеет stderr.
    let handle = file.into_raw_handle();
    // SAFETY: handle — открытый дескриптор файла, который больше никто не закроет.
    unsafe { SetStdHandle(STD_ERROR_HANDLE, handle) != 0 }
}

#[cfg(target_os = "linux")]
fn redirect(file: File) -> bool {
    super::terminal::redirect(file)
}

#[cfg(all(unix, not(target_os = "linux")))]
fn redirect(file: File) -> bool {
    use std::os::fd::AsRawFd;
    unsafe extern "C" {
        fn dup2(old: std::ffi::c_int, new: std::ffi::c_int) -> std::ffi::c_int;
    }
    // SAFETY: оба дескриптора открыты; после dup2 файл закрывается, а копия на 2 остаётся.
    unsafe { dup2(file.as_raw_fd(), 2) != -1 }
}

#[cfg(not(any(windows, unix)))]
fn redirect(_file: File) -> bool {
    false
}

/// Последние `max` байт журнала (с началом из `axiom.log.1`, если журнал короче),
/// с целой первой строкой.
pub fn tail(root: &Path, max: u64) -> String {
    let current = tail_of(&root.join(FILE), max);
    let left = max.saturating_sub(current.len() as u64);
    if left < 1024 {
        return current;
    }
    let old = tail_of(&root.join(OLD), left);
    if old.is_empty() {
        current
    } else {
        old + &current
    }
}

fn tail_of(path: &Path, max: u64) -> String {
    let Ok(mut file) = File::open(path) else {
        return String::new();
    };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let from = len.saturating_sub(max);
    if file.seek(SeekFrom::Start(from)).is_err() {
        return String::new();
    }
    let mut bytes = Vec::new();
    if file.take(max).read_to_end(&mut bytes).is_err() {
        return String::new();
    }
    let text = String::from_utf8_lossy(&bytes);
    // Начали с середины строки — её обрывок ни к чему.
    match (from > 0, text.find('\n')) {
        (true, Some(i)) => text[i + 1..].to_string(),
        _ => text.into_owned(),
    }
}

/// `2026-11-25 19:03:12 UTC`.
pub fn utc_now() -> String {
    utc(crate::storage::now_secs())
}

pub fn utc(secs: u64) -> String {
    let (y, m, d) = crate::stats::civil((secs / 86_400) as i64);
    let s = secs % 86_400;
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02} UTC",
        s / 3600,
        s / 60 % 60,
        s % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_utc() {
        assert_eq!(utc(0), "1970-01-01 00:00:00 UTC");
        assert_eq!(utc(1_764_097_392), "2025-11-25 19:03:12 UTC");
    }

    #[test]
    fn tail_starts_at_a_whole_line_and_reaches_into_the_old_log() {
        let root = std::env::temp_dir().join(format!("axiom-journal-{}", std::process::id()));
        let _ = fs::create_dir_all(&root);
        let _ = fs::write(root.join(OLD), "old one\nold two\n".repeat(200));
        let _ = fs::write(root.join(FILE), "first line\nsecond line\nthird\n");
        let short = tail(&root, 10);
        let long = tail(&root, 4096);
        let _ = fs::remove_dir_all(&root);
        assert_eq!(short, "third\n");
        assert!(long.ends_with("first line\nsecond line\nthird\n"));
        assert!(long.starts_with("old "));
        assert!(long.len() <= 4096);
    }
}
