//! Что играет во вкладке и кнопки для островка (`ui/island.js`): название, исполнитель,
//! пауза, «дальше», «назад» и видео внутри островка (`tabs::mini`).
//!
//! Скрипт [`SCRIPT`] встаёт в главный фрейм раньше скриптов страницы и запоминает
//! обработчики кнопок мультимедиа, которые ставит страница
//! (`navigator.mediaSession.setActionHandler`): по ним работают «дальше» и «назад»,
//! как у кнопок на клавиатуре. Сам `setActionHandler` остаётся настоящим и вызывается
//! как есть. Браузер спрашивает страницу через `window.__axiomMedia` без жеста
//! пользователя ([`QUERY`], [`action`]); страница видит эту функцию, но через неё
//! может только нажать свои же кнопки.
//!
//! Видео в островке ([`Action::Peek`], `tabs::mini`): страница выводит своё видео в
//! верхний слой (`popover`) поверх всего, в заданный прямоугольник ([`mini_script`]), на
//! чёрном фоне и со скруглёнными углами. Размер вкладки при этом не меняется (сайт не
//! перестраивает страницу), браузер показывает только вырез под чёлкой. Само видео
//! остаётся на своём месте в документе и играет дальше; стиль — отдельный лист
//! (`adoptedStyleSheets`), его не блокирует CSP сайта. [`Action::Unpeek`] возвращает всё
//! как было.
//!
//! Видео и звук во встроенных чужих фреймах (плеер с другого сайта) отсюда не видны:
//! для них островок показывает только вкладку и звук.

use serde::{Deserialize, Serialize};

