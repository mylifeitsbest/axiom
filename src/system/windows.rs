//! Регистрация в Windows по схеме «Registered Applications»:
//! <https://learn.microsoft.com/windows/win32/shell/default-programs>

use std::{
    fs,
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, anyhow};
use windows_registry::CURRENT_USER;

use super::{install_path, same_file};

const APP: &str = "Axiom";
const URL_PROGID: &str = "AxiomURL";
const HTML_PROGID: &str = "AxiomHTML";
const CLIENT_KEY: &str = r"Software\Clients\StartMenuInternet\Axiom";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Копирует себя в `%LOCALAPPDATA%\Programs\Axiom`, создаёт ярлык и регистрирует браузер.
pub fn install() -> Result<PathBuf> {
    let current = std::env::current_exe().context("не найден путь к axiom.exe")?;
    let target = install_path().ok_or_else(|| anyhow!("не найдена папка LOCALAPPDATA"))?;
    if !same_file(&current, &target) {
        if let Some(dir) = target.parent() {
            fs::create_dir_all(dir).context("не удалось создать папку установки")?;
        }
        fs::copy(&current, &target).context(
            "не удалось скопировать axiom.exe — закройте установленную копию и повторите",
        )?;
    }
    reregister(&target)?;
    Ok(target)
}

/// Регистрация и ярлык для установленного exe.
pub fn reregister(exe: &Path) -> Result<()> {
    register(exe)?;
    shortcut(exe)
}

/// Ссылки открывает именно этот exe (после `--uninstall` регистрации нет).
pub fn is_registered(exe: &Path) -> bool {
    let quoted = format!("\"{}\"", exe.display()).to_lowercase();
    CURRENT_USER
        .open(format!(r"Software\Classes\{URL_PROGID}\shell\open\command"))
        .and_then(|key| key.get_string(""))
        .is_ok_and(|command| command.to_lowercase().starts_with(&quoted))
}

fn register(exe: &std::path::Path) -> Result<()> {
    let exe = exe.display().to_string();
    let icon = format!("\"{exe}\",0");
    let open_url = format!("\"{exe}\" \"%1\"");

    for (progid, name) in [
        (URL_PROGID, "Axiom URL"),
        (HTML_PROGID, "Axiom HTML Document"),
    ] {
        let key = CURRENT_USER.create(format!(r"Software\Classes\{progid}"))?;
        key.set_string("", name)?;
        if progid == URL_PROGID {
            key.set_string("URL Protocol", "")?;
        }
        key.create("DefaultIcon")?.set_string("", &icon)?;
        key.create(r"shell\open\command")?
            .set_string("", &open_url)?;
        let app = key.create("Application")?;
        app.set_string("ApplicationName", APP)?;
        app.set_string("ApplicationIcon", &icon)?;
    }

    let client = CURRENT_USER.create(CLIENT_KEY)?;
    client.set_string("", APP)?;
    client.create("DefaultIcon")?.set_string("", &icon)?;
    client
        .create(r"shell\open\command")?
        .set_string("", format!("\"{exe}\""))?;

    let caps = client.create("Capabilities")?;
    caps.set_string("ApplicationName", APP)?;
    caps.set_string("ApplicationIcon", &icon)?;
    caps.set_string(
        "ApplicationDescription",
        "Лёгкий браузер: минимум памяти, DuckDuckGo и Google AI Mode",
    )?;
    caps.create("StartMenu")?
        .set_string("StartMenuInternet", APP)?;
    let urls = caps.create("URLAssociations")?;
    urls.set_string("http", URL_PROGID)?;
    urls.set_string("https", URL_PROGID)?;
    let files = caps.create("FileAssociations")?;
    for ext in [".htm", ".html", ".shtml", ".xht", ".xhtml"] {
        files.set_string(ext, HTML_PROGID)?;
    }

    CURRENT_USER
        .create(r"Software\RegisteredApplications")?
        .set_string(APP, format!(r"{CLIENT_KEY}\Capabilities"))?;
    Ok(())
}

fn start_menu_link() -> Option<PathBuf> {
    let base = directories::BaseDirs::new()?;
    Some(
        base.data_dir()
            .join(r"Microsoft\Windows\Start Menu\Programs")
            .join("Axiom.lnk"),
    )
}

/// Ярлык создаём через WScript.Shell: путь передаётся переменной окружения,
/// а не подставляется в текст скрипта.
fn shortcut(exe: &std::path::Path) -> Result<()> {
    let link = start_menu_link().ok_or_else(|| anyhow!("не найдена папка меню «Пуск»"))?;
    let script = "$s = (New-Object -ComObject WScript.Shell).CreateShortcut($env:AXIOM_LNK); \
                  $s.TargetPath = $env:AXIOM_EXE; $s.IconLocation = $env:AXIOM_EXE + ',0'; \
                  $s.Description = 'Axiom'; $s.Save()";
    let status = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .env("AXIOM_LNK", &link)
        .env("AXIOM_EXE", exe)
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .context("не удалось запустить PowerShell для ярлыка")?;
    if status.success() {
        Ok(())
    } else {
        Err(anyhow!("не удалось создать ярлык в меню «Пуск»"))
    }
}

/// Убирает регистрацию и ярлык (`axiom.exe --uninstall`). Профиль не трогает.
pub fn uninstall() -> Result<()> {
    let _ = CURRENT_USER.remove_tree(CLIENT_KEY);
    let _ = CURRENT_USER.remove_tree(format!(r"Software\Classes\{URL_PROGID}"));
    let _ = CURRENT_USER.remove_tree(format!(r"Software\Classes\{HTML_PROGID}"));
    if let Ok(apps) = CURRENT_USER.create(r"Software\RegisteredApplications") {
        let _ = apps.remove_value(APP);
    }
    if let Some(link) = start_menu_link() {
        let _ = fs::remove_file(link);
    }
    Ok(())
}

/// Windows хранит выбор пользователя в `UserChoice`.
pub fn is_default_browser() -> bool {
    CURRENT_USER
        .open(r"Software\Microsoft\Windows\Shell\Associations\UrlAssociations\https\UserChoice")
        .and_then(|key| key.get_string("ProgId"))
        .is_ok_and(|id| id == URL_PROGID)
}

/// Открывает страницу «Приложения по умолчанию» сразу на Axiom.
pub fn open_default_apps() -> Result<()> {
    Command::new("explorer.exe")
        .arg("ms-settings:defaultapps?registeredAppUser=Axiom")
        .spawn()
        .context("не удалось открыть параметры Windows")?;
    Ok(())
}
