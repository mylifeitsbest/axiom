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

  // ---------- строки списка ----------
  function sub(d) {
    switch (d.status) {
      case 'active': {
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
      case 'cancelled': return 'Отменено';
      default: return d.error || 'Не удалось скачать';
    }
  }
  const pct = (d) => (d.total ? Math.min(100, (d.received / d.total) * 100) : 0);

  function rowHtml(d) {
    const { ext, color } = badge(d.name);
    const active = d.status === 'active';
    const openable = d.status === 'done' && d.exists;
    const acts = [];
    if (d.status !== 'cancelled' && d.status !== 'failed' && (d.exists || active)) {
      acts.push(`<button class="ib sm" data-a="show" title="Показать в папке">${icon('folder')}</button>`);
    }
    if (active && canCancel) acts.push(`<button class="ib sm" data-a="cancel" title="Отменить">${icon('x')}</button>`);
    if (!active) acts.push(`<button class="ib sm" data-a="remove" title="Убрать из списка">${icon('x')}</button>`);
    const bar = active ? `<span class="dl-bar${d.total ? '' : ' ind'}"><i style="--p:${pct(d).toFixed(1)}"></i></span>` : '';
    const title = openable ? `Открыть ${d.name}` : d.name;
    return `<div class="dli st-${ax.esc(d.status)}${openable ? ' open' : ''}${d.exists ? '' : ' gone'}" data-id="${d.id}" title="${ax.esc(title)}">
      <span class="dl-ext" style="--f:${d.status === 'cancelled' || d.status === 'failed' || !d.exists ? 'var(--t3)' : color}">${ext ? ax.esc(ext) : icon('file')}</span>
      <span class="dl-main"><span class="dl-name">${ax.esc(d.name)}</span><span class="dl-sub">${ax.esc(sub(d))}</span>${bar}</span>
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
  function renderBtn() {
    const active = items.filter((d) => d.status === 'active');
    btn.hidden = !items.length;
    btn.classList.toggle('busy', active.length > 0);
    const sized = active.filter((d) => d.total);
    const indeterminate = active.length > 0 && sized.length < active.length;
    btn.classList.toggle('spin', indeterminate);
    const total = sized.reduce((s, d) => s + d.total, 0);
    const got = sized.reduce((s, d) => s + Math.min(d.received, d.total), 0);
    btn.style.setProperty('--p', total ? ((got / total) * 100).toFixed(1) : '0');
    btn.title = active.length
      ? `Загрузки: ${active.length === 1 ? 'идёт одна' : `идут ${active.length}`}${total && !indeterminate ? `, ${Math.floor((got / total) * 100)}%` : ''} (Ctrl+J)`
      : 'Загрузки (Ctrl+J)';
  }
  function flash(cls) {
    if (!ax.speed()) return;
    btn.classList.remove(cls);
    void btn.offsetWidth;
    btn.classList.add(cls);
  }
  btn.addEventListener('animationend', (e) => { if (e.target === btn || e.target.closest?.('#dlBtn')) btn.classList.remove('bump', 'done'); });

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
    Object.assign(panel.style, { left: left + 'px', top: (up ? r.top - 6 - h : r.bottom + 6) + 'px' });
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
    if (prev) {
      if (items.some((d) => d.status === 'active' && !prev.has(d.id))) flash('bump');
      else if (items.some((d) => d.status === 'done' && prev.get(d.id) === 'active')) flash('done');
    }
    renderBtn();
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
