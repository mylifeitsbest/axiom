//! `plugin.json` — описание плагина: кто он, на каких страницах работает, что ему
//! можно и какие у него настройки. Всё проверяется при загрузке: плагин с ошибкой
//! не запускается, а в настройках видно, что именно не так.

use std::{
    fs,
    io::Read,
    path::{Component, Path},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::pattern::Pattern;

/// Версия API, которую понимает этот Axiom.
pub const API: u32 = 1;

const MAX_MANIFEST: u64 = 64 * 1024;
const MAX_SCRIPTS: usize = 20;
const MAX_FILES: usize = 20;
const MAX_PATTERNS: usize = 50;
const MAX_FIELDS: usize = 30;
const MAX_OPTIONS: usize = 30;
/// Обычное поле ввода: длиннее текст в настройках плагина не нужен.
const TEXT_LENGTH: usize = 1000;
const MAX_TEXT_LENGTH: usize = 10_000;

#[derive(Debug, Clone, Deserialize)]
pub struct Manifest {
    pub api: u32,
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub homepage: String,
    /// Значок плагина в списке: путь к картинке внутри папки.
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub permissions: Vec<Permission>,
    #[serde(default)]
    pub content_scripts: Vec<ContentScript>,
    /// Кнопка плагина на панели браузера.
    #[serde(default)]
    pub action: Option<Action>,
    #[serde(default)]
    pub settings: Vec<Field>,
}

/// Что плагину можно сверх своих страниц и своего хранилища.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    /// Показывать сообщения внизу окна (`axiom.notify`).
    Notify,
    /// Открывать вкладки (`axiom.tabs.open`).
    Tabs,
}

