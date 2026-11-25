//! Перевод страниц через Google Переводчик, как в Chrome.
//!
//! Всё ведёт Rust: вставляет в страницу скрипт (`page.js`), забирает у него текст
//! порциями, переводит в фоновом потоке и возвращает перевод. Канала «страница →
//! браузер» нет: страница только отвечает на вызовы Rust, а когда, сколько и как
//! часто спрашивать, решает он сам ([`Limiter`], [`Batch::parse`]). Поэтому сайт не
//! может ни заставить браузер слать запросы, ни занять его работой.
//!
//! Текст страницы уходит в Google, только когда человек нажал «Перевести» или
//! сам включил «Всегда переводить» для этого языка. Сам по себе — никогда.

pub mod google;
pub mod lang;

use std::{
    collections::{HashMap, VecDeque},
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};

use crate::tabs::TabId;

/// Скрипт перевода для страницы. Вставляется только при переводе.
const PAGE: &str = include_str!("page.js");

/// Вопрос странице после загрузки: язык из атрибута, образец текста и не
/// переведена ли она уже (страница из кэша «назад-вперёд» или переход по якорю).
pub const PROBE: &str = r#"(() => {
  const t = window.__axiomTr;
  const root = document.querySelector('main,article,[role=main]') || document.body;
  const skip = 'script,style,noscript,template,code,pre,textarea,svg,[translate="no"],.notranslate';
  let text = '';
  if (root) {
    const w = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
    for (let n = w.nextNode(), i = 0; n && text.length < 2000 && i < 5000; n = w.nextNode(), i++) {
      const p = n.parentElement;
      if (p && /\p{L}/u.test(n.nodeValue) && !p.closest(skip)) text += n.nodeValue.replace(/\s+/g, ' ') + ' ';
    }
  }
  const meta = document.querySelector('meta[http-equiv="content-language" i]');
  const lang = document.documentElement.getAttribute('lang') || (meta && meta.content) || '';
  let active = false;
  try { active = !!(t && t.active()); } catch { active = false; }
  return { lang: String(lang).slice(0, 40), text: text.slice(0, 2000), active };
})()"#;

/// Вернуть оригинал и снять наблюдение за страницей.
pub const REVERT: &str = "window.__axiomTr&&window.__axiomTr.revert()";

/// Языки, на которые переводим (код Google, кроме `zh` → `zh-CN`).
pub const TARGETS: [&str; 10] = ["ru", "en", "uk", "de", "fr", "es", "zh", "ja", "ko", "tr"];

/// Больше текста с одной страницы не отправляем (вместе с подгруженным позже).
pub const MAX_PAGE_CHARS: usize = 200_000;
/// Первая порция — маленькая: видимая часть страницы переводится за секунду.
const FIRST_BATCH: usize = 5_000;
/// Остальные порции. 20 тысяч иероглифов — около 200 КБ запроса: Google такое принимает.
const BATCH: usize = 20_000;
/// Один текстовый узел длиннее — не переводим (страница не может прислать огромный кусок).
const MAX_NODE_CHARS: usize = 8_000;
/// Узлов в одном фрагменте и фрагментов в порции.
const MAX_GROUP_ITEMS: usize = 80;
const MAX_GROUP_CHARS: usize = 4_000;
const MAX_GROUPS: usize = 2_000;
/// Одновременных запросов от одной вкладки и от всего браузера.
pub const MAX_PARALLEL: usize = 2;
pub const MAX_RUNNING: usize = 4;
/// Не больше запросов за минуту от одной вкладки и от всего браузера: страница,
/// которая перезагружает себя по кругу, каждый раз получала бы новый запас текста.
const MAX_PER_MINUTE: usize = 20;
pub const MAX_GLOBAL_PER_MINUTE: usize = 60;
/// Сколько раз за минуту переводить заново сам, если страница перезагрузилась.
pub const MAX_RESUMES_PER_MINUTE: usize = 5;
/// Как часто проверять, не появился ли на переведённой странице новый текст.
pub const POLL: Duration = Duration::from_millis(1500);
/// Ответ страницы потерялся (WebKitGTK молча роняет вызов во время загрузки).
pub const ASK_TIMEOUT: Duration = Duration::from_secs(10);
/// Сколько раз спрашивать язык у страницы, которая ещё пуста (сайт рисует себя скриптом).
pub const MAX_PROBES: u8 = 3;
/// Подряд неудачных запросов, после которых бросаем дозапросы нового текста.
pub const MAX_ERRORS: u8 = 3;

