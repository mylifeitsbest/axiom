//! Загрузки в WebKitGTK через обработчики wry.
//!
//! wry вешает обработчики на общий `WebContext` при создании каждого webview:
//! путь выбирает первый из них (служебный webview интерфейса создаётся первым,
//! поэтому обработчики ставятся и ему), а о завершении сообщают все сразу.
//! Повторы отсекаются здесь: загрузка закрывается один раз.

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
            // Путь уже уникальный: wry сам добавляет « (1)» к занятому имени.
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
                let index = pending
                    .iter()
                    .position(|d| path.as_ref().is_some_and(|path| &d.path == path))
                    .or_else(|| pending.iter().position(|d| d.url == url))?;
                Some(pending.remove(index))
            });
            let Some(download) = found else { return };
            let received = path
                .as_ref()
                .and_then(|p| std::fs::metadata(p).ok())
                .map(|m| m.len());
            let outcome = if ok {
                Outcome::Done
            } else {
                Outcome::Failed("Не удалось скачать".into())
            };
            let _ = done_tx.send_event(UserEvent::Download(DownloadEvent::Finished {
                id: download.id,
                outcome,
                path,
                received,
            }));
        })
}
