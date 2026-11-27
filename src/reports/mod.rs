//! Отчёты об ошибках для разработчика.
//!
//! - Журнал: stderr процесса уходит в `axiom.log` в профиле (см. [`journal`]).
//! - Вылет: паника сразу записывает отчёт (сообщение, место, стек), а жёсткий вылет
//!   (сбой в нативном коде, снятый процесс) виден при следующем запуске по оставшейся
//!   метке `axiom.running` — тогда в отчёт идёт конец журнала прошлого запуска.
//! - Падение процесса страницы или интерфейса (см. [`watch`]).
//! - «Сообщить о проблеме» в настройках — его отправляет сам человек, можно с файлами
//!   (снимки экрана, фото, видео, текст; см. [`files`]).
//!
//! Отчёты ждут в папке `reports` профиля и уходят фоновым потоком через системный
//! `curl` на `POST {STATS_URL}/r` (с файлами — на `/rf`). Ещё до записи на диск из
//! текста и журнала вычищаются адреса, пути с именем пользователя, почта и токены
//! (см. [`scrub`]). Автоматические отчёты уходят, только если включено «Отправлять
//! отчёты об ошибках»; выключено — они удаляются, не уходя никуда.

pub mod answers;
#[cfg(target_os = "linux")]
mod coredump;
mod files;
mod journal;
mod meta;
mod scrub;
#[cfg(target_os = "linux")]
mod terminal;
mod watch;

use std::{
    backtrace::Backtrace,
    fs,
    io::Write,
    panic,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicU8, AtomicU32, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use tao::event_loop::EventLoopProxy;

pub use self::files::{MAX_CHUNK, MAX_FILES, SLOTS};
pub use self::watch::{Source, watch};
use crate::events::UserEvent;
use files::Attached;

/// Метка «Axiom запущен»: остаётся на диске, только если процесс не закрылся сам.
const MARKER: &str = "axiom.running";
/// Папка с неотправленными отчётами.
const DIR: &str = "reports";
/// Сервер принимает не больше 64 КБ.
const MAX_BODY: usize = 64 * 1024;
/// Текст отчёта (описание, стек) — не больше 16 КБ, остальное место — журналу.
const MAX_TEXT: usize = 16 * 1024;
/// Что человек пишет в «Что не работает?» (в интерфейсе — до 4000 символов).
pub const MAX_USER_TEXT: usize = 12 * 1024;
/// Сколько журнала читать: после очистки он обычно короче (адреса сокращаются).
const READ_LOG: u64 = 96 * 1024;
/// Неотправленных отчётов храним не больше стольких и не дольше месяца.
const MAX_PENDING: usize = 8;
const MAX_AGE: Duration = Duration::from_secs(30 * 24 * 60 * 60);
/// Падений процессов страниц за один запуск в отчёты попадает не больше стольких.
const MAX_RENDERER: u32 = 5;
/// Одно и то же падение приходит каждому webview процесса: отчёт — раз в минуту.
const SAME_FAILURE: Duration = Duration::from_secs(60);
/// Первая отправка — не сразу при старте; повтор после неудачи — раз в час.
const WARMUP: Duration = Duration::from_secs(30);
const RETRY: Duration = Duration::from_secs(60 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// Вылет: паника, жёсткое завершение или ошибка запуска.
    Crash,
    /// «Сообщить о проблеме».
    User,
    /// Упал процесс страницы, интерфейса или GPU.
    Renderer,
}

impl Kind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Crash => "crash",
            Self::User => "user",
            Self::Renderer => "renderer",
        }
    }
}

/// То, что уходит на сервер: ровно эти поля, без идентификаторов браузера.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Report {
    kind: Kind,
    version: String,
    os: String,
    text: String,
    log: String,
    /// Только у «Сообщить о проблеме»: случайный ключ, по которому браузер потом
    /// спросит ответ разработчика (см. [`answers`]). Ни с чем больше не связан.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    token: Option<String>,
    /// Файлы к «Сообщить о проблеме»: имена и размеры, байты — в папке отчёта.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    files: Vec<Attached>,
}

/// Чем закончилась отправка «Сообщить о проблеме».
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Sent,
    /// Нет связи: отчёт сохранён и уйдёт позже.
    Queued,
    /// Сервер не принял или отправка невозможна.
    Failed,
    /// Выбранный файл не приложить (текст для человека): отчёт не ушёл.
    Files(&'static str),
}

