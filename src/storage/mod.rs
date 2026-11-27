//! Папка профиля и простые JSON-файлы: настройки, сессия, история.

pub mod app_links;
pub mod bookmarks;
pub mod history;
pub mod import;
pub mod permissions;
pub mod session;
pub mod zoom;

use std::{
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow};
use directories::ProjectDirs;
use serde::{Serialize, de::DeserializeOwned};

#[derive(Debug, Clone)]
pub struct Paths {
    /// Корень профиля: `%LOCALAPPDATA%\Axiom\data` на Windows.
    pub root: PathBuf,
}

impl Paths {
    /// `AXIOM_PROFILE=<папка>` — отдельный профиль (портативный режим, тесты).
    pub fn resolve() -> Result<Self> {
        let root = match std::env::var_os("AXIOM_PROFILE").filter(|v| !v.is_empty()) {
            Some(dir) => PathBuf::from(dir),
            None => ProjectDirs::from("", "", "Axiom")
                .ok_or_else(|| anyhow!("не найдена домашняя папка пользователя"))?
                .data_local_dir()
                .to_path_buf(),
        };
        Self::open(root)
    }

    /// Создаёт папку профиля или закрывает от чужих уже существующую.
    fn open(root: PathBuf) -> Result<Self> {
        private_dir(&root)
            .with_context(|| format!("не удалось создать папку профиля {}", root.display()))?;
        owner::close_root(&root);
        Ok(Self { root })
    }

    /// Данные webview вкладок и интерфейса (куки, кэш).
    pub fn webview_data(&self) -> PathBuf {
        self.root.join("webview")
    }
}

/// Убирает в фоне папки `ai-*`: там жила панель ИИ со своим прокси, пока он был
/// отдельным. Теперь панель в общей папке `webview`, а эти — по сотне мегабайт.
/// Не вышло (папку ещё держит уходящий WebView2) — попробуем при следующем запуске.
pub fn drop_ai_data(root: &Path) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    let old: Vec<PathBuf> = entries
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("ai-"))
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.path())
        .collect();
    if old.is_empty() {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("axiom-ai-data".into())
        .spawn(move || {
            for dir in old {
                if let Err(err) = fs::remove_dir_all(&dir) {
                    eprintln!("не удалось убрать {}: {err}", dir.display());
                }
            }
        });
}

/// Больше этого JSON из профиля не читаем. Настоящие файлы — килобайты,
/// история на 5000 адресов — около мегабайта; подложенный файл в гигабайт
/// (или ссылка на `/dev/zero`) забрал бы всю память ещё до запуска окна.
const MAX_JSON: u64 = 32 << 20;

pub fn read_json<T: DeserializeOwned>(path: &Path) -> Option<T> {
    let mut bytes = Vec::new();
    fs::File::open(path)
        .ok()?
        .take(MAX_JSON + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    let parsed = if bytes.len() as u64 > MAX_JSON {
        eprintln!(
            "{}: больше {} МБ, не читаем",
            path.display(),
            MAX_JSON >> 20
        );
        None
    } else {
        // Блокнот сохраняет UTF-8 с BOM — serde_json такое не принимает.
        let json = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes);
        serde_json::from_slice(json).ok()
    };
    if parsed.is_none() {
        // Битый или огромный файл (сбой питания, опечатка при ручной правке) иначе
        // молча заменился бы пустым при следующем сохранении. Откладываем его в сторону.
        let _ = fs::rename(path, path.with_extension("json.bad"));
    }
    parsed
}

/// Атомарная запись: сначала во временный файл, затем переименование.
pub fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    write_bytes(path, &serde_json::to_vec_pretty(value)?)
}

/// То же без отступов: для больших файлов, которые руками не правят (история).
pub fn write_json_compact<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    write_bytes(path, &serde_json::to_vec(value)?)
}

fn write_bytes(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension("json.tmp");
    // Оставшийся от сбоя `.json.tmp` (или подложенную на его месте ссылку на чужой
    // файл) убираем: удаляется сама ссылка, а не то, на что она ведёт. Новую,
    // если её успеют подложить снова, [`create_private`] не откроет.
    let _ = fs::remove_file(&tmp);
    let mut file = create_private(&tmp).with_context(|| format!("запись {}", tmp.display()))?;
    file.write_all(bytes)
        .with_context(|| format!("запись {}", tmp.display()))?;
    // Без этого после сбоя питания переименование может пережить данные,
    // и на месте файла окажется пустой или обнулённый.
    file.sync_all()
        .with_context(|| format!("запись {}", tmp.display()))?;
    drop(file);
    fs::rename(&tmp, path).with_context(|| format!("замена {}", path.display()))?;
    Ok(())
}

