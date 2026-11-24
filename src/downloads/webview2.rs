//! Загрузки в WebView2: событие `DownloadStarting` и объект загрузки с прогрессом.
//!
//! wry уже ставит свой обработчик (разрешить и спрятать окно загрузок Edge),
//! наш срабатывает после него: уточняет путь и следит за прогрессом.
//! Все события WebView2 приходят в потоке интерфейса, поэтому объекты
//! загрузок хранятся в thread-local.
//!
//! Оборванную загрузку Chromium перезапускает сам (до пяти раз), и WebView2
//! сообщает о каждой попытке как о новой загрузке с новым объектом. Попытку
//! с тем же адресом и путём считаем продолжением: номер прежний, а события
//! от устаревших объектов игнорируются.
//!
//! Файл опасного типа (`.exe`, `.msi`, `.bat`…), скачанный без явного действия
//! пользователя, Edge не сохраняет, пока в его окне загрузок не нажмут
//! «Оставить», а это окно мы прячем — загрузка так и висела бы «идущей».
//! Поэтому проверку типа файла (`SaveFileSecurityCheckStarting`) для таких
//! файлов придерживаем и спрашиваем в своём списке. Настройки SmartScreen
//! не трогаем.

use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use tao::event_loop::EventLoopProxy;
use webview2_com::{
    BytesReceivedChangedEventHandler, DownloadStartingEventHandler,
    Microsoft::Web::WebView2::Win32::*, SaveFileSecurityCheckStartingEventHandler,
    StateChangedEventHandler, take_pwstr,
};
use windows_core::{BOOL, HSTRING, Interface, PWSTR};
use wry::{WebView, WebViewExtWindows};

use super::{DownloadEvent, DownloadId, Outcome, next_id, unique_path};
use crate::{events::UserEvent, tabs::TabId};

/// Как часто отправлять прогресс в интерфейс.
const PROGRESS_EVERY: Duration = Duration::from_millis(250);

/// Типы, которые Edge без жеста пользователя или без прошлых визитов на сайт
/// не сохраняет молча: `DANGEROUS` и `ALLOW_ON_USER_GESTURE` для Windows
/// в `download_file_types.asciipb` Chromium. Спрашиваем о них всегда: узнать,
/// остановил бы файл сам Edge, нельзя. По алфавиту, через пробел.
const RISKY: &str = "\
    accda accdb accde accdr ad ade adp app application appref-ms appx appxbundle asp asx bas \
    bat bgi cer cfg chi chm class cmd com cpi cpl crt crx der diagcab diagcfg diagpkg dll \
    drv efi eml exe fon fxp gadget grp hlp hta htt img inf ini ins inx iqy isp isu jar jnlp \
    job js jse library-ms lnk local mad maf mag mam manifest maq mar mas mat mau mav maw mda \
    mdb mde mdt mdw mdz mht mhtml mmc mof msc msg msh msh1 msh1xml msh2 msh2xml mshxml msi \
    msix msixbundle msp mst ocx ops oxt paf partial pcd pif pl plg prf prg ps1 ps1xml ps2 \
    ps2xml psc1 psc2 psm1 pst py pyc pyd pyo pyw pyz pyzw rb reg rels rgs scf scr sct \
    search-ms settingcontent-ms shb shs slk spl swf sys u3p url vb vbe vbs vbscript vdx vsd \
    vsdm vsdx vsmacros vss vssm vssx vst vstm vstx vsw vsx vtx website ws wsc wsf wsh xbap \
    xll xml xnk xrm-ms xsd xsl";

/// Проверка файла опасного типа, отложенная до ответа пользователя.
struct Pending {
    args: ICoreWebView2SaveFileSecurityCheckStartingEventArgs,
    deferral: ICoreWebView2Deferral,
    events: EventLoopProxy<UserEvent>,
}

impl Pending {
    /// Отвечает WebView2: сохранить файл или отменить загрузку.
    fn finish(self, keep: bool) -> windows_core::Result<()> {
        // SAFETY: объекты события живы, пока не завершена отсрочка; поток интерфейса.
        unsafe {
            if keep {
                // Пропускает только проверку типа (на неё и ответил пользователь).
                self.args.SetSuppressDefaultPolicy(true)?;
            } else {
                self.args.SetCancelSave(true)?;
            }
            self.deferral.Complete()
        }
    }
}

struct Tracked {
    /// Объекты всех попыток, последняя — текущая. Прежние держим до конца загрузки:
    /// если отпустить их раньше, WebView2 перестаёт сообщать о её завершении.
    operations: Vec<ICoreWebView2DownloadOperation>,
    url: String,
    path: PathBuf,
    /// Проверка типа файла ждёт решения пользователя.
    pending: Option<Pending>,
    /// Пользователь разрешил сохранить: повторные попытки уже не спрашиваем.
    kept: bool,
    /// Пользователь отказался: прерывание загрузки — это «удалено», а не ошибка.
    discarded: bool,
}

