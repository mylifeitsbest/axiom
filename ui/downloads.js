'use strict';
// Загрузки: кнопка с кольцом прогресса и выпадающий список (Ctrl+J).
// Список и прогресс присылает Rust; отсюда уходят только номера загрузок —
// путь к файлу интерфейс не знает и подменить не может.
function createDownloads(ax) {
  const $ = (s, r = document) => r.querySelector(s);
  const btn = $('#dlBtn');
  const panel = $('#dlp');
  const listEl = $('#dlList');
  const EXT_COLORS = ['#7F77DD', '#1D9E75', '#D85A30', '#378ADD', '#D4537E', '#BA7517', '#639922', '#534AB7'];

  let items = [];
  let canCancel = false;
  let seen = null;
  // Скорость считаем здесь: сглаженная разница байтов между сообщениями.
  const rates = new Map();

  // ---------- форматирование ----------
  const UNITS = ['Б', 'КБ', 'МБ', 'ГБ', 'ТБ'];
  function size(n) {
    if (!Number.isFinite(n) || n < 0) return '';
    let i = 0;
    while (n >= 1024 && i < UNITS.length - 1) { n /= 1024; i++; }
    const digits = i && n < 100 ? 1 : 0;
    return `${n.toLocaleString('ru-RU', { maximumFractionDigits: digits })} ${UNITS[i]}`;
  }
  function eta(sec) {
    if (!Number.isFinite(sec) || sec <= 0) return '';
    if (sec < 60) return `ещё ${Math.max(1, Math.round(sec))} с`;
    if (sec < 3600) return `ещё ${Math.round(sec / 60)} мин`;
    const h = Math.floor(sec / 3600);
    const m = Math.round((sec % 3600) / 60);
    return m ? `ещё ${h} ч ${m} мин` : `ещё ${h} ч`;
  }
  const host = (url) => {
    try { const u = new URL(url); return /^https?:$/.test(u.protocol) ? u.hostname.replace(/^www\./, '') : ''; } catch { return ''; }
  };
  function badge(name) {
    const dot = name.lastIndexOf('.');
    const ext = dot > 0 && dot < name.length - 1 ? name.slice(dot + 1, dot + 5).toUpperCase() : '';
    let h = 0;
    for (const ch of ext) h = (h * 31 + ch.charCodeAt(0)) >>> 0;
    return { ext, color: EXT_COLORS[h % EXT_COLORS.length] };
  }

  function track(d) {
    const now = performance.now();
    const r = rates.get(d.id);
    if (!r) { rates.set(d.id, { t: now, bytes: d.received, speed: 0 }); return; }
    const dt = (now - r.t) / 1000;
    if (dt < 0.2) return;
    const inst = Math.max(0, d.received - r.bytes) / dt;
    r.speed = r.speed ? r.speed * 0.7 + inst * 0.3 : inst;
    r.t = now;
    r.bytes = d.received;
  }

  // Файл опасного типа ждёт решения пользователя — кольцо его не ждёт.
  const running = (d) => d.status === 'active' && !d.ask;

  // ---------- строки списка ----------
  function sub(d) {
    switch (d.status) {
      case 'active': {
        if (d.ask) return d.disguised ? 'Внутри программа, хотя имя у файла обычное' : 'Этот тип файла может навредить компьютеру';
        if (!d.received) return 'Начинается…';
        const speed = rates.get(d.id)?.speed || 0;
        const parts = [d.total ? `${size(d.received)} из ${size(d.total)}` : size(d.received)];
        if (speed > 0) parts.push(`${size(speed)}/с`);
        if (speed > 0 && d.total) parts.push(eta((d.total - d.received) / speed));
        return parts.filter(Boolean).join(' · ');
      }
      case 'done':
        if (!d.exists) return 'Файл удалён или перемещён';
        return [size(d.total ?? d.received), host(d.url)].filter(Boolean).join(' · ');
      case 'cancelled': return d.error || 'Отменено';
      default: return d.error || 'Не удалось скачать';
    }
  }
  const pct = (d) => (d.total ? Math.min(100, (d.received / d.total) * 100) : 0);

  function rowHtml(d) {
    const { ext, color } = badge(d.name);
    const active = d.status === 'active';
    const openable = d.status === 'done' && d.exists;
    const acts = [];
    // Решение о файле опасного типа — кнопками под именем, сбоку ничего.
    if (!d.ask && d.status !== 'cancelled' && d.status !== 'failed' && (d.exists || active)) {
      acts.push(`<button class="ib sm" data-a="show" title="Показать в папке">${icon('folder')}</button>`);
    }
    if (active && !d.ask && canCancel) acts.push(`<button class="ib sm" data-a="cancel" title="Отменить">${icon('x')}</button>`);
    if (!active) acts.push(`<button class="ib sm" data-a="remove" title="Убрать из списка">${icon('x')}</button>`);
    const bar = running(d) ? `<span class="dl-bar${d.total ? '' : ' ind'}"><i style="--p:${pct(d).toFixed(1)}"></i></span>` : '';
    // «Удалить» — та же отмена: файл не сохраняется.
    const ask = d.ask
      ? '<span class="dl-ask"><button class="btn" data-a="cancel">Удалить</button><button class="btn ghost" data-a="keep">Сохранить всё равно</button></span>'
      : '';
    const title = openable ? `Открыть ${d.name}` : d.name;
    const fill = d.ask ? 'var(--dl-warn)' : d.status === 'cancelled' || d.status === 'failed' || !d.exists ? 'var(--t3)' : color;
    return `<div class="dli st-${ax.esc(d.status)}${d.ask ? ' ask' : ''}${openable ? ' open' : ''}${d.exists ? '' : ' gone'}" data-id="${d.id}" title="${ax.esc(title)}">
      <span class="dl-ext" style="--f:${fill}">${ext ? ax.esc(ext) : icon('file')}</span>
      <span class="dl-main"><span class="dl-name">${ax.esc(d.name)}</span><span class="dl-sub">${ax.esc(sub(d))}</span>${bar}${ask}</span>
      <span class="dl-acts">${acts.join('')}</span>
    </div>`;
  }

  function renderList() {
    $('#dlClear').hidden = !items.some((d) => d.status !== 'active');
    listEl.innerHTML = items.length
      ? items.map(rowHtml).join('')
      : `<div class="dlp-empty">${icon('download')}<span>Скачанные файлы появятся здесь</span></div>`;
  }

  // ---------- кнопка ----------
  // Кольцо живёт по своему расписанию: пока значок новой загрузки летит,
  // кольца ещё нет, а загрузка, закончившаяся в полёте, получит галочку
  // уже после посадки. idle → run (кольцо крутится) → fin (галочка) → idle.
  let phase = 'idle';
  let finTimer = 0;
  let doneSince = false; // пока кольцо крутилось, что-то докачалось — в конце будет галочка
  let flying = 0;

  function renderBtn() {
    const active = items.filter(running);
    const asking = items.some((d) => d.ask);
    btn.hidden = !items.length;
    const sized = active.filter((d) => d.total);
    const indeterminate = active.length > 0 && sized.length < active.length;
    btn.classList.toggle('spin', indeterminate);
    const total = sized.reduce((s, d) => s + d.total, 0);
    const got = sized.reduce((s, d) => s + Math.min(d.received, d.total), 0);
    // Короткая дуга видна и на 0% — иначе в начале по кольцу крутилась бы точка.
    btn.style.setProperty('--p', total ? Math.max(4, (got / total) * 100).toFixed(1) : '0');
    btn.title = asking
      ? 'Загрузки: файл ждёт вашего решения (Ctrl+J)'
      : active.length
        ? `Загрузки: ${active.length === 1 ? 'идёт одна' : `идут ${active.length}`}${total && !indeterminate ? `, ${Math.floor((got / total) * 100)}%` : ''} (Ctrl+J)`
        : 'Загрузки (Ctrl+J)';
  }
  function sync() {
    if (flying) return;
    // Метка «ждёт решения» появляется, когда значок файла уже долетел до кнопки.
    btn.classList.toggle('ask', items.some((d) => d.ask));
    if (items.some(running)) setPhase('run');
    else if (phase === 'run' || doneSince) setPhase(doneSince && ax.speed() ? 'fin' : 'idle');
  }
  function setPhase(p) {
    if (p === phase) return;
    clearTimeout(finTimer);
    phase = p;
    if (p !== 'run') doneSince = false;
    btn.classList.toggle('busy', p === 'run');
    btn.classList.toggle('fin', p === 'fin');
    if (p === 'fin') finTimer = setTimeout(() => setPhase('idle'), ax.ms(1300) + 50);
  }
  function flash(cls) {
    if (!ax.speed()) return;
    btn.classList.remove(cls);
    void btn.offsetWidth;
    btn.classList.add(cls);
  }
  // Разовые анимации снимаем по окончании, чтобы их можно было повторить.
  const ONCE = { pop: 'in', dlGlow: 'catch', dlDone: 'done' };
  btn.addEventListener('animationend', (e) => { if (ONCE[e.animationName]) btn.classList.remove(ONCE[e.animationName]); });

  // ---------- «вжух»: новая загрузка влетает в кнопку ----------
  // Страница — отдельный нативный webview поверх области контента, и всё, что
  // интерфейс рисует там, скрыто под ней. Поэтому значок стартует за краем
  // страницы (она сама его обрезает — файл будто выныривает из неё), а дальше
  // летит только над панелью.
  const clamp = (v, a, b) => Math.min(b, Math.max(a, v));
  // Плавность как у cubic-bezier в CSS, но в JS: так известна скорость значка
  // в каждой точке, и хвост за ним тянется нужной длины.
  function bezier(x1, y1, x2, y2) {
    const at = (a, b, t) => 3 * a * t * (1 - t) ** 2 + 3 * b * t * t * (1 - t) + t ** 3;
    return (x) => {
      let lo = 0, hi = 1;
      for (let n = 0; n < 24; n++) { const m = (lo + hi) / 2; if (at(x1, x2, m) < x) lo = m; else hi = m; }
      return at(y1, y2, (lo + hi) / 2);
    };
  }
  const flightEase = bezier(0.5, 0, 0.3, 1);
  // Долю времени значок летит, остаток — тает уже в самой кнопке.
  const ARRIVE = 0.82;

  // Несколько загрузок разом: значки вылетают друг за другом, а не кучей,
  // у каждого своя дорожка (стартует чуть дальше), больше четырёх — без полёта.
  let slot = 0;
  function launch(list) {
    if (!ax.speed() || document.hidden) return;
    for (const d of list) {
      const lane = flying;
      if (lane >= 4) return;
      flying++;
      const now = performance.now();
      slot = Math.max(now, slot + ax.ms(130));
      setTimeout(() => fly(d, lane), slot - now);
    }
  }
  let caught = 0;
  function land() {
    flying--;
    // Вереницу значков кнопка ловит одним движением, а не дёргается на каждый.
    if (performance.now() - caught > ax.ms(400)) { caught = performance.now(); flash('catch'); }
    sync();
  }

  // Дуга — квадратичная кривая Безье от края страницы к центру кнопки.
  function flightPath(i) {
    const b = btn.getBoundingClientRect();
    const c = $('#content').getBoundingClientRect();
    if (btn.hidden || !b.width || !c.width) return null;
    const end = [b.left + b.width / 2, b.top + b.height / 2];
    let start, ctrl;
    if (end[1] < c.top) {
      // Кнопка над страницей: значок поднимается из-за верхнего края и сверху падает в кнопку.
      start = [clamp(end[0] - 150 - 20 * i, c.left + 30, c.right - 30), c.top + 18];
      // Вершина дуги ровно на высоте top, чтобы значок не ушёл за край окна.
      const top = Math.min(end[1], Math.max(14, end[1] - 24));
      ctrl = [start[0] + (end[0] - start[0]) * 0.4, top - Math.sqrt((start[1] - top) * (end[1] - top))];
    } else {
      // Боковая раскладка: вылетает из-за бокового края и сверху падает в кнопку.
      start = [end[0] < c.left ? c.left + 18 : c.right - 18, clamp(end[1] - 150 - 20 * i, c.top + 30, c.bottom - 30)];
      ctrl = [end[0], start[1]];
    }
    const N = 36;
    const pts = [];
    for (let n = 0; n <= N; n++) {
      const t = flightEase(Math.min(1, n / N / ARRIVE));
      const u = 1 - t;
      pts.push([u * u * start[0] + 2 * u * t * ctrl[0] + t * t * end[0], u * u * start[1] + 2 * u * t * ctrl[1] + t * t * end[1], t]);
    }
    return { pts, dir: Math.sign(end[0] - start[0]) || 1 };
  }

  function fly(d, i) {
    const path = flightPath(i);
    if (!path) { land(); return; }
    const { pts, dir } = path;
    const N = pts.length - 1;
    const vel = pts.map((_, n) => {
      const a = pts[Math.max(0, n - 1)];
      const z = pts[Math.min(N, n + 1)];
      return [Math.hypot(z[0] - a[0], z[1] - a[1]), Math.atan2(z[1] - a[1], z[0] - a[0])];
    });
    const vmax = Math.max(1e-6, ...vel.map((v) => v[0]));
    const chip = [];
    const tail = [];
    let ang = vel[0][1];
    pts.forEach(([x, y, t], n) => {
      const offset = n / N;
      const fade = Math.min(1, offset / 0.06, (1 - offset) / (1 - ARRIVE));
      // Вынырнув, значок подрастает, а у кнопки сжимается — «проваливается» в неё.
      const sc = offset > ARRIVE ? 0.5 - ((offset - ARRIVE) / (1 - ARRIVE)) * 0.3
        : t < 0.3 ? 0.7 + t : t < 0.65 ? 1 : 1 - ((t - 0.65) / 0.35) ** 2 * 0.5;
      chip.push({ offset, opacity: fade, transform: `translate(${x}px,${y}px) rotate(${dir * 12 * Math.sin(Math.PI * t)}deg) scale(${sc})` });
      // Угол без скачков через ±180°, иначе хвост крутанётся на полный оборот.
      let a = vel[n][1];
      while (a - ang > Math.PI) a -= 2 * Math.PI;
      while (ang - a > Math.PI) a += 2 * Math.PI;
      ang = a;
      const v = vel[n][0] / vmax;
      tail.push({ offset, opacity: fade * v * 0.6, transform: `translate(${x}px,${y}px) rotate(${a}rad) scaleX(${Math.max(0.01, v * sc)})` });
    });

    const { ext, color } = badge(d.name);
    const el = document.createElement('div');
    el.className = 'dl-fly';
    el.style.setProperty('--f', color);
    el.innerHTML = `<i></i><b${ext.length > 3 ? ' class="l4"' : ''}>${ext ? ax.esc(ext) : icon('file')}</b>`;
    document.body.append(el);
    const duration = ax.ms(640);
    el.firstElementChild.animate(tail, { duration, fill: 'both' });
    const anim = el.lastElementChild.animate(chip, { duration, fill: 'both' });
    // Кнопка «ловит» значок, как только он в неё входит, — пока он тает внутри.
    let landed = false;
    const touch = () => { if (!landed) { landed = true; land(); } };
    const done = () => { el.remove(); touch(); };
    setTimeout(touch, duration * (ARRIVE - 0.04));
    anim.onfinish = anim.oncancel = done;
    // Свёрнутое окно может придержать анимацию — посадка всё равно случится.
    setTimeout(done, duration + 500);
  }

  // ---------- панель ----------
  const isOpen = () => panel.classList.contains('on');
  function position() {
    const anchor = btn.hidden ? $('#setBtn') : btn;
    const r = anchor.getBoundingClientRect();
    const w = Math.min(380, innerWidth - 16);
    panel.style.width = w + 'px';
    const h = panel.offsetHeight;
    const left = Math.min(Math.max(8, r.right - w), innerWidth - 8 - w);
    // В боковой раскладке кнопки внизу — список открывается вверх.
    const up = r.bottom + 6 + h > innerHeight - 8 && r.top - 6 - h >= 8;
    panel.classList.toggle('up', up);
    Object.assign(panel.style, { left: Math.round(left) + 'px', top: Math.round(up ? r.top - 6 - h : r.bottom + 6) + 'px' });
  }
  function open() {
    if (isOpen()) return;
    renderList();
    panel.classList.add('on', 'anim');
    position();
    setTimeout(() => panel.classList.remove('anim'), ax.ms(400) + 50);
    btn.classList.add('on');
    ax.setOverlay('dl', true);
  }
  function close() {
    if (!isOpen()) return;
    panel.classList.remove('on');
    btn.classList.remove('on');
    ax.setOverlay('dl', false);
  }
  const toggle = () => (isOpen() ? close() : open());

  btn.addEventListener('click', toggle);
  document.addEventListener('mousedown', (e) => {
    if (isOpen() && !e.target.closest('#dlp,#dlBtn')) close();
  }, true);
  addEventListener('resize', () => { if (isOpen()) position(); });

  panel.addEventListener('click', (e) => {
    const head = e.target.closest('[data-h]');
    if (head) {
      if (head.dataset.h === 'folder') { ax.send({ cmd: 'downloads_folder' }); close(); }
      if (head.dataset.h === 'clear') ax.send({ cmd: 'downloads_clear' });
      return;
    }
    const row = e.target.closest('.dli');
    if (!row) return;
    const id = +row.dataset.id;
    const a = e.target.closest('[data-a]')?.dataset.a;
    if (a === 'show') { ax.send({ cmd: 'download_show', id }); close(); }
    else if (a === 'cancel') ax.send({ cmd: 'download_cancel', id });
    else if (a === 'keep') ax.send({ cmd: 'download_keep', id });
    else if (a === 'remove') ax.send({ cmd: 'download_remove', id });
    else if (row.classList.contains('open')) { ax.send({ cmd: 'download_open', id }); close(); }
  });

  // ---------- сообщения из Rust ----------
  function onList(m) {
    items = Array.isArray(m.items) ? m.items : [];
    canCancel = !!m.can_cancel;
    const prev = seen;
    seen = new Map(items.map((d) => [d.id, d.status]));
    for (const id of rates.keys()) if (seen.get(id) !== 'active') rates.delete(id);
    const hidden = btn.hidden;
    renderBtn();
    // Первый список — восстановленные с прошлого раза загрузки: им не влетать.
    if (prev) {
      if (hidden && !btn.hidden) flash('in');
      launch(items.filter((d) => !prev.has(d.id)));
      if (items.some((d) => d.status === 'done' && prev.get(d.id) === 'active')) {
        doneSince = true;
        // Остальные ещё качаются — готовый файл отмечаем коротким кругом.
        if (phase === 'run' && items.some((d) => d.status === 'active')) flash('done');
      }
    }
    sync();
    if (isOpen()) { renderList(); position(); }
  }
  function onProgress(m) {
    const d = items.find((x) => x.id === m.id);
    if (!d || d.status !== 'active') return;
    d.received = +m.received || 0;
    if (m.total) d.total = +m.total;
    track(d);
    renderBtn();
    const row = isOpen() && listEl.querySelector(`.dli[data-id="${d.id}"]`);
    if (!row) return;
    row.querySelector('.dl-sub').textContent = sub(d);
    const bar = row.querySelector('.dl-bar');
    if (bar) {
      bar.classList.toggle('ind', !d.total);
      bar.firstElementChild.style.setProperty('--p', pct(d).toFixed(1));
    }
  }

  // Раскладка поменялась (настройки вида) — открытый список едет за кнопкой.
  const reposition = () => { if (isOpen()) requestAnimationFrame(position); };

  return { open, close, toggle, isOpen, reposition, onList, onProgress };
}