/// Что лежит в метке запуска.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Marker {
    version: String,
    started: u64,
    /// Процесс упал с паникой: отчёт уже записан, второй при запуске не нужен.
    #[serde(default)]
    panic: bool,
    /// Linux: запуск шёл в тяжёлом режиме отрисовки (см. `system::gpu`).
    #[serde(default)]
    heavy: bool,
    /// Сколько запусков подряд перед этим закрылись аварийно в тяжёлом режиме.
    #[serde(default)]
    crashes: u32,
}

/// Сколько запусков подряд закрылись аварийно (вылет в нативном коде, не паника)
/// в тяжёлом режиме отрисовки, считая прошлый.
fn streak(previous: Option<&Marker>) -> u32 {
    match previous {
        Some(marker) if marker.heavy && !marker.panic => marker.crashes.saturating_add(1),
        _ => 0,
    }
}

fn read_marker(root: &Path) -> Option<Marker> {
    fs::read(root.join(MARKER))
        .ok()
        .map(|bytes| serde_json::from_slice::<Marker>(&bytes).unwrap_or_default())
}

/// Сколько запусков подряд Axiom закрылся аварийно в тяжёлом режиме отрисовки.
/// Читается до GTK, ещё до проверки, не открыт ли Axiom: метку открытой копии примет
/// за вылет, но такой запуск только передаёт адреса и закрывается.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn heavy_crashes(root: &Path) -> u32 {
    streak(read_marker(root).as_ref())
}

struct State {
    root: PathBuf,
    /// Строка о системе: версия Axiom, ОС, движок.
    about: String,
    private: scrub::Private,
    wake: mpsc::Sender<()>,
}

static STATE: OnceLock<State> = OnceLock::new();
/// «Отправлять отчёты об ошибках»: 0 — настройки ещё не прочитаны, 1 — нет, 2 — да.
static AUTO: AtomicU8 = AtomicU8::new(0);
static RENDERER_REPORTS: AtomicU32 = AtomicU32::new(0);
static LAST_FAILURE: Mutex<Option<(Instant, String)>> = Mutex::new(None);
/// Для метки запуска: тяжёлый ли режим отрисовки и сколько вылетов в нём было подряд.
static RUN: OnceLock<(bool, u32)> = OnceLock::new();

/// Журнал, перехват паники, метка запуска и фоновая отправка. Вызывать один раз,
/// у главного экземпляра (он один пишет в журнал профиля). `heavy` — этот запуск
/// идёт в тяжёлом режиме отрисовки (Linux).
pub fn start(root: &Path, heavy: bool) {
    // Метка осталась — прошлый запуск не закрылся сам. Журнал читаем до того,
    // как его продолжит (или отложит в `axiom.log.1`) этот запуск.
    let previous = read_marker(root);
    let previous_log = previous
        .as_ref()
        .filter(|m| !m.panic)
        .map(|_| journal::tail(root, READ_LOG));
    let crashes = if heavy { streak(previous.as_ref()) } else { 0 };
    let _ = RUN.set((heavy, crashes));
    journal::open(root);

    let system = system();
    // В журнал и, если Axiom запущен из терминала, на экран.
    eprintln!("система: {system}");
    let (wake, rx) = mpsc::channel();
    let state = State {
        root: root.to_path_buf(),
        about: format!("Axiom {} · {system}", crate::update::current_string()),
        private: scrub::Private::current(),
        wake,
    };
    if STATE.set(state).is_err() {
        return;
    }
    install_panic_hook();

    if let (Some(marker), Some(log)) = (previous, previous_log) {
        let started = match marker.started {
            0 => String::new(),
            at => format!(" (был запущен {})", journal::utc(at)),
        };
        eprintln!("прошлый запуск Axiom завершился аварийно{started}");
        let version = if marker.version.is_empty() {
            crate::update::current_string().to_string()
        } else {
            marker.version
        };
        let text = format!(
            "Axiom закрылся аварийно: вылет в нативном коде, зависание и снятие процесса \
             или выключение компьютера без завершения программ{started}. \
             Ниже — конец журнала того запуска."
        );
        // Linux: чей это вылет — Axiom или процесса WebKit — и его стек, если вылет
        // принял systemd-coredump. `coredumpctl` бывает небыстрым — не на пути к окну.
        #[cfg(target_os = "linux")]
        {
            let since = marker.started;
            let _ = thread::Builder::new()
                .name("coredumps".into())
                .spawn(move || {
                    let text = match coredump::since(since) {
                        Some(dumps) => format!("{text}\n\n{dumps}"),
                        None => text,
                    };
                    record(Kind::Crash, &version, &text, &log);
                });
        }
        #[cfg(not(target_os = "linux"))]
        record(Kind::Crash, &version, &text, &log);
    }
    write_marker(root, false);
    files::sweep(&root.join(DIR));
    spawn_sender(rx);
    // Только для проверки отчётов (debug-сборка): паника в фоновом потоке через 5 с.
    #[cfg(debug_assertions)]
    if std::env::var_os("AXIOM_TEST_PANIC").is_some() {
        let _ = thread::Builder::new().name("test-panic".into()).spawn(|| {
            thread::sleep(Duration::from_secs(5));
            panic!("проверка отчёта о панике (AXIOM_TEST_PANIC)");
        });
    }
}