impl Tracked {
    fn current(&self) -> Option<&ICoreWebView2DownloadOperation> {
        self.operations.last()
    }
}

thread_local! {
    static TRACKED: RefCell<HashMap<DownloadId, Tracked>> = RefCell::new(HashMap::new());
}

/// Событие пришло от текущей попытки загрузки, а не от устаревшей.
fn is_current(id: DownloadId, operation: &ICoreWebView2DownloadOperation) -> bool {
    TRACKED.with(|t| {
        t.borrow()
            .get(&id)
            .and_then(Tracked::current)
            .is_some_and(|current| current.as_raw() == operation.as_raw())
    })
}

/// Подписывает webview на загрузки. `tab` — чья это вкладка (для панели ИИ `None`).
pub fn install(webview: &WebView, tab: Option<TabId>, events: EventLoopProxy<UserEvent>) {
    let Ok(core) = webview.webview().cast::<ICoreWebView2_4>() else {
        return;
    };
    let can_ask = watch_checks(&core, &events);
    let handler = DownloadStartingEventHandler::create(Box::new(move |_, args| {
        let Some(args) = args else { return Ok(()) };
        // SAFETY: args — живой COM-объект события, вызываем его в потоке интерфейса.
        unsafe { on_starting(&args, tab, can_ask, &events) }
    }));
    let mut token = 0i64;
    // SAFETY: core принадлежит webview, обработчик живёт, пока жив webview.
    if let Err(err) = unsafe { core.add_DownloadStarting(&handler, &mut token) } {
        eprintln!("загрузки недоступны: {err}");
    }
}

/// Подписывает на проверку типа сохраняемого файла. `false` — WebView2 старше
/// 1.0.2849 и этого события не знает.
fn watch_checks(core: &ICoreWebView2_4, events: &EventLoopProxy<UserEvent>) -> bool {
    let Ok(core) = core.cast::<ICoreWebView2_26>() else {
        return false;
    };
    let events = events.clone();
    let handler = SaveFileSecurityCheckStartingEventHandler::create(Box::new(move |_, args| {
        let Some(args) = args else { return Ok(()) };
        // SAFETY: args — живой COM-объект события, вызываем его в потоке интерфейса.
        unsafe { on_check(&args, &events) }
    }));
    let mut token = 0i64;
    // SAFETY: core принадлежит webview, обработчик живёт, пока жив webview.
    unsafe { core.add_SaveFileSecurityCheckStarting(&handler, &mut token) }.is_ok()
}

/// «Сохранить всё равно» для файла опасного типа.
pub fn keep(id: DownloadId) {
    decide(id, true);
}

/// Отменяет идущую загрузку.
pub fn cancel(id: DownloadId) -> bool {
    // Файл опасного типа ещё ждёт решения: отменить — значит не сохранять.
    if decide(id, false) {
        return true;
    }
    // Клонируем объект из карты: Cancel может сразу вызвать StateChanged, а тот правит карту.
    let Some(operation) = TRACKED.with(|t| t.borrow().get(&id).and_then(Tracked::current).cloned())
    else {
        return false;
    };
    // SAFETY: живой объект загрузки, поток интерфейса.
    unsafe { operation.Cancel() }.is_ok()
}

/// Передаёт решение пользователя отложенной проверке. `false` — решать нечего.
fn decide(id: DownloadId, keep: bool) -> bool {
    // Достаём из карты до вызова WebView2: ответ может сразу вызвать StateChanged.
    let Some(pending) = TRACKED.with(|t| {
        let mut tracked = t.borrow_mut();
        let download = tracked.get_mut(&id)?;
        let pending = download.pending.take()?;
        download.kept = keep;
        download.discarded = !keep;
        Some(pending)
    }) else {
        return false;
    };
    let events = pending.events.clone();
    if pending.finish(keep).is_err() {
        return false;
    }
    if keep {
        let _ = events.send_event(UserEvent::Download(DownloadEvent::Decision {
            id,
            waiting: false,
        }));
    }
    true
}