/// Папка в профиле, которую видит только владелец: на Unix `0700`.
/// Недостающие родительские папки создаются как обычно — их права не наше дело.
/// Уже существующую папку не трогаем (корень профиля закрывает [`Paths::resolve`]).
pub fn private_dir(path: &Path) -> io::Result<()> {
    if path.is_dir() {
        return Ok(());
    }
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    let mut builder = fs::DirBuilder::new();
    owner::dir_mode(&mut builder);
    match builder.create(path) {
        // Папку только что создал кто-то ещё (второй экземпляр браузера).
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists && path.is_dir() => Ok(()),
        result => result,
    }
}

/// Файл в профиле, который читает только владелец: на Unix `0600`.
/// Существующий файл обрезается, как у [`fs::File::create`]. Символьную ссылку
/// на Linux не открываем (`O_NOFOLLOW`): запись не уйдёт в чужой файл.
pub fn create_private(path: &Path) -> io::Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    owner::file_mode(&mut options);
    let file = options.open(path)?;
    // Режим при открытии действует только на новый файл, а этот мог остаться
    // с прежними правами (`.json.tmp` после сбоя).
    owner::fix_file(&file);
    Ok(file)
}

/// То же без обрезки: файл, который держит другой процесс (замок профиля).
pub fn open_private(path: &Path) -> io::Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    owner::file_mode(&mut options);
    let file = options.open(path)?;
    owner::fix_file(&file);
    Ok(file)
}

/// Права «только владелец». На Windows профиль лежит в `%LOCALAPPDATA%`, и его
/// и так закрывают права папки пользователя: там ничего не меняем.
///
/// На Linux куки WebKitGTK лежат в профиле открытым текстом, а домашняя папка
/// бывает открыта другим пользователям (`0755`), и профиль можно вынести
/// на общий диск (`AXIOM_PROFILE`). Поэтому папки профиля — `0700`,
/// наши файлы — `0600`. Ошибки смены прав молча пропускаем: на флешке с FAT
/// права не меняются вовсе, а профиль должен работать и там.
#[cfg(unix)]
mod owner {
    use std::{
        fs,
        os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
        path::Path,
    };

    const DIR: u32 = 0o700;
    const FILE: u32 = 0o600;
    /// Права группы и остальных.
    const OTHERS: u32 = 0o077;
    /// Sticky-бит: папка общая по замыслу, как `/tmp`.
    const STICKY: u32 = 0o1000;

    pub fn dir_mode(builder: &mut fs::DirBuilder) {
        builder.mode(DIR);
    }

    pub fn file_mode(options: &mut fs::OpenOptions) {
        options.mode(FILE);
        #[cfg(target_os = "linux")]
        options.custom_flags(libc::O_NOFOLLOW);
    }

    pub fn fix_file(file: &fs::File) {
        if file
            .metadata()
            .is_ok_and(|meta| meta.permissions().mode() & OTHERS != 0)
        {
            let _ = file.set_permissions(fs::Permissions::from_mode(FILE));
        }
    }

    /// Профиль, созданный прежними версиями (`0755`), закрываем при запуске.
    /// Только сам корень, не родителей. Общую папку со sticky-битом не трогаем,
    /// даже если `AXIOM_PROFILE` указывает прямо на неё.
    pub fn close_root(root: &Path) {
        let Ok(meta) = fs::metadata(root) else {
            return;
        };
        let mode = meta.permissions().mode();
        if meta.is_dir() && mode & OTHERS != 0 && mode & STICKY == 0 {
            let _ = fs::set_permissions(root, fs::Permissions::from_mode(DIR));
        }
    }
}

#[cfg(not(unix))]
mod owner {
    use std::{fs, path::Path};

    pub fn dir_mode(_builder: &mut fs::DirBuilder) {}

    pub fn file_mode(_options: &mut fs::OpenOptions) {}

    pub fn fix_file(_file: &fs::File) {}

    pub fn close_root(_root: &Path) {}
}

