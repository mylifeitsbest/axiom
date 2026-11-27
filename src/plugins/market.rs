//! Магазин плагинов: каталог, установка и обновление, публикация своих.
//!
//! Сервер — `server/store.py`, на том же сервере, что статистика (`stats::base_url`).
//! В сеть Axiom ходит только по действию человека: открыл «Магазин» в «Настройки →
//! Плагины», нажал «Установить», «Обновить» или «Опубликовать» (см. PRIVACY.md).
//!
//! Плагин из магазина ставится в `plugins/<id>/` с отметкой `.axiom-store.json`:
//! только такую папку магазин может заменить новой версией. Свой плагин с тем же
//! именем папки он не тронет.
//!
//! Публикация: браузер шлёт файлы плагина и ключ автора — 128 случайных бит, которые
//! он придумывает при первой публикации и хранит в `plugin-keys.json` профиля. Новую
//! версию примут только с тем же ключом. В магазине плагин появится после проверки.

use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::Stdio,
    thread,
};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use tao::event_loop::EventLoopProxy;

use super::manifest::{self, Manifest};
use crate::{events::UserEvent, storage};

/// Каталог со значками: больше не скачиваем.
const MAX_INDEX: u64 = 16 << 20;
/// Плагин в base64 (до 5 МБ файлов).
const MAX_BUNDLE: u64 = 8 << 20;
const MAX_FILES: usize = 100;
const MAX_BYTES: usize = 5 << 20;
const MAX_FILE: usize = 2 << 20;
/// Ключи автора своих опубликованных плагинов.
const KEYS: &str = "plugin-keys.json";
/// Отметка «поставлен из магазина» в папке плагина.
pub const MARKER: &str = ".axiom-store.json";
/// Что может лежать в папке плагина (как `EXTS` в `server/store.py`).
const EXTS: [&str; 13] = [
    "js", "mjs", "css", "json", "png", "svg", "webp", "jpg", "jpeg", "gif", "ico", "md", "txt",
];
pub const MAX_AUTHOR: usize = 100;
pub const MAX_CONTACT: usize = 200;
pub const MAX_NOTE: usize = 1000;

#[derive(Debug)]
pub enum Event {
    Index(Result<Vec<Item>, String>),
    /// Что с плагинами, которые публиковали из этого профиля.
    Status(BTreeMap<String, Status>),
    Downloaded {
        id: String,
        result: Result<Bundle, String>,
    },
    Published {
        id: String,
        result: Result<String, String>,
    },
}

/// Плагин в каталоге магазина.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Item {
    pub id: String,
    pub version: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub homepage: String,
    #[serde(default)]
    pub permissions: Vec<String>,
    #[serde(default)]
    pub sites: Vec<String>,
    #[serde(default)]
    pub action: bool,
    #[serde(default)]
    pub settings: u32,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub installs: u64,
    #[serde(default)]
    pub official: bool,
    pub sha256: String,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub updated: String,
}

impl Item {
    /// Строка каталога пришла из сети: только разумное.
    fn clean(mut self) -> Option<Self> {
        let hex = self.sha256.len() == 64 && self.sha256.bytes().all(|b| b.is_ascii_hexdigit());
        let version_ok = !self.version.is_empty()
            && self.version.len() <= 32
            && self
                .version
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'+'));
        if !manifest::valid_id(&self.id) || !hex || !version_ok || self.name.trim().is_empty() {
            return None;
        }
        let cut = |s: &mut String, n: usize| *s = s.chars().take(n).collect();
        cut(&mut self.name, 60);
        cut(&mut self.description, 400);
        cut(&mut self.author, MAX_AUTHOR);
        cut(&mut self.updated, 40);
        if !(self.homepage.starts_with("https://") || self.homepage.starts_with("http://"))
            || self.homepage.len() > 300
        {
            self.homepage.clear();
        }
        // Значок — только картинка `data:` (CSP интерфейса другого и не пустит).
        if self
            .icon
            .as_deref()
            .is_some_and(|i| !i.starts_with("data:image/") || i.len() > 128 * 1024)
        {
            self.icon = None;
        }
        self.permissions.retain(|p| p == "notify" || p == "tabs");
        self.sites.truncate(20);
        for s in &mut self.sites {
            cut(s, 200);
        }
        Some(self)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Status {
    /// Последняя присланная версия и что с ней: pending, approved, rejected.
    pub version: String,
    pub status: String,
    #[serde(default)]
    pub reason: String,
    /// Какая версия сейчас в магазине (`None` — никакая или плагин сняли).
    #[serde(default)]
    pub published: Option<String>,
}

