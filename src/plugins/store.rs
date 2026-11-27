//! Хранилище плагина (`axiom.storage`): ключи и значения JSON в
//! `<профиль>/plugin-data/<id>.json`. У каждого плагина своё, до 1 МБ.

use std::path::PathBuf;

use serde_json::{Map, Value};

use crate::storage;

/// Всё хранилище одного плагина в JSON.
pub const MAX_BYTES: usize = 1 << 20;
const MAX_KEY: usize = 200;

pub struct Store {
    path: PathBuf,
    data: Map<String, Value>,
}

impl Store {
    pub fn open(path: PathBuf) -> Self {
        let data = storage::read_json(&path).unwrap_or_default();
        Self { path, data }
    }

    /// `None` — всё хранилище, иначе только эти ключи (которых нет — пропускаются).
    pub fn get(&self, keys: Option<&[String]>) -> Value {
        let map = match keys {
            None => self.data.clone(),
            Some(keys) => keys
                .iter()
                .filter_map(|k| Some((k.clone(), self.data.get(k)?.clone())))
                .collect(),
        };
        Value::Object(map)
    }

    pub fn set(&mut self, items: Map<String, Value>) -> Result<(), String> {
        if let Some(key) = items.keys().find(|k| k.is_empty() || k.len() > MAX_KEY) {
            return Err(format!(
                "ключ «{}»: от 1 до {MAX_KEY} знаков",
                truncate(key)
            ));
        }
        let mut next = self.data.clone();
        for (k, v) in items {
            // `undefined` из JSON не приходит, а `null` — значение как значение.
            next.insert(k, v);
        }
        let size = serde_json::to_vec(&next).map_or(usize::MAX, |b| b.len());
        if size > MAX_BYTES {
            return Err(format!("хранилище плагина больше {} МБ", MAX_BYTES >> 20));
        }
        self.data = next;
        self.save()
    }

    pub fn remove(&mut self, keys: &[String]) -> Result<(), String> {
        let before = self.data.len();
        for k in keys {
            self.data.remove(k);
        }
        if self.data.len() == before {
            return Ok(());
        }
        self.save()
    }

    pub fn clear(&mut self) -> Result<(), String> {
        if self.data.is_empty() {
            return Ok(());
        }
        self.data.clear();
        self.save()
    }

    fn save(&self) -> Result<(), String> {
        if let Some(dir) = self.path.parent() {
            storage::private_dir(dir).map_err(|e| format!("хранилище не сохранено: {e}"))?;
        }
        storage::write_json_compact(&self.path, &self.data)
            .map_err(|e| format!("хранилище не сохранено: {e:#}"))
    }
}

fn truncate(s: &str) -> String {
    s.chars().take(40).collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn set_get_remove_limit() {
        let dir = std::env::temp_dir().join(format!("axiom-store-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("plugin-data").join("x.json");
        let mut store = Store::open(path.clone());
        assert_eq!(store.get(None), json!({}));
        let items = |v: Value| v.as_object().cloned().unwrap_or_default();
        assert!(
            store
                .set(items(json!({ "a": 1, "b": { "c": [1, 2] } })))
                .is_ok()
        );
        assert_eq!(
            store.get(Some(&["a".into(), "zzz".into()])),
            json!({ "a": 1 })
        );
        // Переживает перезапуск.
        let again = Store::open(path.clone());
        assert_eq!(again.get(None), json!({ "a": 1, "b": { "c": [1, 2] } }));
        assert!(store.remove(&["a".into()]).is_ok());
        assert_eq!(store.get(None), json!({ "b": { "c": [1, 2] } }));
        // Больше мегабайта — отказ, прежние данные целы.
        let big = "x".repeat(MAX_BYTES);
        assert!(store.set(items(json!({ "big": big }))).is_err());
        assert_eq!(store.get(None), json!({ "b": { "c": [1, 2] } }));
        assert!(store.set(items(json!({ "": 1 }))).is_err());
        assert!(store.clear().is_ok());
        assert_eq!(Store::open(path).get(None), json!({}));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
