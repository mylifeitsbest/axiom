//! Установленная копия не отстаёт от более новой сборки (Windows).
//!
//! Ссылки из других программ Windows открывает установленной копией
//! (`%LOCALAPPDATA%\Programs\Axiom\axiom.exe`), а обновление на месте меняет
//! только запущенный exe. Если пользователь работает в другой копии (собранной
//! из исходников, скачанной заново), установленная тихо устаревает, и когда
//! браузер закрыт, ссылку открывает старая версия. Поэтому более новая сборка
//! при запуске кладёт себя на место установленной. Понижать версию — никогда.

use std::{
    fs,
    path::{Path, PathBuf},
    thread,
};

use anyhow::{Context, Result, anyhow};

use super::{same_file, windows};
use crate::update::{self, Version};

/// Проверяет и при необходимости обновляет установленную копию в фоновом потоке.
pub fn spawn() {
    thread::spawn(|| match refresh() {
        Ok(Some(old)) => eprintln!(
            "установленная копия обновлена: {old} → {}",
            Version::current()
        ),
        Ok(None) => {}
        Err(err) => eprintln!("установленная копия не обновлена: {err:#}"),
    });
}

/// Версия, с которой обновили установленную копию; `None` — обновлять не нужно.
fn refresh() -> Result<Option<Version>> {
    let Some(target) = target() else {
        return Ok(None);
    };
    let current = std::env::current_exe().context("не найден путь к axiom.exe")?;
    if !target.exe.is_file() || same_file(&current, &target.exe) {
        return Ok(None);
    }
    let installed = file_version(&target.exe)
        .ok_or_else(|| anyhow!("не прочитана версия {}", target.exe.display()))?;
    if installed >= Version::current() {
        return Ok(None);
    }
    replace(&current, &target.exe)?;
    // После `--uninstall` exe остаётся на месте, но регистрацию не возвращаем.
    if target.register && windows::is_registered(&target.exe) {
        windows::reregister(&target.exe)?;
    }
    Ok(Some(installed))
}

struct Target {
    exe: PathBuf,
    /// Обновить и регистрацию в реестре (не для тестовой папки).
    register: bool,
}

/// Отладочная сборка настоящую установку не трогает: только тестовую папку
/// из `AXIOM_INSTALL_DIR`, и без реестра.
fn target() -> Option<Target> {
    #[cfg(debug_assertions)]
    let target = std::env::var_os("AXIOM_INSTALL_DIR")
        .filter(|dir| !dir.is_empty())
        .map(|dir| Target {
            exe: PathBuf::from(dir).join("axiom.exe"),
            register: false,
        });
    #[cfg(not(debug_assertions))]
    let target = super::install_path().map(|exe| Target {
        exe,
        register: true,
    });
    target
}

/// Ставит копию `from` на место `to`. Сначала копия рядом под временным именем
/// (недокопированный файл уберёт `update::cleanup` установленной копии), затем
/// переименование: на месте `to` всегда целый exe. Запущенный exe заменить нельзя,
/// но можно переименовать — как при обновлении, он уезжает в `.old`.
fn replace(from: &Path, to: &Path) -> Result<()> {
    let dir = to
        .parent()
        .ok_or_else(|| anyhow!("не найдена папка установки"))?;
    let part = dir.join(format!("axiom-copy-{}.download", Version::current()));
    fs::copy(from, &part).with_context(|| format!("не удалось скопировать в {}", dir.display()))?;
    if fs::rename(&part, to).is_ok() {
        return Ok(());
    }
    // Установленная копия сейчас запущена (или антивирус ещё держит свежий файл).
    let old = update::old_path(to);
    let _ = fs::remove_file(&old);
    if let Err(err) = update::rename_retry(to, &old) {
        let _ = fs::remove_file(&part);
        return Err(err).context("не удалось освободить место для новой версии");
    }
    if let Err(err) = update::rename_retry(&part, to) {
        let _ = fs::rename(&old, to);
        let _ = fs::remove_file(&part);
        return Err(err).context("не удалось поставить новую версию");
    }
    // Не выйдет, пока старая копия работает: тогда `.old` уберёт её следующий запуск.
    let _ = fs::remove_file(&old);
    Ok(())
}

