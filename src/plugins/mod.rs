//! Плагины: свои скрипты и стили для страниц, которые пишет кто угодно.
//!
//! Плагин — папка в `<профиль>/plugins/` с `plugin.json` (см. `manifest`) и файлами.
//! Его скрипты выполняются во вкладках в изолированном мире: у плагина свой
//! `window`, сайт его не видит и не может вызвать API плагина (`axiom.*`, см.
//! `runtime.js`), а плагин не может вызвать IPC интерфейса браузера. Что плагину
//! можно сверх страницы, решают разрешения из манифеста; проверяет их Rust
//! (`App::on_plugin`), а не страница.
//!
//! Найденный плагин выключен, пока его не включат в «Настройки → Плагины»:
//! подложенная в папку программа сама ничего не запустит.
//!
//! Документация для авторов — `docs/plugins.md`, примеры — `examples/plugins/` и
//! плагины Axiom из магазина (`store/plugins/`, магазин — `market` и `server/store.py`).

pub mod api;
mod bootstrap;
pub mod bridge;
pub mod manifest;
pub mod market;
pub mod pattern;
mod store;

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs,
    io::Read,
    path::{Path, PathBuf},
    rc::Rc,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub use bridge::Message;
use manifest::{Field, Manifest, Permission, RunAt};
use pattern::Pattern;

use crate::storage;

/// Папка плагинов в профиле.
const DIR: &str = "plugins";
/// Хранилища плагинов (`axiom.storage`).
const DATA_DIR: &str = "plugin-data";
/// Что включено и значения настроек.
const STATE: &str = "plugins.json";
/// Больше плагинов в папке не читаем.
const MAX_PLUGINS: usize = 100;
/// Один файл кода или стилей.
const MAX_FILE: u64 = 2 << 20;
/// Весь код и стили одного плагина.
const MAX_CODE: u64 = 5 << 20;
/// Значок плагина.
const MAX_ICON: u64 = 128 << 10;

/// Скрипт плагина, готовый к установке во вкладку.
#[derive(Debug)]
pub struct Script {
    pub plugin: String,
    /// Имя изолированного мира: в DevTools это строка в списке контекстов консоли.
    pub world: String,
    /// Имя функции-моста в мире (Windows) или обработчика сообщений (Linux).
    pub binding: String,
    pub source: String,
    /// Есть скрипты и для фреймов страницы (Linux ставит скрипт и во фреймы).
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub all_frames: bool,
    /// Шаблоны главной страницы: подключать ли к ней канал (Windows).
    #[cfg_attr(not(any(windows, test)), allow(dead_code))]
    matches: Vec<(Vec<Pattern>, Vec<Pattern>)>,
}

impl Script {
    /// Плагин работает на этой странице (в ней самой, не во фрейме).
    #[cfg_attr(not(any(windows, test)), allow(dead_code))]
    pub fn wants(&self, url: &str) -> bool {
        self.matches
            .iter()
            .any(|(m, x)| m.iter().any(|p| p.matches(url)) && !x.iter().any(|p| p.matches(url)))
    }
}

/// Загруженный плагин: манифест и прочитанные файлы. Код и стили читаются только
/// у включённых плагинов: выключенный, сколько бы он ни весил, памяти не занимает.
#[derive(Debug)]
pub struct Plugin {
    pub folder: String,
    dir: PathBuf,
    pub manifest: Manifest,
    /// Код и стили прочитаны (в `scripts`), а не только проверены.
    code: bool,
    scripts: Vec<Loaded>,
    icon: Option<String>,
    action_icon: Option<String>,
    /// Поставлен из магазина: версия по его отметке (`market::MARKER`).
    pub from_store: Option<String>,
}

#[derive(Debug)]
struct Loaded {
    matches: Vec<Pattern>,
    exclude: Vec<Pattern>,
    js: String,
    css: String,
    run_at: RunAt,
    all_frames: bool,
}

