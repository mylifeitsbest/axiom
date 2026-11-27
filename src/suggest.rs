//! Подсказки поисковой системы в адресной строке: набрали «пр» — выбранная
//! система предлагает «привет», «прогноз погоды»… Набранное уходит в эту
//! систему ещё до Enter, поэтому подсказки выключаются в настройках
//! (`search_suggest`) и описаны в PRIVACY.md. Адреса (`youtube.com/…`)
//! и длинный вставленный текст никуда не уходят.
//!
//! Сеть — системный curl в своём потоке, как у перевода и обновлений. В пути
//! один запрос: пока он идёт, копится только последний набранный текст, так что
//! быстрый набор не превращается в очередь запросов.

use std::{
    collections::VecDeque,
    io::Read,
    process::Stdio,
    sync::{Arc, Condvar, Mutex, PoisonError},
    thread,
};

use serde_json::Value;
use tao::event_loop::EventLoopProxy;
use url::Url;

use crate::{
    events::UserEvent,
    tabs::omnibox::{self, Engine, Target},
};

/// Длиннее — это уже не набранный руками запрос, а вставленный текст: не отправляем.
const MAX_QUERY: usize = 120;
/// Сколько подсказок брать из ответа.
const MAX_ITEMS: usize = 8;
const MAX_ITEM: usize = 200;
const MAX_RESPONSE: u64 = 64 * 1024;
/// Запомненные ответы: стёрли букву и набрали снова — подсказки сразу, без сети.
const CACHE: usize = 64;

/// Ответ системы на набранный текст.
#[derive(Debug)]
pub struct Answer {
    pub engine: Engine,
    pub q: String,
    pub items: Vec<String>,
}

struct Job {
    engine: Engine,
    q: String,
    url: String,
    agent: String,
}

#[derive(Default)]
struct Slot {
    job: Mutex<Option<Job>>,
    ready: Condvar,
}

#[derive(Default)]
pub struct Remote {
    slot: Arc<Slot>,
    started: bool,
    cache: VecDeque<(Engine, String, Vec<String>)>,
}

impl Remote {
    /// Подсказки для `q`. `Some` — ответ уже есть: из памяти или пустой, если такой
    /// ввод отправлять нельзя. `None` — запрос ушёл в фоне, ответ придёт событием
    /// [`UserEvent::Completions`].
    pub fn ask(
        &mut self,
        engine: Engine,
        q: &str,
        events: &EventLoopProxy<UserEvent>,
    ) -> Option<Vec<String>> {
        let q = q.trim();
        if !sendable(q) {
            return Some(Vec::new());
        }
        let Some(url) = endpoint(engine, q) else {
            return Some(Vec::new());
        };
        if let Some((.., items)) = self.cache.iter().find(|(e, c, _)| *e == engine && c == q) {
            return Some(items.clone());
        }
        if !self.started {
            let slot = self.slot.clone();
            let events = events.clone();
            let spawned = thread::Builder::new()
                .name("suggest".into())
                .spawn(move || run(&slot, &events));
            if let Err(err) = spawned {
                eprintln!("подсказки поиска: поток не запустился: {err}");
                return Some(Vec::new());
            }
            self.started = true;
        }
        let job = Job {
            engine,
            q: q.to_string(),
            url,
            agent: crate::tabs::identity::browser_agent(),
        };
        *self.slot.job.lock().unwrap_or_else(PoisonError::into_inner) = Some(job);
        self.slot.ready.notify_one();
        None
    }

    /// Запоминает пришедший ответ.
    pub fn remember(&mut self, answer: &Answer) {
        if self
            .cache
            .iter()
            .any(|(e, q, _)| *e == answer.engine && *q == answer.q)
        {
            return;
        }
        if self.cache.len() >= CACHE {
            self.cache.pop_front();
        }
        self.cache
            .push_back((answer.engine, answer.q.clone(), answer.items.clone()));
    }

    /// Другая система или подсказки выключили: старые ответы больше не нужны.
    pub fn forget(&mut self) {
        self.cache.clear();
        self.slot
            .job
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
    }
}

/// Можно ли отправить набранное: это поиск, а не адрес, и не слишком длинный.
fn sendable(q: &str) -> bool {
    !q.is_empty()
        && q.chars().count() <= MAX_QUERY
        && !q.chars().any(char::is_control)
        && matches!(omnibox::resolve(q), Some(Target::Search(_)))
}

/// Адрес подсказок системы (формат OpenSearch: `["запрос", ["подсказка", …]]`).
/// У своего адреса поиска подсказок нет.
fn endpoint(engine: Engine, q: &str) -> Option<String> {
    let (base, key, extra): (&str, &str, &[(&str, &str)]) = match engine {
        Engine::DuckDuckGo => ("https://duckduckgo.com/ac/", "q", &[("type", "list")]),
        Engine::Google => (
            "https://www.google.com/complete/search",
            "q",
            &[
                ("client", "firefox"),
                ("hl", "ru"),
                ("ie", "utf-8"),
                ("oe", "utf-8"),
            ],
        ),
        Engine::Yandex => (
            "https://suggest.yandex.ru/suggest-ff.cgi",
            "part",
            &[("uil", "ru")],
        ),
        Engine::Bing => ("https://www.bing.com/osjson.aspx", "query", &[]),
        Engine::Brave => ("https://search.brave.com/api/suggest", "q", &[]),
        Engine::Startpage => ("https://www.startpage.com/osuggestions", "q", &[]),
        Engine::Ecosia => (
            "https://ac.ecosia.org/autocomplete",
            "q",
            &[("type", "list")],
        ),
        Engine::Custom => return None,
    };
    let mut url = Url::parse(base).ok()?;
    url.query_pairs_mut()
        .extend_pairs(extra)
        .append_pair(key, q);
    Some(url.into())
}