impl Permission {
    /// Как это видит человек в настройках.
    pub fn label(self) -> &'static str {
        match self {
            Self::Notify => "Показывать сообщения",
            Self::Tabs => "Открывать вкладки",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ContentScript {
    pub matches: Vec<String>,
    #[serde(default, alias = "exclude_matches")]
    pub exclude: Vec<String>,
    #[serde(default)]
    pub js: Vec<String>,
    #[serde(default)]
    pub css: Vec<String>,
    #[serde(default)]
    pub run_at: RunAt,
    /// И во фреймах страницы, не только в ней самой. API (`axiom.*`) там не работает.
    #[serde(default)]
    pub all_frames: bool,
}

/// Когда запускать скрипт.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunAt {
    /// До скриптов сайта: страницы ещё нет, `document.documentElement` пуст.
    #[serde(alias = "document_start")]
    Start,
    /// Разметка готова (`DOMContentLoaded`).
    #[default]
    #[serde(alias = "document_end")]
    Ready,
    /// Страница загрузилась целиком, с картинками (`load`).
    #[serde(alias = "document_idle")]
    Load,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Action {
    pub title: String,
    #[serde(default)]
    pub icon: Option<String>,
}

/// Настройка плагина: интерфейс рисует её в карточке плагина.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Field {
    pub key: String,
    #[serde(rename = "type")]
    pub kind: FieldKind,
    pub label: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    #[serde(default)]
    pub default: Value,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<Choice>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_length: Option<usize>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub placeholder: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldKind {
    Toggle,
    Text,
    Number,
    Choice,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Choice {
    pub value: String,
    pub label: String,
}

impl Field {
    /// Значение, которое можно сохранить, или `None`, если оно не подходит полю.
    pub fn clean(&self, v: &Value) -> Option<Value> {
        match self.kind {
            FieldKind::Toggle => v.as_bool().map(Value::Bool),
            FieldKind::Text => {
                let s = v.as_str()?;
                let max = self.max_length.unwrap_or(TEXT_LENGTH).min(MAX_TEXT_LENGTH);
                (s.chars().count() <= max).then(|| Value::String(s.to_owned()))
            }
            FieldKind::Number => {
                let n = v.as_f64().filter(|n| n.is_finite())?;
                let ok = self.min.is_none_or(|min| n >= min) && self.max.is_none_or(|max| n <= max);
                ok.then(|| serde_json::Number::from_f64(n).map(Value::Number))?
            }
            FieldKind::Choice => {
                let s = v.as_str()?;
                self.options
                    .iter()
                    .any(|o| o.value == s)
                    .then(|| Value::String(s.to_owned()))
            }
        }
    }

    /// Значение, когда автор не указал своё (или указал неподходящее).
    fn fallback(&self) -> Value {
        match self.kind {
            FieldKind::Toggle => Value::Bool(false),
            FieldKind::Text => Value::String(String::new()),
            FieldKind::Number => {
                // Ноль, а если он вне границ — ближайшая граница.
                let mut n: f64 = 0.0;
                if let Some(min) = self.min {
                    n = n.max(min);
                }
                if let Some(max) = self.max {
                    n = n.min(max);
                }
                serde_json::Number::from_f64(n).map_or(Value::Null, Value::Number)
            }
            FieldKind::Choice => self
                .options
                .first()
                .map_or(Value::Null, |o| Value::String(o.value.clone())),
        }
    }

    fn check(&mut self) -> Result<(), String> {
        let key_ok = !self.key.is_empty()
            && self.key.len() <= 40
            && self
                .key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_');
        if !key_ok {
            return Err(format!(
                "настройка «{}»: key — латиница, цифры и _, до 40 знаков",
                self.key
            ));
        }
        let key = self.key.clone();
        let err = |why: &str| Err(format!("настройка «{key}»: {why}"));
        if self.label.trim().is_empty() || self.label.chars().count() > 80 {
            return err("label — от 1 до 80 знаков");
        }
        if self.note.chars().count() > 300 || self.placeholder.chars().count() > 100 {
            return err("note — до 300 знаков, placeholder — до 100");
        }
        match self.kind {
            FieldKind::Choice => {
                if self.options.is_empty() || self.options.len() > MAX_OPTIONS {
                    return err("у choice должно быть от 1 до 30 options");
                }
                let bad = self.options.iter().any(|o| {
                    o.value.is_empty()
                        || o.value.len() > 100
                        || o.label.trim().is_empty()
                        || o.label.chars().count() > 80
                });
                if bad {
                    return err("у каждого варианта нужны value (до 100 знаков) и label (до 80)");
                }
            }
            FieldKind::Number => {
                let finite = [self.min, self.max, self.step]
                    .iter()
                    .flatten()
                    .all(|n| n.is_finite());
                if !finite || matches!((self.min, self.max), (Some(a), Some(b)) if a > b) {
                    return err("min, max и step — числа, min не больше max");
                }
            }
            FieldKind::Text => {
                if self
                    .max_length
                    .is_some_and(|n| n == 0 || n > MAX_TEXT_LENGTH)
                {
                    return err("max_length — от 1 до 10000");
                }
            }
            FieldKind::Toggle => {}
        }
        if self.clean(&self.default).is_none() {
            self.default = self.fallback();
        }
        Ok(())
    }
}

impl Manifest {
    /// Читает и проверяет `plugin.json` в папке плагина.
    pub fn load(dir: &Path) -> Result<Self, String> {
        let path = dir.join("plugin.json");
        let mut bytes = Vec::new();
        fs::File::open(&path)
            .map_err(|_| "нет файла plugin.json".to_owned())?
            .take(MAX_MANIFEST + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| format!("plugin.json не читается: {e}"))?;
        if bytes.len() as u64 > MAX_MANIFEST {
            return Err("plugin.json больше 64 КБ".into());
        }
        // Блокнот сохраняет UTF-8 с BOM.
        let text = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes);
        let mut manifest: Self = serde_json::from_slice(text).map_err(|e| explain(&e))?;
        manifest.check()?;
        Ok(manifest)
    }

    fn check(&mut self) -> Result<(), String> {
        if self.api > API {
            return Err(format!(
                "плагину нужен API {}, а этот Axiom знает только {API}: обновите Axiom",
                self.api
            ));
        }
        if self.api == 0 {
            return Err("api — номер версии API, сейчас 1".into());
        }
        if !valid_id(&self.id) {
            return Err(
                "id — от 2 до 40 знаков: строчная латиница, цифры и -, начинается с буквы или цифры"
                    .into(),
            );
        }
        self.name = self.name.trim().to_owned();
        if self.name.is_empty() || self.name.chars().count() > 60 {
            return Err("name — от 1 до 60 знаков".into());
        }
        let version_ok = !self.version.is_empty()
            && self.version.len() <= 32
            && self
                .version
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'+'));
        if !version_ok {
            return Err("version — например 1.0.0".into());
        }
        if self.description.chars().count() > 400 || self.author.chars().count() > 100 {
            return Err("description — до 400 знаков, author — до 100".into());
        }
        if !self.homepage.is_empty() {
            let ok = self.homepage.len() <= 300
                && url::Url::parse(&self.homepage)
                    .is_ok_and(|u| matches!(u.scheme(), "http" | "https"));
            if !ok {
                return Err("homepage — адрес http(s)".into());
            }
        }
        self.permissions.sort();
        self.permissions.dedup();
        if self.content_scripts.is_empty() {
            return Err("нет content_scripts: плагину нечего запускать".into());
        }
        if self.content_scripts.len() > MAX_SCRIPTS {
            return Err(format!("content_scripts — не больше {MAX_SCRIPTS}"));
        }
        for (i, s) in self.content_scripts.iter().enumerate() {
            let n = i + 1;
            if s.js.is_empty() && s.css.is_empty() {
                return Err(format!("content_scripts №{n}: нет ни js, ни css"));
            }
            if s.js.len() + s.css.len() > MAX_FILES {
                return Err(format!("content_scripts №{n}: больше {MAX_FILES} файлов"));
            }
            if s.matches.is_empty() || s.matches.len() + s.exclude.len() > MAX_PATTERNS {
                return Err(format!(
                    "content_scripts №{n}: в matches от 1 шаблона, всего с exclude — до {MAX_PATTERNS}"
                ));
            }
            for p in s.matches.iter().chain(&s.exclude) {
                Pattern::parse(p).map_err(|e| format!("content_scripts №{n}: {e}"))?;
            }
            for f in &s.js {
                file_name(f, &["js", "mjs"]).map_err(|e| format!("content_scripts №{n}: {e}"))?;
            }
            for f in &s.css {
                file_name(f, &["css"]).map_err(|e| format!("content_scripts №{n}: {e}"))?;
            }
        }
        if let Some(icon) = &self.icon {
            file_name(icon, IMAGES).map_err(|e| format!("icon: {e}"))?;
        }
        if let Some(action) = &mut self.action {
            action.title = action.title.trim().to_owned();
            if action.title.is_empty() || action.title.chars().count() > 60 {
                return Err("action.title — от 1 до 60 знаков".into());
            }
            if let Some(icon) = &action.icon {
                file_name(icon, IMAGES).map_err(|e| format!("action.icon: {e}"))?;
            }
        }
        if self.settings.len() > MAX_FIELDS {
            return Err(format!("settings — не больше {MAX_FIELDS}"));
        }
        let mut keys = std::collections::HashSet::new();
        for field in &mut self.settings {
            field.check()?;
            if !keys.insert(field.key.clone()) {
                return Err(format!("настройка «{}» повторяется", field.key));
            }
        }
        Ok(())
    }