/// Скачанный плагин: файлы уже проверены (пути, размеры, sha256).
#[derive(Debug)]
pub struct Bundle {
    pub version: String,
    pub sha256: String,
    pub files: Vec<(String, Vec<u8>)>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Marker {
    pub id: String,
    pub version: String,
    pub sha256: String,
}

/// Магазин для интерфейса.
#[derive(Debug, Serialize)]
pub struct View<'a> {
    /// Каталог хоть раз пришёл.
    pub loaded: bool,
    pub loading: bool,
    pub error: Option<&'a str>,
    pub items: Vec<ItemView<'a>>,
    pub busy: &'a [String],
}

#[derive(Debug, Serialize)]
pub struct ItemView<'a> {
    #[serde(flatten)]
    pub item: &'a Item,
    /// Версия, которая стоит у человека (`None` — не стоит).
    pub installed: Option<&'a str>,
    /// Стоит из магазина (иначе — свой плагин с тем же id: обновлять его нельзя).
    pub from_store: bool,
}

/// Состояние магазина в этом запуске.
#[derive(Debug, Default)]
pub struct State {
    pub items: Option<Vec<Item>>,
    pub loading: bool,
    pub error: Option<String>,
    /// Ставятся, обновляются или публикуются прямо сейчас.
    pub busy: Vec<String>,
    pub status: BTreeMap<String, Status>,
}

impl State {
    pub fn item(&self, id: &str) -> Option<&Item> {
        self.items.as_ref()?.iter().find(|i| i.id == id)
    }

    pub fn set_busy(&mut self, id: &str, on: bool) {
        self.busy.retain(|b| b != id);
        if on {
            self.busy.push(id.to_owned());
        }
    }
}

/// Плагин поставлен из магазина: его отметка.
pub fn marker(dir: &Path) -> Option<Marker> {
    storage::read_json(&dir.join(MARKER))
}

fn send(events: &EventLoopProxy<UserEvent>, event: Event) {
    let _ = events.send_event(UserEvent::Market(event));
}

fn spawn(name: &str, job: impl FnOnce() + Send + 'static) {
    if let Err(err) = thread::Builder::new().name(name.into()).spawn(job) {
        eprintln!("магазин плагинов: поток не запустился: {err}");
    }
}

/// Каталог магазина.
pub fn load_index(events: &EventLoopProxy<UserEvent>) {
    let events = events.clone();
    spawn("market-index", move || {
        let url = format!("{}/store/index", crate::stats::base_url());
        let result = request(&url, None, MAX_INDEX, 30).and_then(|(code, body)| {
            match code {
                200 => parse_index(&body),
                // Сервер ещё без магазина (его обновляют отдельно от браузера).
                404 => Err("магазин пока не открылся — загляните чуть позже".into()),
                _ => Err(error_text(code, &body)),
            }
        });
        send(&events, Event::Index(result));
    });
}

fn parse_index(body: &[u8]) -> Result<Vec<Item>, String> {
    #[derive(Deserialize)]
    struct Index {
        plugins: Vec<Value>,
    }
    let index: Index =
        serde_json::from_slice(body).map_err(|_| "магазин ответил непонятно".to_owned())?;
    Ok(index
        .plugins
        .into_iter()
        .filter_map(|v| serde_json::from_value::<Item>(v).ok()?.clean())
        .collect())
}

/// Что с плагинами, опубликованными из этого профиля (если такие есть).
pub fn load_status(root: &Path, events: &EventLoopProxy<UserEvent>) {
    let keys = keys(root);
    if keys.is_empty() {
        return;
    }
    let events = events.clone();
    spawn("market-status", move || {
        let items: Vec<Value> = keys
            .iter()
            .map(|(id, key)| json!({ "id": id, "key": key }))
            .collect();
        let body = json!({ "items": items }).to_string().into_bytes();
        let url = format!("{}/store/status", crate::stats::base_url());
        let Ok((200, raw)) = request(&url, Some(body), 1 << 20, 30) else {
            return;
        };
        #[derive(Deserialize)]
        struct Reply {
            items: BTreeMap<String, Status>,
        }
        if let Ok(reply) = serde_json::from_slice::<Reply>(&raw) {
            send(&events, Event::Status(reply.items));
        }
    });
}