/// Служебный таймер (раз в полминуты): не дать журналу разрастись.
pub fn tick() {
    if let Some(state) = STATE.get() {
        journal::keep_small(&state.root);
    }
}

/// Обычный выход: метка больше не нужна.
pub fn finish() {
    if let Some(state) = STATE.get() {
        let _ = fs::remove_file(state.root.join(MARKER));
    }
    // Последние строки ещё в трубе к терминалу: поток, который их переписывает,
    // при выходе не дождутся.
    #[cfg(target_os = "linux")]
    terminal::flush();
}

/// Axiom не смог запуститься: отчёт сразу и попытка отправить его, не дожидаясь
/// следующего запуска (тот может упасть так же, а окна человек не увидит).
pub fn fatal(text: &str, auto: bool) {
    let Some(state) = STATE.get() else { return };
    let log = journal::tail(&state.root, READ_LOG);
    record(Kind::Crash, crate::update::current_string(), text, &log);
    finish();
    set_auto(auto);
    send_pending();
}

/// Включить или выключить автоматическую отправку (настройка «Отправлять отчёты об ошибках»).
pub fn set_auto(on: bool) {
    let before = AUTO.swap(if on { 2 } else { 1 }, Ordering::Relaxed);
    if before != 2
        && on
        && let Some(state) = STATE.get()
    {
        let _ = state.wake.send(());
    }
}

/// Сколько байт журнала уйдёт с «Сообщить о проблеме» (после очистки).
pub fn log_size() -> usize {
    STATE.get().map_or(0, |state| {
        let mut report = Report {
            kind: Kind::User,
            version: crate::update::current_string().into(),
            os: os_name().into(),
            text: state.about.clone(),
            log: journal::tail(&state.root, READ_LOG),
            token: None,
            files: Vec::new(),
        };
        report.clean(&state.private);
        report.log.len()
    })
}

/// Кусок файла, выбранного в «Сообщить о проблеме» (`slot` — его место в форме,
/// `offset` — сколько байт уже пришло, `data` — base64). Ответ — сколько байт файла
/// теперь на диске или почему файл не принят (текст для человека).
pub fn attach(slot: usize, offset: u64, data: &str) -> Result<u64, &'static str> {
    let Some(state) = STATE.get() else {
        return Err(files::Refused::Io.text());
    };
    files::write_chunk(&state.root, slot, offset, data).map_err(files::Refused::text)
}

/// Файл убрали из формы.
pub fn detach(slot: usize) {
    if let Some(state) = STATE.get() {
        files::drop_slot(&state.root, slot);
    }
}

/// «Сообщить о проблеме»: отправить сейчас в фоне; без связи — сохранить на потом.
/// `files` — места выбранных файлов и их имена. Итог приходит событием
/// `UserEvent::Reported`.
pub fn send_user(
    text: &str,
    with_log: bool,
    files: &[(usize, String)],
    events: EventLoopProxy<UserEvent>,
) {
    let Some(state) = STATE.get() else {
        let _ = events.send_event(UserEvent::Reported(Outcome::Failed));
        return;
    };
    let token = answers::token();
    let taken = match files::take(&state.root, files, &stamp()) {
        Ok(taken) => taken,
        Err(refused) => {
            let _ = events.send_event(UserEvent::Reported(Outcome::Files(refused.text())));
            return;
        }
    };
    let mut report = Report {
        kind: Kind::User,
        version: crate::update::current_string().into(),
        os: os_name().into(),
        text: format!("{}\n\n{}", state.about, text.trim()),
        log: if with_log {
            journal::tail(&state.root, READ_LOG)
        } else {
            String::new()
        },
        token: Some(token.clone()),
        files: Vec::new(),
    };
    let text = text.to_string();
    thread::spawn(move || {
        // Метаданные фото и видео вырезаются здесь: на видео в 50 МБ это заметно.
        let dir = taken.map(|taken| {
            report.files = files::clean(&taken.dir, &taken.files, &state.private);
            taken.dir
        });
        report.clean(&state.private);
        let outcome = match endpoint("r").map(|url| post_report(&url, &report, dir.as_deref())) {
            Some(Posted::Ok) => Outcome::Sent,
            Some(Posted::Rejected) | None => Outcome::Failed,
            Some(Posted::Later) => match save(&state.root, &report, dir.as_deref()) {
                Ok(()) => Outcome::Queued,
                Err(_) => Outcome::Failed,
            },
        };
        // Ушло или уйдёт само — ждём ответа.
        if outcome != Outcome::Failed {
            answers::remember(&state.root, token, &text);
        }
        // Ушёл или не сохранился — файлы больше не нужны (сохранённый их уже забрал).
        if let Some(dir) = dir {
            let _ = fs::remove_dir_all(dir);
        }
        let _ = events.send_event(UserEvent::Reported(outcome));
    });
}

