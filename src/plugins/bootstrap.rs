//! Скрипт плагина для страницы: сведения о плагине, его скрипты и среда
//! `runtime.js` одним куском. Его ставят во вкладку как скрипт изолированного
//! мира, выполняется до скриптов сайта в каждом новом документе.

use serde_json::{Map, Value, json};

use super::{Plugin, manifest::RunAt};

const RUNTIME: &str = include_str!("runtime.js");

pub fn source(plugin: &Plugin, values: &Map<String, Value>, binding: &str) -> String {
    let m = &plugin.manifest;
    let p = json!({
        "id": m.id,
        "name": m.name,
        "version": m.version,
        "bridge": binding,
        "settings": values,
    });
    let s: Vec<Value> = plugin
        .scripts
        .iter()
        .map(|s| {
            json!({
                "m": s.matches.iter().map(|p| p.js()).collect::<Vec<_>>(),
                "x": s.exclude.iter().map(|p| p.js()).collect::<Vec<_>>(),
                "at": match s.run_at {
                    RunAt::Start => "start",
                    RunAt::Ready => "ready",
                    RunAt::Load => "load",
                },
                "all": s.all_frames,
                "css": s.css,
            })
        })
        .collect();
    let code: usize = plugin
        .scripts
        .iter()
        .map(|s| s.js.len() + s.css.len())
        .sum();
    let mut out = String::with_capacity(code + RUNTIME.len() + 1024);
    out.push_str("(function () {\nconst P = ");
    out.push_str(&p.to_string());
    out.push_str(";\nconst S = ");
    out.push_str(&Value::Array(s).to_string());
    // Код плагина — функциями: запустит их среда, когда придёт время (`run_at`).
    // Без 'use strict' снаружи: код плагина сам решает, строгий он или нет.
    out.push_str(";\nconst RUN = [\n");
    for s in &plugin.scripts {
        if s.js.is_empty() {
            out.push_str("null,\n");
        } else {
            out.push_str("function () {\n");
            out.push_str(&s.js);
            out.push_str("\n},\n");
        }
    }
    out.push_str("];\n");
    out.push_str(RUNTIME);
    out.push_str("\n})();\n//# sourceURL=axiom-plugin/");
    out.push_str(&m.id);
    out.push_str(".js\n");
    out
}