/// Папка, из которой плагин не загрузился, и почему.
#[derive(Debug)]
struct Broken {
    folder: String,
    error: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct State {
    enabled: BTreeSet<String>,
    values: BTreeMap<String, Map<String, Value>>,
}

pub struct Registry {
    root: PathBuf,
    plugins: Vec<Plugin>,
    broken: Vec<Broken>,
    state: State,
    stores: HashMap<String, store::Store>,
    /// Скрипты включённых плагинов для вкладок (пересобираются при каждой перемене).
    scripts: Rc<[Script]>,
    /// Последние сообщения и вкладки каждого плагина: против бесконечных циклов.
    limits: HashMap<String, Limits>,
}

#[derive(Default)]
struct Limits {
    notify: Vec<Instant>,
    tabs: Vec<Instant>,
}

impl Registry {
    pub fn load(paths: &storage::Paths) -> Self {
        let root = paths.root.clone();
        let state = storage::read_json(&root.join(STATE)).unwrap_or_default();
        let mut registry = Self {
            root,
            plugins: Vec::new(),
            broken: Vec::new(),
            state,
            stores: HashMap::new(),
            scripts: Rc::from(Vec::new()),
            limits: HashMap::new(),
        };
        registry.rescan();
        registry
    }

    pub fn dir(&self) -> PathBuf {
        self.root.join(DIR)
    }

    /// Перечитывает папку плагинов: новые, изменённые и удалённые плагины.
    pub fn rescan(&mut self) {
        let dir = self.dir();
        if let Err(err) = storage::private_dir(&dir) {
            eprintln!("папка плагинов {}: {err}", dir.display());
        }
        self.plugins.clear();
        self.broken.clear();
        let mut folders: Vec<(String, PathBuf)> = fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().is_dir())
            .filter_map(|e| Some((e.file_name().into_string().ok()?, e.path())))
            .filter(|(name, _)| !name.starts_with('.'))
            .collect();
        folders.sort();
        let enabled = self.state.enabled.clone();
        for (folder, path) in folders.into_iter().take(MAX_PLUGINS) {
            match load_plugin(&path, &folder, |m| enabled.contains(&m.id)) {
                Ok(plugin) => {
                    if let Some(other) = self
                        .plugins
                        .iter()
                        .find(|p| p.manifest.id == plugin.manifest.id)
                    {
                        self.broken.push(Broken {
                            error: format!(
                                "id «{}» уже у плагина в папке «{}»",
                                plugin.manifest.id, other.folder
                            ),
                            folder,
                        });
                    } else {
                        self.plugins.push(plugin);
                    }
                }
                Err(error) => self.broken.push(Broken { folder, error }),
            }
        }
        // Хранилища удалённых плагинов не держим в памяти (файлы остаются).
        let ids: BTreeSet<&str> = self
            .plugins
            .iter()
            .map(|p| p.manifest.id.as_str())
            .collect();
        self.stores.retain(|id, _| ids.contains(id.as_str()));
        self.rebuild();
    }

    pub fn get(&self, id: &str) -> Option<&Plugin> {
        self.plugins.iter().find(|p| p.manifest.id == id)
    }

    /// Плагин есть и включён.
    pub fn active(&self, id: &str) -> Option<&Plugin> {
        self.get(id).filter(|_| self.state.enabled.contains(id))
    }

    pub fn scripts(&self) -> Rc<[Script]> {
        self.scripts.clone()
    }

    /// Включает или выключает плагин. `Ok(false)` — ничего не поменялось; `Err` —
    /// код плагина не читается (его файлы поменяли или испортили после загрузки).
    pub fn set_enabled(&mut self, id: &str, on: bool) -> Result<bool, String> {
        let Some(i) = self.plugins.iter().position(|p| p.manifest.id == id) else {
            return Ok(false);
        };
        if on && !self.plugins[i].code {
            let (dir, folder) = (self.plugins[i].dir.clone(), self.plugins[i].folder.clone());
            match load_plugin(&dir, &folder, |_| true) {
                Ok(full) if full.manifest.id == id => self.plugins[i] = full,
                Ok(_) => {
                    self.rescan();
                    return Err("id плагина поменялся — нажмите «Обновить»".into());
                }
                Err(error) => {
                    self.plugins.remove(i);
                    self.broken.push(Broken {
                        folder,
                        error: error.clone(),
                    });
                    return Err(error);
                }
            }
        }
        if !on {
            self.plugins[i].drop_code();
        }
        let changed = if on {
            self.state.enabled.insert(id.to_owned())
        } else {
            self.state.enabled.remove(id)
        };
        if changed {
            self.save_state();
            self.rebuild();
        }
        Ok(changed)
    }