/// Скачивает плагин из магазина и проверяет его по каталогу.
pub fn download(item: &Item, events: &EventLoopProxy<UserEvent>) {
    let (id, version, sha256) = (item.id.clone(), item.version.clone(), item.sha256.clone());
    let events = events.clone();
    spawn("market-download", move || {
        let url = format!("{}/store/p/{id}", crate::stats::base_url());
        let result = request(&url, None, MAX_BUNDLE, 120).and_then(|(code, body)| {
            if code != 200 {
                return Err(error_text(code, &body));
            }
            unpack(&body, &sha256).map(|files| Bundle {
                version,
                sha256,
                files,
            })
        });
        send(&events, Event::Downloaded { id, result });
    });
}

/// Пакет магазина → файлы. Сначала сверяем sha256 с каталогом: пакет не подменили.
fn unpack(body: &[u8], sha256: &str) -> Result<Vec<(String, Vec<u8>)>, String> {
    let got = hex(&Sha256::digest(body));
    if !got.eq_ignore_ascii_case(sha256) {
        return Err("плагин не совпал с каталогом магазина — попробуйте ещё раз".into());
    }
    #[derive(Deserialize)]
    struct Pack {
        files: BTreeMap<String, String>,
    }
    let pack: Pack =
        serde_json::from_slice(body).map_err(|_| "магазин прислал непонятный плагин".to_owned())?;
    if pack.files.is_empty() || pack.files.len() > MAX_FILES {
        return Err("в плагине нет файлов или их слишком много".into());
    }
    let mut total = 0;
    let mut files = Vec::with_capacity(pack.files.len());
    for (rel, b64) in pack.files {
        if !valid_path(&rel) {
            return Err(format!("в плагине недопустимый файл «{rel}»"));
        }
        let data = super::decode_base64(&b64).ok_or_else(|| format!("файл «{rel}» повреждён"))?;
        total += data.len();
        if data.len() > MAX_FILE || total > MAX_BYTES {
            return Err("плагин слишком большой".into());
        }
        files.push((rel, data));
    }
    if !files.iter().any(|(rel, _)| rel == "plugin.json") {
        return Err("в плагине нет plugin.json".into());
    }
    Ok(files)
}

/// Путь файла в папке плагина из магазина: без `..`, скрытых папок и чужих типов.
fn valid_path(rel: &str) -> bool {
    if rel.is_empty() || rel.len() > 200 || rel.contains(['\\', ':']) || rel.starts_with('/') {
        return false;
    }
    // По строке, а не по `components()`: тот молча выбрасывает `.` из середины пути.
    let parts: Vec<&str> = rel.split('/').collect();
    let plain = parts.len() <= 8 && parts.iter().all(|p| !p.is_empty() && !p.starts_with('.'));
    let ext = Path::new(rel)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    plain && EXTS.contains(&ext.as_str())
}

/// Ставит скачанный плагин в `plugins/<id>/`. `Ok(true)` — новый, `Ok(false)` —
/// заменил прежнюю версию из магазина. Свою папку человека с тем же именем не трогает.
pub fn install(dir: &Path, id: &str, bundle: &Bundle) -> Result<bool, String> {
    storage::private_dir(dir).map_err(|e| format!("папка плагинов: {e}"))?;
    sweep(dir);
    let target = dir.join(id);
    let fresh = !target.exists();
    if !fresh && marker(&target).is_none() {
        return Err(format!(
            "в папке плагинов уже есть свой плагин «{id}» — переименуйте или удалите его папку"
        ));
    }
    let tag = &crate::instance::random_token()[..8];
    let tmp = dir.join(format!(".install-{id}-{tag}"));
    let written = write_bundle(&tmp, id, bundle);
    if let Err(err) = written {
        let _ = fs::remove_dir_all(&tmp);
        return Err(err);
    }
    if fresh {
        return fs::rename(&tmp, &target).map(|()| true).map_err(|e| {
            let _ = fs::remove_dir_all(&tmp);
            format!("не удалось поставить плагин: {e}")
        });
    }
    // Прежнюю версию — в сторону, новую — на её место; не вышло — вернуть прежнюю.
    let old = dir.join(format!(".old-{id}-{tag}"));
    fs::rename(&target, &old).map_err(|e| {
        let _ = fs::remove_dir_all(&tmp);
        format!("не удалось заменить плагин (он открыт в другой программе?): {e}")
    })?;
    if let Err(e) = fs::rename(&tmp, &target) {
        let _ = fs::rename(&old, &target);
        let _ = fs::remove_dir_all(&tmp);
        return Err(format!("не удалось заменить плагин: {e}"));
    }
    let _ = fs::remove_dir_all(&old);
    Ok(false)
}

