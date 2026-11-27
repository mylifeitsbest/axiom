//! Блокировка рекламы в WebKitGTK: правила — в формате блокировщиков Safari
//! (его умеет выдавать движок Brave), WebKit компилирует их в байт-код в профиле
//! (`<профиль>/adblock/webkit`) и сам применяет: запросы не уходят, элементы
//! скрыты (общие правила — одной таблицей стилей на все страницы). Компиляция —
//! один раз на набор списков: при следующих запусках WebKit только читает готовое,
//! а Axiom не читает и не разбирает сами списки.
//!
//! «Не блокировать на этом сайте» — фильтр снимается с вкладки, когда она начинает
//! грузить страницу этого сайта, и ставится обратно на другом сайте.
//!
//! Счётчик: заблокировав загрузку, WebKit пишет в консоль страницы «Content blocker
//! prevented frame displaying … from loading a resource from …» — расширение
//! веб-процесса (`src/tabs/webext.c`) ловит эти строки и шлёт вкладке сообщение
//! `axiom-adblock`.

use std::{
    cell::RefCell,
    ffi::{CStr, CString},
    fs,
    path::{Path, PathBuf},
    ptr,
    time::UNIX_EPOCH,
};

use gtk::glib::{self, WeakRef, prelude::ObjectExt, translate::ToGlibPtr};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tao::event_loop::EventLoopProxy;
use webkit2gtk::{LoadEvent, UserContentManagerExt, UserMessageExt, WebViewExt, ffi as wk};
use wry::{WebView, WebViewExtUnix};

use super::{Event, ListInfo, lists};
use crate::{events::UserEvent, tabs::TabId};

/// Имя правил в хранилище WebKit и в менеджерах вкладок.
const ID: &str = "axiom-adblock";
/// Поменялся перевод правил — старый байт-код не годится.
const FORMAT: &str = "1";
const STORE: &str = "webkit";
/// Из каких списков скомпилированы правила в `STORE`.
const STAMP: &str = "webkit.json";

#[derive(Debug, Serialize, Deserialize)]
struct Stamp {
    fingerprint: String,
    lists: Vec<StampList>,
}

#[derive(Debug, Serialize, Deserialize)]
struct StampList {
    id: String,
    version: u64,
    modified: String,
    downloaded: bool,
}

/// Отпечаток списков, из которых собирались бы правила: встроенных (отпечаток вшит
/// build.rs) и скачанных (размер и время записи) — без чтения самих списков.
fn fingerprint(dir: &Path) -> String {
    let mut sha = Sha256::new();
    sha.update(FORMAT);
    sha.update(env!("CARGO_PKG_VERSION"));
    sha.update(env!("AXIOM_ADBLOCK_LISTS"));
    for source in &lists::SOURCES {
        let (len, written) = fs::metadata(lists::path_of(dir, source))
            .map(|m| {
                let written = m
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_nanos());
                (m.len(), written)
            })
            .unwrap_or((0, 0));
        sha.update(source.id);
        sha.update(len.to_le_bytes());
        sha.update(written.to_le_bytes());
    }
    format!("{:x}", sha.finalize())
}

/// Правила этих списков уже скомпилированы: что за списки (для настроек).
pub fn compiled(dir: &Path) -> Option<Vec<ListInfo>> {
    let stamp: Stamp = crate::storage::read_json(&dir.join(STAMP))?;
    if stamp.fingerprint != fingerprint(dir) || !dir.join(STORE).is_dir() {
        return None;
    }
    Some(
        stamp
            .lists
            .into_iter()
            .filter_map(|l| {
                let source = lists::SOURCES.iter().find(|s| s.id == l.id)?;
                Some(ListInfo {
                    id: source.id,
                    title: source.title,
                    version: l.version,
                    modified: l.modified,
                    downloaded: l.downloaded,
                })
            })
            .collect(),
    )
}

/// Правила для WebKit (JSON) и отметка о том, из чего они.
pub struct Rules {
    stamp: Stamp,
    json: Option<Vec<u8>>,
}

impl Rules {
    /// В фоновом потоке.
    pub fn build(dir: &Path, texts: Vec<String>, lists: &[ListInfo]) -> Self {
        let stamp = Stamp {
            fingerprint: fingerprint(dir),
            lists: lists
                .iter()
                .map(|l| StampList {
                    id: l.id.into(),
                    version: l.version,
                    modified: l.modified.clone(),
                    downloaded: l.downloaded,
                })
                .collect(),
        };
        Self {
            stamp,
            json: convert(texts),
        }
    }
}

/// Списки → JSON блокировщика Safari.
fn convert(texts: Vec<String>) -> Option<Vec<u8>> {
    let mut set = adblock::lists::FilterSet::new(true);
    for text in texts {
        set.add_filter_list(text, adblock::lists::ParseOptions::default());
    }
    let (rules, _) = set.into_content_blocking().ok()?;
    match serde_json::to_vec(&rules) {
        Ok(json) => Some(json),
        Err(err) => {
            eprintln!("блокировка рекламы: правила для WebKit не собраны: {err}");
            None
        }
    }
}