    /// Значения настроек плагина: сохранённые, а где их нет — по умолчанию.
    pub fn values(&self, id: &str) -> Map<String, Value> {
        let Some(plugin) = self.get(id) else {
            return Map::new();
        };
        let saved = self.state.values.get(id);
        plugin
            .manifest
            .settings
            .iter()
            .map(|f| {
                let v = saved
                    .and_then(|s| s.get(&f.key))
                    .and_then(|v| f.clean(v))
                    .unwrap_or_else(|| f.default.clone());
                (f.key.clone(), v)
            })
            .collect()
    }

    /// Новые значения настроек от интерфейса. Неизвестные ключи и неподходящие
    /// значения не сохраняются. Возвращает итоговые значения.
    pub fn set_values(
        &mut self,
        id: &str,
        incoming: &Map<String, Value>,
    ) -> Option<Map<String, Value>> {
        let plugin = self.get(id)?;
        let fields: &[Field] = &plugin.manifest.settings;
        let mut saved = self.state.values.get(id).cloned().unwrap_or_default();
        for (key, value) in incoming {
            if let Some(v) = fields
                .iter()
                .find(|f| &f.key == key)
                .and_then(|f| f.clean(value))
            {
                saved.insert(key.clone(), v);
            }
        }
        saved.retain(|k, _| fields.iter().any(|f| &f.key == k));
        self.state.values.insert(id.to_owned(), saved);
        self.save_state();
        self.rebuild();
        Some(self.values(id))
    }

    /// Удаляет папку плагина и его данные.
    pub fn remove(&mut self, folder: &str) -> Result<(), String> {
        let dir = self.dir();
        let target = dir.join(folder);
        // Только папка прямо внутри plugins/: имя пришло от интерфейса.
        let inside = !folder.is_empty()
            && Path::new(folder).components().count() == 1
            && !folder.contains(['/', '\\', ':'])
            && folder != "."
            && folder != ".."
            && target.parent() == Some(dir.as_path());
        if !inside || !target.is_dir() {
            return Err("плагин не найден".into());
        }
        let id = self
            .plugins
            .iter()
            .find(|p| p.folder == folder)
            .map(|p| p.manifest.id.clone());
        fs::remove_dir_all(&target).map_err(|e| format!("не удалось удалить папку: {e}"))?;
        if let Some(id) = id {
            self.state.enabled.remove(&id);
            self.state.values.remove(&id);
            self.stores.remove(&id);
            let _ = fs::remove_file(self.data_dir().join(format!("{id}.json")));
            self.save_state();
        }
        self.rescan();
        Ok(())
    }

    pub fn store(&mut self, id: &str) -> &mut store::Store {
        let path = self.data_dir().join(format!("{id}.json"));
        self.stores
            .entry(id.to_owned())
            .or_insert_with(|| store::Store::open(path))
    }

    fn data_dir(&self) -> PathBuf {
        self.root.join(DATA_DIR)
    }

    /// Не чаще `n` раз за `window`: иначе плагин с ошибкой в цикле завалил бы
    /// окно сообщениями или вкладками.
    pub fn allow(&mut self, id: &str, what: Permission) -> bool {
        let (n, window) = match what {
            Permission::Notify => (3, Duration::from_secs(5)),
            Permission::Tabs => (5, Duration::from_secs(10)),
        };
        let limits = self.limits.entry(id.to_owned()).or_default();
        let list = match what {
            Permission::Notify => &mut limits.notify,
            Permission::Tabs => &mut limits.tabs,
        };
        let now = Instant::now();
        list.retain(|t| now.duration_since(*t) < window);
        if list.len() >= n {
            return false;
        }
        list.push(now);
        true
    }

