//! Профиль не достаётся старой сборке.
//!
//! На одной машине бывает несколько копий Axiom: установленная (её Windows
//! запускает по ссылкам из других программ), собранная из исходников, скачанная.
//! Старая копия не знает новых полей профиля и, сохраняя его, потеряла бы их.
//! Поэтому в профиле записано, какая версия открывала его последней и откуда;
//! более старая сборка отдаёт адреса ей и выходит, ничего не трогая. Запускаем
//! по записи только Axiom из папки установки или из нашей же папки (см. `next`).

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
    let file = paths.root.join(FILE);
    let Some(last) = read_json::<LastRun>(&file) else {
        return false;
    };
    let Ok(me) = std::env::current_exe() else {
        return false;
    };
    let exe = match next(&last, Version::current(), &me, &homes(&me)) {
        Next::Run(exe) => exe,
        Next::Stay => return false,
        Next::Distrust => {
            // Запись подсунули, или exe перенесли: работаем сами,
            // а свою запись `record` сделает при запуске.
            eprintln!(
                "{FILE}: {} — не Axiom из его папки, запись удалена",
                last.exe.display()
            );
            let _ = std::fs::remove_file(&file);
            return false;
        }
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

/// Что делать с записью о последнем запуске.
#[derive(Debug, PartialEq)]
enum Next {
    /// Профиль открывала более новая версия — отдать адреса ей.
    Run(PathBuf),
    /// Запись о нас же или о старой версии, или её exe уже нет — работаем сами.
    Stay,
    /// Версия новее, но exe лежит не там, где Axiom, или не похож на него.
    Distrust,
}

/// Откуда exe можно запустить по записи: рядом с нами (новый `axiom.exe`
/// рядом со старым `axiom-prev.exe`) и папка установки — её копию Windows
/// запускает по ссылкам из других программ, и её же обновляет более новая сборка.
fn homes(me: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = me.parent().map(Path::to_path_buf).into_iter().collect();
    #[cfg(windows)]
    dirs.extend(super::install_path().and_then(|p| p.parent().map(Path::to_path_buf)));
    // Сюда Axiom кладут пакеты .deb и pacman.
    #[cfg(not(windows))]
    dirs.push(PathBuf::from("/usr/bin"));
    dirs
}

/// Решение по записи. Запись в профиле может подменить любой, кто пишет в его
/// папку (портативный профиль на общем диске, синхронизация, чужая программа),
/// поэтому exe из неё запускаем, только если это обычный файл с именем Axiom
/// в одной из папок `homes`.
fn next(last: &LastRun, current: Version, me: &Path, homes: &[PathBuf]) -> Next {
    let Some(version) = Version::parse(&last.version) else {
        return Next::Stay;
    };
    let exe = &last.exe;
    // После обновления на месте путь тот же, а версия в записи — уже наша.
    if version <= current || !exe.is_absolute() || !exe.exists() || same_file(exe, me) {
        return Next::Stay;
    }
    let regular = std::fs::symlink_metadata(exe).is_ok_and(|m| m.file_type().is_file());
    let at_home = exe
        .parent()
        .and_then(|dir| dir.canonicalize().ok())
        .is_some_and(|dir| {
            homes
                .iter()
                .any(|home| home.canonicalize().is_ok_and(|home| home == dir))
        });
    if regular && at_home && expected_name(exe) {
        Next::Run(exe.clone())
    } else {
        Next::Distrust
    }
}

/// Имена, под которыми лежит Axiom: `axiom.exe` (установка, сборка), файл со
/// страницы релиза и пакет на Linux. Браузер, сохраняя файл второй раз,
/// добавляет к имени « (1)».
fn expected_name(exe: &Path) -> bool {
    const NAMES: &[&str] = &["axiom", "axiom-windows-x64", "axiom-browser"];
    let suffix = if cfg!(windows) { ".exe" } else { "" };
    let Some(name) = exe.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    let name = name.to_ascii_lowercase();
    let Some(stem) = name.strip_suffix(suffix) else {
        return false;
    };
    let copy = stem.rsplit_once(" (").and_then(|(base, n)| {
        let digits = n.strip_suffix(')')?;
        (!digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())).then_some(base)
    });
    NAMES.contains(&copy.unwrap_or(stem))
}

/// Отмечает в профиле, что его открыла эта версия. Возвращает версию,
/// которая открывала профиль до неё, если та старее: Axiom обновился.
pub fn record(paths: &Paths) -> Option<Version> {
    // Отладочная сборка на настоящем профиле себя не отмечает: после смены номера
    // версии повседневная сборка иначе отдавала бы ссылки ей.
    if cfg!(debug_assertions) && std::env::var_os("AXIOM_PROFILE").is_none() {
        return None;
    }
    let exe = std::env::current_exe().ok()?;
    let file = paths.root.join(FILE);
    let before = read_json::<LastRun>(&file);
    if let Err(err) = save(&file, before.as_ref(), exe) {
        eprintln!("версия в профиле не отмечена: {err:#}");
    }
    before
        .and_then(|last| Version::parse(&last.version))
        .filter(|&v| v < Version::current())
}