    /// Плагину можно `perm`.
    pub fn allows(&self, perm: Permission) -> bool {
        self.permissions.contains(&perm)
    }
}

/// Ошибка разбора `plugin.json` по-русски и с местом в файле.
fn explain(e: &serde_json::Error) -> String {
    use serde_json::error::Category;
    let raw = e.to_string();
    let msg = raw.split(" at line ").next().unwrap_or(&raw);
    // `missing field `name`` → name
    let quoted = |s: &str| s.split('`').nth(1).unwrap_or("").to_owned();
    let what = match e.classify() {
        Category::Eof => "файл обрывается — не хватает закрывающей скобки или кавычки".to_owned(),
        Category::Syntax => "ошибка в записи JSON (запятая, кавычки, скобки)".to_owned(),
        Category::Io => "файл не читается".to_owned(),
        Category::Data if msg.starts_with("missing field") => format!("нет поля «{}»", quoted(msg)),
        Category::Data if msg.starts_with("duplicate field") => {
            format!("поле «{}» повторяется", quoted(msg))
        }
        Category::Data if msg.starts_with("unknown variant") => {
            let known: Vec<String> = msg
                .split("expected ")
                .nth(1)
                .unwrap_or("")
                .split('`')
                .skip(1)
                .step_by(2)
                .map(str::to_owned)
                .collect();
            format!(
                "неизвестное значение «{}», можно: {}",
                quoted(msg),
                known.join(", ")
            )
        }
        Category::Data if msg.starts_with("invalid type") => {
            let expected = msg.split("expected ").nth(1).unwrap_or("");
            let expected = match expected {
                "a string" => "строка в кавычках",
                "a boolean" => "true или false",
                "u32" | "u64" | "usize" | "f64" | "i64" => "число",
                "a sequence" => "список [ … ]",
                s if s.starts_with("struct") || s.starts_with("a map") => "объект { … }",
                other => other,
            };
            format!("неверный тип значения, нужно: {expected}")
        }
        Category::Data => msg.to_owned(),
    };
    format!(
        "ошибка в plugin.json: {what} (строка {}, столбец {})",
        e.line(),
        e.column()
    )
}

