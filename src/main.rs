//! Axiom — лёгкий браузер на системном WebView.
//!
//! Архитектура: одно окно `tao`, в нём служебный webview с интерфейсом (`ui/`)
//! и дочерние webview для вкладок и панели ИИ поверх области контента.
//! Интерфейс общается с Rust через IPC `wry`, сайты доступа к IPC не имеют.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod ai_panel;
mod app;
mod downloads;
mod events;
mod instance;
mod ipc;
mod settings;
mod stats;
mod storage;
mod system;
mod tabs;
mod translate;
mod ui;
mod update;

use anyhow::Result;
use tao::{
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder},
};

use crate::{app::App, events::UserEvent};

fn main() {
    #[cfg(target_os = "linux")]
    webkit_workarounds();
    if let Err(err) = run() {
        report_fatal(&err);
        std::process::exit(1);
    }
}

/// Linux с видеокартой NVIDIA: у WebKitGTK вывод кадров через DMA-BUF на этих
/// драйверах мигает, дёргает видео и теряет кадры (известная проблема WebKitGTK,
/// её же обходят приложения на Tauri). Включаем запасной путь, если человек не
/// задал переменную сам. Вызывается первой строкой `main`, пока потоков ещё нет.
#[cfg(target_os = "linux")]
fn webkit_workarounds() {
    const VAR: &str = "WEBKIT_DISABLE_DMABUF_RENDERER";
    let nvidia = std::path::Path::new("/proc/driver/nvidia/version").exists()
        || std::path::Path::new("/sys/module/nvidia").exists();
    if nvidia && std::env::var_os(VAR).is_none() {
        // SAFETY: начало `main`, других потоков ещё нет — окружение никто не читает.
        unsafe { std::env::set_var(VAR, "1") };
    }
}

fn run() -> Result<()> {
    // `args_os`: `args()` паникует на имени файла не в UTF-8 (бывает на Linux).
    let args: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    // Служебные команды без окна — только первым аргументом. Windows подставляет
    // адрес ссылки в `"axiom.exe" "%1"` как есть, и ссылка с кавычкой внутри
    // (`https://x/" --uninstall "`) иначе добавила бы свои ключи.
    let command = args.first().map(String::as_str);
    if command == Some("--uninstall") {
        return system::uninstall();
    }
    if command == Some("--install") {
        let path = system::install()?;
        println!("Axiom установлен: {}", path.display());
        return Ok(());
    }
    if command == Some("--check-import") {
        return check_import();
    }
    let updated = command == Some(update::UPDATED_FLAG);
    let handed_off = command == Some(system::handoff::FLAG);
    let urls: Vec<String> = args
        .into_iter()
        .filter(|a| !a.starts_with("--"))
        .map(absolute_file)
        .collect();

    let paths = storage::Paths::resolve()?;
    // Профиль уже открывала более новая версия: адреса — ей, а профиль не трогаем.
    if !updated && !handed_off && system::handoff::to_newer(&paths, &urls) {
        return Ok(());
    }
    // Новую версию запустила старая: ждём, пока та закроется, иначе адреса ушли бы ей.
    if updated {
        update::finish(&paths);
    }
    // Если Axiom уже открыт — передаём ему адреса и выходим.
    let Some(server) = instance::claim(&paths, &urls) else {
        return Ok(());
    };
    update::cleanup();
    stats::start(&paths.root);
    system::started(&paths);

    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let mut app = App::new(&event_loop, paths, &urls, updated)?;
    events::spawn_ticker(event_loop.create_proxy());
    let registration = server.map(|server| server.spawn(event_loop.create_proxy()));

    event_loop.run(move |event, _target, control_flow| {
        *control_flow = ControlFlow::Wait;
        let flow = match event {
            Event::WindowEvent { event, .. } => match event {
                WindowEvent::CloseRequested => app::Flow::Exit,
                WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                    app.on_resize();
                    app::Flow::Continue
                }
                _ => app::Flow::Continue,
            },
            Event::UserEvent(user_event) => app.handle(user_event),
            // Выход из системы, перезагрузка, Restart Manager: tao присылает только
            // это событие и сразу завершает процесс — CloseRequested не будет.
            Event::LoopDestroyed => app::Flow::Exit,
            _ => app::Flow::Continue,
        };
        if flow == app::Flow::Exit {
            app.shutdown();
            // Порт и токен на диске больше ни к чему: следующий запуск не будет
            // стучаться в закрытый порт.
            if let Some(registration) = &registration {
                registration.release();
            }
            *control_flow = ControlFlow::Exit;
        }
    });
}

/// `--check-import`: что будет перенесено из каждого найденного браузера, без записи.
fn check_import() -> Result<()> {
    let paths = storage::Paths::resolve()?;
    let found = storage::import::detect();
    if found.is_empty() {
        println!("Других браузеров не найдено");
    }
    for browser in found {
        let profile = if browser.profile.is_empty() {
            String::new()
        } else {
            format!(", профиль «{}»", browser.profile)
        };
        match storage::import::import(browser.id, &paths) {
            Ok(data) => println!(
                "{}{profile}: закладок {}, записей истории {}",
                browser.name,
                data.bookmarks.len(),
                data.history.len()
            ),
            Err(err) => println!("{}{profile}: ошибка: {err:#}", browser.name),
        }
    }
    Ok(())
}

/// `axiom page.html` из консоли: относительный путь превращаем в полный,
/// иначе уже запущенное окно (у него другая рабочая папка) приняло бы его за адрес сайта.
fn absolute_file(arg: String) -> String {
    let path = std::path::Path::new(&arg);
    if path.is_relative()
        && path.is_file()
        && let Ok(full) = std::path::absolute(path)
    {
        return full.display().to_string();
    }
    arg
}

/// Пишет фатальную ошибку в stderr и в файл рядом с профилем,
/// потому что в release-сборке на Windows консоли нет.
fn report_fatal(err: &anyhow::Error) {
    let text = format!("Axiom не смог запуститься: {err:#}");
    eprintln!("{text}");
    if let Ok(paths) = storage::Paths::resolve() {
        let _ = std::fs::write(paths.root.join("last-error.log"), &text);
    }
}
