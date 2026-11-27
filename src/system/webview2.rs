//! Windows: нет WebView2 — предложить его установить, а не закрыться молча.
//!
//! Страницы Axiom показывает через Microsoft Edge WebView2. В Windows 11 он есть
//! всегда, в Windows 10 обычно ставится вместе с Edge, но бывает, что его нет: Edge
//! удалили программой-«чистильщиком» или это урезанная сборка Windows. Тогда окно
//! не создавалось («The system cannot find the file specified»), Axiom закрывался,
//! ничего не показав, и человек запускал его снова и снова.
//!
//! Теперь ещё до окна: WebView2 не найден — спрашиваем, установить ли его. Да —
//! скачиваем загрузчик Microsoft (около 2 МБ, остальное он докачивает сам), запускаем
//! его с окном установки и, когда он закончит, открываемся как обычно. Нет или не
//! вышло — объясняем, где взять WebView2, и закрываемся с отчётом.

use std::{
    fs,
    path::Path,
    process::Command,
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};

/// Загрузчик WebView2 Runtime (Evergreen Bootstrapper) — ссылка со страницы Microsoft
/// о распространении WebView2; ведёт на файл `MicrosoftEdgeWebview2Setup.exe`.
const BOOTSTRAPPER: &str = "https://go.microsoft.com/fwlink/p/?LinkId=2124703";
/// Страница WebView2 у Microsoft: там тот же загрузчик и полный установщик.
const PAGE: &str = "https://developer.microsoft.com/microsoft-edge/webview2/";

// Строки разбиты вручную: сам MessageBox переносит русский текст посреди слова.
const ASK: &str = "Axiom показывает страницы с помощью компонента\n\
Microsoft Edge WebView2, а в этой Windows его нет.\n\
Так бывает, если Microsoft Edge удалили.\n\n\
Установить WebView2 сейчас? Axiom скачает установщик\n\
с сайта Microsoft (около 2 МБ), дальше он сам докачает\n\
остальное. Когда установка закончится, Axiom откроется.";

/// WebView2 есть или его удалось поставить — можно строить окно.
pub fn ensure() -> Result<()> {
    if installed() {
        return Ok(());
    }
    eprintln!("WebView2 не найден — предлагаем установить");
    if !ask(ASK) {
        tell(&format!(
            "Без WebView2 Axiom не сможет открыться.\n\n\
             Установить его можно и позже со страницы Microsoft:\n\
             {PAGE}\n\
             (Ctrl+C скопирует этот текст.)"
        ));
        bail!("WebView2 не найден, от установки отказались");
    }
    match install() {
        Ok(()) => {
            let version = wry::webview_version().unwrap_or_default();
            eprintln!("WebView2 установлен: {version}");
            Ok(())
        }
        Err(err) => {
            eprintln!("WebView2 не установился: {err:#}");
            tell(&format!(
                "Не получилось установить WebView2:\n{err:#}.\n\n\
                 Скачайте его со страницы Microsoft —\n\
                 раздел «Evergreen Bootstrapper», кнопка Download:\n\
                 {PAGE}\n\
                 (Ctrl+C скопирует этот текст.)\n\n\
                 После установки запустите Axiom снова."
            ));
            Err(err.context("WebView2 не найден, установить не вышло"))
        }
    }
}

fn installed() -> bool {
    // Только для проверки этого окна (debug-сборка): будто WebView2 нет.
    if cfg!(debug_assertions) && std::env::var_os("AXIOM_NO_WEBVIEW2").is_some() {
        return false;
    }
    wry::webview_version().is_ok()
}

fn install() -> Result<()> {
    let dir = std::env::temp_dir().join(format!("axiom-webview2-{}", std::process::id()));
    fs::create_dir_all(&dir).context("не удалось создать временную папку")?;
    let setup = dir.join("MicrosoftEdgeWebview2Setup.exe");
    let result = download(&setup).and_then(|()| run(&setup));
    let _ = fs::remove_dir_all(&dir);
    // Загрузчик закончил: WebView2 обычно виден сразу, но дадим ему пару секунд. Код
    // ошибки не мешает, если WebView2 всё же встал (или его уже поставили рядом).
    let wait = if result.is_ok() { 10 } else { 2 };
    let until = Instant::now() + Duration::from_secs(wait);
    while !installed() {
        if Instant::now() >= until {
            result?;
            bail!("установщик закончил, но WebView2 так и не появился");
        }
        thread::sleep(Duration::from_millis(500));
    }
    Ok(())
}