/// Скрипт главного фрейма вкладки (см. описание модуля).
pub const SCRIPT: &str = r#"(() => {
  const W = window;
  const D = document;
  const call = Reflect.apply;
  const quiet = (f) => { try { return f(); } catch {} };
  const handlers = new Map();
  quiet(() => {
    const P = MediaSession.prototype;
    const d = Object.getOwnPropertyDescriptor(P, 'setActionHandler');
    const native = d.value;
    const ms = () => quiet(() => navigator.mediaSession);
    const wrapped = new Proxy(native, {
      apply(target, self, args) {
        quiet(() => {
          if (self !== ms()) return;
          const a = String(args[0]);
          if (typeof args[1] === 'function') handlers.set(a, args[1]); else handlers.delete(a);
        });
        return call(target, self, args);
      },
    });
    Object.defineProperty(P, 'setActionHandler', { ...d, value: wrapped });
  });
  // Последний заигравший элемент, в том числе не в документе (`new Audio()`).
  let last = null;
  quiet(() => W.addEventListener('play', (e) => quiet(() => {
    if (e.target instanceof HTMLMediaElement) last = new WeakRef(e.target);
  }), true));
  const lastEl = () => (last ? last.deref() || null : null);
  const media = () => {
    const list = Array.from(D.querySelectorAll('video,audio'));
    const l = lastEl();
    if (l && !list.includes(l)) list.push(l);
    return list;
  };
  const playing = (m) => !m.paused && !m.ended;
  const area = (m) => (m.tagName === 'VIDEO' ? m.clientWidth * m.clientHeight : 0);
  const pick = () => media().filter(playing).sort((a, b) => area(b) - area(a))[0] || lastEl();
  // Видео, которое стоит показывать в островке: настоящее и не крошечное (не превью и не реклама в углу).
  const big = (m) => !!m && m.tagName === 'VIDEO' && m.videoWidth > 0 && m.clientWidth >= 200
    && m.clientHeight >= 112;
  const press = (name) => {
    const h = handlers.get(name);
    if (!h) return false;
    quiet(() => h({ action: name }));
    return true;
  };
  // ---------- видео в островке: поверх всего на чёрном, в заданном месте ----------
  const MARK = 'data-axiom-mini';
  let sheet = null;
  let shownEl = null;
  const ours = new WeakSet(); // popover поставили мы, а не страница
  const num = (v) => (typeof v === 'number' && isFinite(v) ? Math.round(v) : null);
  function size() {
    const v = pick();
    return big(v) ? { w: v.videoWidth, h: v.videoHeight } : null;
  }
  // Прямоугольник — в логических px окна; `s` — масштаб экрана. У страницы ещё свой
  // масштаб (Ctrl+колесо): CSS px = логические / (devicePixelRatio / s).
  function mini(x, y, w, h, s) {
    const z = s > 0 ? W.devicePixelRatio / s : 1;
    [x, y, w, h] = [x, y, w, h].map((v) => num(v / (z > 0 ? z : 1)));
    if (x == null || y == null || !(w > 0) || !(h > 0)) return false;
    const shown = shownEl ? shownEl.deref() : null;
    const v = shown && shown.isConnected ? shown : pick();
    // Уже показанное видео стоит размером с остров (и вкладка на Linux тоже) — оно
    // меньше «видео для островка», но переставлять его можно.
    if (v !== shown && !big(v)) return false;
    if (!sheet) sheet = new CSSStyleSheet();
    sheet.replaceSync(`video[${MARK}]{position:fixed!important;left:${x}px!important;top:${y}px!important;right:auto!important;bottom:auto!important;width:${w}px!important;height:${h}px!important;max-width:none!important;max-height:none!important;min-width:0!important;min-height:0!important;margin:0!important;padding:0!important;border:0!important;border-radius:14px!important;object-fit:contain!important;background:#000!important;transform:none!important;opacity:1!important;visibility:visible!important;clip-path:none!important;filter:none!important;z-index:2147483647!important;overflow:hidden!important}video[${MARK}]::backdrop{background:#000!important}`);
    quiet(() => { if (!D.adoptedStyleSheets.includes(sheet)) D.adoptedStyleSheets = [...D.adoptedStyleSheets, sheet]; });
    v.setAttribute(MARK, '');
    if (!v.hasAttribute('popover')) { ours.add(v); v.setAttribute('popover', 'manual'); }
    if (!v.matches(':popover-open')) quiet(() => v.showPopover());
    shownEl = new WeakRef(v);
    return v.matches(':popover-open');
  }
  function unmini() {
    const v = shownEl ? shownEl.deref() : null;
    shownEl = null;
    if (v) {
      quiet(() => v.hidePopover());
      v.removeAttribute(MARK);
      if (ours.has(v)) { ours.delete(v); v.removeAttribute('popover'); }
    }
    quiet(() => { D.adoptedStyleSheets = D.adoptedStyleSheets.filter((s) => s !== sheet); });
    return true;
  }
  function info() {
    const ms = quiet(() => navigator.mediaSession);
    const md = ms && ms.metadata;
    const el = pick();
    let art = '';
    quiet(() => { for (const a of md.artwork) { const s = String(a.src); if (s.startsWith('https://')) art = s; } });
    return {
      title: md ? String(md.title || '') : '',
      artist: md ? String(md.artist || '') : '',
      art,
      playing: el ? !el.paused : !!ms && ms.playbackState === 'playing',
      toggle: !!el || handlers.has('play') || handlers.has('pause'),
      next: handlers.has('nexttrack'),
      prev: handlers.has('previoustrack'),
      video: big(el),
      muted: el ? !!el.muted : false,
      pip: !!D.pictureInPictureElement,
    };
  }
  function act(a, args) {
    if (a === 'next') return press('nexttrack');
    if (a === 'prev') return press('previoustrack');
    if (a === 'toggle') {
      const on = media().filter(playing);
      if (on.length) {
        if (!press('pause')) for (const m of on) quiet(() => m.pause());
        return true;
      }
      if (press('play')) return true;
      const m = lastEl() || media()[0];
      if (m) quiet(() => m.play().catch(() => {}));
      return !!m;
    }
    if (a === 'size') return size();
    // Видео в обычное окошко «картинка в картинке» (его можно таскать и закрыть).
    if (a === 'pip') {
      unmini();
      const v = pick();
      if (!big(v)) return false;
      quiet(() => v.requestPictureInPicture().catch(() => {}));
      return true;
    }
    if (a === 'unpip') {
      if (D.pictureInPictureElement) quiet(() => D.exitPictureInPicture().catch(() => {}));
      return true;
    }
    if (a === 'mini') return mini(...args);
    if (a === 'unmini') return unmini();
    return false;
  }
  Object.defineProperty(W, '__axiomMedia', { value: (a, ...args) => quiet(() => (a ? act(String(a), args) : info())) ?? null });
})();"#;

/// Скрипт для вкладок: на всех платформах (на Linux тоже: что играет, кнопки и видео
/// внутри островка, см. `tabs::mini`).
pub const PAGE_SCRIPT: &str = SCRIPT;

/// Спросить страницу, что играет (ответ — [`Info::parse`]).
pub const QUERY: &str =
    "typeof window.__axiomMedia === 'function' ? window.__axiomMedia('') : null";

/// Кнопка островка.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    /// Пауза или «играть».
    Toggle,
    Next,
    Prev,
    /// Навели мышь на чёлку: видео — в островок под ней (`tabs::mini`). Браузер сначала
    /// спрашивает размер видео (`size`: `{"w":…,"h":…}` или `null`), потом ставит его
    /// ([`mini_script`]).
    Peek,
    /// Убрать видео из островка: страница снова как была, видео играет дальше.
    Unpeek,
    /// Видео — в обычное окошко «картинка в картинке» (только по клику: `run_as_click`).
    Pip,
    /// Закрыть окошко «картинка в картинке», видео играет дальше во вкладке.
    Unpip,
}