/// Подсказки из ответа: без повторов, без самого набранного и без мусора.
fn parse(body: &str, q: &str) -> Vec<String> {
    let Ok(value) = serde_json::from_str::<Value>(body) else {
        return Vec::new();
    };
    let Some(list) = value.get(1).and_then(Value::as_array) else {
        return Vec::new();
    };
    let typed = q.trim().to_lowercase();
    let mut out: Vec<String> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for raw in list.iter().filter_map(Value::as_str) {
        let item = raw.split_whitespace().collect::<Vec<_>>().join(" ");
        if item.is_empty() || item.chars().count() > MAX_ITEM || item.chars().any(char::is_control)
        {
            continue;
        }
        let low = item.to_lowercase();
        if low == typed || seen.contains(&low) {
            continue;
        }
        seen.push(low);
        out.push(item);
        if out.len() >= MAX_ITEMS {
            break;
        }
    }
    out
}

fn run(slot: &Slot, events: &EventLoopProxy<UserEvent>) {
    loop {
        let job = {
            let mut guard = slot.job.lock().unwrap_or_else(PoisonError::into_inner);
            loop {
                if let Some(job) = guard.take() {
                    break job;
                }
                guard = slot
                    .ready
                    .wait(guard)
                    .unwrap_or_else(PoisonError::into_inner);
            }
        };
        // Нет сети или система не ответила — подсказок просто нет, в журнал
        // не пишем: в нём не место набранному тексту.
        let Some(body) = fetch(&job) else {
            continue;
        };
        let answer = Answer {
            items: parse(&body, &job.q),
            engine: job.engine,
            q: job.q,
        };
        if events.send_event(UserEvent::Completions(answer)).is_err() {
            return;
        }
    }
}

fn fetch(job: &Job) -> Option<String> {
    let mut cmd = crate::stats::curl();
    // Повторные ключи curl заменяют прежние: свои сроки и строка браузера.
    cmd.args([
        "--fail",
        "--location",
        "--max-redirs",
        "3",
        "--proto",
        "=https",
        "--proto-redir",
        "=https",
        "--connect-timeout",
        "3",
        "--max-time",
        "5",
        "-H",
        "Accept: application/json, text/javascript, */*",
        "-H",
        "Accept-Language: ru-RU,ru;q=0.9,en;q=0.8",
    ])
    .arg("--user-agent")
    .arg(&job.agent)
    .arg(&job.url)
    .stdout(Stdio::piped());
    let mut child = cmd.spawn().ok()?;
    let mut out = Vec::new();
    if let Some(stdout) = child.stdout.take() {
        let _ = stdout.take(MAX_RESPONSE + 1).read_to_end(&mut out);
    }
    if out.len() as u64 > MAX_RESPONSE {
        let _ = child.kill();
        let _ = child.wait();
        return None;
    }
    let status = child.wait().ok()?;
    status
        .success()
        .then(|| String::from_utf8_lossy(&out).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sends_only_searches() {
        assert!(sendable("пр"));
        assert!(sendable("привет мир"));
        assert!(!sendable(""));
        assert!(!sendable("youtube.com"));
        assert!(!sendable("https://example.com/?q=секрет"));
        assert!(!sendable("localhost:8080"));
        assert!(!sendable("192.168.1.1"));
        assert!(!sendable(&"а".repeat(MAX_QUERY + 1)));
        assert!(!sendable("строка\nдве"));
    }

    #[test]
    fn endpoints_carry_the_query() -> Result<(), url::ParseError> {
        for engine in [
            Engine::DuckDuckGo,
            Engine::Google,
            Engine::Yandex,
            Engine::Bing,
            Engine::Brave,
            Engine::Startpage,
            Engine::Ecosia,
        ] {
            let Some(raw) = endpoint(engine, "пр & q=1") else {
                panic!("{engine:?}: нет адреса подсказок");
            };
            let url = Url::parse(&raw)?;
            assert_eq!(url.scheme(), "https", "{raw}");
            assert!(
                url.query_pairs().any(|(_, v)| v == "пр & q=1"),
                "{engine:?}: запрос не на месте: {raw}"
            );
        }
        assert!(endpoint(Engine::Custom, "пр").is_none());
        Ok(())
    }

    #[test]
    fn parses_opensearch_answers() {
        let google = r#"["пр",["привет","прогноз погоды","Привет","пр","  прокси \n для тг "],[],{"google:suggestsubtypes":[]}]"#;
        assert_eq!(
            parse(google, "пр"),
            ["привет", "прогноз погоды", "прокси для тг"]
        );
        // Startpage присылает \u-escapes.
        assert_eq!(parse(r#"["пр",["привет"]]"#, "пр"), ["привет"]);
        assert!(parse("", "пр").is_empty());
        assert!(parse("<html>", "пр").is_empty());
        assert!(parse(r#"{"a":1}"#, "пр").is_empty());
        assert!(parse(r#"["пр",[1,null,{"x":2}]]"#, "пр").is_empty());
        let many = format!(
            r#"["a",[{}]]"#,
            (0..20)
                .map(|i| format!("\"a{i}\""))
                .collect::<Vec<_>>()
                .join(",")
        );
        assert_eq!(parse(&many, "a").len(), MAX_ITEMS);
        let long = format!(
            r#"["a",["{}","ok\u0000x","fine"]]"#,
            "a".repeat(MAX_ITEM + 1)
        );
        assert_eq!(parse(&long, "a"), ["fine"]);
    }
}