pub const IMAGES: &[&str] = &["png", "svg", "webp", "jpg", "jpeg", "gif", "ico"];

/// `id` плагина: от 2 до 40 знаков `[a-z0-9-]`, не с дефиса.
pub fn valid_id(id: &str) -> bool {
    (2..=40).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !id.starts_with('-')
}

/// Путь к файлу внутри папки плагина: относительный, без `..`, с нужным расширением.
pub fn file_name(rel: &str, exts: &[&str]) -> Result<(), String> {
    let bad = |why: &str| Err(format!("файл «{rel}»: {why}"));
    if rel.is_empty() || rel.len() > 200 {
        return bad("путь от 1 до 200 знаков");
    }
    if rel.contains('\\') || rel.contains(':') || rel.starts_with('/') {
        return bad("путь внутри папки плагина, через /");
    }
    let path = Path::new(rel);
    if !path.components().all(|c| matches!(c, Component::Normal(_))) {
        return bad("без .. и .");
    }
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    if !exts.contains(&ext.as_str()) {
        return bad(&format!("нужен файл .{}", exts.join(", .")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn parse(v: Value) -> Result<Manifest, String> {
        let mut m: Manifest = serde_json::from_value(v).map_err(|e| e.to_string())?;
        m.check()?;
        Ok(m)
    }

    fn base() -> Value {
        json!({
            "api": 1, "id": "word-count", "name": " Счётчик слов ", "version": "1.0.0",
            "content_scripts": [{ "matches": ["<all_urls>"], "js": ["content.js"] }]
        })
    }

    #[test]
    fn minimal_ok() {
        let m = parse(base());
        assert!(m.is_ok(), "{m:?}");
        let m = m.unwrap_or_else(|_| unreachable!());
        assert_eq!(m.name, "Счётчик слов");
        assert_eq!(m.content_scripts[0].run_at, RunAt::Ready);
    }

    #[test]
    fn chrome_names_accepted() {
        let mut v = base();
        v["content_scripts"][0]["run_at"] = json!("document_start");
        v["content_scripts"][0]["exclude_matches"] = json!(["*://example.com/*"]);
        let m = parse(v).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(m.content_scripts[0].run_at, RunAt::Start);
        assert_eq!(m.content_scripts[0].exclude.len(), 1);
    }

    #[test]
    fn rejects_bad_fields() {
        let cases: Vec<(&str, Value)> = vec![
            ("api", json!(2)),
            ("api", json!(0)),
            ("id", json!("Word Count")),
            ("id", json!("-x")),
            ("id", json!("x")),
            ("name", json!("")),
            ("version", json!("1 0")),
            ("homepage", json!("javascript:alert(1)")),
            ("permissions", json!(["everything"])),
            ("content_scripts", json!([])),
            (
                "content_scripts",
                json!([{ "matches": [], "js": ["a.js"] }]),
            ),
            ("content_scripts", json!([{ "matches": ["*"] }])),
            (
                "content_scripts",
                json!([{ "matches": ["ftp://x/*"], "js": ["a.js"] }]),
            ),
            (
                "content_scripts",
                json!([{ "matches": ["*"], "js": ["../a.js"] }]),
            ),
            (
                "content_scripts",
                json!([{ "matches": ["*"], "js": ["/etc/a.js"] }]),
            ),
            (
                "content_scripts",
                json!([{ "matches": ["*"], "js": ["C:/a.js"] }]),
            ),
            (
                "content_scripts",
                json!([{ "matches": ["*"], "js": ["a.exe"] }]),
            ),
            ("action", json!({ "title": "" })),
            ("icon", json!("logo.exe")),
        ];
        for (key, value) in cases {
            let mut v = base();
            v[key] = value.clone();
            assert!(parse(v).is_err(), "{key} = {value}");
        }
    }

    #[test]
    fn settings_defaults_and_values() {
        let mut v = base();
        v["settings"] = json!([
            { "key": "on", "type": "toggle", "label": "Вкл", "default": true },
            { "key": "name", "type": "text", "label": "Имя", "max_length": 5 },
            { "key": "size", "type": "number", "label": "Размер", "min": 10, "max": 20, "default": 99 },
            { "key": "mode", "type": "choice", "label": "Режим", "options": [
                { "value": "a", "label": "А" }, { "value": "b", "label": "Б" }
            ], "default": "zzz" }
        ]);
        let m = parse(v).unwrap_or_else(|e| panic!("{e}"));
        let f = &m.settings;
        assert_eq!(f[0].default, json!(true));
        assert_eq!(f[1].default, json!(""));
        assert_eq!(f[2].default, json!(10.0));
        assert_eq!(f[3].default, json!("a"));
        assert_eq!(f[1].clean(&json!("12345")), Some(json!("12345")));
        assert_eq!(f[1].clean(&json!("123456")), None);
        assert_eq!(f[2].clean(&json!(15)), Some(json!(15.0)));
        assert_eq!(f[2].clean(&json!(21)), None);
        assert_eq!(f[2].clean(&json!("15")), None);
        assert_eq!(f[3].clean(&json!("b")), Some(json!("b")));
        assert_eq!(f[3].clean(&json!("c")), None);
        assert_eq!(f[0].clean(&json!(1)), None);
    }

    #[test]
    fn settings_errors() {
        for bad in [
            json!([{ "key": "a b", "type": "toggle", "label": "x" }]),
            json!([{ "key": "a", "type": "toggle", "label": "" }]),
            json!([{ "key": "a", "type": "choice", "label": "x", "options": [] }]),
            json!([{ "key": "a", "type": "number", "label": "x", "min": 5, "max": 1 }]),
            json!([{ "key": "a", "type": "toggle", "label": "x" }, { "key": "a", "type": "toggle", "label": "y" }]),
            json!([{ "key": "a", "type": "slider", "label": "x" }]),
        ] {
            let mut v = base();
            v["settings"] = bad.clone();
            assert!(parse(v).is_err(), "{bad}");
        }
    }

    #[test]
    fn errors_in_russian() {
        let err = |text: &str| {
            serde_json::from_str::<Manifest>(text)
                .map(|_| String::new())
                .unwrap_or_else(|e| explain(&e))
        };
        assert!(
            err(r#"{ "api": 1 }"#).contains("нет поля «id»"),
            "{}",
            err(r#"{ "api": 1 }"#)
        );
        let variant =
            err(r#"{ "api": 1, "id": "a1", "name": "x", "version": "1", "permissions": ["all"] }"#);
        assert!(
            variant.contains("неизвестное значение «all», можно: notify, tabs"),
            "{variant}"
        );
        assert!(err(r#"{ "api": "1" }"#).contains("нужно: число"));
        assert!(err(r#"{ "api": 1, "#).contains("обрывается"));
        assert!(err(r#"{ "api": 1,, }"#).contains("строка 1"));
    }

    #[test]
    fn ids() {
        assert!(valid_id("ab"));
        assert!(valid_id("youtube-shorts-off"));
        assert!(valid_id("2fa"));
        assert!(!valid_id("A"));
        assert!(!valid_id("a_b"));
        assert!(!valid_id(&"a".repeat(41)));
    }
}