    fn save_state(&self) {
        if let Err(err) = storage::write_json(&self.root.join(STATE), &self.state) {
            eprintln!("не удалось сохранить plugins.json: {err:#}");
        }
    }

    fn rebuild(&mut self) {
        let scripts: Vec<Script> = self
            .plugins
            .iter()
            .filter(|p| self.state.enabled.contains(&p.manifest.id))
            .map(|p| {
                let id = &p.manifest.id;
                let binding = format!("__axiomBridge_{}", id.replace('-', "_"));
                Script {
                    plugin: id.clone(),
                    world: format!("Axiom plugin: {id}"),
                    source: bootstrap::source(p, &self.values(id), &binding),
                    binding,
                    all_frames: p.scripts.iter().any(|s| s.all_frames),
                    matches: p
                        .scripts
                        .iter()
                        .map(|s| (s.matches.clone(), s.exclude.clone()))
                        .collect(),
                }
            })
            .collect();
        self.scripts = Rc::from(scripts);
    }

    /// Список для «Настройки → Плагины».
    /// `published` — что с плагинами, которые публиковали из этого профиля.
    pub fn view<'a>(&'a self, published: &'a BTreeMap<String, market::Status>) -> List<'a> {
        List {
            dir: self.dir().display().to_string(),
            items: self
                .plugins
                .iter()
                .map(|p| {
                    let m = &p.manifest;
                    let mut sites: Vec<&str> = m
                        .content_scripts
                        .iter()
                        .flat_map(|s| s.matches.iter().map(String::as_str))
                        .collect();
                    let mut seen = std::collections::HashSet::new();
                    sites.retain(|s| seen.insert(*s));
                    View {
                        id: &m.id,
                        folder: &p.folder,
                        name: &m.name,
                        version: &m.version,
                        description: &m.description,
                        author: &m.author,
                        homepage: &m.homepage,
                        enabled: self.state.enabled.contains(&m.id),
                        permissions: m.permissions.iter().map(|p| p.label()).collect(),
                        sites,
                        icon: p.icon.as_deref(),
                        action: m.action.as_ref().map(|a| ActionView {
                            title: &a.title,
                            icon: p.action_icon.as_deref().or(p.icon.as_deref()),
                        }),
                        settings: &m.settings,
                        values: self.values(&m.id),
                        from_store: p.from_store.is_some(),
                        published: published.get(&m.id),
                    }
                })
                .collect(),
            broken: self
                .broken
                .iter()
                .map(|b| BrokenView {
                    folder: &b.folder,
                    error: &b.error,
                })
                .collect(),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct List<'a> {
    pub dir: String,
    pub items: Vec<View<'a>>,
    pub broken: Vec<BrokenView<'a>>,
}

#[derive(Debug, Serialize)]
pub struct View<'a> {
    pub id: &'a str,
    pub folder: &'a str,
    pub name: &'a str,
    pub version: &'a str,
    pub description: &'a str,
    pub author: &'a str,
    pub homepage: &'a str,
    pub enabled: bool,
    pub permissions: Vec<&'static str>,
    pub sites: Vec<&'a str>,
    pub icon: Option<&'a str>,
    pub action: Option<ActionView<'a>>,
    pub settings: &'a [Field],
    pub values: Map<String, Value>,
    /// Поставлен из магазина (публиковать его нельзя — он чужой).
    pub from_store: bool,
    /// Публиковали из этого профиля: что с ним в магазине.
    pub published: Option<&'a market::Status>,
}

#[derive(Debug, Serialize)]
pub struct ActionView<'a> {
    pub title: &'a str,
    pub icon: Option<&'a str>,
}

#[derive(Debug, Serialize)]
pub struct BrokenView<'a> {
    pub folder: &'a str,
    pub error: &'a str,
}

impl Plugin {
    /// Выключили: код больше не нужен до следующего включения.
    fn drop_code(&mut self) {
        for s in &mut self.scripts {
            s.js = String::new();
            s.css = String::new();
        }
        self.code = false;
    }
}

