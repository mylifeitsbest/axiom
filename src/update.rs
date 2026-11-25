//! Обновления: Axiom сам узнаёт о новом релизе и предлагает обновиться.
//!
//! Номер последней версии берём из перенаправления `releases/latest` →
//! `releases/tag/vX.Y.Z`: это обычная страница GitHub, а не API, поэтому лимита
//! запросов нет, и кроме номера версии ничего не скачивается. Сеть — через
//! системный `curl` (есть в Windows 10 и 11 и почти в любом Linux): своего
//! HTTP-клиента в браузере нет, а тащить TLS ради пары запросов тяжело.
//!
//! Windows: новый exe скачивается рядом с текущим, сверяется с SHA256SUMS.txt
//! релиза и встаёт на его место. Работающий exe можно переименовать, но нельзя
//! удалить, поэтому старый файл уезжает в `axiom.exe.old`, а новый процесс
//! (`--updated`) ждёт, пока старый закроется. `axiom.exe.old` остаётся, пока
//! новая версия не проработает 10 минут: если она не запускается, достаточно
//! закрыть Axiom, удалить `axiom.exe` и переименовать `axiom.exe.old` в `axiom.exe`.
//!
//! Linux: Axiom стоит пакетом, обновлять его — дело apt или pacman.
//! Показываем готовую команду.

use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};
use serde::Serialize;
use tao::event_loop::EventLoopProxy;

use crate::events::UserEvent;

const REPO: &str = "https://github.com/mylifeitsbest/axiom";
#[cfg_attr(not(windows), allow(dead_code))]
const ASSET: &str = "Axiom-windows-x64.exe";
#[cfg_attr(not(windows), allow(dead_code))]
const SUMS: &str = "SHA256SUMS.txt";
/// Аргумент запуска новой версии сразу после обновления.
pub const UPDATED_FLAG: &str = "--updated";
/// Как часто проверять, пока Axiom открыт.
const EVERY: Duration = Duration::from_secs(6 * 60 * 60);
/// Повтор после неудачной проверки (например, не было сети).
const RETRY: Duration = Duration::from_secs(30 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version(u32, u32, u32);

impl Version {
    /// `v1.2.3` или `1.2.3`. Предварительные версии (`1.2.3-rc1`) не принимаем.
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        let mut parts = s.strip_prefix('v').unwrap_or(s).split('.').map(|p| {
            if !p.is_empty() && p.len() <= 9 && p.bytes().all(|b| b.is_ascii_digit()) {
                p.parse().ok()
            } else {
                None
            }
        });
        let version = Self(parts.next()??, parts.next()??, parts.next()??);
        parts.next().is_none().then_some(version)
    }

    pub fn current() -> Self {
        Self::parse(env!("CARGO_PKG_VERSION")).unwrap_or(Self(0, 0, 0))
    }

    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self(major, minor, patch)
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

/// Чем обновляется эта копия Axiom. Каждая система использует свои варианты.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum How {
    /// Сам: скачать, проверить и перезапуститься (Windows).
    InPlace,
    Apt,
    Pacman,
    /// Пакет `axiom-browser-bin` из AUR (или собранный по его PKGBUILD через makepkg).
    Aur,
    /// Только вручную: страница релиза (архив на Linux, другие системы).
    Manual,
}

impl How {
    fn detect() -> Self {
        #[cfg(debug_assertions)]
        if let Ok(how) = std::env::var("AXIOM_UPDATE_HOW") {
            match how.as_str() {
                "apt" => return Self::Apt,
                "pacman" => return Self::Pacman,
                "aur" => return Self::Aur,
                "manual" => return Self::Manual,
                _ => {}
            }
        }
        #[cfg(windows)]
        let how = Self::InPlace;
        #[cfg(target_os = "linux")]
        let how = linux_how();
        #[cfg(not(any(windows, target_os = "linux")))]
        let how = Self::Manual;
        how
    }