/// Упал процесс движка (см. [`watch`]): в журнал всегда, в отчёт — если `report`
/// и такое не приходило только что.
fn process_failed(source: &str, what: &str, report: bool) {
    eprintln!("{what} — {source}");
    if !report || !first_in_a_while(what) {
        return;
    }
    if RENDERER_REPORTS.fetch_add(1, Ordering::Relaxed) >= MAX_RENDERER {
        return;
    }
    let text = format!("{what}\nГде: {source}");
    let Some(state) = STATE.get() else { return };
    // Хватит конца журнала: что Axiom делал перед падением.
    let log = journal::tail(&state.root, READ_LOG / 4);
    record(Kind::Renderer, crate::update::current_string(), &text, &log);
}

fn first_in_a_while(what: &str) -> bool {
    let Ok(mut last) = LAST_FAILURE.lock() else {
        return true;
    };
    let now = Instant::now();
    let repeat = last
        .as_ref()
        .is_some_and(|(at, prev)| prev == what && now.duration_since(*at) < SAME_FAILURE);
    *last = Some((now, what.to_string()));
    !repeat
}

// ---------- паника ----------

fn install_panic_hook() {
    let default = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        // Стандартный обработчик печатает сообщение в stderr — то есть в журнал.
        default(info);
        let Some(state) = STATE.get() else { return };
        let message = info.payload_as_str().unwrap_or("(без сообщения)");
        let place = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_default();
        let current = thread::current();
        let thread = current.name().unwrap_or("без имени");
        let base = exe_base().map_or(String::new(), |b| format!("\nБаза exe: {b:#x}"));
        // `{:#}` — с адресами: в release-сборке без символов имён в стеке нет,
        // но адрес минус база exe указывает место в той же сборке.
        let text = format!(
            "Паника в потоке «{thread}»: {message}\nМесто: {place}{base}\n\n{:#}",
            Backtrace::force_capture()
        );
        let log = journal::tail(&state.root, READ_LOG);
        record(Kind::Crash, crate::update::current_string(), &text, &log);
        write_marker(&state.root, true);
    }));
}

/// Адрес загрузки exe: адреса в стеке release-сборки без символов считаются от него.
#[cfg(windows)]
fn exe_base() -> Option<usize> {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetModuleHandleW(name: *const u16) -> *mut std::ffi::c_void;
    }
    // SAFETY: null — модуль самого процесса; дескриптор не освобождается.
    let base = unsafe { GetModuleHandleW(std::ptr::null()) };
    (!base.is_null()).then_some(base as usize)
}

#[cfg(target_os = "linux")]
fn exe_base() -> Option<usize> {
    let exe = std::env::current_exe().ok()?;
    let exe = exe.to_str()?;
    let maps = fs::read_to_string("/proc/self/maps").ok()?;
    let line = maps.lines().find(|l| l.ends_with(exe))?;
    usize::from_str_radix(line.split('-').next()?, 16).ok()
}

#[cfg(not(any(windows, target_os = "linux")))]
fn exe_base() -> Option<usize> {
    None
}

fn write_marker(root: &Path, panic: bool) {
    static STARTED: OnceLock<u64> = OnceLock::new();
    let (heavy, crashes) = RUN.get().copied().unwrap_or_default();
    let marker = Marker {
        version: crate::update::current_string().into(),
        started: *STARTED.get_or_init(crate::storage::now_secs),
        panic,
        heavy,
        crashes,
    };
    if let Ok(bytes) = serde_json::to_vec(&marker) {
        let _ = fs::write(root.join(MARKER), bytes);
    }
}

// ---------- отчёты на диске ----------

impl Report {
    /// Очистка от личного и подгонка под 64 КБ: текст — до 16 КБ, журнал — его конец.
    fn clean(&mut self, private: &scrub::Private) {
        self.text = cut_end(&private.scrub(&self.text), MAX_TEXT);
        self.log = private.scrub(&self.log);
        // Экранирование в JSON (обратные слэши путей, кавычки) удлиняет текст:
        // меряем готовое тело и отрезаем начало журнала, пока не влезет.
        for _ in 0..8 {
            let size = serde_json::to_vec(self).map_or(usize::MAX, |b| b.len());
            if size <= MAX_BODY {
                return;
            }
            let over = size - MAX_BODY;
            let keep = self.log.len().saturating_sub(over + 512);
            self.log = cut_start(&self.log, keep);
        }
        self.log.clear();
    }
}