/// Загружает плагин из папки. Код и стили читаются, если `with_code` (для
/// включённых), иначе только проверяется, что файлы на месте и не слишком велики.
fn load_plugin(
    dir: &Path,
    folder: &str,
    with_code: impl FnOnce(&Manifest) -> bool,
) -> Result<Plugin, String> {
    let manifest = Manifest::load(dir)?;
    let code = with_code(&manifest);
    let root = fs::canonicalize(dir).map_err(|e| format!("папка не открывается: {e}"))?;
    let too_big = || format!("код и стили плагина больше {} МБ", MAX_CODE >> 20);
    let mut total = 0u64;
    let mut text = |rel: &str| -> Result<String, String> {
        if !code {
            total += check_inside(&root, rel, MAX_FILE)?;
            return if total > MAX_CODE {
                Err(too_big())
            } else {
                Ok(String::new())
            };
        }
        let bytes = read_inside(&root, rel, MAX_FILE)?;
        total += bytes.len() as u64;
        if total > MAX_CODE {
            return Err(too_big());
        }
        // Блокнот сохраняет UTF-8 с BOM.
        let bytes = bytes
            .strip_prefix(&[0xEF, 0xBB, 0xBF])
            .unwrap_or(&bytes)
            .to_vec();
        String::from_utf8(bytes).map_err(|_| format!("файл «{rel}» не в UTF-8"))
    };
    let mut scripts = Vec::new();
    for cs in &manifest.content_scripts {
        let parse = |list: &[String]| -> Result<Vec<Pattern>, String> {
            list.iter().map(|p| Pattern::parse(p)).collect()
        };
        let mut js = Vec::new();
        for f in &cs.js {
            js.push(text(f)?);
        }
        let mut css = Vec::new();
        for f in &cs.css {
            css.push(text(f)?);
        }
        scripts.push(Loaded {
            matches: parse(&cs.matches)?,
            exclude: parse(&cs.exclude)?,
            // `;` между файлами: файл без точки с запятой в конце не склеится со следующим.
            js: if code {
                js.join("\n;\n")
            } else {
                String::new()
            },
            css: css.join("\n"),
            run_at: cs.run_at,
            all_frames: cs.all_frames,
        });
    }
    let image = |rel: &Option<String>| -> Result<Option<String>, String> {
        rel.as_deref()
            .map(|rel| read_inside(&root, rel, MAX_ICON).map(|bytes| data_url(rel, &bytes)))
            .transpose()
    };
    let icon = image(&manifest.icon)?;
    let action_icon = image(&manifest.action.as_ref().and_then(|a| a.icon.clone()))?;
    Ok(Plugin {
        folder: folder.to_owned(),
        dir: dir.to_path_buf(),
        manifest,
        code,
        scripts,
        icon,
        action_icon,
        from_store: market::marker(dir).map(|m| m.version),
    })
}

/// Файл внутри папки плагина (ссылкой из неё не выйти): его размер.
fn check_inside(root: &Path, rel: &str, max: u64) -> Result<u64, String> {
    inside(root, rel, max).map(|(_, len)| len)
}

fn inside(root: &Path, rel: &str, max: u64) -> Result<(PathBuf, u64), String> {
    let path = fs::canonicalize(root.join(rel)).map_err(|_| format!("нет файла «{rel}»"))?;
    if !path.starts_with(root) {
        return Err(format!("файл «{rel}» вне папки плагина"));
    }
    let meta = fs::metadata(&path).map_err(|e| format!("файл «{rel}»: {e}"))?;
    if !meta.is_file() {
        return Err(format!("«{rel}» — не файл"));
    }
    if meta.len() > max {
        return Err(format!("файл «{rel}» больше {} КБ", max >> 10));
    }
    Ok((path, meta.len()))
}

/// Читает файл внутри папки плагина.
fn read_inside(root: &Path, rel: &str, max: u64) -> Result<Vec<u8>, String> {
    let (path, _) = inside(root, rel, max)?;
    let file = fs::File::open(&path).map_err(|e| format!("файл «{rel}» не читается: {e}"))?;
    let mut bytes = Vec::new();
    file.take(max + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("файл «{rel}» не читается: {e}"))?;
    Ok(bytes)
}