fn write_bundle(tmp: &Path, id: &str, bundle: &Bundle) -> Result<(), String> {
    for (rel, data) in &bundle.files {
        let path = tmp.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("не удалось записать плагин: {e}"))?;
        }
        fs::write(&path, data).map_err(|e| format!("не удалось записать плагин: {e}"))?;
    }
    let manifest =
        Manifest::load(tmp).map_err(|e| format!("плагин из магазина не загрузился: {e}"))?;
    if manifest.id != id {
        return Err("магазин прислал другой плагин".into());
    }
    let mark = Marker {
        id: id.to_owned(),
        version: manifest.version,
        sha256: bundle.sha256.clone(),
    };
    storage::write_json(&tmp.join(MARKER), &mark)
        .map_err(|e| format!("не удалось записать плагин: {e:#}"))
}

/// Остатки прерванной установки (выключили питание посреди замены).
fn sweep(dir: &Path) {
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if (name.starts_with(".install-") || name.starts_with(".old-")) && entry.path().is_dir() {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

/// Что человек пишет о себе и о плагине при публикации.
#[derive(Debug, Clone)]
pub struct Publish {
    pub author: String,
    pub contact: String,
    pub note: String,
}

/// Отправляет свой плагин на проверку. Файлы читаются сразу (в этом потоке),
/// отправка — в фоне.
pub fn publish(
    root: &Path,
    dir: &Path,
    id: &str,
    about: Publish,
    events: &EventLoopProxy<UserEvent>,
) -> Result<(), String> {
    let files = collect(dir)?;
    let key = key_for(root, id)?;
    let body = json!({
        "key": key,
        "author": about.author,
        "contact": about.contact,
        "note": about.note,
        "files": files,
    })
    .to_string()
    .into_bytes();
    let (events, id) = (events.clone(), id.to_owned());
    spawn("market-publish", move || {
        let url = format!("{}/store/submit", crate::stats::base_url());
        let result = request(&url, Some(body), 1 << 20, 180).and_then(|(code, raw)| {
            if code != 200 {
                return Err(error_text(code, &raw));
            }
            let reply: Value = serde_json::from_slice(&raw).unwrap_or_default();
            Ok(reply["version"].as_str().unwrap_or_default().to_owned())
        });
        send(&events, Event::Published { id, result });
    });
    Ok(())
}

/// Файлы папки плагина для публикации: `{путь: base64}`. Скрытые файлы и
/// неподходящие типы (исходники картинок, архивы) не уходят.
fn collect(dir: &Path) -> Result<Map<String, Value>, String> {
    let mut out = Map::new();
    let mut total = 0;
    let mut stack = vec![PathBuf::new()];
    while let Some(rel_dir) = stack.pop() {
        let entries =
            fs::read_dir(dir.join(&rel_dir)).map_err(|e| format!("папка плагина: {e}"))?;
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let rel = rel_dir.join(&name);
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                if rel.components().count() < 8 {
                    stack.push(rel);
                }
                continue;
            }
            let rel = rel.to_string_lossy().replace('\\', "/");
            if !kind.is_file() || !valid_path(&rel) {
                continue;
            }
            let data = fs::read(entry.path()).map_err(|e| format!("файл «{rel}»: {e}"))?;
            total += data.len();
            if data.len() > MAX_FILE {
                return Err(format!("файл «{rel}» больше 2 МБ"));
            }
            if total > MAX_BYTES {
                return Err("плагин больше 5 МБ".into());
            }
            out.insert(rel, Value::String(super::base64(&data)));
            if out.len() > MAX_FILES {
                return Err(format!("в папке плагина больше {MAX_FILES} файлов"));
            }
        }
    }
    Ok(out)
}