    fn command(self) -> &'static str {
        match self {
            Self::Apt => "sudo apt update && sudo apt install --only-upgrade axiom-browser",
            // В Arch обновляют всю систему сразу: частичное обновление (-Sy) её ломает.
            Self::Pacman => "sudo pacman -Syu",
            // Скрипт сам выберет yay/paru или makepkg и не обновляет всю систему.
            Self::Aur => {
                "curl -fsSL https://raw.githubusercontent.com/mylifeitsbest/axiom/main/install.sh | sh"
            }
            Self::InPlace | Self::Manual => "",
        }
    }
}

/// Пакет из нашего репозитория обновится командой; поставленный вручную .deb —
/// только новым .deb со страницы релиза.
#[cfg(target_os = "linux")]
fn linux_how() -> How {
    let exists = |p: &str| Path::new(p).exists();
    if exists("/var/lib/dpkg/info/axiom-browser.list")
        && (exists("/etc/apt/sources.list.d/axiom.list")
            || exists("/etc/apt/sources.list.d/axiom.sources"))
    {
        return How::Apt;
    }
    // Каталог пакета в базе pacman: `<имя>-<версия>-<сборка>`.
    let pacman_has = |name: &str| {
        std::fs::read_dir("/var/lib/pacman/local").is_ok_and(|dir| {
            dir.flatten().any(|e| {
                e.file_name()
                    .to_str()
                    .and_then(|n| n.strip_prefix(name))
                    .and_then(|rest| rest.strip_prefix('-'))
                    .is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit()))
            })
        })
    };
    if pacman_has("axiom-browser-bin") {
        return How::Aur;
    }
    let pacman_pkg = pacman_has("axiom-browser");
    let pacman_repo = std::fs::read_to_string("/etc/pacman.conf")
        .is_ok_and(|conf| conf.lines().any(|l| l.trim() == "[axiom]"));
    if pacman_pkg && pacman_repo {
        How::Pacman
    } else {
        How::Manual
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    #[default]
    Idle,
    Checking,
    Downloading,
    Restarting,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Checked {
    #[default]
    Never,
    Ok,
    Failed,
}

/// Результаты фоновых потоков.
#[derive(Debug)]
pub enum UpdateEvent {
    Checked(Result<Version, String>),
    /// Сколько байт скачано (только Windows).
    #[cfg_attr(not(windows), allow(dead_code))]
    Progress(u64),
    Downloaded(Result<Downloaded, String>),
}

/// Скачанный и проверенный exe новой версии (сам скачивает только Windows).
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Debug)]
pub struct Downloaded {
    part: PathBuf,
    /// SHA256 из `SHA256SUMS.txt`: перед запуском сверяем ещё раз.
    #[cfg_attr(not(windows), allow(dead_code))]
    sha256: String,
}

/// Что видит интерфейс: кнопка «Обновить», её панель и строка в настройках.
#[derive(Debug, Serialize)]
pub struct View {
    pub current: String,
    /// Более новая версия, если она есть.
    pub latest: Option<String>,
    pub phase: Phase,
    /// Сколько байт нового exe уже скачано.
    pub received: u64,
    /// Почему не удалось обновиться.
    pub error: String,
    pub checked: Checked,
    pub check_error: String,
    pub how: How,
    /// Команда для терминала (apt, pacman).
    pub command: &'static str,
    /// Страница релиза с описанием изменений.
    pub page: String,
    /// Ответ на «Проверить» в настройках: интерфейс покажет итог.
    pub manual: bool,
    /// Идут загрузки файлов: перезапуск их прервёт.
    pub busy: bool,
}

pub struct Updater {
    phase: Phase,
    latest: Option<Version>,
    received: u64,
    error: String,
    checked: Checked,
    check_error: String,
    next_check: Instant,
    /// Идущую проверку запросил пользователь — итог нужно показать.
    manual: bool,
    how: How,
}

impl Updater {
    pub fn new() -> Self {
        Self {
            phase: Phase::Idle,
            latest: None,
            received: 0,
            error: String::new(),
            checked: Checked::Never,
            check_error: String::new(),
            next_check: Instant::now(),
            manual: false,
            how: How::detect(),
        }
    }

