'use strict';
// Островок — чёрная чёлка посередине верхнего ряда, как вырез на макбуке.
// В тишине его нет совсем. Он выезжает сверху, когда на другой вкладке видео (играет
// или стоит на паузе) или идёт загрузка, и уезжает, когда всё кончилось. Пока он виден,
// вкладки расступаются: в ряд вкладок вставляется пустой промежуток (.isl-gap). Порядок
// вкладок и их разметку островок не трогает, перетаскивание о нём не знает.
// Наведение на чёлку показывает само видео: оно вылетает из неё вниз, а когда мышь
// уходит с чёлки и с видео, уезжает обратно (src/tabs/mini.rs). Кнопка «видео в окне»
// выносит его в обычное окошко «картинка в картинке». Звук без видео (музыка) островок
// не показывает. Его можно свернуть в тонкую полоску и развернуть кликом по ней.
// Где он живёт, зависит от раскладки: в одну и две строки — чёлкой посередине ряда
// вкладок; при вкладках сбоку — на выбор: чёлкой над страницей (страница на это время
// опускается на полосу) или карточкой во всю ширину боковой панели над вкладками.
// Linux: снимка страницы под окнами интерфейса нет — список, зашедший на страницу,
// спрятал бы её (серый экран). Поэтому там чёлка в ряду вкладок и над страницей
// раскрывается не вниз, а вширь: название и кнопки прямо в ней (`wide`) — когда видео
// из чёлки нет (музыка, загрузка, видео в островке выключено); карточка в боковой панели
// раскрывается как обычно — страницу она не задевает. Видео из чёлки на Linux есть: это
// сама вкладка поверх страницы (src/tabs/mini.rs), «Видео в окне» — нет.
function createIsland(ax) {
  const $ = (s, r = document) => r.querySelector(s);
  const $$ = (s, r = document) => Array.from(r.querySelectorAll(s));

  const W = 240; // ширина чёлки
  const H = 30; // высота чёлки
  const VW = 400; // ширина острова с видео под чёлкой (src/tabs/mini.rs)
  const EARS = 10; // скругления у верхнего края — по 10 px с каждой стороны
  const zoneOf = (w) => w + 2 * (EARS + 8); // сколько места в ряду вкладок держим под чёлку
  const OPEN_W = 480;
  const ROW = 50; // высота строки в раскрытом островке
  const DONE_MS = 2600; // сколько после загрузки висит галочка

  const el = document.createElement('div');
  el.className = 'isl';
  el.setAttribute('role', 'status');
  el.innerHTML = '<div class="isl-c"><span class="isl-l"></span><span class="isl-t"></span><span class="isl-r"></span></div><div class="isl-x"></div><span class="isl-pb"><i></i></span>';
  $('#app').append(el);
  const gap = document.createElement('div');
  gap.className = 'isl-gap';
  gap.setAttribute('aria-hidden', 'true');
  // Место под карточку в боковой панели (вкладки сбоку): над вкладками.
  const slot = document.createElement('div');
  slot.className = 'isl-slot';
  slot.setAttribute('aria-hidden', 'true');
  const CARD = 34; // высота карточки в боковой панели

  let enabled = true;
  let showVideo = true; // видео из чёлки при наведении
  let showPip = true; // кнопка «видео в окне»
  let collapsed = false; // свёрнут в полоску
  let sidePlace = 'panel'; // вкладки сбоку: 'panel' — в боковой панели, 'top' — над страницей
  let barDown = false; // панель у нижнего края окна: чёлка над страницей
  // Раскрытый островок может зайти на страницу: под ним снимок страницы (Windows).
  // На Linux снимков нет — см. `inline`.
  let overPage = true;
  let wide = false; // Linux: чёлка раскрыта вширь, кнопки прямо в ней
  let peekH = 0; // высота острова с видео (от Rust) — под неё место в боковой панели
  let downloads = [];
  let lastStatus = null;
  let done = null; // { id, name } — только что докачался
  let doneTimer = 0;
  const since = new Map(); // вкладка → когда зазвучала: показываем последнюю
  const media = new Map(); // вкладка → что играет (ответ страницы)
  // Вкладка с видео, которую островок держит без звука: видео на паузе (поставили из
  // островка или ушли с вкладки, где оно на паузе). Отпускает, когда вкладку открыли,
  // закрыли или на другой вкладке заиграло видео.
  let hold = null;
  let lastActive;
  let peeking = null; // вкладка, чьё видео сейчас показано из чёлки
  let leaving = null; // с этой вкладки только что ушли — есть ли там видео?
  let shown = false;
  let opened = false;
  let pinned = false; // раскрыли кликом: держим, пока не кликнут мимо
  let overlay = false;

  // ---------- что показывать ----------
  const running = (d) => d.status === 'active' && !d.ask;
  // Видео, которое смотрят (не немое превью и не просто звук).
  const isVideo = (id) => { const i = media.get(id); return !!i && i.video && !i.muted; };
  function videoTab() {
    const tabs = ax.tabs();
    const now = performance.now();
    for (const t of tabs) {
      if (t.audible && !t.sleeping) {
        // Зазвучала — спросим страницу, видео ли это (ответ — в onMedia).
        if (!since.has(t.id)) { since.set(t.id, now); ask(t.id); }
      } else since.delete(t.id);
    }
    for (const id of since.keys()) if (!tabs.some((t) => t.id === id)) since.delete(id);
    let best = null;
    for (const t of tabs) {
      if (t.id === ax.active() || !since.has(t.id) || !isVideo(t.id)) continue;
      if (!best || since.get(t.id) > since.get(best.id)) best = t;
    }
    // Заиграло другое видео — вкладку на паузе больше не держим.
    if (best && hold != null && best.id !== hold) hold = null;
    // Видео сейчас в островке — он есть, даже если оно на паузе.
    const kept = (id) => tabs.find((t) => t.id === id && t.id !== ax.active() && !t.sleeping) || null;
    if (!best && peeking != null) best = kept(peeking);
    if (!best && hold != null) {
      best = kept(hold);
      if (!best) hold = null;
    }
    return best;
  }
  function model() {
    const tab = videoTab();
    return {
      tab,
      run: downloads.filter(running),
      done,
      // Видео показано из чёлки — чёлка шириной с него, кнопки прямо в ней.
      video: !!tab && peeking === tab.id,
      // Linux: чёлка раскрыта вширь — те же кнопки, но без видео под ней.
      wide: wide && !collapsed && inline(),
      // Видео в окошке «картинка в картинке».
      pip: !!tab && !!media.get(tab.id)?.pip,
    };
  }
  const busy = (m) => !!(m.tab || m.run.length || m.done);

  // ---------- форматирование ----------
  const UNITS = ['Б', 'КБ', 'МБ', 'ГБ', 'ТБ'];
  function size(n) {
    if (!Number.isFinite(n) || n < 0) return '';
    let i = 0;
    while (n >= 1024 && i < UNITS.length - 1) { n /= 1024; i++; }
    return `${n.toLocaleString('ru-RU', { maximumFractionDigits: i && n < 100 ? 1 : 0 })} ${UNITS[i]}`;
  }
  const host = (url) => { try { return new URL(url).hostname.replace(/^www\./, ''); } catch { return ''; } };
  function ext(name) {
    const dot = name.lastIndexOf('.');
    return dot > 0 && dot < name.length - 1 ? name.slice(dot + 1, dot + 5).toUpperCase() : '';
  }
  function pct(list) {
    let got = 0;
    let total = 0;
    for (const d of list) {
      if (!d.total) return null; // размер неизвестен хотя бы у одной — кольцо без процентов
      got += d.received || 0;
      total += d.total;
    }
    return total ? Math.min(100, (got / total) * 100) : null;
  }
  const dlSub = (d) => (d.received ? (d.total ? `${size(d.received)} из ${size(d.total)}` : size(d.received)) : 'Начинается…');
  const ring = (p) => `<svg class="isl-ring${p == null ? ' ind' : ''}" viewBox="0 0 20 20"><circle class="rt" cx="10" cy="10" r="8"/><circle class="rp" cx="10" cy="10" r="8" pathLength="100" style="stroke-dashoffset:${p == null ? 72 : (100 - p).toFixed(1)}"/></svg>`;
  const eq = '<span class="isl-eq"><i></i><i></i><i></i><i></i></span>';
  const badge = (name) => { const e = ext(name); return `<span class="isl-ext">${e ? ax.esc(e) : icon('file')}</span>`; };
  const btn = (a, ic, title, cls = '') => `<button class="ib sm${cls}" data-a="${a}" title="${title}">${icon(ic)}</button>`;
  const fold = () => btn('fold', 'fold', 'Свернуть островок', ' isl-fold');
  // Название — со страницы (что за видео), иначе заголовок вкладки.
  const titleOf = (t) => media.get(t.id)?.title || t.title || host(t.url);
  const playingOf = (t) => { const i = media.get(t.id); return i ? i.playing : !!t.audible; };

  // ---------- чёлка ----------
  function renderCompact(m) {
    const l = el.querySelector('.isl-l');
    const t = el.querySelector('.isl-t');
    const r = el.querySelector('.isl-r');
    let left;
    let title;
    if (m.tab) {
      left = `tab:${m.tab.url}`;
      title = titleOf(m.tab);
    } else if (m.run.length) {
      left = `dl:${m.run[0].name}`;
      title = m.run.length > 1 ? `Загрузки: ${m.run.length}` : m.run[0].name;
    } else if (m.done) {
      left = `dl:${m.done.name}`;
      title = m.done.name;
    }
    if (left && l.dataset.k !== left) {
      l.dataset.k = left;
      l.innerHTML = m.tab ? ax.favHtml(m.tab.url) : badge(left.slice(3));
    }
    if (title != null && t.textContent !== title) t.textContent = title;
    // Кнопки в самой чёлке — как в строке вкладки раскрытого островка.
    const c = el.querySelector('.isl-c');
    if (m.tab) c.dataset.tab = m.tab.id; else delete c.dataset.tab;
    // Раскрытая вширь чёлка загрузки — её кнопки относятся к этой загрузке.
    const d = !m.tab && m.wide ? dlOf(m) : null;
    if (d) c.dataset.dl = d.id; else delete c.dataset.dl;
    const i = m.tab ? media.get(m.tab.id) || {} : {};
    const state = m.run.length ? 'ring' : m.done ? 'ok' : !m.tab ? '' : m.pip ? 'pip' : playingOf(m.tab) ? 'eq' : 'paused';
    const buttons = m.video || (m.wide && m.tab);
    const right = buttons ? `vid:${playingOf(m.tab)}:${!!i.prev}:${!!i.next}:${showPip}:${m.run.length ? 'ring' : ''}`
      : d ? `dl:${d.id}:${m.run.length}` : state;
    if (r.dataset.k !== right) {
      r.dataset.k = right;
      r.innerHTML = buttons ? videoButtons(m)
        : d ? dlButtons(m)
        : (state === 'ring' ? ring(pct(m.run))
          : state === 'ok' ? `<span class="isl-ok">${icon('check')}</span>`
          : state === 'pip' ? `<span class="isl-st" title="Видео в окне">${icon('pip')}</span>`
          : state === 'paused' ? btn('toggle', 'play', 'Играть')
          : state === 'eq' ? eq : '') + fold();
    } else if (m.run.length) {
      setRing(r, pct(m.run));
    }
    // Свёрнутый: тонкая полоска, загрузка — полоской прогресса внутри.
    const p = m.run.length ? pct(m.run) : null;
    el.classList.toggle('dl', !!m.run.length);
    el.querySelector('.isl-pb i').style.width = `${m.run.length ? (p ?? 35) : 100}%`;
  }
  function videoButtons(m) {
    const i = media.get(m.tab.id) || {};
    const playing = playingOf(m.tab);
    return [
      m.run.length ? ring(pct(m.run)) : '',
      i.prev ? btn('prev', 'prev', 'Назад') : '',
      btn('toggle', playing ? 'pause' : 'play', playing ? 'Пауза' : 'Играть'),
      i.next ? btn('next', 'next', 'Дальше') : '',
      showPip ? btn('pip', 'pip', 'Видео в окне — его можно таскать и закрыть') : '',
      btn('go', 'arrowR', 'Перейти на вкладку'),
      fold(),
    ].join('');
  }
  // Загрузка в раскрытой вширь чёлке (Linux): первая идущая или только что скачанная.
  const dlOf = (m) => m.run[0] || m.done || null;
  function dlButtons(m) {
    const going = m.run.length > 0;
    return [
      going ? ring(pct(m.run)) : `<span class="isl-ok">${icon('check')}</span>`,
      going ? '' : btn('open', 'file', 'Открыть'),
      btn('show', 'folder', 'Показать в папке'),
      m.run.length === 1 ? btn('cancel', 'x', 'Отменить') : '',
      fold(),
    ].join('');
  }
  function setRing(scope, p) {
    const svg = scope.querySelector('.isl-ring');
    if (!svg) return;
    svg.classList.toggle('ind', p == null);
    svg.querySelector('.rp').style.strokeDashoffset = p == null ? 72 : (100 - p).toFixed(1);
  }

  // ---------- раскрытый островок ----------
  // Раскрывается, когда показать само видео нельзя (оно в окне, видео в островке
  // выключено) и для загрузок.
  function videoRow(t) {
    const i = media.get(t.id) || {};
    const playing = playingOf(t);
    const sub = host(t.url) + (i.pip ? ' · в окне' : playing ? '' : ' · пауза');
    const player = [
      i.prev ? btn('prev', 'prev', 'Назад') : '',
      i.toggle ? btn('toggle', playing ? 'pause' : 'play', playing ? 'Пауза' : 'Играть', ' big') : '',
      i.next ? btn('next', 'next', 'Дальше') : '',
    ].join('');
    const pip = i.pip ? btn('unpip', 'up', 'Вернуть видео из окна')
      : showPip && i.video ? btn('pip', 'pip', 'Видео в окне — его можно таскать и закрыть') : '';
    return `<div class="isl-row" data-tab="${t.id}" title="Перейти на вкладку">
      <span class="isl-cv">${ax.favHtml(t.url)}</span>
      <span class="isl-m"><b>${ax.esc(titleOf(t))}</b><span>${ax.esc(sub)}</span></span>
      <span class="isl-a">${player ? `<span class="isl-p">${player}</span>` : ''}${pip}
        ${btn('go', 'arrowR', 'Перейти на вкладку')}</span>
    </div>`;
  }
  function rowsHtml(m) {
    const rows = [];
    if (m.tab) rows.push(videoRow(m.tab));
    for (const d of m.run.slice(0, 3)) {
      rows.push(`<div class="isl-row" data-dl="${d.id}">
        <span class="isl-cv">${badge(d.name)}</span>
        <span class="isl-m"><b>${ax.esc(d.name)}</b><span class="isl-sub">${ax.esc(dlSub(d))}</span></span>
        <span class="isl-a">${ring(pct([d]))}${btn('show', 'folder', 'Показать в папке')}${btn('cancel', 'x', 'Отменить')}</span>
      </div>`);
    }
    if (m.done && !m.run.some((d) => d.id === m.done.id)) {
      rows.push(`<div class="isl-row done" data-dl="${m.done.id}" title="Открыть">
        <span class="isl-cv">${badge(m.done.name)}</span>
        <span class="isl-m"><b>${ax.esc(m.done.name)}</b><span>Скачано</span></span>
        <span class="isl-a"><span class="isl-ok">${icon('check')}</span>${btn('show', 'folder', 'Показать в папке')}</span>
      </div>`);
    }
    return rows;
  }
  let openKey = '';
  function renderOpen(m) {
    const rows = rowsHtml(m);
    const key = rows.join('');
    if (key === openKey) return;
    openKey = key;
    el.querySelector('.isl-x').innerHTML = key;
    el.style.setProperty('--ow', `${OPEN_W}px`);
    el.style.setProperty('--oh', `${rows.length * ROW + 16}px`);
  }
  function updateProgress(m) {
    for (const d of m.run) {
      const row = el.querySelector(`.isl-row[data-dl="${d.id}"]`);
      if (!row) continue;
      const sub = row.querySelector('.isl-sub');
      if (sub) sub.textContent = dlSub(d);
      setRing(row, pct([d]));
    }
  }

  // Раскрытый островок ниже панели заходит на страницу: её webview нативный и лежит
  // выше интерфейса, поэтому на это время — снимок страницы, как под меню. Без снимка
  // (Linux) страница спряталась бы целиком — там список над страницей не раскрывается.
  function syncOverlay() {
    let want = false;
    if (overPage && opened && mode() !== 'panel') {
      const c = $('#content').getBoundingClientRect();
      want = el.querySelectorAll('.isl-row').length * ROW + 16 > c.top + 1;
    }
    if (want !== overlay) { overlay = want; ax.setOverlay('isl', want); }
  }
  function setOpen(on) {
    // Под чёлкой видео или островок свёрнут — не раскрываем. Список зашёл бы на
    // страницу без снимка под ним (Linux) — вместо него чёлка раскрывается вширь.
    if (on && (!shown || collapsed || model().video || inline())) return;
    if (on === opened) return;
    opened = on;
    if (!on) pinned = false;
    if (on) {
      openKey = '';
      renderOpen(model());
      ask();
    }
    el.classList.toggle('open', on);
    syncOverlay();
    poll();
    if (mode() === 'panel') sync();
  }
  // Linux, чёлка в ряду вкладок или над страницей: раскрыться вширь, кнопки прямо в ней.
  // Островок остаётся высотой с чёлку и на страницу не заходит.
  function setWide(on) {
    if (on && (!shown || collapsed || !inline())) return;
    if (on === wide) return;
    wide = on;
    if (!on) pinned = false;
    if (on) ask();
    sync();
    poll();
  }
  // Навели мышь или кликнули по чёлке: список вниз или (Linux) вширь.
  const expand = () => (inline() ? setWide(true) : setOpen(true));
  const collapseAll = () => { setOpen(false); setWide(false); };

  // ---------- что играет: спрашиваем страницу ----------
  let pollTimer = 0;
  let asked = null;
  function ask(id = asked) {
    if (id != null) ax.send({ cmd: 'media', id });
  }
  // Пока островок показывает вкладку — раз в 3 с, раскрытый — чаще.
  function poll() {
    clearTimeout(pollTimer);
    if (!shown || asked == null) return;
    pollTimer = setTimeout(() => { ask(); poll(); }, opened || wide ? 1500 : 3000);
  }
  function press(id, action) {
    ax.send({ cmd: 'media', id, action });
    setTimeout(() => ask(id), 400);
    setTimeout(() => ask(id), 1200);
  }

  // ---------- где островок ----------
  // 'bar' — чёлка в ряду вкладок (одна и две строки), 'top' — чёлка над страницей
  // (и когда панель у нижнего края окна), 'panel' — карточка в боковой панели (вкладки сбоку).
  const mode = () => (ax.layout() !== 'side' ? (barDown ? 'top' : 'bar') : sidePlace === 'top' ? 'top' : 'panel');
  // Раскрытый вниз список зашёл бы на страницу, а снимка под ним нет (Linux): чёлка в
  // ряду вкладок и над страницей раскрывается вширь (`setWide`), карточка в боковой
  // панели — вниз, как обычно (страницу она не задевает).
  const inline = () => !overPage && mode() !== 'panel';
  function place(m) {
    if (!enabled) return null;
    const k = mode();
    if (k === 'bar') {
      // Пока выехало видео (или чёлка раскрыта вширь), чёлка шириной с него (см. zone).
      const z = zone((m.video || m.wide) && !collapsed);
      return z && { ...z, mode: k };
    }
    if (k === 'top') {
      const c = $('#content').getBoundingClientRect();
      const cx = c.left + c.width / 2;
      return { mode: k, cx, l: cx - W / 2, r: cx + W / 2 };
    }
    const side = $('#side');
    if (!side.offsetParent) return null;
    if (slot.parentNode !== side) side.insertBefore(slot, $('#tabs').parentNode === side ? $('#tabs') : null);
    const b = side.getBoundingClientRect();
    const top = slot.getBoundingClientRect().top;
    return { mode: k, cx: b.left + b.width / 2, l: b.left, r: b.right, left: b.left, width: b.width, top };
  }

  // ---------- место в ряду вкладок ----------
  // Чёлка стоит посередине окна, а если ряд вкладок туда не доходит — посередине ряда.
  // В одну строку ряд узкий (справа адресная строка): чёлка встаёт в свободное место
  // между «+» и адресной строкой — вкладки не двигаются; тесно — посередине ряда.
  // Вкладки, ужавшись до предела, не помещаются по сторонам — чёлка уже (не уже 150 px),
  // а совсем тесно — островка нет.
  // Пока выехало видео, чёлка шириной с него и вкладки расступаются шире, но середина
  // та же: сдвинься чёлка, она уехала бы из-под мыши, видео спряталось бы, и всё
  // поехало бы обратно. Шире не помещается — видео выезжает из обычной чёлки поверх
  // соседних вкладок, пока на него смотрят.
  function zone(video) {
    const tabs = $('#tabs');
    if (!enabled || ax.layout() === 'side' || !tabs || !tabs.offsetParent) return null;
    const b = tabs.getBoundingClientRect();
    const row = rowTabs(b);
    const at = (cx, iw) => ({ cx, l: cx - zoneOf(iw) / 2, r: cx + zoneOf(iw) / 2, iw });
    let z = null;
    if (ax.layout() === 'one') {
      const end = row.end + 6;
      const free = b.right - end;
      // Свободное место после «+» — чёлка в нём, сузившись (но не уже 150 px).
      const fit = Math.min(W, free - zoneOf(0));
      if (fit >= 150) z = at(end + free / 2, Math.floor(fit));
    }
    const mid = ax.layout() === 'one' ? b.left + b.width / 2 : innerWidth / 2;
    for (let iw = W; !z && iw >= 150; iw -= 10) {
      const half = zoneOf(iw) / 2;
      if (b.width < 2 * half) continue;
      const c = at(Math.min(b.right - half, Math.max(b.left + half, mid)), iw);
      if (row.fits(c)) z = c;
    }
    if (z && video) {
      const v = at(z.cx, VW);
      if (v.l >= b.left && v.r <= b.right && row.fits(v)) return v;
    }
    return z;
  }
  // Вкладки ряда сами по себе, без промежутка под чёлку: где кончался бы «+» (end) и
  // встанут ли вкладки с «+» по сторонам от чёлки (fits).
  // Промежуток в расчёт не берём: он сам зависит от места чёлки — по нему чёлка и
  // промежуток в узком окне догоняли друг друга, и вкладки скакали.
  // Ширины — как их раздаёт ряд: каждая вкладка хочет свои 190 px (flex-basis) в пределах
  // min/max-width, а когда справа тесно, все уступают поровну (s); в одну строку первой
  // ужимается открытая вкладка, остальные и так узкие. Открытая вкладка слева от чёлки
  // сама не ужмётся: ряд жмёт вкладки, только когда им не хватает места справа.
  function rowTabs(b) {
    const list = $('#tablist');
    const g = parseFloat(getComputedStyle(list).columnGap) || 0;
    const plus = $('#newTabBtn').getBoundingClientRect().width;
    const tabs = $$('.tab:not(.out)', list).map((t) => {
      const s = getComputedStyle(t);
      return { lo: parseFloat(s.minWidth) || 0, hi: parseFloat(s.maxWidth) || Infinity, basis: parseFloat(s.flexBasis) || 0 };
    });
    const n = tabs.length;
    // Вкладки from…to-1 (каждая со своим зазором g), когда все уступили по s px.
    const span = (from, to, s) => {
      let w = 0;
      for (let i = from; i < to; i++) w += Math.max(tabs[i].lo, Math.min(tabs[i].hi, tabs[i].basis - s)) + g;
      return w;
    };
    const most = Math.max(0, ...tabs.map((t) => t.basis)); // уступив столько, все — по min-width
    const end = b.left + Math.min(Math.max(0, span(0, n, 0) - g), b.width - g - plus) + g + plus;
    const fits = (z) => {
      // Без промежутка: все вкладки и «+» до чёлки.
      if (b.left + span(0, n, 0) + plus <= z.l + 2) return true;
      // Промежуток перед вкладкой k: справа от чёлки вкладки k… и «+», ужатые сколько
      // нужно (s), слева — остальные, ужатые так же. Чем меньше k, тем теснее справа.
      const right = (k, s) => z.r + span(k, n, s) + g + plus <= b.right + 1;
      for (let k = n; k >= 0; k--) {
        if (!right(k, most)) return false;
        let s = 0;
        if (!right(k, 0)) {
          let lo = 0;
          let hi = most;
          for (let i = 0; i < 14; i++) { const mid = (lo + hi) / 2; if (right(k, mid)) hi = mid; else lo = mid; }
          s = hi;
        }
        if (k === 0 || b.left + span(0, k, s) - g <= z.l + 2) return true;
      }
      return false;
    };
    return { end, fits };
  }
  let gapTimer = 0;
  // Промежуток тянется от конца вкладки слева до правого края чёлки. Где ему встать,
  // решаем перебором: от конца ряда к началу ставим промежуток перед очередной вкладкой,
  // даём вкладкам ужаться под его ширину (она зависит от того, где он начался, —
  // несколько проходов; сходится, потому что промежуток до края чёлки растёт медленнее,
  // чем вкладки сжимаются) и берём первое место, где ни одна вкладка и «+» не заходят
  // на чёлку. Так пустого места слева от чёлки меньше одной вкладки.
  function placeGap(z) {
    const list = $('#tablist');
    clearTimeout(gapTimer);
    const was = gap.parentNode === list ? gap.getBoundingClientRect().width : 0;
    const wasNext = gap.parentNode === list ? gap.nextElementSibling : undefined;
    gap.style.transition = 'none';
    gap.style.width = '0px';
    list.append(gap);
    if (!overlaps(z)) {
      // Вкладки до чёлки не доходят: она стоит в пустом месте ряда, расступаться некому.
      gap.remove();
      gap.style.transition = '';
      return;
    }
    const tabs = $$('.tab:not(.out)', list);
    const fit = (k) => {
      list.insertBefore(gap, tabs[k] || null);
      let width = 0;
      for (let i = 0; i < 12; i++) {
        gap.style.width = `${width}px`;
        const next = Math.max(0, Math.ceil(z.r - gap.getBoundingClientRect().left));
        if (Math.abs(next - width) <= 1) { width = Math.max(width, next); break; }
        width = next;
      }
      gap.style.width = `${width}px`;
      return width;
    };
    let width = 0;
    let ok = false;
    for (let k = tabs.length; k >= 0 && !ok; k--) {
      width = fit(k);
      ok = !overlaps(z);
    }
    if (!ok) {
      // Нигде не влезли (zone() считает с запасом, но вдруг): вкладки остаются на своих
      // местах под чёлкой, а не уезжают за «+» и адресную строку.
      gap.remove();
      gap.style.width = '0px';
      gap.style.transition = '';
      return;
    }
    // Плавно: от прежней ширины (на прежнем месте) или от нуля.
    gap.style.width = `${gap.nextElementSibling === wasNext ? was : 0}px`;
    void gap.offsetWidth;
    gap.style.transition = '';
    gap.style.width = `${width}px`;
  }
  // Заходит ли какая-нибудь вкладка (или «+») на место чёлки. Промежуток считается по
  // вкладкам в момент появления чёлки, а они могут ещё ехать (открылась, закрылась,
  // переключилась вкладка): тогда пересчитываем.
  let lastZone = null;
  // Не годится и место, при котором «+» уехал за край ряда или вкладки, ужавшись до
  // предела, уехали под него: вкладки не влезли.
  function overlaps(z) {
    const hit = (b) => b.right > z.l + 2 && b.left < z.r - 2;
    const plus = $('#newTabBtn').getBoundingClientRect();
    const rects = $$('.tab:not(.out)', $('#tablist')).map((t) => t.getBoundingClientRect());
    return rects.some(hit) || hit(plus) || plus.right > $('#tabs').getBoundingClientRect().right + 1
      || (rects.length > 0 && rects[rects.length - 1].right > plus.left + 1);
  }
  function fixGap() {
    if (!shown || collapsed || !lastZone || lastZone.mode !== 'bar' || $('#tablist .tab.drag')) return;
    if (overlaps(lastZone)) placeGap(lastZone);
  }
  // После анимаций вкладок и на всякий случай раз в полсекунды, пока чёлка видна.
  // Вкладка доехала до своей ширины (переключили, закрыли) — в одну строку от этого
  // зависит и место чёлки: считаем заново.
  $('#tablist').addEventListener('transitionend', (e) => {
    if (e.target !== gap) requestAnimationFrame(e.propertyName === 'max-width' && shown ? sync : fixGap);
  });
  $('#tablist').addEventListener('animationend', () => requestAnimationFrame(fixGap));
  setInterval(fixGap, 500);

  // Боковая панель: место под карточку растёт и уменьшается плавно, вкладки едут вниз.
  let slotTimer = 0;
  function setSlot(h) {
    clearTimeout(slotTimer);
    if (h > 0) {
      slot.style.height = `${h}px`;
    } else if (slot.parentNode) {
      slot.style.height = '0px';
      slotTimer = setTimeout(() => { if (!shown || mode() !== 'panel') slot.remove(); }, ax.ms(420) + 60);
    }
  }
  // Над страницей: страница опускается на полосу под чёлку (свёрнутому — на тонкую).
  // Linux: сразу, без анимации отступа — иначе webview страницы менял бы размер каждый
  // кадр (GTK size_allocate и новые буферы кадра WebKitGTK), и так при каждом появлении
  // и уходе чёлки.
  function setStrip(on) {
    const app = $('#app');
    app.classList.toggle('isl-instant', !overPage);
    app.classList.toggle('isl-strip', on && !collapsed);
    app.classList.toggle('isl-strip-mini', on && collapsed);
  }

  function closeGap() {
    if (!gap.parentNode) return;
    gap.style.width = '0px';
    clearTimeout(gapTimer);
    gapTimer = setTimeout(() => { if (!shown || collapsed) gap.remove(); }, ax.ms(420) + 60);
  }

  // ---------- переключение вкладок ----------
  function onActive() {
    const now = ax.active();
    if (now === lastActive) return;
    const prev = lastActive;
    lastActive = now;
    if (hold === now) hold = null;
    // Ушли с вкладки, где включали видео (пусть оно на паузе или без звука), — островок
    // её подхватит: из него видео можно досмотреть. Ответ страницы — в onMedia.
    const left = prev != null && ax.tabs().find((t) => t.id === prev && !t.sleeping && /^https?:/.test(t.url));
    leaving = left ? prev : null;
    if (left) ask(prev);
    // Вернулись на вкладку, чьё видео показано из чёлки, — оно снова на странице.
    if (now != null && peeking === now) ax.send({ cmd: 'media', id: now, action: 'unpeek' });
  }

  // Где чёлка — Rust ставит под неё остров с видео (src/tabs/mini.rs). Остров заходит
  // под низ чёлки на её скругление, и вместе они — одно чёрное целое.
  let dockKey = '';
  function sendDock(z) {
    // В боковой панели видео выезжает под карточкой, во всю её ширину.
    const dock = !z ? null : z.mode === 'panel'
      ? { cx: Math.round(z.cx), top: Math.round(z.top + CARD), width: Math.round(z.width), notch_top: Math.round(z.top) }
      : { cx: Math.round(z.cx), top: H - 8, width: VW, notch_top: 0 };
    const key = JSON.stringify(dock);
    if (key === dockKey) return;
    dockKey = key;
    ax.send(dock ? { cmd: 'island_dock', dock } : { cmd: 'island_dock' });
  }

  // ---------- главное ----------
  let showTimer = 0;
  function sync() {
    onActive();
    // Свернули или переехали в боковую панель — раскрытой вширь чёлки больше нет.
    if (wide && (collapsed || !inline())) { wide = false; pinned = false; }
    const m = model();
    // Место под чёлку; пока из неё выехало видео, чёлка шириной с него, и вкладки
    // расступаются шире (а потом смыкаются обратно).
    const z = place(m);
    const want = enabled && !!z && busy(m);
    el.classList.toggle('vid', want && m.video && !collapsed);
    el.classList.toggle('wide', want && m.wide && !m.video);
    el.classList.toggle('panel', !!z && z.mode === 'panel');
    el.classList.toggle('mini', collapsed);
    el.title = collapsed ? 'Развернуть островок' : '';
    // Свёрнутый островок видео не показывает.
    sendDock(want && !collapsed ? z : null);
    // Видео встало под чёлку — раскрытый островок оказался бы под ним: сворачиваем.
    // Linux: карточку из боковой панели перенесли над страницу — список тоже убираем.
    if (opened && (m.video || collapsed || inline())) setOpen(false);
    const id = m.tab ? m.tab.id : null;
    if (id !== asked) {
      asked = id;
      ask();
    }
    if (want) {
      // Свёрнутый — тонкая полоска поверх вкладок, расступаться им не нужно.
      // Не во время перетаскивания вкладки: промежуток перестроим, когда её отпустят.
      lastZone = z;
      el.style.setProperty('--iw', `${z.iw || W}px`);
      if (z.mode === 'bar') {
        if (collapsed) closeGap();
        else if (!$('#tablist .tab.drag')) placeGap(z);
        el.style.left = `${Math.round(z.cx)}px`;
        el.style.top = '';
      } else {
        closeGap();
      }
      setStrip(z.mode === 'top');
      if (z.mode === 'top') { el.style.left = `${Math.round(z.cx)}px`; el.style.top = ''; }
      if (z.mode === 'panel') {
        el.style.left = `${Math.round(z.left)}px`;
        el.style.top = `${Math.round(z.top)}px`;
        el.style.setProperty('--pw', `${Math.round(z.width)}px`);
        const rows = opened ? el.querySelectorAll('.isl-row').length : 0;
        setSlot(collapsed ? 10 : opened ? rows * ROW + 16 : m.video ? CARD + peekH : CARD);
      } else {
        setSlot(0);
      }
      renderCompact(m);
      if (!shown) {
        shown = true;
        clearTimeout(showTimer);
        // Сначала вкладки расступаются, потом выезжает чёлка.
        showTimer = setTimeout(() => el.classList.add('on'), ax.ms(160));
        poll();
      }
      if (opened) {
        renderOpen(m);
        updateProgress(m);
        syncOverlay();
      }
    } else if (shown) {
      shown = false;
      clearTimeout(showTimer);
      clearTimeout(pollTimer);
      if (wide) { wide = false; pinned = false; }
      setOpen(false);
      el.classList.remove('on');
      showTimer = setTimeout(() => { closeGap(); setSlot(0); setStrip(false); }, ax.ms(180));
    } else {
      closeGap();
      setSlot(0);
      setStrip(false);
    }
  }

  // ---------- мышь ----------
  let hoverTimer = 0;
  // Наведение на чёлку вкладки с видео показывает само видео (уберёт Rust, когда мышь
  // уйдёт и с видео). Видео в окне или видео в островке выключено — раскрывается список
  // (на Linux — чёлка вширь, см. `inline`).
  function peek() {
    const m = model();
    if (!showVideo || !m.tab || m.pip || peeking != null || !isVideo(m.tab.id)) return false;
    peeking = m.tab.id;
    ax.send({ cmd: 'media', id: peeking, action: 'peek' });
    sync();
    return true;
  }
  // Наведение — только настоящее движение мыши над чёлкой: когда чёлка сама выезжает под
  // стоящий курсор, браузер тоже шлёт «мышь вошла», а раскрываться тогда не надо.
  let hovering = false;
  el.addEventListener('mousemove', (e) => {
    if (collapsed || hovering || (!e.movementX && !e.movementY)) return;
    hovering = true;
    clearTimeout(hoverTimer);
    hoverTimer = setTimeout(() => { if (!peek()) expand(); }, 140);
  });
  el.addEventListener('mouseleave', () => {
    hovering = false;
    clearTimeout(hoverTimer);
    if (!pinned) hoverTimer = setTimeout(collapseAll, 260);
  });
  document.addEventListener('mousedown', (e) => { if ((opened || wide) && !e.target.closest('.isl')) collapseAll(); }, true);
  function setCollapsed(on) {
    if (collapsed === on) return;
    collapsed = on;
    hovering = false;
    if (on && wide) { wide = false; pinned = false; }
    if (on && peeking != null) ax.send({ cmd: 'media', id: peeking, action: 'unpeek' });
    ax.collapse(on);
    sync();
  }
  el.addEventListener('click', (e) => {
    // Свёрнутый: клик по полоске — развернуть.
    if (collapsed) { setCollapsed(false); return; }
    const a = e.target.closest('[data-a]')?.dataset.a;
    if (a === 'fold') { setOpen(false); setCollapsed(true); return; }
    // Кнопки в самой чёлке — как в строке вкладки (или загрузки) раскрытого островка.
    const row = e.target.closest('.isl-row') || (a && e.target.closest('.isl-c[data-tab],.isl-c[data-dl]'));
    if (!row) { pinned = !(opened || wide) || !pinned; expand(); return; }
    if (row.dataset.tab) {
      const id = +row.dataset.tab;
      if (a === 'toggle' || a === 'next' || a === 'prev') {
        if (a === 'toggle') {
          // На паузе вкладка замолчит, но островок её подержит — можно нажать «играть».
          hold = id;
          const i = media.get(id);
          if (i) { i.playing = !i.playing; renderOpen(model()); renderCompact(model()); }
        }
        press(id, a);
        return;
      }
      if (a === 'pip' || a === 'unpip') {
        hold = id;
        setOpen(false);
        press(id, a);
        return;
      }
      collapseAll();
      ax.send({ cmd: 'activate_tab', id });
      return;
    }
    const id = +row.dataset.dl;
    if (a === 'show') { ax.send({ cmd: 'download_show', id }); collapseAll(); }
    else if (a === 'cancel') ax.send({ cmd: 'download_cancel', id });
    else if (a === 'open' || row.classList.contains('done')) { ax.send({ cmd: 'download_open', id }); collapseAll(); }
  });

  // ---------- сообщения ----------
  function onDownloads(m) {
    const items = Array.isArray(m.items) ? m.items : [];
    const before = lastStatus;
    lastStatus = new Map(items.map((d) => [d.id, d.status]));
    downloads = items;
    // Первый список — загрузки с прошлого раза, их не отмечаем.
    const fin = before && items.find((d) => d.status === 'done' && before.get(d.id) === 'active');
    if (fin) {
      done = { id: fin.id, name: fin.name };
      clearTimeout(doneTimer);
      doneTimer = setTimeout(() => { done = null; sync(); }, DONE_MS);
    }
    if (done && !items.some((d) => d.id === done.id)) done = null;
    sync();
  }
  function onProgress(m) {
    const d = downloads.find((x) => x.id === m.id);
    if (!d || d.status !== 'active') return;
    d.received = +m.received || 0;
    if (m.total) d.total = +m.total;
    if (!shown) return;
    const mm = model();
    renderCompact(mm);
    if (opened) updateProgress(mm);
  }
  function onPeek(m) {
    const id = +m.id;
    if (m.on) peeking = id; else if (peeking === id) peeking = null;
    peekH = m.on && +m.height > 0 ? +m.height : 0;
    sync();
  }
  function onMedia(m) {
    const id = +m.id;
    if (m.info && typeof m.info === 'object') media.set(id, m.info); else media.delete(id);
    // Видео в островке встало на паузу (кликом по нему или кнопкой) — держим вкладку,
    // чтобы из островка можно было продолжить.
    if (peeking === id && m.info && !m.info.playing) hold = id;
    // С этой вкладки ушли, а там видео, которое смотрели (не немое превью): держим её.
    if (leaving === id) {
      leaving = null;
      if (isVideo(id) && id !== ax.active()) hold = id;
    }
    sync();
  }

  // И когда островка не видно: свёрнутое окно сжимает интерфейс в ноль, островок
  // прячется, а развернули — должен вернуться. Ряд вкладок меняется и без окна:
  // адресная строка раздаётся при вводе и доезжает до своей ширины после разворота.
  const ro = new ResizeObserver(() => sync());
  ro.observe($('#row1'));
  ro.observe($('#side'));
  ro.observe($('#tabs'));
  addEventListener('resize', () => sync());

  // Начали вводить адрес — видео втягивается в чёлку, чтобы не закрывать подсказки.
  function onOmniFocus() {
    if (peeking != null) ax.send({ cmd: 'media', id: peeking, action: 'unpeek' });
  }

  return {
    sync,
    onDownloads,
    onProgress,
    onMedia,
    onPeek,
    onOmniFocus,
    // Настройки: островок, видео из чёлки, кнопка «видео в окне», свёрнут ли он; может ли
    // раскрытый островок зайти на страницу (есть ли под ним снимок — нет на Linux).
    configure(o) {
      const was = [enabled, showVideo, showPip, collapsed, sidePlace, barDown, overPage].join();
      enabled = o.enabled !== false;
      showVideo = o.video !== false;
      showPip = o.pip !== false;
      collapsed = !!o.collapsed;
      sidePlace = o.side === 'top' ? 'top' : 'panel';
      barDown = !!o.down;
      overPage = o.overPage !== false;
      if ([enabled, showVideo, showPip, collapsed, sidePlace, barDown, overPage].join() !== was) {
        el.querySelector('.isl-r').dataset.k = '';
        sync();
      }
    },
    isOpen: () => opened,
  };
}