/// Не длиннее `max` байт, обрезая конец по границе символа.
fn cut_end(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

/// Последние `keep` байт, с целой первой строкой.
fn cut_start(text: &str, keep: usize) -> String {
    if text.len() <= keep {
        return text.to_string();
    }
    let mut start = text.len() - keep;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    let tail = &text[start..];
    match tail.find('\n') {
        Some(i) if i + 1 < tail.len() => tail[i + 1..].to_string(),
        _ => tail.to_string(),
    }
}

/// Записать автоматический отчёт (уже очищенным) и разбудить отправку.
/// Для паники — синхронно. Отчёты выключены — не пишем ничего.
fn record(kind: Kind, version: &str, text: &str, log: &str) {
    let Some(state) = STATE.get() else { return };
    if AUTO.load(Ordering::Relaxed) == 1 {
        return;
    }
    let mut report = Report {
        kind,
        version: version.into(),
        os: os_name().into(),
        text: format!("{}\n\n{text}", state.about),
        log: log.into(),
        token: None,
        files: Vec::new(),
    };
    report.clean(&state.private);
    if let Err(err) = save(&state.root, &report, None) {
        eprintln!("отчёт об ошибке не записан: {err:#}");
        return;
    }
    let _ = state.wake.send(());
}

/// Сохранить отчёт до следующей попытки. `files` — папка с его файлами: она
/// переезжает к отчёту (`reports/<имя>/`).
fn save(root: &Path, report: &Report, files: Option<&Path>) -> anyhow::Result<()> {
    let dir = root.join(DIR);
    fs::create_dir_all(&dir)?;
    // Место под новый: самые старые уходят.
    let pending = pending(&dir);
    let extra = (pending.len() + 1).saturating_sub(MAX_PENDING);
    for (path, _) in pending.iter().take(extra) {
        remove(path);
    }
    let path = dir.join(format!("{}-{}.json", stamp(), report.kind.as_str()));
    // Сначала файлы, потом сам отчёт: отправка смотрит только на отчёты.
    let mut report = report.clone();
    match files {
        Some(from) if !report.files.is_empty() => {
            if fs::rename(from, files::dir_of(&path)).is_err() {
                report.files.clear();
            }
        }
        _ => report.files.clear(),
    }
    crate::storage::write_json_compact(&path, &report)
}

/// Имя, которое сортируется как время: миллисекунды и номер внутри одной миллисекунды.
fn stamp() -> String {
    static SEQ: AtomicU32 = AtomicU32::new(0);
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    let seq = SEQ.fetch_add(1, Ordering::Relaxed) % 10_000;
    format!("{millis:013}-{seq:04}")
}

/// Удалить отчёт вместе с его файлами.
fn remove(path: &Path) {
    let _ = fs::remove_file(path);
    let _ = fs::remove_dir_all(files::dir_of(path));
}

/// Неотправленные отчёты, старые первыми. Битые и просроченные удаляются.
fn pending(dir: &Path) -> Vec<(PathBuf, Report)> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    files.sort();
    let mut out = Vec::new();
    for path in files {
        let old = fs::metadata(&path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > MAX_AGE);
        let report = fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice::<Report>(&b).ok());
        match report {
            Some(report) if !old => out.push((path, report)),
            _ => remove(&path),
        }
    }
    out
}

// ---------- отправка ----------

fn spawn_sender(rx: mpsc::Receiver<()>) {
    let spawned = thread::Builder::new()
        .name("reports".into())
        .spawn(move || {
            thread::sleep(WARMUP);
            loop {
                send_pending();
                match rx.recv_timeout(RETRY) {
                    Ok(()) | Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                }
                // Падения часто идут пачкой (все вкладки одного процесса): даём им записаться.
                thread::sleep(Duration::from_secs(3));
                while rx.try_recv().is_ok() {}
            }
        });
    if let Err(err) = spawned {
        eprintln!("отправка отчётов об ошибках не запущена: {err}");
    }
}