    /// Плановая проверка из служебного таймера.
    pub fn tick(&mut self, enabled: bool, events: &EventLoopProxy<UserEvent>) {
        if enabled && self.phase == Phase::Idle && Instant::now() >= self.next_check {
            self.check(events, false);
        }
    }

    pub fn check(&mut self, events: &EventLoopProxy<UserEvent>, manual: bool) -> bool {
        if self.phase == Phase::Checking {
            // Фоновая проверка уже идёт — её итог и покажем.
            self.manual |= manual;
            return manual;
        }
        if matches!(self.phase, Phase::Downloading | Phase::Restarting) {
            return false;
        }
        self.phase = Phase::Checking;
        self.manual = manual;
        let events = events.clone();
        thread::spawn(move || {
            let latest = fetch_latest().map_err(|e| format!("{e:#}"));
            let _ = events.send_event(UserEvent::Update(UpdateEvent::Checked(latest)));
        });
        true
    }

    /// Итог проверки. `true` — её запросил пользователь.
    pub fn on_checked(&mut self, latest: Result<Version, String>) -> bool {
        if self.phase == Phase::Checking {
            self.phase = Phase::Idle;
        }
        match latest {
            Ok(version) => {
                self.next_check = Instant::now() + EVERY;
                self.checked = Checked::Ok;
                self.check_error.clear();
                self.latest = (version > Version::current()).then_some(version);
            }
            Err(error) => {
                self.next_check = Instant::now() + RETRY;
                self.checked = Checked::Failed;
                self.check_error = error;
            }
        }
        std::mem::take(&mut self.manual)
    }

    /// «Обновить»: скачать новую версию в фоне (только там, где умеем сами).
    pub fn install(&mut self, events: &EventLoopProxy<UserEvent>) -> bool {
        let Some(version) = self.latest else {
            return false;
        };
        if self.how != How::InPlace || !matches!(self.phase, Phase::Idle | Phase::Failed) {
            return false;
        }
        self.phase = Phase::Downloading;
        self.received = 0;
        self.error.clear();
        let events = events.clone();
        thread::spawn(move || {
            let result = download(version, &events).map_err(|e| format!("{e:#}"));
            let _ = events.send_event(UserEvent::Update(UpdateEvent::Downloaded(result)));
        });
        true
    }

    pub fn on_progress(&mut self, received: u64) {
        if self.phase == Phase::Downloading {
            self.received = received;
        }
    }

    /// Новый exe скачан и проверен: ставим его на место и запускаем.
    /// `true` — новая версия запущена, этому процессу пора выйти.
    pub fn on_downloaded(&mut self, result: Result<Downloaded, String>) -> bool {
        let installed = result.and_then(|new| {
            install_and_launch(&new.part, &new.sha256).map_err(|e| format!("{e:#}"))
        });
        match installed {
            Ok(()) => {
                self.phase = Phase::Restarting;
                true
            }
            Err(error) => {
                self.phase = Phase::Failed;
                self.error = error;
                false
            }
        }
    }

    pub fn view(&self, manual: bool, busy: bool) -> View {
        let page = match self.latest {
            Some(v) => format!("{REPO}/releases/tag/v{v}"),
            None => format!("{REPO}/releases/latest"),
        };
        View {
            current: Version::current().to_string(),
            latest: self.latest.map(|v| v.to_string()),
            phase: self.phase,
            received: self.received,
            error: self.error.clone(),
            checked: self.checked,
            check_error: self.check_error.clone(),
            how: self.how,
            command: self.how.command(),
            page,
            manual,
            busy,
        }
    }
}

// ---------- сеть ----------

/// Адрес репозитория. В debug-сборке его можно подменить локальным сервером
/// (`AXIOM_UPDATE_URL=http://127.0.0.1:8765`), чтобы проверить обновление целиком.
fn base_url() -> String {
    #[cfg(debug_assertions)]
    if let Ok(url) = std::env::var("AXIOM_UPDATE_URL")
        && url.starts_with("http://127.0.0.1:")
    {
        return url.trim_end_matches('/').to_string();
    }
    REPO.to_string()
}