unsafe fn on_starting(
    args: &ICoreWebView2DownloadStartingEventArgs,
    tab: Option<TabId>,
    can_ask: bool,
    events: &EventLoopProxy<UserEvent>,
) -> windows_core::Result<()> {
    // SAFETY (весь блок): методы живых COM-объектов, указатели на локальные переменные.
    unsafe {
        let mut cancelled = BOOL::default();
        args.Cancel(&mut cancelled)?;
        if cancelled.as_bool() {
            return Ok(());
        }
        let operation = args.DownloadOperation()?;
        let mut raw = PWSTR::null();
        operation.Uri(&mut raw)?;
        let url = take_pwstr(raw);

        let mut raw = PWSTR::null();
        args.ResultFilePath(&mut raw)?;
        let mut path = PathBuf::from(take_pwstr(raw));
        let resumed = TRACKED.with(|t| {
            t.borrow()
                .iter()
                .find(|(_, d)| d.url == url && d.path == path)
                .map(|(&id, _)| id)
        });
        // WebView2 перезаписал бы файл с тем же именем — добавляем номер.
        if resumed.is_none() && path.exists() {
            path = unique_path(&path);
            args.SetResultFilePath(&HSTRING::from(path.as_os_str()))?;
        }
        // Окно загрузок Edge не показываем: у нас своя панель. Без события проверки
        // спросить о файле опасного типа нечем — пусть спросит Edge.
        if can_ask || !is_risky(&path) {
            args.SetHandled(true)?;
        }

        let id = resumed.unwrap_or_else(next_id);
        if resumed.is_none() {
            let (_, total) = bytes(&operation)?;
            let _ = events.send_event(UserEvent::Download(DownloadEvent::Started {
                id,
                url: url.clone(),
                path: path.clone(),
                total,
                tab,
            }));
        }

        let progress_events = events.clone();
        let last = Cell::new(None::<Instant>);
        let progress = BytesReceivedChangedEventHandler::create(Box::new(move |operation, _| {
            let Some(operation) = operation else {
                return Ok(());
            };
            if last.get().is_some_and(|t| t.elapsed() < PROGRESS_EVERY)
                || !is_current(id, &operation)
            {
                return Ok(());
            }
            last.set(Some(Instant::now()));
            let (received, total) = bytes(&operation)?;
            let _ = progress_events.send_event(UserEvent::Download(DownloadEvent::Progress {
                id,
                received,
                total,
            }));
            Ok(())
        }));
        let mut token = 0i64;
        operation.add_BytesReceivedChanged(&progress, &mut token)?;

        let state_events = events.clone();
        let state = StateChangedEventHandler::create(Box::new(move |operation, _| {
            let Some(operation) = operation else {
                return Ok(());
            };
            if !is_current(id, &operation) {
                return Ok(());
            }
            let Some(mut outcome) = outcome_of(&operation)? else {
                return Ok(());
            };
            let mut raw = PWSTR::null();
            operation.ResultFilePath(&mut raw)?;
            let path = PathBuf::from(take_pwstr(raw));
            let (received, _) = bytes(&operation)?;
            let tracked = TRACKED.with(|t| t.borrow_mut().remove(&id));
            if tracked.is_some_and(|d| d.discarded) && outcome != Outcome::Done {
                outcome = Outcome::Discarded;
            }
            let _ = state_events.send_event(UserEvent::Download(DownloadEvent::Finished {
                id,
                outcome,
                path: Some(path),
                received: Some(received),
            }));
            Ok(())
        }));
        operation.add_StateChanged(&state, &mut token)?;

        TRACKED.with(|t| {
            t.borrow_mut()
                .entry(id)
                .or_insert_with(|| Tracked {
                    operations: Vec::new(),
                    url,
                    path,
                    pending: None,
                    kept: false,
                    discarded: false,
                })
                .operations
                .push(operation)
        });
    }
    Ok(())
}

/// Проверка типа сохраняемого файла: о файле опасного типа спрашиваем пользователя.
unsafe fn on_check(
    args: &ICoreWebView2SaveFileSecurityCheckStartingEventArgs,
    events: &EventLoopProxy<UserEvent>,
) -> windows_core::Result<()> {
    // SAFETY (весь блок): методы живого COM-объекта, указатели на локальные переменные.
    unsafe {
        let mut raw = PWSTR::null();
        args.FilePath(&mut raw)?;
        let path = PathBuf::from(take_pwstr(raw));
        let mut raw = PWSTR::null();
        args.DocumentOriginUri(&mut raw)?;
        let url = take_pwstr(raw);
        // Остальные типы Edge пропускает сам, не задерживая загрузку.
        if !is_risky(&path) {
            return Ok(());
        }
        let found = TRACKED.with(|t| {
            let tracked = t.borrow();
            tracked
                .iter()
                .find(|(_, d)| d.path == path)
                .or_else(|| tracked.iter().find(|(_, d)| d.url == url && !d.kept))
                .map(|(&id, d)| (id, d.kept))
        });
        // Не загрузка из нашего списка — решает Edge.
        let Some((id, kept)) = found else {
            return Ok(());
        };
        if kept {
            // Новая попытка после обрыва связи: пользователь уже разрешил.
            return args.SetSuppressDefaultPolicy(true);
        }
        let pending = Pending {
            args: args.clone(),
            deferral: args.GetDeferral()?,
            events: events.clone(),
        };
        let stale = TRACKED.with(|t| {
            t.borrow_mut()
                .get_mut(&id)
                .and_then(|d| d.pending.replace(pending))
        });
        // Прежняя попытка тоже ждала ответа — её место заняла новая.
        if let Some(stale) = stale {
            let _ = stale.finish(false);
        }
        let _ = events.send_event(UserEvent::Download(DownloadEvent::Decision {
            id,
            waiting: true,
        }));
    }
    Ok(())
}