fn keys(root: &Path) -> BTreeMap<String, String> {
    storage::read_json(&root.join(KEYS)).unwrap_or_default()
}

/// Ключ автора для плагина: прежний или новый (и сразу на диск — без него новую
/// версию не примут).
fn key_for(root: &Path, id: &str) -> Result<String, String> {
    let mut all = keys(root);
    if let Some(key) = all.get(id).filter(|k| k.len() == 32) {
        return Ok(key.clone());
    }
    let key = crate::instance::random_token();
    all.insert(id.to_owned(), key.clone());
    storage::write_json(&root.join(KEYS), &all)
        .map_err(|e| format!("ключ автора не сохранён: {e:#}"))?;
    Ok(key)
}

/// GET (`body` — `None`) или POST JSON. → код ответа и тело. Ошибка — нет связи.
fn request(
    url: &str,
    body: Option<Vec<u8>>,
    max: u64,
    seconds: u32,
) -> Result<(u16, Vec<u8>), String> {
    let mut cmd = crate::stats::curl();
    cmd.args(["--max-time", &seconds.to_string()])
        .args(["--max-filesize", &max.to_string()])
        .args(["--write-out", "\n%{http_code}"])
        .stdout(Stdio::piped());
    if body.is_some() {
        cmd.args(["-X", "POST", "-H", "Content-Type: application/json"])
            .args(["--data-binary", "@-"])
            .stdin(Stdio::piped());
    }
    cmd.arg(url);
    let offline = || "нет связи с магазином".to_owned();
    let mut child = cmd.spawn().map_err(|_| offline())?;
    if let (Some(body), Some(mut stdin)) = (body, child.stdin.take()) {
        stdin.write_all(&body).map_err(|_| offline())?;
    }
    let mut out = Vec::new();
    if let Some(stdout) = child.stdout.take() {
        let _ = stdout.take(max + 16).read_to_end(&mut out);
    }
    let ok = child.wait().map_err(|_| offline())?.success();
    if !ok || out.len() as u64 > max + 8 {
        return Err(offline());
    }
    let cut = out.iter().rposition(|&b| b == b'\n').ok_or_else(offline)?;
    let code = std::str::from_utf8(&out[cut + 1..])
        .ok()
        .and_then(|s| s.trim().parse::<u16>().ok())
        .ok_or_else(offline)?;
    out.truncate(cut);
    Ok((code, out))
}