#[cfg(windows)]
const NULL_DEVICE: &str = "NUL";
#[cfg(not(windows))]
const NULL_DEVICE: &str = "/dev/null";

fn curl() -> Command {
    #[cfg(windows)]
    let mut cmd = {
        use std::os::windows::process::CommandExt;
        // Полный путь: чужой curl.exe из текущей папки или PATH не подставится.
        let exe = std::env::var_os("SystemRoot")
            .map(|root| PathBuf::from(root).join(r"System32\curl.exe"))
            .unwrap_or_else(|| PathBuf::from("curl.exe"));
        let mut cmd = Command::new(exe);
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW: без мигающей консоли
        cmd
    };
    #[cfg(not(windows))]
    let mut cmd = Command::new("curl");
    // `-q` первым: не читать пользовательский .curlrc.
    cmd.args(["-q", "--silent", "--show-error", "--connect-timeout", "15"])
        .arg("--user-agent")
        .arg(format!("Axiom/{}", Version::current()))
        .stdin(Stdio::null());
    if base_url().starts_with("https://") {
        // Только HTTPS, в том числе после перенаправлений на хранилище файлов.
        cmd.args(["--proto", "=https", "--proto-redir", "=https"]);
    }
    cmd
}

fn curl_error(out: &std::process::Output) -> anyhow::Error {
    match out.status.code() {
        // Нет DNS, соединения или ответа вовремя.
        Some(5 | 6 | 7 | 28 | 35) => anyhow!("нет связи с GitHub"),
        Some(22) => anyhow!("файл не найден на GitHub"),
        _ => {
            let text = String::from_utf8_lossy(&out.stderr);
            let text = text.trim();
            let text = text
                .strip_prefix("curl: ")
                .and_then(|t| t.split_once(") ").map(|(_, rest)| rest))
                .unwrap_or(text);
            if text.is_empty() {
                anyhow!("curl завершился с ошибкой")
            } else {
                anyhow!("{text}")
            }
        }
    }
}

/// Номер последнего релиза из адреса, куда GitHub перенаправляет `releases/latest`.
fn fetch_latest() -> Result<Version> {
    let out = curl()
        .args(["--max-time", "30", "--output", NULL_DEVICE])
        .args(["--write-out", "%{redirect_url}"])
        .arg(format!("{}/releases/latest", base_url()))
        .output()
        .context("не удалось запустить curl")?;
    if !out.status.success() {
        return Err(curl_error(&out));
    }
    let target = String::from_utf8_lossy(&out.stdout);
    version_from_tag_url(target.trim()).ok_or_else(|| anyhow!("GitHub не назвал последнюю версию"))
}

fn version_from_tag_url(url: &str) -> Option<Version> {
    let (_, rest) = url.split_once("/releases/tag/")?;
    Version::parse(rest.split(['?', '#', '/']).next()?)
}

// ---------- установка (Windows) ----------