/// Отправить всё, что ждёт. Автоматические отчёты — только при включённой настройке.
fn send_pending() {
    let Some(state) = STATE.get() else { return };
    let auto = AUTO.load(Ordering::Relaxed);
    // Тестовые запуски release-сборки (`AXIOM_NO_STATS=1`) сами отчётов не шлют.
    let quiet = std::env::var_os("AXIOM_NO_STATS").is_some_and(|v| !v.is_empty() && v != "0");
    let Some(url) = endpoint("r") else { return };
    for (path, report) in pending(&state.root.join(DIR)) {
        if report.kind != Kind::User {
            match auto {
                // Настройки ещё не прочитаны — подождём.
                0 => continue,
                // Человек выключил отчёты: не храним то, что не уйдёт.
                1 => {
                    remove(&path);
                    continue;
                }
                _ if quiet => continue,
                _ => {}
            }
        }
        let mut report = report;
        // Отчёт мог записать прежний Axiom: очищаем ещё раз по нынешним правилам.
        report.clean(&state.private);
        match post_report(&url, &report, Some(&files::dir_of(&path))) {
            Posted::Ok | Posted::Rejected => remove(&path),
            // Нет связи или сервер просит подождать — остальные тоже не уйдут.
            Posted::Later => return,
        }
    }
}

enum Posted {
    Ok,
    /// Сервер отказал (неверный отчёт) — повторять бессмысленно.
    Rejected,
    /// Нет связи, ошибка сервера или «слишком часто» — попробовать позже.
    Later,
}

/// Адрес на сервере (`r` — приём отчётов, `a` — ответы). В debug — только
/// подменённый локальный (`AXIOM_STATS_URL`): отладочные запуски не ходят на боевой сервер.
fn endpoint(path: &str) -> Option<String> {
    if cfg!(debug_assertions) && std::env::var_os("AXIOM_STATS_URL").is_none() {
        return None;
    }
    let base = crate::stats::base_url();
    if cfg!(debug_assertions) && base == crate::stats::STATS_URL {
        return None;
    }
    Some(format!("{base}/{path}"))
}

/// Отчёт без файлов — JSON на `/r`, с файлами (`dir` — их папка) — на `/rf`:
/// строка JSON со списком файлов и сами файлы подряд. Старый сервер про `/rf` не знает
/// и отвечает 404 (а старый Caddy — 413): такой отчёт ждёт, а не теряется.
fn post_report(url: &str, report: &Report, dir: Option<&Path>) -> Posted {
    let dir = dir.filter(|_| !report.files.is_empty());
    let attached = dir.map_or_else(Vec::new, |dir| files::on_disk(dir, &report.files));
    let (Some(dir), false) = (dir, attached.is_empty()) else {
        let report = Report {
            files: Vec::new(),
            ..report.clone()
        };
        let Ok(body) = serde_json::to_vec(&report) else {
            return Posted::Rejected;
        };
        return post(url, "application/json", 0, |stdin| stdin.write_all(&body));
    };
    let report = Report {
        files: attached,
        ..report.clone()
    };
    let Ok(header) = serde_json::to_vec(&report) else {
        return Posted::Rejected;
    };
    let size: u64 = report.files.iter().map(|f| f.size).sum();
    post(
        &format!("{url}f"),
        "application/x-axiom-report",
        size,
        |stdin| files::write_body(stdin, &header, dir, report.files.len()),
    )
}

/// `POST` через curl: `body` пишет тело в его stdin, `big` — сколько в нём байт файлов
/// (от этого зависит, сколько ждать).
fn post(
    url: &str,
    content_type: &str,
    big: u64,
    body: impl FnOnce(&mut std::process::ChildStdin) -> std::io::Result<()>,
) -> Posted {
    let null = if cfg!(windows) { "NUL" } else { "/dev/null" };
    let mut cmd = crate::stats::curl();
    cmd.args(["-X", "POST", "-H"])
        .arg(format!("Content-Type: {content_type}"))
        .args(["--data-binary", "@-", "--output", null])
        .args(["--write-out", "%{http_code}"]);
    if big > 0 {
        // Видео на медленной связи идёт минутами: ждём по 32 КБ/с, но не дольше получаса,
        // а совсем застрявшую отправку (меньше 1 КБ/с за минуту) бросаем.
        let secs = (60 + big / (32 * 1024)).min(30 * 60);
        cmd.arg("--max-time").arg(secs.to_string()).args([
            "--speed-limit",
            "1024",
            "--speed-time",
            "60",
            "-H",
            "Expect:",
        ]);
    }
    cmd.arg(url).stdin(Stdio::piped()).stdout(Stdio::piped());
    let Ok(mut child) = cmd.spawn() else {
        return Posted::Later;
    };
    // curl читает тело целиком до соединения, поэтому запись не ждёт сети.
    let written = child
        .stdin
        .take()
        .is_some_and(|mut stdin| body(&mut stdin).is_ok());
    let Ok(out) = child.wait_with_output() else {
        return Posted::Later;
    };
    let code: u16 = String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse()
        .unwrap_or(0);
    match code {
        200..=299 if written => Posted::Ok,
        // 404 и 413 — сервер или Caddy ещё не обновлены (нет `/r`, лимит 4 КБ):
        // отчёт ждёт, а не теряется. 408 и 429 — просьба подождать.
        400..=499 if !matches!(code, 404 | 408 | 413 | 429) => Posted::Rejected,
        _ => Posted::Later,
    }
}