fn save(file: &Path, before: Option<&LastRun>, exe: PathBuf) -> anyhow::Result<()> {
    let now = LastRun {
        version: Version::current().to_string(),
        exe,
    };
    if before == Some(&now) {
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

    /// `axiom`, а на Windows — `axiom.exe`.
    fn exe(stem: &str) -> String {
        if cfg!(windows) {
            format!("{stem}.exe")
        } else {
            stem.to_string()
        }
    }

    /// Временные папки: `home` — где лежим мы сами, `away` — где-то ещё.
    struct Dirs {
        root: PathBuf,
        home: PathBuf,
        away: PathBuf,
        me: PathBuf,
    }

    impl Dirs {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!("axiom-{name}-{}", std::process::id()));
            let (home, away) = (root.join("home"), root.join("away"));
            let _ = std::fs::create_dir_all(&home);
            let _ = std::fs::create_dir_all(&away);
            let me = home.join(exe("axiom-prev"));
            let _ = std::fs::write(&me, b"MZ");
            Self {
                root,
                home,
                away,
                me,
            }
        }

        fn file(&self, dir: &Path, name: &str) -> PathBuf {
            let path = dir.join(name);
            let _ = std::fs::write(&path, b"MZ");
            path
        }

        /// Решение для записи `version`/`exe`, когда мы — версия `current`.
        fn next(&self, version: &str, exe: &Path, current: &str) -> Next {
            let current = Version::parse(current).unwrap_or_else(Version::current);
            next(&last(version, exe), current, &self.me, &homes(&self.me))
        }
    }

    impl Drop for Dirs {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn hands_off_only_to_existing_newer_exe() {
        let d = Dirs::new("handoff-test");
        let newer = d.file(&d.home, &exe("axiom"));

        assert_eq!(d.next("0.4.1", &newer, "0.4.0"), Next::Run(newer.clone()));
        assert_eq!(d.next("0.10.0", &newer, "0.9.9"), Next::Run(newer.clone()));
        assert_eq!(d.next("0.4.0", &newer, "0.4.0"), Next::Stay);
        assert_eq!(d.next("0.3.9", &newer, "0.4.0"), Next::Stay);
        // Обновились на месте: запись указывает на нас же.
        assert_eq!(d.next("0.4.1", &d.me, "0.4.0"), Next::Stay);
        let gone = d.home.join(exe("axiom-browser"));
        assert_eq!(d.next("0.4.1", &gone, "0.4.0"), Next::Stay);
        let relative = PathBuf::from(exe("axiom"));
        assert_eq!(d.next("0.4.1", &relative, "0.4.0"), Next::Stay);
        assert_eq!(d.next("новая", &newer, "0.4.0"), Next::Stay);
    }

    #[test]
    fn runs_only_axiom_from_its_folders() {
        let d = Dirs::new("handoff-trust");
        // Файл со страницы релиза, скачанный второй раз.
        let release = d.file(&d.home, &exe("Axiom-windows-x64 (2)"));
        assert_eq!(d.next("0.4.1", &release, "0.4.0"), Next::Run(release));
        // Чужая программа рядом с нами, Axiom где-то ещё и похожие имена.
        let setup = d.file(&d.home, &exe("setup"));
        let away = d.file(&d.away, &exe("axiom"));
        let lookalike = d.file(&d.home, &exe("axiom (x)"));
        let script = d.file(&d.home, "axiom.cmd");
        for exe in [setup, away, lookalike, script] {
            assert_eq!(d.next("0.4.1", &exe, "0.4.0"), Next::Distrust, "{exe:?}");
        }
        // Папка с именем программы — не программа.
        let dir = d.home.join(exe("axiom-windows-x64"));
        let _ = std::fs::create_dir_all(&dir);
        assert_eq!(d.next("0.4.1", &dir, "0.4.0"), Next::Distrust);
    }

    #[test]
    fn axiom_file_names() {
        for name in [
            "axiom",
            "Axiom",
            "AXIOM-windows-x64",
            "axiom-browser",
            "axiom (1)",
        ] {
            assert!(expected_name(Path::new(&exe(name))), "{name}");
        }
        for name in ["axiom-test", "cmd", "axiom ()", "axiom (1a)", "axiomx", ""] {
            assert!(!expected_name(Path::new(&exe(name))), "{name}");
        }
        if cfg!(windows) {
            assert!(!expected_name(Path::new("axiom")));
            assert!(!expected_name(Path::new("axiom.exe.old")));
        }
    }

    #[test]
    fn records_this_version() {
        let dir = std::env::temp_dir().join(format!("axiom-lastrun-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let me = dir.join("me.exe");
        let _ = std::fs::write(&me, b"MZ");
        let file = dir.join(FILE);
        let saved = save(&file, None, me.clone())
            .ok()
            .and_then(|()| read_json::<LastRun>(&file));
        let _ = std::fs::remove_dir_all(&dir);

        let saved = saved.unwrap_or_else(|| last("", Path::new("")));
        assert_eq!(saved, last(&Version::current().to_string(), &me));
        // Своя же запись никуда не отправляет.
        assert_eq!(
            next(&saved, Version::current(), &me, &homes(&me)),
            Next::Stay
        );
    }
}