/// Скомпилированные правила (`WebKitUserContentFilter`, своя ссылка).
struct Filter(*mut wk::WebKitUserContentFilter);

impl Drop for Filter {
    fn drop(&mut self) {
        // SAFETY: указатель — наша ссылка из `*_finish`, отдаём её один раз.
        unsafe { wk::webkit_user_content_filter_unref(self.0) };
    }
}

thread_local! {
    static DIR: RefCell<PathBuf> = const { RefCell::new(PathBuf::new()) };
    static FILTER: RefCell<Option<Filter>> = const { RefCell::new(None) };
    static VIEWS: RefCell<Vec<WeakRef<webkit2gtk::WebView>>> = const { RefCell::new(Vec::new()) };
}

pub fn prepare(dir: &Path) {
    DIR.with(|d| *d.borrow_mut() = dir.to_path_buf());
}

pub fn install(webview: &WebView, tab: TabId) {
    let view = webview.webview();
    VIEWS.with(|v| {
        let mut views = v.borrow_mut();
        views.retain(|w| w.upgrade().is_some());
        views.push(view.downgrade());
    });
    apply(&view, view.uri().as_deref());
    // Новая страница: фильтр — по её сайту, счёт — с нуля. Перенаправление на другой
    // сайт — снова по сайту.
    view.connect_load_changed(move |view, event| match event {
        LoadEvent::Started => {
            super::reset(tab);
            apply(view, view.uri().as_deref());
        }
        LoadEvent::Redirected => apply(view, view.uri().as_deref()),
        _ => {}
    });
    view.connect_user_message_received(move |_, message| {
        if message.name().as_deref() != Some("axiom-adblock") {
            return false;
        }
        if super::active() {
            super::count(tab);
            // Отладочная сборка: проверка на Linux в CI (.github/linux-gui/adblock.sh).
            #[cfg(debug_assertions)]
            eprintln!("блокировка рекламы: заблокировано во вкладке {tab}");
        }
        true
    });
}

/// Ставит или снимает фильтр у вкладки, которая открывает `url`.
fn apply(view: &webkit2gtk::WebView, url: Option<&str>) {
    let Some(manager) = view.user_content_manager() else {
        return;
    };
    let want = super::active() && !url.is_some_and(super::site_allowed);
    FILTER.with(|f| match (&*f.borrow(), want) {
        (Some(filter), true) => {
            // SAFETY: менеджер и фильтр живые; тот же id заменяет прежний.
            unsafe {
                wk::webkit_user_content_manager_add_filter(
                    ToGlibPtr::<*mut wk::WebKitUserContentManager>::to_glib_none(&manager).0,
                    filter.0,
                );
            }
        }
        _ => manager.remove_filter_by_id(ID),
    });
}

/// Включили, выключили или поменяли сайты: всем вкладкам — по их нынешнему адресу.
/// Открытая страница получит новое со следующей загрузкой.
pub fn refresh() {
    let views: Vec<_> = VIEWS.with(|v| {
        let mut views = v.borrow_mut();
        views.retain(|w| w.upgrade().is_some());
        views.iter().filter_map(WeakRef::upgrade).collect()
    });
    for view in views {
        apply(&view, view.uri().as_deref());
    }
}

fn store() -> Option<*mut wk::WebKitUserContentFilterStore> {
    let path = DIR.with(|d| d.borrow().join(STORE));
    crate::storage::private_dir(&path).ok()?;
    let path = CString::new(path.to_string_lossy().into_owned()).ok()?;
    // SAFETY: путь — строка C; хранилище — новая ссылка, её отдаёт колбэк.
    Some(unsafe { wk::webkit_user_content_filter_store_new(path.as_ptr()) })
}

/// Что ждёт ответа WebKit.
struct Pending {
    store: *mut wk::WebKitUserContentFilterStore,
    /// Компилировали (`save`) эти списки; `None` — читали готовое (`load`).
    stamp: Option<Stamp>,
    events: EventLoopProxy<UserEvent>,
}

fn report(events: &EventLoopProxy<UserEvent>, result: Result<(), String>) {
    let _ = events.send_event(UserEvent::Adblock(Event::Compiled(result)));
}

/// Правила собраны: скомпилировать в хранилище и раздать вкладкам.
pub fn compile(rules: Rules, events: EventLoopProxy<UserEvent>) {
    let Some(json) = rules.json else {
        report(&events, Err("правила для WebKit не собрались".into()));
        return;
    };
    let (Some(store), Ok(id)) = (store(), CString::new(ID)) else {
        report(&events, Err("нет папки для правил WebKit".into()));
        return;
    };
    let pending = Box::into_raw(Box::new(Pending {
        store,
        stamp: Some(rules.stamp),
        events,
    }));
    let bytes = glib::Bytes::from_owned(json);
    // SAFETY: строки и байты живут до конца вызова (GBytes WebKit держит сам);
    // `pending` вернётся в колбэк ровно один раз.
    unsafe {
        wk::webkit_user_content_filter_store_save(
            store,
            id.as_ptr(),
            ToGlibPtr::<*mut glib::ffi::GBytes>::to_glib_none(&bytes).0,
            ptr::null_mut(),
            Some(done),
            pending.cast(),
        );
    }
}