// ---------- о системе ----------

fn os_name() -> &'static str {
    crate::stats::os_name()
}

/// `Windows 11 (сборка 26200, 25H2) · WebView2 142.0.3595.53`.
fn system() -> String {
    let engine = if cfg!(windows) {
        "WebView2"
    } else {
        "WebKitGTK"
    };
    let engine = match wry::webview_version() {
        Ok(v) => format!("{engine} {v}"),
        Err(_) => format!("{engine} не найден"),
    };
    format!("{} · {engine}", os_detail())
}

#[cfg(windows)]
fn os_detail() -> String {
    let key = windows_registry::LOCAL_MACHINE.open(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion");
    let get = |name: &str| {
        key.as_ref()
            .ok()
            .and_then(|k| k.get_string(name).ok())
            .unwrap_or_default()
    };
    let build = get("CurrentBuild");
    let display = get("DisplayVersion");
    // ProductName и на Windows 11 говорит «Windows 10»: различаем по номеру сборки.
    let name = match build.parse::<u32>() {
        Ok(b) if b >= 22_000 => "Windows 11",
        Ok(_) => "Windows 10",
        Err(_) => "Windows",
    };
    match (build.is_empty(), display.is_empty()) {
        (false, false) => format!("{name} (сборка {build}, {display})"),
        (false, true) => format!("{name} (сборка {build})"),
        _ => name.to_string(),
    }
}

#[cfg(target_os = "linux")]
fn os_detail() -> String {
    let release = fs::read_to_string("/etc/os-release").unwrap_or_default();
    let name = release
        .lines()
        .find_map(|l| l.strip_prefix("PRETTY_NAME="))
        .map(|v| v.trim_matches('"').to_string())
        .unwrap_or_else(|| "Linux".into());
    let mut parts = vec![name];
    if let Ok(session) = std::env::var("XDG_SESSION_TYPE") {
        parts.push(session);
    }
    if let Ok(desktop) = std::env::var("XDG_CURRENT_DESKTOP") {
        parts.push(desktop);
    }
    if Path::new("/proc/driver/nvidia/version").exists() {
        parts.push("NVIDIA".into());
    }
    if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_some() {
        parts.push("без DMA-BUF".into());
    }
    parts.join(", ")
}

#[cfg(not(any(windows, target_os = "linux")))]
fn os_detail() -> String {
    std::env::consts::OS.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(text: &str, log: &str) -> Report {
        Report {
            kind: Kind::Crash,
            version: "0.5.4".into(),
            os: "windows".into(),
            text: text.into(),
            log: log.into(),
            token: None,
            files: Vec::new(),
        }
    }

    #[test]
    fn report_json_has_only_the_known_fields() -> serde_json::Result<()> {
        let json = serde_json::to_value(report("t", "l"))?;
        assert_eq!(
            json,
            serde_json::json!({
                "kind": "crash", "version": "0.5.4", "os": "windows", "text": "t", "log": "l"
            })
        );
        // Сообщение человека — ещё и с ключом ответа; очистка его не трогает.
        let mut user = report("t", "l");
        user.kind = Kind::User;
        user.token = Some("0123456789abcdef0123456789abcdef".into());
        user.clean(&scrub::Private::new(vec![], vec![], vec![]));
        let json = serde_json::to_value(&user)?;
        assert_eq!(json["token"], "0123456789abcdef0123456789abcdef");
        assert_eq!(json.as_object().map(|o| o.len()), Some(6));
        Ok(())
    }

    #[test]
    fn clean_scrubs_and_fits_into_64_kb() -> serde_json::Result<()> {
        let private = scrub::Private::new(vec![r"C:\Users\Roman".into()], vec![], vec![]);
        // Обратные слэши удваиваются в JSON: журнал из путей вдвое «тяжелее».
        let line = "открыта https://secret.example/page?id=1 в C:\\Users\\Roman\\x \\\\ \"\n";
        let mut r = report(&"паника ".repeat(5000), &line.repeat(4000));
        r.clean(&private);
        let body = serde_json::to_vec(&r)?;
        assert!(body.len() <= MAX_BODY, "{}", body.len());
        assert!(r.text.len() <= MAX_TEXT);
        assert!(!r.log.is_empty());
        assert!(
            r.log.ends_with(
                &line
                    .replace("https://secret.example/page?id=1", "secret.example")
                    .replace("C:\\Users\\Roman", "~")
            )
        );
        assert!(
            r.log.starts_with("открыта "),
            "журнал начинается с целой строки"
        );
        assert!(!r.log.contains("Roman") && !r.log.contains("id=1"));
        Ok(())
    }

    #[test]
    fn cuts_keep_char_boundaries() {
        assert_eq!(cut_end("ёжик", 3), "ё");
        assert_eq!(cut_start("один\nдва\nтри", 7), "три");
        assert_eq!(cut_start("абв", 3), "в");
    }

    #[test]
    fn pending_reports_are_bounded() -> anyhow::Result<()> {
        let root = std::env::temp_dir().join(format!("axiom-reports-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for i in 0..(MAX_PENDING + 3) {
            save(&root, &report(&format!("r{i}"), ""), None)?;
        }
        let _ = fs::write(root.join(DIR).join("0-broken.json"), b"{");
        let left = pending(&root.join(DIR));
        let _ = fs::remove_dir_all(&root);
        assert_eq!(left.len(), MAX_PENDING);
        // Остались самые новые, битый удалён.
        assert_eq!(left.last().map(|(_, r)| r.text.as_str()), Some("r10"));
        assert!(left.iter().all(|(p, _)| !p.ends_with("0-broken.json")));
        Ok(())
    }

    #[test]
    fn saved_report_keeps_its_files_until_it_is_gone() -> anyhow::Result<()> {
        let root = std::env::temp_dir().join(format!("axiom-report-files-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let sending = root.join("sending");
        fs::create_dir_all(&sending)?;
        fs::write(sending.join("0"), b"GIF89a")?;
        let mut with_files = report("с файлом", "");
        with_files.kind = Kind::User;
        with_files.files = vec![Attached {
            name: "a.gif".into(),
            size: 6,
        }];
        save(&root, &with_files, Some(&sending))?;
        assert!(!sending.exists());
        let (path, saved) = pending(&root.join(DIR))
            .pop()
            .ok_or_else(|| anyhow::anyhow!("отчёт не сохранён"))?;
        assert_eq!(saved.files, with_files.files);
        assert_eq!(fs::read(files::dir_of(&path).join("0"))?, b"GIF89a");
        // Без папки файлов список не сохраняется: отправлять было бы нечего.
        save(&root, &with_files, None)?;
        let newest = pending(&root.join(DIR)).pop().map(|(_, r)| r.files);
        assert_eq!(newest, Some(Vec::new()));
        // Новые отчёты вытеснили его — файлы ушли вместе с ним.
        for i in 0..MAX_PENDING {
            save(&root, &report(&format!("r{i}"), ""), None)?;
        }
        assert!(!path.exists() && !files::dir_of(&path).exists());
        let _ = fs::remove_dir_all(&root);
        Ok(())
    }

    #[test]
    fn heavy_crashes_count_only_native_crashes_in_a_row() -> serde_json::Result<()> {
        let marker = |heavy, crashes, panic| Marker {
            heavy,
            crashes,
            panic,
            ..Marker::default()
        };
        // Закрылся сам — метки нет, счёт с нуля.
        assert_eq!(streak(None), 0);
        assert_eq!(streak(Some(&marker(true, 0, false))), 1);
        assert_eq!(streak(Some(&marker(true, 2, false))), 3);
        // Паника — не вылет отрисовки; лёгкий режим счёт обнуляет.
        assert_eq!(streak(Some(&marker(true, 2, true))), 0);
        assert_eq!(streak(Some(&marker(false, 2, false))), 0);
        // Метка прежней версии, без новых полей.
        let old: Marker = serde_json::from_str(r#"{"version":"0.6.0","started":1}"#)?;
        assert_eq!(streak(Some(&old)), 0);

        let root = std::env::temp_dir().join(format!("axiom-streak-{}", std::process::id()));
        let _ = fs::create_dir_all(&root);
        let _ = fs::write(
            root.join(MARKER),
            serde_json::to_vec(&marker(true, 1, false))?,
        );
        let crashes = heavy_crashes(&root);
        let _ = fs::remove_dir_all(&root);
        assert_eq!(crashes, 2);
        assert_eq!(heavy_crashes(&root), 0);
        Ok(())
    }

    #[test]
    fn same_failure_is_reported_once_a_minute() {
        let what = "тест: упал процесс страницы (уникальная строка)";
        assert!(first_in_a_while(what));
        assert!(!first_in_a_while(what));
        assert!(first_in_a_while("тест: другое падение"));
    }
}