/// Настройки перевода (часть settings.json).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Prefs {
    /// На какой язык переводить.
    pub to: String,
    /// Языки, которые переводятся сами, без нажатия.
    pub always: Vec<String>,
    /// Показывать кнопку перевода на страницах не на языке `to`.
    pub offer: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            to: "ru".into(),
            always: Vec::new(),
            offer: true,
        }
    }
}

const MAX_ALWAYS: usize = 50;

impl Prefs {
    /// Присланное интерфейсом: язык перевода из списка, коды языков без мусора.
    pub fn is_valid(&self) -> bool {
        TARGETS.contains(&self.to.as_str())
            && self.always.len() <= MAX_ALWAYS
            && self
                .always
                .iter()
                .all(|l| lang::normalize(l).as_deref() == Some(l.as_str()))
    }

    /// Чистит прочитанное с диска и убирает повторы.
    pub fn sanitized(mut self) -> Self {
        if !TARGETS.contains(&self.to.as_str()) {
            self.to = Prefs::default().to;
        }
        let mut seen = Vec::new();
        for code in self.always.iter().filter_map(|l| lang::normalize(l)) {
            if !seen.contains(&code) && code != self.to && seen.len() < MAX_ALWAYS {
                seen.push(code);
            }
        }
        self.always = seen;
        self
    }

    pub fn auto(&self, lang: &str) -> bool {
        self.always.iter().any(|l| lang::same(l, lang))
    }
}

/// Результаты вызовов страницы и фоновых потоков.
#[derive(Debug)]
pub enum Event {
    /// Ответ на [`PROBE`].
    Probed { tab: TabId, epoch: u64, raw: String },
    /// Страница отдала порцию текста (ответ на `take`).
    Taken {
        tab: TabId,
        epoch: u64,
        raw: String,
        limit: usize,
    },
    /// Google перевёл порцию (или не смог).
    Translated {
        tab: TabId,
        epoch: u64,
        result: Result<google::Output, String>,
    },
    /// Пора снова спросить страницу (язык ещё не ясен или ждём нового текста).
    Poll { tab: TabId, epoch: u64 },
}

/// Что видит интерфейс у кнопки перевода.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// Кнопки нет: страница на нужном языке, язык не ясен или предлагать не просили.
    None,
    /// Можно перевести.
    Offer,
    /// Переводим.
    Busy,
    /// Переведено.
    Done,
}

#[derive(Debug, Serialize)]
pub struct View<'a> {
    pub id: TabId,
    pub phase: Phase,
    /// Язык оригинала; пусто или `und` — неизвестен.
    pub lang: &'a str,
    /// На какой язык переведена (или будет переведена) страница.
    pub to: &'a str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// Ждём ответа о языке.
    Probing,
    /// Уже на нужном языке (или язык не определить).
    Native,
    /// На другом языке: предлагаем перевод.
    Foreign,
    Busy,
    Done,
}

/// Состояние перевода одной вкладки.
#[derive(Debug)]
pub struct Page {
    /// Номер поколения: ответы для прежней страницы или прежнего перевода отбрасываются.
    pub epoch: u64,
    /// Документ без `#якоря`: переход по якорю не сбрасывает перевод.
    pub url: String,
    pub lang: String,
    pub stage: Stage,
    pub to: String,
    pub limiter: Limiter,
    pub in_flight: usize,
    pub asking: Option<Instant>,
    pub polling: bool,
    pub probes: u8,
    /// Документ начал грузиться заново (F5, пробуждение, якорь): перевод продолжить.
    pub resume: bool,
    /// Сколько раз переводили заново после перезагрузки (окно в минуту).
    pub resumes: Window,
    /// От Google ещё не было ответа на этой странице.
    pub first: bool,
    /// Про предел размера уже сказали.
    pub capped: bool,
    pub errors: u8,
}