fn is_risky(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
        return false;
    };
    RISKY
        .split_ascii_whitespace()
        .any(|risky| risky.eq_ignore_ascii_case(ext))
}

/// Получено байт и сколько всего (если сервер прислал размер).
fn bytes(operation: &ICoreWebView2DownloadOperation) -> windows_core::Result<(u64, Option<u64>)> {
    let (mut received, mut total) = (0i64, 0i64);
    // SAFETY: живой объект загрузки, указатели на локальные переменные.
    unsafe {
        operation.BytesReceived(&mut received)?;
        operation.TotalBytesToReceive(&mut total)?;
    }
    let total = u64::try_from(total).ok().filter(|&t| t > 0);
    Ok((u64::try_from(received).unwrap_or(0), total))
}

/// Чем закончилась загрузка; `None` — ещё идёт.
fn outcome_of(operation: &ICoreWebView2DownloadOperation) -> windows_core::Result<Option<Outcome>> {
    let mut state = COREWEBVIEW2_DOWNLOAD_STATE::default();
    // SAFETY: живой объект загрузки, указатель на локальную переменную.
    unsafe { operation.State(&mut state)? };
    if state == COREWEBVIEW2_DOWNLOAD_STATE_IN_PROGRESS {
        return Ok(None);
    }
    if state == COREWEBVIEW2_DOWNLOAD_STATE_COMPLETED {
        return Ok(Some(Outcome::Done));
    }
    let mut reason = COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON::default();
    // SAFETY: см. выше.
    unsafe { operation.InterruptReason(&mut reason)? };
    if reason == COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_USER_PAUSED {
        // Паузы в нашем интерфейсе нет — продолжаем, если её поставил кто-то ещё.
        // SAFETY: см. выше.
        let _ = unsafe { operation.Resume() };
        return Ok(None);
    }
    if reason == COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_USER_CANCELED {
        return Ok(Some(Outcome::Cancelled));
    }
    Ok(Some(Outcome::Failed(describe(reason).into())))
}

fn describe(reason: COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON) -> &'static str {
    match reason {
        COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_NETWORK_FAILED
        | COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_NETWORK_DISCONNECTED
        | COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_NETWORK_TIMEOUT => "Ошибка сети",
        COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_NETWORK_SERVER_DOWN
        | COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_SERVER_FAILED
        | COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_SERVER_BAD_CONTENT
        | COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_SERVER_UNEXPECTED_RESPONSE
        | COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_SERVER_NO_RANGE => "Ошибка сервера",
        COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_SERVER_CONTENT_LENGTH_MISMATCH => {
            "Соединение оборвалось"
        }
        COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_SERVER_UNAUTHORIZED
        | COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_SERVER_FORBIDDEN => "Сервер не дал скачать",
        COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_SERVER_CERTIFICATE_PROBLEM => {
            "Ошибка сертификата сайта"
        }
        COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_NO_SPACE => "Нет места на диске",
        COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_ACCESS_DENIED => "Нет доступа к папке",
        COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_NAME_TOO_LONG => "Слишком длинное имя файла",
        COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_TOO_LARGE => "Файл слишком большой",
        COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_MALICIOUS
        | COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_BLOCKED_BY_POLICY
        | COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_SECURITY_CHECK_FAILED => {
            "Заблокировано системой безопасности"
        }
        COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_USER_SHUTDOWN => "Прервано: браузер закрыт",
        _ => "Не удалось скачать",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn risky_types() {
        let types: Vec<_> = RISKY.split_ascii_whitespace().collect();
        assert!(
            types.windows(2).all(|w| w[0] < w[1]),
            "RISKY не по алфавиту"
        );
        assert_eq!(types.len(), 168);
        assert!(is_risky(Path::new(r"C:\Загрузки\setup.EXE")));
        assert!(is_risky(Path::new("install.msi")));
        assert!(is_risky(Path::new("run.bat")));
        assert!(!is_risky(Path::new("photos.zip")));
        assert!(!is_risky(Path::new("firmware.bin")));
        assert!(!is_risky(Path::new("README")));
    }
}
