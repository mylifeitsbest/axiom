//! Профиль не достаётся старой сборке.
//!
//! На одной машине бывает несколько копий Axiom: установленная (её Windows
//! запускает по ссылкам из других программ), собранная из исходников, скачанная.
//! Старая копия не знает новых полей профиля и, сохраняя его, потеряла бы их.
//! Поэтому в профиле записано, какая версия открывала его последней и откуда;
//! более старая сборка отдаёт адреса ей и выходит, ничего не трогая.

use std::{
    path::{Path, PathBuf},
    process::Command,
};

use serde::{Deserialize, Serialize};

use super::same_file;
use crate::{
    storage::{Paths, read_json, write_json},
    update::Version,
};

const FILE: &str = "last-run.json";
/// Первый аргумент запуска от старой сборки: передавать адреса дальше нельзя,
/// даже если запись в профиле успела смениться (иначе копии могли бы гонять их по кругу).
pub const FLAG: &str = "--handoff";

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct LastRun {
    version: String,
    exe: PathBuf,
}

/// `true` — адреса отданы более новой версии, этому процессу пора выйти.
pub fn to_newer(paths: &Paths, urls: &[String]) -> bool {
    let Some(last) = read_json::<LastRun>(&paths.root.join(FILE)) else {
        return false;
    };
    let Ok(me) = std::env::current_exe() else {
        return false;
    };
    let Some(exe) = newer_exe(&last, Version::current(), &me) else {
        return false;
    };
    match Command::new(&exe).arg(FLAG).args(urls).spawn() {
        Ok(_) => true,
        Err(err) => {
            // Лучше открыть ссылку старой версией, чем потерять её.
            eprintln!("не удалось запустить {}: {err}", exe.display());
            false
        }
    }
}

/// Exe версии новее нашей, если он ещё на месте и это не мы сами
/// (после обновления на месте путь тот же, а версия в записи — уже наша).
fn newer_exe(last: &LastRun, current: Version, me: &Path) -> Option<PathBuf> {
    let version = Version::parse(&last.version)?;
    let exe = &last.exe;
    (version > current && exe.is_absolute() && exe.is_file() && !same_file(exe, me))
        .then(|| exe.clone())
}

/// Отмечает в профиле, что его открыла эта версия.
pub fn record(paths: &Paths) {
    // Отладочная сборка на настоящем профиле себя не отмечает: после смены номера
    // версии повседневная сборка иначе отдавала бы ссылки ей.
    if cfg!(debug_assertions) && std::env::var_os("AXIOM_PROFILE").is_none() {
        return;
    }
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    if let Err(err) = save(&paths.root.join(FILE), exe) {
        eprintln!("версия в профиле не отмечена: {err:#}");
    }
}

fn save(file: &Path, exe: PathBuf) -> anyhow::Result<()> {
    let now = LastRun {
        version: Version::current().to_string(),
        exe,
    };
    if read_json::<LastRun>(file).as_ref() == Some(&now) {
        return Ok(());
    }
    write_json(file, &now)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn last(version: &str, exe: &Path) -> LastRun {
        LastRun {
            version: version.into(),
            exe: exe.to_path_buf(),
        }
    }

    #[test]
    fn hands_off_only_to_existing_newer_exe() {
        let dir = std::env::temp_dir().join(format!("axiom-handoff-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let newer = dir.join("newer.exe");
        let me = dir.join("me.exe");
        let _ = std::fs::write(&newer, b"MZ");
        let _ = std::fs::write(&me, b"MZ");
        let v = |s: &str| Version::parse(s).unwrap_or_else(Version::current);

        let found = newer_exe(&last("0.4.1", &newer), v("0.4.0"), &me);
        let same = newer_exe(&last("0.4.0", &newer), v("0.4.0"), &me);
        let older = newer_exe(&last("0.3.9", &newer), v("0.4.0"), &me);
        let minor = newer_exe(&last("0.10.0", &newer), v("0.9.9"), &me);
        // Обновились на месте: запись указывает на нас же.
        let itself = newer_exe(&last("0.4.1", &me), v("0.4.0"), &me);
        let gone = newer_exe(&last("0.4.1", &dir.join("gone.exe")), v("0.4.0"), &me);
        let relative = newer_exe(&last("0.4.1", Path::new("newer.exe")), v("0.4.0"), &me);
        let broken = newer_exe(&last("новая", &newer), v("0.4.0"), &me);
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(found, Some(newer.clone()));
        assert_eq!(same, None);
        assert_eq!(older, None);
        assert_eq!(minor, Some(newer));
        assert_eq!(itself, None);
        assert_eq!(gone, None);
        assert_eq!(relative, None);
        assert_eq!(broken, None);
    }

    #[test]
    fn records_this_version() {
        let dir = std::env::temp_dir().join(format!("axiom-lastrun-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let me = dir.join("me.exe");
        let _ = std::fs::write(&me, b"MZ");
        let file = dir.join(FILE);
        let saved = save(&file, me.clone())
            .ok()
            .and_then(|()| read_json::<LastRun>(&file));
        let _ = std::fs::remove_dir_all(&dir);

        let saved = saved.unwrap_or_else(|| last("", Path::new("")));
        assert_eq!(saved, last(env!("CARGO_PKG_VERSION"), &me));
        // Своя же запись никуда не отправляет.
        assert_eq!(newer_exe(&saved, Version::current(), &me), None);
    }
}