impl Page {
    pub fn new(epoch: u64, url: String) -> Self {
        Self {
            epoch,
            url,
            lang: String::new(),
            stage: Stage::Probing,
            to: String::new(),
            limiter: Limiter::default(),
            in_flight: 0,
            asking: None,
            polling: false,
            probes: 0,
            resume: false,
            resumes: Window::default(),
            first: true,
            capped: false,
            errors: 0,
        }
    }

    pub fn translating(&self) -> bool {
        matches!(self.stage, Stage::Busy | Stage::Done)
    }

    /// Начать перевод заново (новое поколение): прежние ответы больше не нужны.
    pub fn begin(&mut self, epoch: u64, to: &str) {
        self.epoch = epoch;
        self.stage = Stage::Busy;
        self.to = to.to_string();
        self.limiter = Limiter::default();
        self.in_flight = 0;
        self.asking = None;
        self.polling = false;
        self.resume = false;
        self.first = true;
        self.capped = false;
        self.errors = 0;
    }

    /// Перевод выключен: новое поколение, язык прежний.
    pub fn stop(&mut self, epoch: u64, stage: Stage) {
        self.epoch = epoch;
        self.stage = stage;
        self.in_flight = 0;
        self.asking = None;
        self.polling = false;
        self.resume = false;
    }

    pub fn phase(&self, prefs: &Prefs) -> Phase {
        match self.stage {
            Stage::Probing | Stage::Native => Phase::None,
            Stage::Foreign if prefs.offer => Phase::Offer,
            Stage::Foreign => Phase::None,
            Stage::Busy => Phase::Busy,
            Stage::Done => Phase::Done,
        }
    }

    /// Сколько текста попросить у страницы. `None` — сейчас нельзя.
    pub fn next_limit(&mut self, now: Instant) -> Option<usize> {
        if !self.translating() || self.in_flight >= MAX_PARALLEL || !self.limiter.allow(now) {
            return None;
        }
        if self
            .asking
            .is_some_and(|at| now.duration_since(at) < ASK_TIMEOUT)
        {
            return None;
        }
        let batch = if self.limiter.sent == 0 {
            FIRST_BATCH
        } else {
            BATCH
        };
        Some(batch.min(self.limiter.left())).filter(|&n| n > 0)
    }
}

/// Скрипт, который ставит перевод на страницу: вернуть оригинал, если он был, и начать заново.
pub fn start_script() -> String {
    format!("{PAGE}\n;window.__axiomTr.revert();window.__axiomTr.start();")
}

pub fn take_script(limit: usize) -> String {
    format!("window.__axiomTr?window.__axiomTr.take({limit}):null")
}

/// Перевод — странице. Простой текст в JSON: никакого HTML страница не разбирает.
pub fn apply_script(items: &[(u64, String)]) -> Option<String> {
    let json = serde_json::to_string(items).ok()?;
    Some(format!("window.__axiomTr&&window.__axiomTr.apply({json})"))
}

/// Ограничения одной страницы: всего текста и запросов в минуту.
#[derive(Debug, Default)]
pub struct Limiter {
    pub sent: usize,
    recent: Window,
}

impl Limiter {
    pub fn allow(&mut self, now: Instant) -> bool {
        self.recent.allow(now, MAX_PER_MINUTE) && self.left() > 0
    }

    pub fn left(&self) -> usize {
        MAX_PAGE_CHARS.saturating_sub(self.sent)
    }

    pub fn record(&mut self, now: Instant, chars: usize) {
        self.sent += chars;
        self.recent.record(now);
    }
}