/// Версия exe из ресурса VERSIONINFO (его встраивает build.rs с версией пакета).
/// Старую копию ради этого не запускаем: она открыла бы окно на профиле.
fn file_version(exe: &Path) -> Option<Version> {
    use std::{ffi::c_void, os::windows::ffi::OsStrExt};

    /// Начало VS_FIXEDFILEINFO: остальные поля не нужны.
    #[repr(C)]
    struct FixedFileInfo {
        signature: u32,
        _struct_version: u32,
        file_version_ms: u32,
        file_version_ls: u32,
    }
    const SIGNATURE: u32 = 0xFEEF_04BD;

    #[link(name = "version")]
    unsafe extern "system" {
        fn GetFileVersionInfoSizeW(name: *const u16, handle: *mut u32) -> u32;
        fn GetFileVersionInfoW(name: *const u16, handle: u32, len: u32, data: *mut c_void) -> i32;
        fn VerQueryValueW(
            block: *const c_void,
            sub_block: *const u16,
            buffer: *mut *mut c_void,
            len: *mut u32,
        ) -> i32;
    }

    let name: Vec<u16> = exe.as_os_str().encode_wide().chain([0]).collect();
    let mut handle = 0u32;
    // SAFETY: имя — строка UTF-16 с нулём на конце, `handle` — локальная переменная.
    let size = unsafe { GetFileVersionInfoSizeW(name.as_ptr(), &mut handle) };
    if size == 0 {
        return None;
    }
    let mut data = vec![0u8; size as usize];
    // SAFETY: буфер ровно того размера, который назвала GetFileVersionInfoSizeW.
    let ok = unsafe { GetFileVersionInfoW(name.as_ptr(), 0, size, data.as_mut_ptr().cast()) };
    if ok == 0 {
        return None;
    }
    let root: Vec<u16> = "\\".encode_utf16().chain([0]).collect();
    let mut info: *mut c_void = std::ptr::null_mut();
    let mut len = 0u32;
    // SAFETY: `data` заполнен GetFileVersionInfoW; `info` укажет внутрь него.
    let ok = unsafe { VerQueryValueW(data.as_ptr().cast(), root.as_ptr(), &mut info, &mut len) };
    if ok == 0 || info.is_null() || (len as usize) < size_of::<FixedFileInfo>() {
        return None;
    }
    // SAFETY: VerQueryValueW вернул указатель на VS_FIXEDFILEINFO длиной `len` внутри
    // живого `data`; читаем без требований к выравниванию.
    let fixed = unsafe { std::ptr::read_unaligned(info.cast::<FixedFileInfo>()) };
    if fixed.signature != SIGNATURE {
        return None;
    }
    Some(Version::new(
        fixed.file_version_ms >> 16,
        fixed.file_version_ms & 0xFFFF,
        fixed.file_version_ls >> 16,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("axiom-{name}-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        dir
    }

    #[test]
    fn reads_version_resource() {
        // build.rs встраивает VERSIONINFO во все exe пакета, тестовый тоже.
        let exe = std::env::current_exe().unwrap_or_default();
        assert_eq!(file_version(&exe), Some(Version::current()));

        let dir = temp_dir("refresh-version");
        let text = dir.join("not-an-exe.exe");
        let _ = fs::write(&text, b"MZ, but not really");
        let none = file_version(&text);
        let missing = file_version(&dir.join("missing.exe"));
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(none, None);
        assert_eq!(missing, None);
    }

    #[test]
    fn replaces_file_atomically() {
        let dir = temp_dir("refresh-replace");
        let (from, to) = (dir.join("new.exe"), dir.join("axiom.exe"));
        let _ = fs::write(&from, b"new");
        let _ = fs::write(&to, b"old");
        let result = replace(&from, &to);
        let installed = fs::read(&to).unwrap_or_default();
        let mut left: Vec<String> = fs::read_dir(&dir)
            .map(|d| {
                d.flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        left.sort();
        let _ = fs::remove_dir_all(&dir);
        assert!(result.is_ok());
        assert_eq!(installed, b"new");
        // Ни временной копии, ни `.old`.
        assert_eq!(left, ["axiom.exe", "new.exe"]);
    }
}
