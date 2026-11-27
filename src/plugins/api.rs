//! Запросы скриптов плагина к браузеру (`axiom.*` в `runtime.js`) и ответы на них.
//! Запрос приходит из страницы, поэтому разбирается строго, а разрешения
//! проверяет `App::on_plugin`, а не скрипт.

use serde::Deserialize;
use serde_json::{Map, Value, json};

/// Больше одного сообщения плагина не разбираем (хранилище всё равно до 1 МБ).
pub const MAX_MESSAGE: usize = (1 << 20) + 64 * 1024;
/// Сообщение внизу окна.
pub const MAX_NOTIFY: usize = 300;
pub const MAX_URL: usize = 8 * 1024;
const MAX_KEYS: usize = 1000;

#[derive(Debug, Deserialize)]
pub struct Request {
    /// Номер запроса для ответа; у `hello` его нет.
    #[serde(default)]
    pub id: Option<u64>,
    #[serde(flatten)]
    pub op: Op,
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(tag = "op")]
pub enum Op {
    /// Скрипт плагина запустился в самой странице: сюда доставлять события.
    #[serde(rename = "hello")]
    Hello {},
    #[serde(rename = "storage.get")]
    StorageGet { keys: Option<Vec<String>> },
    #[serde(rename = "storage.set")]
    StorageSet { items: Map<String, Value> },
    #[serde(rename = "storage.remove")]
    StorageRemove { keys: Vec<String> },
    #[serde(rename = "storage.clear")]
    StorageClear {},
    #[serde(rename = "notify")]
    Notify { text: String },
    #[serde(rename = "tabs.open")]
    TabsOpen {
        url: String,
        #[serde(default)]
        background: bool,
    },
}

impl Request {
    pub fn parse(raw: &str) -> Result<Self, String> {
        if raw.len() > MAX_MESSAGE {
            return Err("сообщение плагина слишком большое".into());
        }
        let req: Self = serde_json::from_str(raw).map_err(|e| format!("неверный запрос: {e}"))?;
        match &req.op {
            Op::StorageGet { keys: Some(keys) } | Op::StorageRemove { keys }
                if keys.len() > MAX_KEYS =>
            {
                Err("слишком много ключей".into())
            }
            _ => Ok(req),
        }
    }
}

/// Ответ на запрос `id`: значение или текст ошибки (станет `Error` в обещании).
pub fn reply(id: u64, result: Result<Value, String>) -> String {
    match result {
        Ok(value) => json!({ "id": id, "ok": value }),
        Err(error) => json!({ "id": id, "error": error }),
    }
    .to_string()
}

/// Событие для скрипта: `action` — нажали кнопку плагина, `settings` — новые настройки.
pub fn event(name: &str, values: Option<&Map<String, Value>>) -> String {
    match values {
        Some(values) => json!({ "event": name, "values": values }),
        None => json!({ "event": name }),
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ops() {
        let ok = |raw: &str| Request::parse(raw).map(|r| (r.id, r.op));
        assert_eq!(ok(r#"{"op":"hello"}"#), Ok((None, Op::Hello {})));
        assert_eq!(
            ok(r#"{"id":3,"op":"storage.get","keys":null}"#),
            Ok((Some(3), Op::StorageGet { keys: None }))
        );
        assert_eq!(
            ok(r#"{"id":4,"op":"tabs.open","url":"https://a.b/"}"#),
            Ok((
                Some(4),
                Op::TabsOpen {
                    url: "https://a.b/".into(),
                    background: false
                }
            ))
        );
        assert_eq!(
            ok(r#"{"id":5,"op":"storage.clear"}"#),
            Ok((Some(5), Op::StorageClear {}))
        );
        assert!(Request::parse(r#"{"id":1,"op":"history.read"}"#).is_err());
        assert!(Request::parse(r#"{"id":1,"op":"notify"}"#).is_err());
        assert!(Request::parse("[1]").is_err());
        let many = format!(
            r#"{{"id":1,"op":"storage.remove","keys":[{}]}}"#,
            vec!["\"k\""; 2000].join(",")
        );
        assert!(Request::parse(&many).is_err());
    }

    #[test]
    fn replies() {
        let back = |s: String| serde_json::from_str::<Value>(&s).unwrap_or_default();
        assert_eq!(
            back(reply(1, Ok(json!({"a": 1})))),
            json!({"id": 1, "ok": {"a": 1}})
        );
        assert_eq!(
            back(reply(2, Err("нет".into()))),
            json!({"id": 2, "error": "нет"})
        );
        assert_eq!(back(event("action", None)), json!({"event": "action"}));
    }
}