impl Action {
    fn name(self) -> &'static str {
        match self {
            Self::Toggle => "toggle",
            Self::Next => "next",
            Self::Prev => "prev",
            Self::Peek => "size",
            Self::Unpeek => "unmini",
            Self::Pip => "pip",
            Self::Unpip => "unpip",
        }
    }
}

/// Скрипт кнопки.
pub fn action(a: Action) -> String {
    format!(
        "typeof window.__axiomMedia === 'function' && window.__axiomMedia('{}')",
        a.name()
    )
}

/// Вывести видео поверх страницы в прямоугольник `x, y, w, h` (логические px окна от
/// верха вкладки; `scale` — масштаб экрана, свой масштаб страницы она учтёт сама).
/// Ответ — `true`, если видео в верхнем слое. Повторный вызов только двигает его.
pub fn mini_script(x: f64, y: f64, w: f64, h: f64, scale: f64) -> String {
    format!(
        "typeof window.__axiomMedia === 'function' && window.__axiomMedia('mini', {}, {}, {}, {}, {})",
        x.round(),
        y.round(),
        w.round(),
        h.round(),
        if scale.is_finite() && scale > 0.0 {
            scale
        } else {
            1.0
        }
    )
}

const MAX_TEXT: usize = 300;
const MAX_URL: usize = 2048;

/// Ответ страницы. Всё, что пришло от неё, укорочено и без управляющих символов;
/// интерфейс всё равно показывает это только как текст.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Info {
    pub title: String,
    pub artist: String,
    /// Обложка — только `https://`.
    pub art: String,
    pub playing: bool,
    /// Есть ли что ставить на паузу.
    pub toggle: bool,
    pub next: bool,
    pub prev: bool,
    /// Играет видео, которое можно показать в островке.
    pub video: bool,
    /// У самого видео выключен звук (немой автозапуск, превью) — это не «смотрю видео».
    pub muted: bool,
    /// Видео сейчас в окошке «картинка в картинке».
    pub pip: bool,
}

impl Info {
    /// `None` — страница не ответила (нет скрипта, ушла, ошибка).
    pub fn parse(raw: &str) -> Option<Self> {
        let mut info: Self = serde_json::from_str(raw).ok()?;
        info.title = clean(&info.title, MAX_TEXT);
        info.artist = clean(&info.artist, MAX_TEXT);
        if !info.art.starts_with("https://")
            || info.art.len() > MAX_URL
            || info.art.chars().any(char::is_control)
        {
            info.art.clear();
        }
        Some(info)
    }
}

fn clean(s: &str, max: usize) -> String {
    let s: String = s
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(max)
        .collect();
    s.trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_an_answer() {
        let raw = r#"{"title":"Песня","artist":"Группа","art":"https://i.ytimg.com/a.jpg","playing":true,"toggle":true,"next":true,"prev":false,"video":true}"#;
        let info = Info::parse(raw);
        assert!(info.is_some());
        let info = info.unwrap_or_default();
        assert_eq!(info.title, "Песня");
        assert_eq!(info.artist, "Группа");
        assert_eq!(info.art, "https://i.ytimg.com/a.jpg");
        assert!(info.playing && info.toggle && info.next && !info.prev && info.video);
    }

    #[test]
    fn no_answer_is_none() {
        assert_eq!(Info::parse(""), None);
        assert_eq!(Info::parse("null"), None);
        assert_eq!(Info::parse("true"), None);
    }

    #[test]
    fn cleans_page_text() {
        let long = "я".repeat(1000);
        let raw =
            format!(r#"{{"title":"{long}","artist":"a\nb\u0007","art":"javascript:alert(1)"}}"#);
        let info = Info::parse(&raw);
        assert!(info.is_some());
        let info = info.unwrap_or_default();
        assert_eq!(info.title.chars().count(), MAX_TEXT);
        assert_eq!(info.artist, "a b");
        assert!(info.art.is_empty());
        let http = Info::parse(r#"{"art":"http://x/a.jpg"}"#);
        assert_eq!(http.map(|i| i.art.is_empty()), Some(true));
    }

    #[test]
    fn actions() {
        assert_eq!(
            action(Action::Next),
            "typeof window.__axiomMedia === 'function' && window.__axiomMedia('next')"
        );
        assert_eq!(
            action(Action::Peek),
            "typeof window.__axiomMedia === 'function' && window.__axiomMedia('size')"
        );
        assert_eq!(
            serde_json::from_str::<Action>(r#""unpeek""#).ok(),
            Some(Action::Unpeek)
        );
        assert_eq!(
            mini_script(8.4, 30.0, 384.0, 216.6, 1.25),
            "typeof window.__axiomMedia === 'function' && window.__axiomMedia('mini', 8, 30, 384, 217, 1.25)"
        );
        assert!(serde_json::from_str::<Action>(r#""eval""#).is_err());
        assert_eq!(
            serde_json::from_str::<Action>(r#""pip""#).ok(),
            Some(Action::Pip)
        );
    }
}