/// Правила этих списков уже скомпилированы: WebKit их только прочитает.
pub fn load(events: EventLoopProxy<UserEvent>) {
    let (Some(store), Ok(id)) = (store(), CString::new(ID)) else {
        let _ = events.send_event(UserEvent::Adblock(Event::Stale));
        return;
    };
    let pending = Box::into_raw(Box::new(Pending {
        store,
        stamp: None,
        events,
    }));
    // SAFETY: как в `compile`.
    unsafe {
        wk::webkit_user_content_filter_store_load(
            store,
            id.as_ptr(),
            ptr::null_mut(),
            Some(done),
            pending.cast(),
        );
    }
}

unsafe extern "C" fn done(
    _source: *mut glib::gobject_ffi::GObject,
    result: *mut gtk::gio::ffi::GAsyncResult,
    data: glib::ffi::gpointer,
) {
    // SAFETY: `data` — `Pending` из `compile`/`load`, колбэк зовётся один раз.
    let pending = unsafe { Box::from_raw(data.cast::<Pending>()) };
    let loading = pending.stamp.is_none();
    let mut error: *mut glib::ffi::GError = ptr::null_mut();
    // SAFETY: хранилище живо до конца колбэка, результат — от него же.
    let filter = unsafe {
        if loading {
            wk::webkit_user_content_filter_store_load_finish(pending.store, result, &mut error)
        } else {
            wk::webkit_user_content_filter_store_save_finish(pending.store, result, &mut error)
        }
    };
    let message = (!error.is_null()).then(|| {
        // SAFETY: ошибка от WebKit, освобождаем её здесь.
        unsafe {
            let text = CStr::from_ptr((*error).message)
                .to_string_lossy()
                .into_owned();
            glib::ffi::g_error_free(error);
            text
        }
    });
    // SAFETY: ссылка на хранилище из `store()` больше не нужна.
    unsafe { glib::gobject_ffi::g_object_unref(pending.store.cast()) };
    let dir = DIR.with(|d| d.borrow().clone());
    if filter.is_null() {
        let text = message.unwrap_or_else(|| "WebKit не принял правила".into());
        eprintln!("блокировка рекламы: {text}");
        if loading {
            // Готового нет или оно испорчено: собрать и скомпилировать заново.
            let _ = fs::remove_file(dir.join(STAMP));
            let _ = pending.events.send_event(UserEvent::Adblock(Event::Stale));
        } else {
            report(
                &pending.events,
                Err(format!("WebKit не принял правила: {text}")),
            );
        }
        return;
    }
    if let Some(stamp) = &pending.stamp
        && let Err(err) = crate::storage::write_json(&dir.join(STAMP), stamp)
    {
        eprintln!("блокировка рекламы: отметка о правилах не записана: {err:#}");
    }
    FILTER.with(|f| *f.borrow_mut() = Some(Filter(filter)));
    super::mark_ready();
    refresh();
    eprintln!(
        "блокировка рекламы: правила WebKit {}",
        if loading {
            "прочитаны"
        } else {
            "скомпилированы"
        }
    );
    report(&pending.events, Ok(()));
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn rules_for_webkit() {
        super::super::domain::install();
        let dir = std::env::temp_dir().join("axiom-adblock-no-such-dir");
        let texts: Vec<String> = lists::load(&dir).into_iter().map(|l| l.text).collect();
        let json = convert(texts).unwrap();
        let rules: Vec<serde_json::Value> = serde_json::from_slice(&json).unwrap();
        // WebKit больше 150 000 правил в одном списке не принимает.
        assert!(
            rules.len() > 10_000 && rules.len() < 150_000,
            "{}",
            rules.len()
        );
        let has = |kind: &str| rules.iter().any(|r| r["action"]["type"] == kind);
        assert!(has("block") && has("css-display-none") && has("ignore-previous-rules"));
    }

    #[test]
    fn stamp_follows_downloaded_lists() {
        let dir = std::env::temp_dir().join(format!("axiom-adblock-stamp-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join(STORE)).unwrap();
        let before = fingerprint(&dir);
        assert_eq!(before, fingerprint(&dir));
        let stamp = Stamp {
            fingerprint: before.clone(),
            lists: vec![StampList {
                id: "easylist".into(),
                version: 1,
                modified: String::new(),
                downloaded: false,
            }],
        };
        crate::storage::write_json(&dir.join(STAMP), &stamp).unwrap();
        assert_eq!(compiled(&dir).unwrap()[0].title, "EasyList");
        // Скачали свежий список — прежние правила не годятся.
        fs::write(dir.join("easylist.txt"), "[Adblock Plus 2.0]").unwrap();
        assert_ne!(fingerprint(&dir), before);
        assert!(compiled(&dir).is_none());
        let _ = fs::remove_dir_all(&dir);
    }
}