#[cfg(windows)]
fn download(version: Version, events: &EventLoopProxy<UserEvent>) -> Result<Downloaded> {
    use std::fs;

    let exe = std::env::current_exe().context("не найден путь к axiom.exe")?;
    let dir = exe
        .parent()
        .ok_or_else(|| anyhow!("не найдена папка Axiom"))?;
    let part = dir.join(format!("axiom-{version}.download"));
    let base = format!("{}/releases/download/v{version}", base_url());

    let sums = curl()
        .args(["--fail", "--location", "--max-time", "60"])
        .arg(format!("{base}/{SUMS}"))
        .output()
        .context("не удалось запустить curl")?;
    if !sums.status.success() {
        return Err(curl_error(&sums).context("контрольные суммы"));
    }
    let expected = checksum_for(&String::from_utf8_lossy(&sums.stdout), ASSET)
        .ok_or_else(|| anyhow!("в {SUMS} нет строки для {ASSET}"))?;

    // Сразу видно, можно ли писать в папку (например, Axiom лежит в Program Files).
    fs::File::create(&part).with_context(|| {
        format!(
            "нет доступа к папке {} — скачайте новую версию вручную",
            dir.display()
        )
    })?;
    let mut child = curl()
        .args(["--fail", "--location", "--retry", "2", "--max-time", "900"])
        .arg("--output")
        .arg(&part)
        .arg(format!("{base}/{ASSET}"))
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .context("не удалось запустить curl")?;
    let mut reported = 0;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        let size = fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
        if size != reported {
            reported = size;
            let _ = events.send_event(UserEvent::Update(UpdateEvent::Progress(size)));
        }
        thread::sleep(Duration::from_millis(250));
    };
    let checked = if status.success() {
        verify(&part, &expected)
    } else {
        let out = child.wait_with_output()?;
        Err(curl_error(&out))
    };
    if let Err(err) = checked {
        let _ = fs::remove_file(&part);
        return Err(err);
    }
    Ok(Downloaded {
        part,
        sha256: expected,
    })
}

#[cfg(not(windows))]
fn download(_version: Version, _events: &EventLoopProxy<UserEvent>) -> Result<Downloaded> {
    bail!("на этой системе Axiom обновляется пакетным менеджером")
}

/// Строка `sha256  имя` из вывода `sha256sum`.
#[cfg_attr(not(windows), allow(dead_code))]
fn checksum_for(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let (hash, file) = line.trim().split_once(char::is_whitespace)?;
        let file = file.trim_start().trim_start_matches('*');
        (file == name && hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
            .then(|| hash.to_ascii_lowercase())
    })
}

#[cfg(windows)]
fn verify(path: &Path, expected: &str) -> Result<()> {
    let mut file = std::fs::File::open(path).context("скачанный файл пропал")?;
    check(&mut file, expected)
}

/// Сумма файла совпадает с `SHA256SUMS.txt`, и это программа Windows.
#[cfg(windows)]
fn check(file: &mut std::fs::File, expected: &str) -> Result<()> {
    use sha2::{Digest, Sha256};
    use std::io::Read;

    let mut hasher = Sha256::new();
    let mut head = [0u8; 2];
    let mut buf = vec![0u8; 64 * 1024];
    let mut first = true;
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        if first && n >= 2 {
            head.copy_from_slice(&buf[..2]);
        }
        first = false;
        hasher.update(&buf[..n]);
    }
    let actual: String = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if actual != expected {
        bail!("контрольная сумма не совпала, файл повреждён при скачивании");
    }
    if &head != b"MZ" {
        bail!("скачанный файл — не программа Windows");
    }
    Ok(())
}

/// Старый exe на время, пока он ещё работает.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn old_path(exe: &Path) -> PathBuf {
    let mut name = exe.file_name().unwrap_or_default().to_os_string();
    name.push(".old");
    exe.with_file_name(name)
}

/// Антивирус может ненадолго держать свежий файл — пробуем несколько раз.
#[cfg(windows)]
pub(crate) fn rename_retry(from: &Path, to: &Path) -> std::io::Result<()> {
    let mut tries = 0;
    loop {
        match std::fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(_) if tries < 20 => {
                tries += 1;
                thread::sleep(Duration::from_millis(150));
            }
            Err(err) => return Err(err),
        }
    }
}