/// Ошибка сервера человеческим текстом: `{"error": "…"}` или код ответа.
fn error_text(code: u16, body: &[u8]) -> String {
    let text = serde_json::from_slice::<Value>(body).ok().and_then(|v| {
        v["error"]
            .as_str()
            .map(|s| s.chars().take(300).collect::<String>())
    });
    match (text, code) {
        (Some(text), _) => text,
        (None, 413) => "плагин слишком большой для магазина".into(),
        (None, 429) => "слишком часто — попробуйте позже".into(),
        (None, 404) => "в магазине такого нет".into(),
        (None, code) => format!("магазин ответил ошибкой {code}"),
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pack(files: &[(&str, &str)]) -> (Vec<u8>, String) {
        let map: BTreeMap<&str, String> = files
            .iter()
            .map(|(p, t)| (*p, super::super::base64(t.as_bytes())))
            .collect();
        let body = serde_json::to_vec(&json!({ "files": map })).unwrap_or_default();
        let sha = hex(&Sha256::digest(&body));
        (body, sha)
    }

    const MANIFEST: &str = r#"{"api":1,"id":"hello","name":"Привет","version":"1.2.0",
        "content_scripts":[{"matches":["*"],"js":["a.js"]}]}"#;

    #[test]
    fn unpack_checks_hash_and_paths() {
        let (body, sha) = pack(&[
            ("plugin.json", MANIFEST),
            ("a.js", "1"),
            ("img/i.svg", "<svg/>"),
        ]);
        let files = unpack(&body, &sha).unwrap_or_default();
        assert_eq!(files.len(), 3);
        assert!(unpack(&body, &"0".repeat(64)).is_err());
        for bad in [
            "../x.js",
            ".git/config",
            "a\\b.js",
            "run.exe",
            "/abs.js",
            "C:/x.js",
            "a/./b.js",
        ] {
            let (body, sha) = pack(&[("plugin.json", MANIFEST), (bad, "1")]);
            assert!(unpack(&body, &sha).is_err(), "{bad}");
        }
        let (body, sha) = pack(&[("a.js", "1")]);
        assert!(unpack(&body, &sha).is_err(), "без plugin.json");
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("axiom-market-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap_or_default();
        dir
    }

    fn bundle(version: &str, code: &str) -> Bundle {
        let manifest = MANIFEST.replace("1.2.0", version);
        Bundle {
            version: version.into(),
            sha256: "ab".repeat(32),
            files: vec![
                ("plugin.json".into(), manifest.into_bytes()),
                ("a.js".into(), code.as_bytes().to_vec()),
            ],
        }
    }

    #[test]
    fn install_update_and_own_folder() {
        let dir = temp("install");
        assert_eq!(install(&dir, "hello", &bundle("1.0.0", "old")), Ok(true));
        assert_eq!(
            marker(&dir.join("hello")).map(|m| m.version),
            Some("1.0.0".into())
        );
        assert_eq!(install(&dir, "hello", &bundle("1.1.0", "new")), Ok(false));
        assert_eq!(
            fs::read_to_string(dir.join("hello/a.js")).unwrap_or_default(),
            "new"
        );
        assert_eq!(
            marker(&dir.join("hello")).map(|m| m.version),
            Some("1.1.0".into())
        );
        // Никаких временных папок после себя.
        let left: Vec<String> = fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(left, vec!["hello".to_owned()]);
        // Своя папка человека с тем же именем — не трогаем.
        let _ = fs::remove_file(dir.join("hello").join(MARKER));
        let err = install(&dir, "hello", &bundle("2.0.0", "x"))
            .err()
            .unwrap_or_default();
        assert!(err.contains("уже есть свой плагин"), "{err}");
        assert_eq!(
            fs::read_to_string(dir.join("hello/a.js")).unwrap_or_default(),
            "new"
        );
        // Пакет с чужим id не ставится.
        let err = install(&dir, "other", &bundle("1.0.0", "x"))
            .err()
            .unwrap_or_default();
        assert!(err.contains("другой плагин"), "{err}");
        assert!(!dir.join("other").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn collect_skips_hidden_and_foreign_files() {
        let dir = temp("collect");
        fs::create_dir_all(dir.join("img")).unwrap_or_default();
        fs::create_dir_all(dir.join(".git")).unwrap_or_default();
        for (rel, text) in [
            ("plugin.json", MANIFEST),
            ("a.js", "1"),
            ("img/i.svg", "<svg/>"),
            (".git/config", "secret"),
            (MARKER, "{}"),
            ("design.psd", "x"),
        ] {
            fs::write(dir.join(rel), text).unwrap_or_default();
        }
        let files = collect(&dir).unwrap_or_default();
        let mut names: Vec<&String> = files.keys().collect();
        names.sort();
        assert_eq!(names, ["a.js", "img/i.svg", "plugin.json"]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn author_key_is_kept() {
        let root = temp("keys");
        let a = key_for(&root, "hello").unwrap_or_default();
        assert_eq!(a.len(), 32);
        assert_eq!(key_for(&root, "hello").unwrap_or_default(), a);
        assert_ne!(key_for(&root, "other").unwrap_or_default(), a);
        assert_eq!(keys(&root).len(), 2);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn index_items_are_cleaned() {
        let body = json!({ "plugins": [
            { "id": "ok-one", "version": "1.0.0", "name": "Ок", "sha256": "a".repeat(64),
              "icon": "javascript:alert(1)", "homepage": "javascript:x", "permissions": ["tabs", "root"] },
            { "id": "Bad Id", "version": "1", "name": "x", "sha256": "a".repeat(64) },
            { "id": "no-hash", "version": "1", "name": "x", "sha256": "zz" }
        ]})
        .to_string();
        let items = parse_index(body.as_bytes()).unwrap_or_default();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].icon, None);
        assert_eq!(items[0].homepage, "");
        assert_eq!(items[0].permissions, ["tabs"]);
    }

    #[test]
    fn server_errors_are_readable() {
        assert_eq!(
            error_text(403, r#"{"error":"id занят"}"#.as_bytes()),
            "id занят"
        );
        assert_eq!(
            error_text(413, b"too large"),
            "плагин слишком большой для магазина"
        );
        assert_eq!(error_text(502, b""), "магазин ответил ошибкой 502");
    }
}