/// Текущее время в секундах Unix.
pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn huge_file_is_set_aside() {
        let root = std::env::temp_dir().join(format!("axiom-json-huge-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let file = root.join("history.json");
        let mut big = vec![b' '; (MAX_JSON + 1) as usize];
        big[0] = b'[';
        big[MAX_JSON as usize] = b']';
        fs::write(&file, &big).unwrap();
        drop(big);
        let read: Option<Vec<u8>> = read_json(&file);
        let aside = root.join("history.json.bad").exists();
        let _ = fs::remove_dir_all(&root);
        assert!(read.is_none());
        assert!(
            aside,
            "огромный файл должен уйти в .json.bad, а не пропасть"
        );
    }

    #[test]
    fn stale_tmp_is_replaced() {
        let root = std::env::temp_dir().join(format!("axiom-json-tmp-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let file = root.join("settings.json");
        fs::write(root.join("settings.json.tmp"), b"old").unwrap();
        #[cfg(unix)]
        let victim = {
            // Ссылка на месте временного файла: запись не должна уйти в чужой файл.
            let victim = root.join("victim");
            fs::write(&victim, b"keep").unwrap();
            let _ = fs::remove_file(root.join("settings.json.tmp"));
            std::os::unix::fs::symlink(&victim, root.join("settings.json.tmp")).unwrap();
            victim
        };
        write_json(&file, &vec![1, 2, 3]).unwrap();
        let back: Option<Vec<u8>> = read_json(&file);
        #[cfg(unix)]
        let untouched = fs::read(&victim).unwrap();
        let _ = fs::remove_dir_all(&root);
        assert_eq!(back, Some(vec![1, 2, 3]));
        #[cfg(unix)]
        assert_eq!(untouched, b"keep");
    }

    #[test]
    fn broken_file_is_set_aside() {
        let root = std::env::temp_dir().join(format!("axiom-json-test-{}", std::process::id()));
        let _ = fs::create_dir_all(&root);
        let file = root.join("settings.json");
        let _ = fs::write(&file, b"{\"onboarded\": tru");
        let read: Option<serde_json::Value> = read_json(&file);
        let bad = fs::read(root.join("settings.json.bad")).unwrap_or_default();
        let gone = !file.exists();
        assert!(write_json(&file, &serde_json::json!({ "a": 1 })).is_ok());
        let back: Option<serde_json::Value> = read_json(&file);
        let _ = fs::remove_dir_all(&root);
        assert!(read.is_none());
        assert!(gone);
        assert_eq!(bad, b"{\"onboarded\": tru");
        assert_eq!(back, Some(serde_json::json!({ "a": 1 })));
    }

    #[test]
    fn profile_is_created_with_parents() {
        let base = std::env::temp_dir().join(format!("axiom-private-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let root = base.join("nested").join("profile");
        let paths = Paths::open(root.clone());
        let engine = root.join("webview");
        let engine_ok = private_dir(&engine).is_ok() && private_dir(&engine).is_ok();
        let file = root.join("settings.json");
        let written = write_json(&file, &serde_json::json!({ "a": 1 })).is_ok()
            && write_json(&file, &serde_json::json!({ "a": 2 })).is_ok();
        let back: Option<serde_json::Value> = read_json(&file);
        let tmp_left = root.join("settings.json.tmp").exists();
        // На месте папки — файл: это ошибка, а не «папка уже есть».
        let blocked = root.join("blocked");
        let _ = fs::write(&blocked, b"x");
        let blocked_err = private_dir(&blocked).is_err();
        let _ = fs::remove_dir_all(&base);
        assert!(paths.is_ok_and(|p| p.root == root));
        assert!(engine_ok);
        assert!(written);
        assert_eq!(back, Some(serde_json::json!({ "a": 2 })));
        assert!(!tmp_left);
        assert!(blocked_err);
    }

    #[cfg(unix)]
    #[test]
    fn profile_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        fn mode(path: &Path) -> u32 {
            fs::metadata(path)
                .map(|m| m.permissions().mode() & 0o7777)
                .unwrap_or(0)
        }
        let base = std::env::temp_dir().join(format!("axiom-owner-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let _ = fs::create_dir_all(&base);

        // Новый профиль и всё, что пишем в него сами.
        let fresh = base.join("fresh");
        let opened = Paths::open(fresh.clone()).is_ok();
        let engine = fresh.join("webview");
        let _ = private_dir(&engine);
        let file = fresh.join("instance.json");
        let _ = write_json(&file, &serde_json::json!({ "port": 1 }));
        // `.tmp`, оставшийся от сбоя с открытыми правами, тоже закрывается.
        let stale = fresh.join("history.json.tmp");
        let _ = fs::write(&stale, b"old");
        let _ = fs::set_permissions(&stale, fs::Permissions::from_mode(0o644));
        let _ = write_json(&fresh.join("history.json"), &serde_json::json!([]));
        let fresh_modes = (
            mode(&fresh),
            mode(&engine),
            mode(&file),
            mode(&fresh.join("history.json")),
        );

        // Профиль прежних версий: корень закрывается, родитель — нет.
        let parent = base.join("shared");
        let old = parent.join("axiom");
        let _ = fs::create_dir_all(&old);
        let _ = fs::set_permissions(&parent, fs::Permissions::from_mode(0o755));
        let _ = fs::set_permissions(&old, fs::Permissions::from_mode(0o755));
        let reopened = Paths::open(old.clone()).is_ok();
        let old_modes = (mode(&parent), mode(&old));

        // Общую папку со sticky-битом (как `/tmp`) не трогаем.
        let common = base.join("common");
        let _ = fs::create_dir_all(&common);
        let _ = fs::set_permissions(&common, fs::Permissions::from_mode(0o1777));
        let common_ok = Paths::open(common.clone()).is_ok();
        let common_mode = mode(&common);

        let _ = fs::set_permissions(&common, fs::Permissions::from_mode(0o700));
        let _ = fs::remove_dir_all(&base);
        assert!(opened && reopened && common_ok);
        assert_eq!(fresh_modes, (0o700, 0o700, 0o600, 0o600));
        assert_eq!(old_modes, (0o755, 0o700));
        assert_eq!(common_mode, 0o1777);
    }
}
