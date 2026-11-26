'use strict';
// Новая вкладка: часы, поиск, плитки частых сайтов, а в пустом профиле — перенос
// закладок и пара сочетаний клавиш. Живёт в webview интерфейса, IPC здесь есть.
// Подсказки поиска общие с адресной строкой (app.js): тот же список и те же клавиши.
function createNewtab(ax) {
  const $ = (s) => document.querySelector(s);
  const field = $('#ntSearch');
  const input = $('#ntInput');
  const tilesEl = $('#tiles');
  const emptyEl = $('#ntEmpty');
  const undoEl = $('#ntUndo');
  const MAX_TILES = 8;
  let shown = false;

  // ---------- часы ----------
  function tick() {
    const now = new Date();
    $('#clock').textContent = now.toLocaleTimeString('ru-RU', { hour: '2-digit', minute: '2-digit' });
    $('#date').textContent = now.toLocaleDateString('ru-RU', { weekday: 'long', day: 'numeric', month: 'long' });
  }
  // Будим себя раз в минуту, ровно к её началу.
  (function loop() {
    if (shown) tick();
    setTimeout(loop, 60050 - (Date.now() % 60000));
  })();

  // ---------- поиск ----------
  input.addEventListener('input', () => ax.sug.input(input));
  input.addEventListener('keydown', (e) => ax.sug.key(e));
  input.addEventListener('blur', () => ax.sug.hide());
  field.addEventListener('mousedown', (e) => { if (e.target !== input) { e.preventDefault(); input.focus(); } });

  // Начали печатать, а фокус не в поле (кликнули мимо, ушли Tab) — буквы идут в поиск.
  document.addEventListener('keydown', (e) => {
    if (!shown || e.defaultPrevented || e.isComposing || ax.blocked()) return;
    if (e.key.length !== 1 || e.ctrlKey || e.metaKey || e.altKey) return;
    const t = e.target;
    if (t.closest?.('input,textarea,select,[contenteditable]')) return;
    if (e.key === ' ' && t.closest?.('button')) return;
    input.focus({ preventScroll: true });
  });

  // ---------- плитки ----------
  // Есть ли у сайта значок: запоминаем, чтобы при следующем открытии
  // не мелькала буква, пока значок грузится из кэша.
  const ICONS_KEY = 'axiom.newtab.icons';
  let known = {};
  try { known = JSON.parse(localStorage.getItem(ICONS_KEY) || '{}') || {}; } catch { known = {}; }
  function remember(origin, ok) {
    if (known[origin] === ok) return;
    delete known[origin];
    known[origin] = ok;
    const keys = Object.keys(known);
    if (keys.length > 64) delete known[keys[0]];
    try { localStorage.setItem(ICONS_KEY, JSON.stringify(known)); } catch { /* без памяти тоже работает */ }
  }

  function color(host) {
    let h = 0;
    for (const ch of host) h = (h * 31 + ch.charCodeAt(0)) >>> 0;
    return ax.colors[h % ax.colors.length];
  }

  function makeTile(site) {
    const u = new URL(site.url);
    const host = u.hostname.replace(/^www\./, '');
    const el = document.createElement('div');
    el.className = 'tile';
    el.dataset.url = site.url;
    el.innerHTML = `<button class="tile-go"><span class="plate" style="--f:${color(host)}"><b>${ax.esc((host[0] || '?').toUpperCase())}</b></span><span class="tl"></span></button>`
      + `<button class="tile-x">${icon('x')}</button>`;
    const plate = el.querySelector('.plate');
    if (/^https?:$/.test(u.protocol)) {
      const img = new Image();
      img.alt = '';
      img.draggable = false;
      img.decoding = 'async';
      const ok = () => {
        plate.classList.toggle('lo', img.naturalWidth > 0 && img.naturalWidth <= 16);
        plate.classList.add('ok');
        remember(u.origin, 1);
      };
      img.onload = ok;
      img.onerror = () => { img.remove(); plate.classList.remove('ok', 'now'); remember(u.origin, 0); };
      img.src = `${u.origin}/favicon.ico`;
      // Значок уже в кэше или был в прошлый раз — сразу без буквы и без проявления.
      if (img.complete && img.naturalWidth) { plate.classList.add('now'); ok(); }
      else if (known[u.origin] === 1) plate.classList.add('ok', 'now');
      plate.append(img);
    }
    return el;
  }

  function setTitle(el, site) {
    const host = site.title || site.url;
    el.querySelector('.tl').textContent = host;
    el.querySelector('.tile-go').title = site.url;
    const x = el.querySelector('.tile-x');
    x.title = 'Убрать с новой вкладки';
    x.setAttribute('aria-label', `Убрать ${host} с новой вкладки`);
  }

  const tileEls = new Map();
  let tilesKey = null;
  // Плитки обновляются на каждое сообщение о вкладках: пересобираем их, только
  // когда поменялся сам список, и переиспользуем прежние элементы — иначе значки
  // загружались бы заново и на миг становились бледными.
  function renderTiles(sites, animate) {
    const list = sites.filter((s) => /^https?:\/\//.test(s.url)).slice(0, MAX_TILES);
    const key = list.map((s) => `${s.url} ${s.title}`).join('\n');
    if (key === tilesKey) return;
    const live = animate && tilesKey !== null && ax.speed() > 0;
    tilesKey = key;
    const before = live ? new Map([...tileEls.values()].map((el) => [el, el.getBoundingClientRect()])) : null;
    const keep = new Set(list.map((s) => s.url));
    for (const [url, el] of tileEls) if (!keep.has(url)) { tileEls.delete(url); el.remove(); }
    let prev = null;
    for (const s of list) {
      let el = tileEls.get(s.url);
      if (!el) {
        el = makeTile(s);
        tileEls.set(s.url, el);
        if (live) {
          el.classList.add('in');
          el.addEventListener('animationend', () => el.classList.remove('in'), { once: true });
        }
      }
      el.classList.remove('out');
      setTitle(el, s);
      const want = prev ? prev.nextElementSibling : tilesEl.firstElementChild;
      if (el !== want) tilesEl.insertBefore(el, want);
      prev = el;
    }
    // Соседи убранной плитки плавно занимают её место.
    if (before) {
      for (const [el, a] of before) {
        if (!el.isConnected) continue;
        const b = el.getBoundingClientRect();
        const dx = a.left - b.left;
        const dy = a.top - b.top;
        if (dx || dy) el.animate([{ transform: `translate(${dx}px,${dy}px)` }, { transform: 'none' }], { duration: ax.ms(320), easing: 'cubic-bezier(.22,1,.36,1)' });
      }
    }
  }

  tilesEl.addEventListener('click', (e) => {
    const tile = e.target.closest('.tile');
    if (!tile || tile.classList.contains('out')) return;
    if (e.target.closest('.tile-x')) hideTile(tile);
    else if (e.target.closest('.tile-go')) ax.send({ cmd: 'navigate', input: tile.dataset.url });
  });
  // Средняя кнопка — в новой вкладке, как у ссылок.
  tilesEl.addEventListener('mousedown', (e) => { if (e.button === 1) e.preventDefault(); });
  tilesEl.addEventListener('auxclick', (e) => {
    const tile = e.target.closest('.tile');
    if (e.button !== 1 || !tile || e.target.closest('.tile-x')) return;
    e.preventDefault();
    ax.send({ cmd: 'new_tab', input: tile.dataset.url });
  });

  // ---------- убрать плитку и «Вернуть» ----------
  let undoTimer;
  let undoUrl = '';
  function hideTile(tile) {
    const url = tile.dataset.url;
    const name = tile.querySelector('.tl').textContent;
    tile.classList.add('out');
    // Плитка тает, потом Rust присылает новый список: на её место встаёт следующий сайт.
    setTimeout(() => ax.send({ cmd: 'hide_site', url, hidden: true }), ax.ms(180));
    // Если ответ не пришёл (сайт уже был убран), плитка не должна остаться невидимой.
    setTimeout(() => { if (tile.isConnected) tile.classList.remove('out'); }, ax.ms(180) + 1500);
    undoUrl = url;
    undoEl.innerHTML = `<span>${ax.esc(name)} убран с новой вкладки</span><button data-undo>Вернуть</button>`;
    undoEl.classList.add('on');
    undoLater(9000);
  }
  function undoLater(after) {
    clearTimeout(undoTimer);
    undoTimer = setTimeout(hideUndo, after);
  }
  function hideUndo() {
    clearTimeout(undoTimer);
    undoEl.classList.remove('on');
    undoUrl = '';
  }
  // Пока мышь на плашке, она не исчезает.
  undoEl.addEventListener('mouseenter', () => clearTimeout(undoTimer));
  undoEl.addEventListener('mouseleave', () => { if (undoUrl) undoLater(3000); });
  undoEl.addEventListener('click', (e) => {
    if (!e.target.closest('[data-undo]') || !undoUrl) return;
    ax.send({ cmd: 'hide_site', url: undoUrl, hidden: false });
    hideUndo();
  });

  // ---------- история ----------
  // Ссылка в углу: тихая, пока на неё не навели, как подписи под плитками.
  const histBtn = document.createElement('button');
  histBtn.className = 'nt-hist';
  histBtn.title = 'История (Ctrl+H)';
  histBtn.innerHTML = `${icon('clock')}<span>История</span>`;
  histBtn.addEventListener('click', () => ax.openHistory());
  $('#newtab').append(histBtn);

  // ---------- пустой профиль ----------
  let importing = '';
  let importNote = '';
  let importedOk = false;
  let emptyKey = '';
  function renderEmpty(on) {
    emptyEl.hidden = !on;
    if (!on) return;
    const sys = ax.system();
    const browsers = Array.isArray(sys.browsers) ? sys.browsers : [];
    const b = importedOk ? null : browsers[0];
    const mod = sys.platform === 'macos' ? '⌘' : 'Ctrl';
    const key = JSON.stringify([b?.id, browsers.length, importing, importNote, mod]);
    if (key === emptyKey) return;
    emptyKey = key;
    const imp = b
      ? `<button class="nt-imp${importing ? ' busy' : ''}" data-browser="${ax.esc(b.id)}">${icon('download')}<span>${importing ? 'Переносим…' : `Перенести закладки из ${ax.esc(b.from || b.name)}`}</span></button>`
        + (browsers.length > 1 && !importing ? '<button class="nt-alt" data-more>Из другого браузера</button>' : '')
      : '';
    emptyEl.innerHTML = `${imp}${importNote ? `<div class="nt-note">${ax.esc(importNote)}</div>` : ''}`
      + `<div class="nt-keys"><span><kbd>${mod}</kbd><kbd>K</kbd>спросить ИИ</span><span><kbd>${mod}</kbd><kbd>L</kbd>адресная строка</span></div>`;
  }
  emptyEl.addEventListener('click', (e) => {
    const imp = e.target.closest('.nt-imp');
    if (imp && !importing) {
      importing = imp.dataset.browser;
      importNote = '';
      ax.send({ cmd: 'import', browser: importing });
      renderEmpty(true);
    } else if (e.target.closest('[data-more]')) {
      ax.openSettings('data');
    }
  });

  // ---------- снаружи ----------
  // `on` — активна новая вкладка; `sites` — плитки из истории.
  function render(on, sites) {
    if (!on) {
      if (shown) hideUndo();
      shown = false;
      return;
    }
    const was = shown;
    if (!was) {
      // Каждая новая вкладка начинается с чистого поля.
      input.value = '';
      tick();
    }
    shown = true;
    // Плиток от Rust ещё не было (только запустились): не показываем и «пустой профиль».
    if (!Array.isArray(sites)) return;
    // Только что открытая вкладка проявляется целиком, плитки при этом не двигаются.
    renderTiles(sites, was);
    renderEmpty(tileEls.size === 0);
  }

  // Перенос мог начаться и здесь, и в настройках. Удачный убирает кнопку,
  // а итог пишем, только если начинали отсюда.
  function onImported(m) {
    if (m.ok) importedOk = true;
    // Rust переносит по одному браузеру: любой ответ значит, что можно снова нажимать.
    const mine = importing === m.browser;
    importing = '';
    if (mine) {
      // Если в истории что-то было, вместо этого блока появятся плитки.
      if (!m.ok) importNote = `Не получилось: ${String(m.error || 'неизвестная ошибка').slice(0, 140)}`;
      else if (m.bookmarks) importNote = `Перенесено закладок: ${m.bookmarks}. Они в подсказках адресной строки`;
      else importNote = m.history ? '' : 'Переносить нечего: в браузере нет закладок и истории';
    }
    emptyKey = '';
    if (shown) renderEmpty(tileEls.size === 0);
  }

  function onSystem() {
    if (shown) renderEmpty(tileEls.size === 0);
  }

  return {
    render,
    onImported,
    onSystem,
    input,
    anchor: () => field,
    focus() { input.focus({ preventScroll: true }); },
  };
}