/// Ставит новый exe на место текущего и запускает его. При ошибке всё возвращается как было.
#[cfg(windows)]
fn install_and_launch(part: &Path, sha256: &str) -> Result<()> {
    use std::fs;

    let exe = std::env::current_exe().context("не найден путь к axiom.exe")?;
    let old = old_path(&exe);
    let _ = fs::remove_file(&old);
    rename_retry(&exe, &old).context("не удалось освободить место для новой версии")?;
    if let Err(err) = rename_retry(part, &exe) {
        let _ = fs::rename(&old, &exe);
        let _ = fs::remove_file(part);
        return Err(err).context("не удалось поставить новую версию");
    }
    let undo = || {
        let _ = fs::rename(&exe, part);
        let _ = fs::rename(&old, &exe);
        let _ = fs::remove_file(part);
    };
    // Между проверкой после скачивания и переименованием файл могли подменить.
    // Сверяем его ещё раз уже на месте и держим открытым так, что ни записать,
    // ни переименовать его нельзя, пока новая версия не запущена.
    let locked = open_locked(&exe)
        .context("не удалось открыть новую версию")
        .and_then(|mut file| check(&mut file, sha256).map(|()| file));
    let file = match locked {
        Ok(file) => file,
        Err(err) => {
            undo();
            return Err(err);
        }
    };
    let spawned = Command::new(&exe).arg(UPDATED_FLAG).spawn();
    drop(file);
    if let Err(err) = spawned {
        undo();
        return Err(err).context("не удалось запустить новую версию");
    }
    Ok(())
}

/// Открывает файл на чтение, разрешая другим только читать его. Антивирус
/// может ненадолго держать свежий файл — пробуем несколько раз.
#[cfg(windows)]
fn open_locked(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_SHARE_READ: u32 = 0x1;
    let mut tries = 0;
    loop {
        match std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .open(path)
        {
            Ok(file) => return Ok(file),
            Err(_) if tries < 20 => {
                tries += 1;
                thread::sleep(Duration::from_millis(150));
            }
            Err(err) => return Err(err),
        }
    }
}

#[cfg(not(windows))]
fn install_and_launch(_part: &Path, _sha256: &str) -> Result<()> {
    bail!("на этой системе Axiom обновляется пакетным менеджером")
}

/// Exe ещё запущен: Windows не даёт открыть его на запись (а переименовать даёт).
#[cfg(windows)]
fn running(exe: &Path) -> bool {
    std::fs::OpenOptions::new().write(true).open(exe).is_err()
}

/// Новая версия только что запущена старой: ждём, пока старая закроется.
/// Её exe (`axiom.exe.old`) остаётся на месте до [`cleanup`]: если новая версия
/// не заработает, к старой можно вернуться. Вызывается до захвата единственного экземпляра.
#[cfg(windows)]
pub fn finish(paths: &crate::storage::Paths) {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let old = old_path(&exe);
    let deadline = Instant::now() + Duration::from_secs(20);
    while old.exists() && running(&old) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(100));
    }
    // WebView2 старого процесса закрывается чуть позже него самого. Если создать
    // окружение раньше, новые webview прицепятся к уходящему процессу браузера.
    let locks = webview_locks(&paths.root);
    let deadline = Instant::now() + Duration::from_secs(8);
    while locks
        .iter()
        .any(|l| l.exists() && std::fs::remove_file(l).is_err())
        && Instant::now() < deadline
    {
        thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(not(windows))]
pub fn finish(_paths: &crate::storage::Paths) {}

/// `lockfile` Chromium в папках данных WebView2: вкладки, интерфейс и панели ИИ.
#[cfg(windows)]
fn webview_locks(root: &Path) -> Vec<PathBuf> {
    let mut dirs = vec![root.join("webview")];
    if let Ok(entries) = std::fs::read_dir(root) {
        dirs.extend(
            entries
                .flatten()
                .filter(|e| e.file_name().to_string_lossy().starts_with("ai-"))
                .map(|e| e.path()),
        );
    }
    dirs.into_iter()
        .map(|d| d.join("EBWebView").join("lockfile"))
        .collect()
}

/// Сколько новая версия должна проработать, прежде чем удалить прежний exe:
/// если она не запускается или сразу падает, `axiom.exe.old` можно вернуть на место.
#[cfg_attr(not(windows), allow(dead_code))]
const KEEP_OLD: Duration = Duration::from_secs(10 * 60);

