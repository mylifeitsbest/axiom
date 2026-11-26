//! Загрузки в WebKitGTK через обработчики wry.
//!
//! wry вешает обработчики на общий `WebContext` при создании каждого webview:
//! путь выбирает первый из них (служебный webview интерфейса создаётся первым,
//! поэтому обработчики ставятся и ему), а о завершении сообщают все сразу.
//! Повторы отсекаются здесь: загрузка закрывается один раз.
//!
//! Путь wry предлагает сам — в папке «Загрузки», а без неё (нет `user-dirs.dirs`)
//! в текущей папке процесса. Поэтому место выбирает [`super::place`]: всегда
//! «Загрузки», файл опасного типа — под временным именем, пока человек не решит.

use std::{cell::RefCell, path::PathBuf};

use tao::event_loop::EventLoopProxy;
use wry::WebViewBuilder;

use super::{DownloadEvent, DownloadId, Outcome, next_id};
use crate::events::UserEvent;

struct Pending {
    id: DownloadId,
    url: String,
    path: PathBuf,
}

thread_local! {
    static PENDING: RefCell<Vec<Pending>> = const { RefCell::new(Vec::new()) };
}

pub fn attach<'a>(
    builder: WebViewBuilder<'a>,
    events: &EventLoopProxy<UserEvent>,
) -> WebViewBuilder<'a> {
    let (start_tx, done_tx) = (events.clone(), events.clone());
    builder
        .with_download_started_handler(move |url, path| {
            // Без папки «Загрузки» файлу некуда лечь: в текущую папку не кладём.
            let Some(dir) = super::folder() else {
                eprintln!("загрузка отменена: не найдена папка «Загрузки»");
                return false;
            };
            if let Err(err) = std::fs::create_dir_all(&dir) {
                eprintln!("загрузка отменена: {}: {err}", dir.display());
                return false;
            }
            *path = super::place(&dir, path);
            let id = next_id();
            PENDING.with(|p| {
                p.borrow_mut().push(Pending {
                    id,
                    url: url.clone(),
                    path: path.clone(),
                })
            });
            let _ = start_tx.send_event(UserEvent::Download(DownloadEvent::Started {
                id,
                url,
                path: path.clone(),
                total: None,
                tab: None,
            }));
            true
        })
        .with_download_completed_handler(move |url, path, ok| {
            let found = PENDING.with(|p| {
                let mut pending = p.borrow_mut();
                // По адресу — только если путь неизвестен: обработчик вызывается
                // по разу на каждый webview, и повторы закрыли бы другую загрузку
                // того же адреса, которая ещё идёт.
                let index = match &path {
                    Some(path) => pending.iter().position(|d| &d.path == path),
                    None => pending.iter().position(|d| d.url == url),
                }?;
                Some(pending.remove(index))
            });
            let Some(download) = found else { return };
            let path = path.unwrap_or(download.path);
            let received = std::fs::metadata(&path).ok().map(|m| m.len());
            // Файл опасного типа скачан под временным именем: оставить его
            // или удалить, решает человек в списке загрузок.
            let event = if ok && super::confirmed(&path).is_some() {
                DownloadEvent::Disguised {
                    id: download.id,
                    path,
                    received: received.unwrap_or(0),
                    program: false,
                }
            } else {
                let outcome = if ok {
                    Outcome::Done
                } else {
                    Outcome::Failed("Не удалось скачать".into())
                };
                DownloadEvent::Finished {
                    id: download.id,
                    outcome,
                    path: Some(path),
                    received,
                }
            };
            let _ = done_tx.send_event(UserEvent::Download(event));
        })
}