/// Скользящее окно в минуту: сколько раз что-то случилось.
#[derive(Debug, Default)]
pub struct Window {
    recent: VecDeque<Instant>,
}

impl Window {
    pub fn allow(&mut self, now: Instant, max: usize) -> bool {
        while self
            .recent
            .front()
            .is_some_and(|&at| now.duration_since(at) >= Duration::from_secs(60))
        {
            self.recent.pop_front();
        }
        self.recent.len() < max
    }

    pub fn record(&mut self, now: Instant) {
        self.recent.push_back(now);
    }
}

/// Фрагмент страницы (обычно абзац): номера текстовых узлов и их текст.
pub type Group = Vec<(u64, String)>;

/// Порция текста от страницы после проверки.
#[derive(Debug, Default, PartialEq)]
pub struct Batch {
    pub groups: Vec<Group>,
    pub chars: usize,
    pub more: bool,
}

impl Batch {
    /// Ответ страницы на `take(limit)` — чужие данные: размер, число узлов и
    /// длину текста ограничиваем сами, что не влезло — отбрасываем.
    pub fn parse(raw: &str, limit: usize) -> Self {
        #[derive(Deserialize)]
        struct Raw {
            #[serde(default)]
            b: Vec<Vec<(u64, String)>>,
            #[serde(default)]
            more: bool,
        }
        // Узлы по 4 байта UTF-8 плюс экранирование и номера — с запасом.
        if raw.len() > (limit + MAX_GROUP_CHARS + MAX_NODE_CHARS) * 8 + 64 * 1024 {
            return Self::default();
        }
        let Ok(page) = serde_json::from_str::<Raw>(raw.trim()) else {
            return Self::default();
        };
        // Последний фрагмент может немного перелезть за предел — как и у страницы.
        let budget = limit + MAX_GROUP_CHARS;
        let mut batch = Self {
            more: page.more,
            ..Self::default()
        };
        for group in page.b.into_iter().take(MAX_GROUPS) {
            let group: Group = group
                .into_iter()
                .take(MAX_GROUP_ITEMS)
                .filter(|(_, text)| {
                    !text.trim().is_empty() && text.chars().count() <= MAX_NODE_CHARS
                })
                .collect();
            let size: usize = group.iter().map(|(_, t)| t.chars().count()).sum();
            // Запятые и числа между ссылками идут вместе с абзацем, но фрагмент без букв не нужен.
            let letters = group
                .iter()
                .any(|(_, t)| t.chars().any(char::is_alphabetic));
            if !letters || size > MAX_GROUP_CHARS + MAX_NODE_CHARS {
                continue;
            }
            // Первый фрагмент берём всегда: иначе один длинный абзац остановил бы перевод.
            if !batch.groups.is_empty() && batch.chars + size > budget {
                batch.more = true;
                break;
            }
            batch.chars += size;
            batch.groups.push(group);
        }
        batch
    }
}

/// Ответ на [`PROBE`].
#[derive(Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct Probe {
    pub lang: String,
    pub text: String,
    pub active: bool,
}

impl Probe {
    pub fn parse(raw: &str) -> Self {
        let mut probe: Self = serde_json::from_str(raw.trim()).unwrap_or_default();
        probe.lang = probe.lang.chars().take(40).collect();
        probe.text = probe.text.chars().take(2000).collect();
        probe
    }
}

/// Адрес документа без якоря.
pub fn document_url(url: &str) -> String {
    url.split('#').next().unwrap_or(url).to_string()
}

/// Состояния всех вкладок.
#[derive(Debug, Default)]
pub struct Translator {
    pub pages: HashMap<TabId, Page>,
    next_epoch: u64,
    /// Запросов к Google сейчас в работе (по всему браузеру).
    pub running: usize,
    /// Запросы к Google за последнюю минуту (по всему браузеру).
    pub recent: Window,
}

impl Translator {
    pub fn epoch(&mut self) -> u64 {
        self.next_epoch += 1;
        self.next_epoch
    }

