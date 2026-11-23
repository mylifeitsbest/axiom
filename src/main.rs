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
mod storage;
mod system;
mod tabs;
mod ui;
mod update;

use anyhow::Result;
use tao::{
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder},
};

use crate::{app::App, events::UserEvent};

fn main() {
    if let Err(err) = run() {
        report_fatal(&err);
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // Служебные команды без окна.
    if args.iter().any(|a| a == "--uninstall") {
        return system::uninstall();
    }
    if args.iter().any(|a| a == "--install") {
        let path = system::install()?;
        println!("Axiom установлен: {}", path.display());
        return Ok(());
    }
    if args.iter().any(|a| a == "--check-chrome") {
        let paths = storage::Paths::resolve()?;
        let data = storage::chrome::import(&paths)?;
        println!(
            "Chrome, профиль «{}»: закладок {}, записей истории {}",
            data.profile,
            data.bookmarks.len(),
            data.history.len()
        );
        return Ok(());
    }
    let updated = args.iter().any(|a| a == update::UPDATED_FLAG);
    let urls: Vec<String> = args.into_iter().filter(|a| !a.starts_with("--")).collect();

    let paths = storage::Paths::resolve()?;
    // Новую версию запустила старая: ждём, пока та закроется, иначе адреса ушли бы ей.
    if updated {
        update::finish(&paths);
    }
    // Если Axiom уже открыт — передаём ему адреса и выходим.
    let Some(server) = instance::claim(&paths, &urls) else {
        return Ok(());
    };
    update::cleanup();

    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let mut app = App::new(&event_loop, paths, &urls, updated)?;
    events::spawn_ticker(event_loop.create_proxy());
    if let Some(server) = server {
        server.spawn(event_loop.create_proxy());
    }

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
            _ => app::Flow::Continue,
        };
        if flow == app::Flow::Exit {
            app.shutdown();
            *control_flow = ControlFlow::Exit;
        }
    });
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