/// Картинка для интерфейса: `data:` (его CSP другие картинки не пускает).
/// В `<img>` SVG не выполняет скриптов, так что чужой значок безопасен.
fn data_url(rel: &str, bytes: &[u8]) -> String {
    let ext = Path::new(rel)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    let mime = match ext.as_str() {
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "ico" => "image/x-icon",
        _ => "image/png",
    };
    format!("data:{mime};base64,{}", base64(bytes))
}

/// base64 (стандартный алфавит, с `=`) → байты; `None` — не base64.
fn decode_base64(text: &str) -> Option<Vec<u8>> {
    let value = |c: u8| -> Option<u32> {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        };
        Some(u32::from(v))
    };
    let bytes = text.as_bytes();
    if !bytes.len().is_multiple_of(4) {
        return None;
    }
    let chunks = bytes.len() / 4;
    let mut out = Vec::with_capacity(chunks * 3);
    for (i, chunk) in bytes.chunks(4).enumerate() {
        let pad = chunk.iter().rev().take_while(|&&c| c == b'=').count();
        if pad > 2 || (pad > 0 && i + 1 != chunks) {
            return None;
        }
        let mut n = 0u32;
        for (j, &c) in chunk.iter().enumerate() {
            let v = if j >= 4 - pad { 0 } else { value(c)? };
            n = (n << 6) | v;
        }
        out.extend_from_slice(&n.to_be_bytes()[1..4 - pad]);
    }
    Some(out)
}