    /// Состояние вкладки, если ответ относится к нему (то же поколение).
    pub fn current(&mut self, tab: TabId, epoch: u64) -> Option<&mut Page> {
        self.pages.get_mut(&tab).filter(|p| p.epoch == epoch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_keeps_fields_private() {
        // Подсказки полей пароля и скрытые поля в перевод не уходят — и перед отправкой тоже.
        assert!(PAGE.contains(r#"input[type="password" i],input[type="hidden" i]"#));
        assert!(PAGE.contains("if (el.closest(SKIP_ATTR)) return [];"));
        // Значение поля переводится только у кнопок.
        assert!(PAGE.contains("/^(submit|button|reset)$/i.test(el.type)"));
    }

    #[test]
    fn prefs_validation() {
        assert!(Prefs::default().is_valid());
        let bad_target = Prefs {
            to: "xx".into(),
            ..Prefs::default()
        };
        assert!(!bad_target.is_valid());
        let bad_code = Prefs {
            always: vec!["en".into(), "<img>".into()],
            ..Prefs::default()
        };
        assert!(!bad_code.is_valid());
        let not_normal = Prefs {
            always: vec!["en-US".into()],
            ..Prefs::default()
        };
        assert!(!not_normal.is_valid());
        let too_many = Prefs {
            always: vec!["en".into(); MAX_ALWAYS + 1],
            ..Prefs::default()
        };
        assert!(!too_many.is_valid());

        let messy = Prefs {
            to: "de".into(),
            always: vec![
                "EN-us".into(),
                "en".into(),
                "de".into(),
                "??".into(),
                "fr".into(),
            ],
            offer: false,
        }
        .sanitized();
        assert_eq!(messy.always, vec!["en".to_string(), "fr".to_string()]);
        assert!(messy.auto("en-GB"));
        assert!(!messy.auto("de"));
        let unknown_target = Prefs {
            to: "klingon".into(),
            ..Prefs::default()
        };
        assert_eq!(unknown_target.sanitized().to, "ru");
    }

    #[test]
    fn batch_from_page() {
        let raw = r#"{"b":[[[1,"Hello "],[2,"world"]],[[3,"Second paragraph"]]],"more":true}"#;
        let batch = Batch::parse(raw, 5000);
        assert_eq!(batch.chars, 6 + 5 + 16);
        assert!(batch.more);
        assert_eq!(batch.groups.len(), 2);
        assert_eq!(
            batch.groups[0],
            vec![(1, "Hello ".into()), (2, "world".into())]
        );

        // Нет скрипта, ошибка в странице, мусор — пустая порция.
        assert_eq!(Batch::parse("null", 5000), Batch::default());
        assert_eq!(Batch::parse("", 5000), Batch::default());
        assert_eq!(
            Batch::parse(r#"{"b":[[["x","y"]]]}"#, 5000),
            Batch::default()
        );
        assert_eq!(
            Batch::parse(r#"{"b":[[[-1,"y"]]]}"#, 5000),
            Batch::default()
        );
    }

    #[test]
    fn batch_limits_untrusted_page() {
        // Слишком длинные и пустые узлы выбрасываются, числа рядом с текстом остаются,
        // фрагмент без букв не нужен вовсе.
        let long = "a".repeat(MAX_NODE_CHARS + 1);
        let raw = format!(
            r#"{{"b":[[[1,"123 456"],[2,"{long}"],[3,"ok"],[4,"  "]],[[5,"2024"],[6,", "]]]}}"#
        );
        let batch = Batch::parse(&raw, 5000);
        assert_eq!(
            batch.groups,
            vec![vec![(1, "123 456".to_string()), (3, "ok".to_string())]]
        );

        // Страница прислала больше, чем просили: лишнее отбрасываем.
        let piece = "word ".repeat(100); // 500 символов
        let groups: Vec<String> = (0..40).map(|i| format!(r#"[[{i},"{piece}"]]"#)).collect();
        let raw = format!(r#"{{"b":[{}],"more":false}}"#, groups.join(","));
        let batch = Batch::parse(&raw, 5000);
        assert!(batch.chars <= 5000 + MAX_GROUP_CHARS);
        assert_eq!(batch.groups.len(), 18);
        assert!(batch.more);

        // Огромный ответ не разбираем вовсе.
        let huge = format!(r#"{{"b":[[[1,"{}"]]]}}"#, "x".repeat(1_000_000));
        assert_eq!(Batch::parse(&huge, 5000), Batch::default());

        // Слишком много узлов в одном фрагменте — берём первые.
        let items: Vec<String> = (0..200).map(|i| format!(r#"[{i},"w"]"#)).collect();
        let raw = format!(r#"{{"b":[[{}]]}}"#, items.join(","));
        assert_eq!(Batch::parse(&raw, 5000).groups[0].len(), MAX_GROUP_ITEMS);
    }

    #[test]
    fn limiter_caps_rate_and_total() {
        let start = Instant::now();
        let mut limiter = Limiter::default();
        for _ in 0..MAX_PER_MINUTE {
            assert!(limiter.allow(start));
            limiter.record(start, 100);
        }
        assert!(!limiter.allow(start + Duration::from_secs(30)));
        // Через минуту окно освобождается.
        assert!(limiter.allow(start + Duration::from_secs(61)));

        let mut window = Window::default();
        for _ in 0..3 {
            assert!(window.allow(start, 3));
            window.record(start);
        }
        assert!(!window.allow(start + Duration::from_secs(59), 3));
        assert!(window.allow(start + Duration::from_secs(60), 3));

        let mut full = Limiter::default();
        full.record(start, MAX_PAGE_CHARS);
        assert_eq!(full.left(), 0);
        assert!(!full.allow(start));
    }

    #[test]
    fn page_asks_small_first_then_big() {
        let now = Instant::now();
        let mut page = Page::new(1, "https://example.com/".into());
        assert_eq!(page.next_limit(now), None); // ещё не переводим
        page.begin(2, "ru");
        assert_eq!(page.next_limit(now), Some(FIRST_BATCH));
        page.asking = Some(now);
        assert_eq!(page.next_limit(now), None); // ждём ответа
        // Ответ потерялся — через таймаут спрашиваем снова.
        assert_eq!(page.next_limit(now + ASK_TIMEOUT), Some(FIRST_BATCH));
        page.asking = None;
        page.limiter.record(now, 4000);
        assert_eq!(page.next_limit(now), Some(BATCH));
        page.in_flight = MAX_PARALLEL;
        assert_eq!(page.next_limit(now), None);
        page.in_flight = 0;
        page.limiter.record(now, MAX_PAGE_CHARS - 4000 - 100);
        assert_eq!(page.next_limit(now), Some(100));
    }

    #[test]
    fn probe_and_url_helpers() {
        let p = Probe::parse(r#"{"lang":"en-US","text":"Hello","active":true}"#);
        assert_eq!(p.lang, "en-US");
        assert!(p.active);
        assert_eq!(Probe::parse("oops"), Probe::default());
        assert_eq!(
            document_url("https://en.wikipedia.org/wiki/Rust#History"),
            "https://en.wikipedia.org/wiki/Rust"
        );
        let js = apply_script(&[(1, "Привет \"мир\"</script>".into())]).unwrap_or_default();
        assert!(js.contains(r#"[[1,"Привет \"мир\"</script>"]]"#));
    }

    #[test]
    fn phase_follows_offer_setting() {
        let mut page = Page::new(1, String::new());
        page.stage = Stage::Foreign;
        assert_eq!(page.phase(&Prefs::default()), Phase::Offer);
        let quiet = Prefs {
            offer: false,
            ..Prefs::default()
        };
        assert_eq!(page.phase(&quiet), Phase::None);
        page.stage = Stage::Done;
        assert_eq!(page.phase(&quiet), Phase::Done);
    }
}