/// Остатки прошлых обновлений: недокачанные файлы сразу, а прежний exe —
/// когда эта версия проработает [`KEEP_OLD`].
pub fn cleanup() {
    #[cfg(windows)]
    {
        let Ok(exe) = std::env::current_exe() else {
            return;
        };
        let old = old_path(&exe);
        if old.exists() {
            thread::spawn(move || {
                thread::sleep(KEEP_OLD);
                let _ = std::fs::remove_file(old);
            });
        }
        let Some(dir) = exe.parent() else {
            return;
        };
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if name.starts_with("axiom-") && name.ends_with(".download") {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_versions() {
        assert_eq!(Version::parse("v0.3.0"), Some(Version(0, 3, 0)));
        assert_eq!(Version::parse("1.10.2"), Some(Version(1, 10, 2)));
        assert_eq!(Version::parse("v1.2"), None);
        assert_eq!(Version::parse("v1.2.3.4"), None);
        assert_eq!(Version::parse("v1.2.3-rc1"), None);
        assert_eq!(Version::parse("v+1.2.3"), None);
        assert!(Version(0, 10, 0) > Version(0, 9, 9));
        assert!(Version(1, 0, 0) > Version(0, 99, 99));
        assert_eq!(Version::current().to_string(), env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn reads_tag_from_redirect() {
        let url = "https://github.com/mylifeitsbest/axiom/releases/tag/v0.3.1";
        assert_eq!(version_from_tag_url(url), Some(Version(0, 3, 1)));
        assert_eq!(
            version_from_tag_url("https://github.com/mylifeitsbest/axiom/releases"),
            None
        );
        assert_eq!(version_from_tag_url(""), None);
    }

    #[test]
    fn finds_checksum() {
        let hash = "a".repeat(64);
        let sums = format!(
            "{}  axiom_amd64.deb\n{hash}  Axiom-windows-x64.exe\n{}  Axiom-windows-x64.zip\n",
            "b".repeat(64),
            "c".repeat(64)
        );
        assert_eq!(checksum_for(&sums, ASSET), Some(hash.clone()));
        assert_eq!(checksum_for(&format!("{hash} *{ASSET}"), ASSET), Some(hash));
        assert_eq!(checksum_for("zzz  Axiom-windows-x64.exe", ASSET), None);
        assert_eq!(checksum_for("", ASSET), None);
    }

    #[test]
    fn old_file_sits_next_to_exe() {
        let exe = Path::new(r"C:\Programs\Axiom\axiom.exe");
        assert_eq!(old_path(exe), Path::new(r"C:\Programs\Axiom\axiom.exe.old"));
    }

    #[cfg(windows)]
    #[test]
    fn checked_file_cannot_be_swapped() {
        use sha2::{Digest, Sha256};
        use std::fs;

        let dir = std::env::temp_dir().join(format!("axiom-update-lock-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let exe = dir.join("axiom.exe");
        let body = b"MZ not really a program".to_vec();
        let _ = fs::write(&exe, &body);
        let sum: String = Sha256::digest(&body)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();

        let mut locked = open_locked(&exe);
        let checked = locked.as_mut().ok().map(|f| check(f, &sum).is_ok());
        // Пока файл открыт для проверки и запуска, подменить его нельзя.
        let renamed = fs::rename(&exe, dir.join("other.exe")).is_ok();
        let written = fs::OpenOptions::new().write(true).open(&exe).is_ok();
        drop(locked);
        let wrong = fs::File::open(&exe).map(|mut f| check(&mut f, &"0".repeat(64)).is_err());
        let renamed_after = fs::rename(&exe, dir.join("other.exe")).is_ok();
        let _ = fs::remove_dir_all(&dir);

        assert_eq!(checked, Some(true));
        assert!(!renamed && !written);
        assert!(wrong.is_ok_and(|e| e));
        assert!(renamed_after);
    }

    #[cfg(windows)]
    #[test]
    fn running_exe_is_detected() {
        let me = std::env::current_exe().unwrap_or_default();
        assert!(running(&me));
        let file = std::env::temp_dir().join(format!("axiom-not-running-{}", std::process::id()));
        let _ = std::fs::write(&file, b"MZ");
        let idle = running(&file);
        let _ = std::fs::remove_file(&file);
        assert!(!idle);
    }
}