fn base64(bytes: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(ABC[((n >> (18 - 6 * i)) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_rfc() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64(&[0xff, 0xfe, 0x00]), "//4A");
        for sample in [&b""[..], b"f", b"fo", b"foo", b"foob", b"\xff\x00\x10 \xfe"] {
            assert_eq!(decode_base64(&base64(sample)).as_deref(), Some(sample));
        }
        assert_eq!(decode_base64("Zm9v YmFy"), None);
        assert_eq!(decode_base64("Zg="), None);
        assert_eq!(decode_base64("Zg==Zm9v"), None);
        assert_eq!(decode_base64("Z==="), None);
    }

    fn temp_profile(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("axiom-plugins-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join(DIR)).unwrap_or_default();
        dir
    }

    fn write(dir: &Path, rel: &str, text: &str) {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap_or_default();
        }
        fs::write(path, text).unwrap_or_default();
    }

    const MANIFEST: &str = r#"{
        "api": 1, "id": "hello", "name": "Привет", "version": "1.0.0",
        "permissions": ["notify"],
        "content_scripts": [{ "matches": ["example.com"], "js": ["a.js", "b.js"], "css": ["s.css"] }],
        "action": { "title": "Сказать привет" },
        "settings": [{ "key": "loud", "type": "toggle", "label": "Громко", "default": true }]
    }"#;

    #[test]
    fn scan_enable_values_remove() {
        let root = temp_profile("scan");
        let paths = storage::Paths { root: root.clone() };
        let plugins = root.join(DIR);
        write(&plugins, "hello/plugin.json", MANIFEST);
        write(&plugins, "hello/a.js", "console.log('a')");
        write(&plugins, "hello/b.js", "console.log('b')");
        write(&plugins, "hello/s.css", "p{color:red}");
        // Тот же id в другой папке и битый манифест.
        write(&plugins, "zcopy/plugin.json", MANIFEST);
        write(&plugins, "zcopy/a.js", "");
        write(&plugins, "zcopy/b.js", "");
        write(&plugins, "zcopy/s.css", "");
        write(&plugins, "bad/plugin.json", "{ nope");

        let mut reg = Registry::load(&paths);
        assert_eq!(reg.plugins.len(), 1);
        assert_eq!(reg.broken.len(), 2);
        assert!(
            reg.view(&BTreeMap::new())
                .broken
                .iter()
                .any(|b| b.folder == "zcopy" && b.error.contains("уже"))
        );
        // Новый плагин выключен.
        assert!(reg.scripts().is_empty());
        assert!(reg.active("hello").is_none());

        // Код выключенного плагина не читается.
        assert!(
            reg.get("hello")
                .is_some_and(|p| !p.code && p.scripts[0].js.is_empty())
        );
        assert_eq!(reg.set_enabled("hello", true), Ok(true));
        assert_eq!(reg.set_enabled("hello", true), Ok(false));
        let scripts = reg.scripts();
        assert_eq!(scripts.len(), 1);
        assert!(scripts[0].wants("https://www.example.com/x"));
        assert!(!scripts[0].wants("https://example.org/"));
        assert!(
            scripts[0]
                .source
                .contains("console.log('a')\n;\nconsole.log('b')")
        );
        assert!(scripts[0].source.contains("p{color:red}"));
        assert_eq!(reg.values("hello").get("loud"), Some(&Value::Bool(true)));

        let mut incoming = Map::new();
        incoming.insert("loud".into(), Value::Bool(false));
        incoming.insert("unknown".into(), Value::Bool(true));
        let values = reg.set_values("hello", &incoming).unwrap_or_default();
        assert_eq!(values.get("loud"), Some(&Value::Bool(false)));
        assert!(!values.contains_key("unknown"));
        assert!(reg.scripts()[0].source.contains(r#""loud":false"#));

        // Состояние переживает перезапуск, код включённого читается сразу.
        let reg2 = Registry::load(&paths);
        assert!(reg2.active("hello").is_some_and(|p| p.code));
        assert!(reg2.scripts()[0].source.contains("console.log('a')"));
        assert_eq!(reg2.values("hello").get("loud"), Some(&Value::Bool(false)));

        assert!(reg.remove("../hello").is_err());
        assert!(reg.remove("nothing").is_err());
        assert!(reg.remove("hello").is_ok());
        assert!(!plugins.join("hello").exists());
        // Место освободилось: копия с тем же id теперь загружается, но выключена.
        assert_eq!(reg.get("hello").map(|p| p.folder.as_str()), Some("zcopy"));
        assert!(reg.active("hello").is_none());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn files_stay_inside() {
        let root = temp_profile("inside");
        let plugins = root.join(DIR);
        write(&root, "secret.js", "steal()");
        write(
            &plugins,
            "evil/plugin.json",
            r#"{ "api": 1, "id": "evil", "name": "x", "version": "1",
                 "content_scripts": [{ "matches": ["*"], "js": ["missing.js"] }] }"#,
        );
        let err = load_plugin(&plugins.join("evil"), "evil", |_| true)
            .err()
            .unwrap_or_default();
        assert!(err.contains("missing.js"), "{err}");
        let canon = fs::canonicalize(plugins.join("evil")).unwrap_or_default();
        assert!(read_inside(&canon, "../../secret.js", MAX_FILE).is_err());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn broken_on_enable() {
        let root = temp_profile("enable");
        let paths = storage::Paths { root: root.clone() };
        let plugins = root.join(DIR);
        write(&plugins, "hello/plugin.json", MANIFEST);
        write(&plugins, "hello/a.js", "1");
        write(&plugins, "hello/b.js", "2");
        write(&plugins, "hello/s.css", "");
        let mut reg = Registry::load(&paths);
        // Файл испортили, пока плагин был выключен: включить его нельзя, он в «сломанных».
        fs::write(plugins.join("hello/a.js"), [0xff, 0xfe, 0x00]).unwrap_or_default();
        let err = reg.set_enabled("hello", true).err().unwrap_or_default();
        assert!(err.contains("UTF-8"), "{err}");
        assert!(reg.get("hello").is_none());
        assert!(reg.scripts().is_empty());
        assert_eq!(reg.view(&BTreeMap::new()).broken.len(), 1);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn rate_limits() {
        let root = temp_profile("limits");
        let mut reg = Registry::load(&storage::Paths { root: root.clone() });
        assert!((0..3).all(|_| reg.allow("x", Permission::Notify)));
        assert!(!reg.allow("x", Permission::Notify));
        assert!(reg.allow("y", Permission::Notify));
        assert!((0..5).all(|_| reg.allow("x", Permission::Tabs)));
        assert!(!reg.allow("x", Permission::Tabs));
        let _ = fs::remove_dir_all(&root);
    }
}