fn download(to: &Path) -> Result<()> {
    let out = crate::update::curl()
        // Только HTTPS, и после перенаправления на хранилище файлов Microsoft.
        .args(["--proto", "=https", "--proto-redir", "=https"])
        .args(["--fail", "--location", "--retry", "2", "--max-time", "300"])
        .arg("--output")
        .arg(to)
        .arg(BOOTSTRAPPER)
        .output()
        .context("не удалось запустить curl")?;
    if out.status.success() {
        return Ok(());
    }
    Err(match out.status.code() {
        Some(5 | 6 | 7 | 28 | 35) => anyhow!("нет связи с сайтом Microsoft"),
        _ => {
            let text = String::from_utf8_lossy(&out.stderr);
            anyhow!(
                "не удалось скачать установщик ({})",
                text.trim().trim_start_matches("curl: ")
            )
        }
    })
}

/// Запустить загрузчик с его окном установки и дождаться, пока он закончит.
fn run(setup: &Path) -> Result<()> {
    match Command::new(setup).arg("/install").status() {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => {
            let code = status.code().unwrap_or(-1);
            bail!(
                "установщик Microsoft завершился с кодом {:#010x}",
                code as u32
            )
        }
        // ERROR_ELEVATION_REQUIRED: установщик просит права администратора. Запуск
        // через оболочку покажет запрос UAC, но дождаться процесса так нельзя —
        // ждём самого WebView2 (установщик показывает своё окно с ходом установки).
        Err(err) if err.raw_os_error() == Some(740) => {
            elevated(setup)?;
            let until = Instant::now() + Duration::from_secs(15 * 60);
            while !installed() {
                if Instant::now() >= until {
                    bail!("установка WebView2 не закончилась за 15 минут");
                }
                thread::sleep(Duration::from_secs(1));
            }
            Ok(())
        }
        Err(err) => Err(anyhow!(err).context("не удалось запустить установщик")),
    }
}

#[link(name = "user32")]
unsafe extern "system" {
    fn MessageBoxW(hwnd: isize, text: *const u16, caption: *const u16, kind: u32) -> i32;
}

#[link(name = "shell32")]
unsafe extern "system" {
    fn ShellExecuteW(
        hwnd: isize,
        operation: *const u16,
        file: *const u16,
        parameters: *const u16,
        directory: *const u16,
        show: i32,
    ) -> isize;
}

const MB_OK: u32 = 0x0;
const MB_YESNO: u32 = 0x4;
const MB_ICONWARNING: u32 = 0x30;
const MB_ICONINFORMATION: u32 = 0x40;
/// Окна Axiom ещё нет: без этих флагов вопрос мог бы встать позади других окон.
const MB_SETFOREGROUND: u32 = 0x1_0000;
const MB_TOPMOST: u32 = 0x4_0000;
const IDYES: i32 = 6;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn message(text: &str, kind: u32) -> i32 {
    let (text, caption) = (wide(text), wide("Axiom"));
    // SAFETY: строки с нулём на конце живут до конца вызова; окна-владельца нет.
    unsafe {
        MessageBoxW(
            0,
            text.as_ptr(),
            caption.as_ptr(),
            kind | MB_SETFOREGROUND | MB_TOPMOST,
        )
    }
}

/// «Да» / «Нет»; `true` — «Да».
fn ask(text: &str) -> bool {
    message(text, MB_YESNO | MB_ICONINFORMATION) == IDYES
}

fn tell(text: &str) {
    message(text, MB_OK | MB_ICONWARNING);
}

/// Запуск с запросом прав администратора (`runas`), без ожидания процесса.
fn elevated(setup: &Path) -> Result<()> {
    const SW_SHOWNORMAL: i32 = 1;
    let (verb, file, args) = (
        wide("runas"),
        wide(&setup.display().to_string()),
        wide("/install"),
    );
    // SAFETY: строки с нулём на конце живут до конца вызова.
    let code = unsafe {
        ShellExecuteW(
            0,
            verb.as_ptr(),
            file.as_ptr(),
            args.as_ptr(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    // Меньше 33 — код ошибки (так устроен ShellExecute); 5 — отказ в запросе UAC.
    match code {
        5 => bail!("установка отменена в запросе прав администратора"),
        c if c <= 32 => bail!("не удалось запустить установщик (код {c})"),
        _ => Ok(()),
    }
}
