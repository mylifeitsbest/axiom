'use strict';
// Диспетчер задач — вкладка axiom://tasks (Shift+Esc). Своего webview у неё нет:
// пока она активна, app.js показывает эту страницу в области контента (как историю).
// Раз в секунду просим у Rust замер: сколько оперативной памяти и процессора
// занимает каждая вкладка и сам браузер (src/taskman). Память — частный рабочий
// набор, как в диспетчере задач Windows; процесс, общий для нескольких вкладок,
// Rust делит между ними поровну, поэтому строки в сумме дают итог.
//
// Тот же замер показывает и плашка во вкладке под курсором (createTabMem ниже).

const TK_MB = 1024 * 1024;
const TK_GB = TK_MB * 1024;
const tkNum = (n, digits) => n.toLocaleString('ru-RU', { minimumFractionDigits: digits, maximumFractionDigits: digits });
// Число и единица отдельно: крупная цифра и мелкая единица в сводке.
function tkMemParts(bytes) {
  const b = Math.max(0, +bytes || 0);
  if (b >= TK_GB) return [tkNum(b / TK_GB, b >= 10 * TK_GB ? 1 : 2), 'ГБ'];
  const mb = b / TK_MB;
  return [mb < 10 ? tkNum(mb, 1) : tkNum(Math.round(mb), 0), 'МБ'];
}
const tkMem = (bytes) => tkMemParts(bytes).join(' ');
function tkCpu(p) {
  const v = Math.max(0, +p || 0);
  if (v < 0.05) return '0 %';
  return `${v < 10 ? tkNum(v, 1) : tkNum(Math.round(v), 0)} %`;
}
function tkPlural(n, one, few, many) {
  const a = n % 10, b = n % 100;
  if (a === 1 && b !== 11) return one;
  if (a >= 2 && a <= 4 && (b < 12 || b > 14)) return few;
  return many;
}
const tkHost = (url) => { try { return new URL(url).host.replace(/^www\./, ''); } catch { return ''; } };
const tkInternal = (t) => String(t?.url || '').startsWith('axiom://');

function createTasks(ax) {
  const $ = (s, r = document) => r.querySelector(s);
  // Сколько секунд держит мини-график.
  const SPAN = 60;
  // Части самого браузера: значок, название, что это. `linux` — подпись там,
  // где по вкладкам память не видна (WebKitGTK не говорит, чей процесс).
  const PARTS = {
    ui: { ic: 'layout', name: 'Интерфейс Axiom', note: 'Панель вкладок, адресная строка и служебные вкладки — эта тоже' },
    app: { ic: 'spark', name: 'Axiom', note: 'Окно, вкладки, история и загрузки' },
    engine: { ic: 'globe', name: 'Движок WebView2', note: 'Главный процесс: окна страниц, сеть, куки' },
    gpu: { ic: 'monitor', name: 'Видеокарта', note: 'Отрисовка страниц и видео' },
    utility: { ic: 'database', name: 'Сеть, звук и службы', note: 'Загрузка страниц, звук, хранилище сайтов' },
    ai: { ic: 'sparkles', name: 'Панель ИИ', note: 'Google AI Mode' },
    pages: { ic: 'file', name: 'Фоновые процессы страниц', note: 'Запасной процесс и фоновые скрипты сайтов', linux: ['Страницы сайтов', 'Все открытые вкладки вместе: какая сколько, WebKitGTK не говорит'] },
    other: { ic: 'cpu', name: 'Прочее', note: 'Служебные процессы движка' },
  };
  const SORTS = { mem: 'Память', cpu: 'Процессор', order: 'Как на панели' };

  const page = document.createElement('div');
  page.className = 'tasks';
  page.id = 'tasks';
  page.innerHTML = `<div class="tk-in">
      <header class="tk-head">
        <span class="tk-t"><h2>Диспетчер задач</h2><p>Сколько оперативной памяти и процессора занимает каждая вкладка и сам браузер. Обновляется каждую секунду.</p></span>
        <button class="btn" data-k="sleep-all" title="Выгрузить из памяти фоновые вкладки, которые можно усыпить">${icon('moon')}<span>Усыпить фоновые</span></button>
      </header>
      <section class="tk-sum">
        <div class="tk-hero">
          <div class="tk-hero-top">
            <span class="tk-hero-n">
              <span class="tk-lbl">Axiom занимает</span>
              <span class="tk-big" id="tkMem"><b>—</b><small></small></span>
              <span class="tk-sub" id="tkMemSub">Считаем…</span>
            </span>
            <span class="tk-spark" id="tkMemSpark" data-kind="mem" aria-hidden="true"></span>
          </div>
          <div class="tk-meter" id="tkMeter" role="img" aria-label="Оперативная память компьютера">
            <i class="m-ax"></i><i class="m-oth"></i>
          </div>
          <div class="tk-legend" id="tkLegend"></div>
        </div>
        <div class="tk-tiles">
          <div class="tk-tile">
            <span class="tk-lbl">Процессор</span>
            <b class="tk-val" id="tkCpu">—</b>
            <span class="tk-sub" id="tkCpuSub">Все ядра вместе</span>
            <span class="tk-spark sm" id="tkCpuSpark" data-kind="cpu" aria-hidden="true"></span>
          </div>
          <div class="tk-tile">
            <span class="tk-lbl">Вкладки</span>
            <b class="tk-val" id="tkTabs">—</b>
            <span class="tk-sub" id="tkTabsSub"></span>
          </div>
        </div>
      </section>
      <div class="tk-sec"><h4>Вкладки</h4><span class="tk-sort" role="radiogroup" aria-label="Порядок">${Object.entries(SORTS).map(([k, v]) => `<button role="radio" data-sort="${k}">${v}</button>`).join('')}</span></div>
      <div class="tk-card" id="tkList" role="list"></div>
      <div class="tk-linux" id="tkLinux" hidden>${icon('cpu')}<span>На Linux движок не говорит, какой процесс чей, поэтому память по вкладкам не видна — все страницы вместе в строке «Страницы сайтов» ниже.</span></div>
      <div class="tk-sec"><h4>Сам браузер</h4></div>
      <div class="tk-card" id="tkParts" role="list"></div>
      <div class="note tk-note">${icon('cpu')}<span>Память — сколько оперативной памяти занято только под этот процесс: та же цифра, что в диспетчере задач Windows. Если вкладки одного сайта делят процесс, он делится между ними поровну. Спящая вкладка не занимает ничего и проснётся, когда вы её откроете. <button class="tk-link" data-k="settings">Когда вкладки засыпают сами</button></span></div>
    </div>`;
  $('#content').append(page);

  const list = $('#tkList', page);
  const partsEl = $('#tkParts', page);
  const tip = document.createElement('div');
  tip.className = 'tk-tip';
  page.append(tip);

  let shown = false;
  let poll = 0;
  let last = null;
  let sort = 'mem';
  // История итогов для мини-графиков: [{ at, mem, cpu }], не дольше SPAN секунд.
  let hist = [];
  // Вкладки, которые просили усыпить: «засыпает…», пока Rust не скажет, что спит.
  const sleeping = new Map();
  const rows = new Map();
  const partRows = new Map();

  // ---------- сводка ----------
  function renderSummary(v) {
    const [n, unit] = tkMemParts(v.total.memory);
    $('#tkMem b', page).textContent = n;
    $('#tkMem small', page).textContent = unit;
    const ram = +v.ram_total || 0;
    const free = Math.min(ram, +v.ram_free || 0);
    const ax = Math.min(ram, v.total.memory);
    const other = Math.max(0, ram - free - ax);
    const pct = ram ? (ax / ram) * 100 : 0;
    $('#tkMemSub', page).textContent = ram
      ? `${pct < 1 ? 'меньше 1' : tkNum(Math.round(pct), 0)} % оперативной памяти компьютера · ${v.total.procs} ${tkPlural(v.total.procs, 'процесс', 'процесса', 'процессов')}`
      : `${v.total.procs} ${tkPlural(v.total.procs, 'процесс', 'процесса', 'процессов')}`;
    const meter = $('#tkMeter', page);
    meter.hidden = !ram;
    $('#tkLegend', page).hidden = !ram;
    if (ram) {
      meter.style.setProperty('--ax', `${(ax / ram) * 100}%`);
      meter.style.setProperty('--oth', `${(other / ram) * 100}%`);
      meter.setAttribute('aria-label', `Оперативная память компьютера ${tkMem(ram)}: Axiom ${tkMem(ax)}, другие программы ${tkMem(other)}, свободно ${tkMem(free)}`);
      $('#tkLegend', page).innerHTML = `<span><i class="k-ax"></i>Axiom <b>${tkMem(ax)}</b></span>`
        + `<span><i class="k-oth"></i>Другие программы <b>${tkMem(other)}</b></span>`
        + `<span><i class="k-free"></i>Свободно <b>${tkMem(free)}</b> из ${tkMem(ram)}</span>`;
    }
    $('#tkCpu', page).textContent = v.cpu_ready ? tkCpu(v.total.cpu) : '—';
    $('#tkCpuSub', page).textContent = v.cpus ? `Все ${v.cpus} ${tkPlural(v.cpus, 'поток', 'потока', 'потоков')} процессора вместе` : 'Все ядра вместе';
    const tabs = ax_tabs().filter((t) => !tkInternal(t));
    const live = tabs.filter((t) => !t.sleeping).length;
    const zz = tabs.length - live;
    $('#tkTabs', page).textContent = `${live} ${tkPlural(live, 'работает', 'работают', 'работают')}`;
    $('#tkTabsSub', page).textContent = zz
      ? `${zz} ${tkPlural(zz, 'спит', 'спят', 'спят')} и не ${zz === 1 ? 'занимает' : 'занимают'} памяти`
      : tabs.length ? 'Спящих нет' : 'Открытых сайтов нет';
    drawSpark($('#tkMemSpark', page), 'mem');
    drawSpark($('#tkCpuSpark', page), 'cpu');
  }
  const ax_tabs = () => (typeof ax.tabs === 'function' ? ax.tabs() : []) || [];

  // ---------- мини-графики: итог за последнюю минуту ----------
  // Шкалы графика: время — сколько уже накопилось (от 10 с до минуты), чтобы линия
  // сразу шла через весь график; процессор — от нуля, не ниже 10 %; память — вокруг
  // её значений с запасом не меньше 32 МБ, чтобы шум в мегабайт не выглядел горой.
  function scales(kind, pts, w, h) {
    const vals = pts.map((p) => (kind === 'mem' ? p.mem : p.cpu));
    const now = pts[pts.length - 1].at;
    const span = Math.min(SPAN * 1000, Math.max(10000, now - pts[0].at));
    let lo = 0;
    let hi = Math.max(10, Math.max(...vals) * 1.2);
    if (kind === 'mem') {
      const min = Math.min(...vals), max = Math.max(...vals);
      const pad = Math.max(32 * TK_MB, (max - min) * 0.35);
      lo = Math.max(0, min - pad);
      hi = max + pad;
    }
    return {
      vals, now, span,
      x: (at) => w - ((now - at) / span) * w,
      y: (v) => h - 3 - ((v - lo) / (hi - lo || 1)) * (h - 8),
    };
  }
  const sparkPts = (kind) => hist.filter((p) => (kind === 'mem' ? p.mem : p.cpu) != null);
  function drawSpark(box, kind) {
    const pts = sparkPts(kind);
    if (pts.length < 2) { box.innerHTML = ''; return; }
    const w = box.clientWidth || 200;
    const h = box.clientHeight || 44;
    const sc = scales(kind, pts, w, h);
    const line = pts.map((p) => `${sc.x(p.at).toFixed(1)},${sc.y(kind === 'mem' ? p.mem : p.cpu).toFixed(1)}`).join(' ');
    const first = sc.x(pts[0].at).toFixed(1);
    // Процессор — с заливкой от нуля, память — одной линией (её шкала не от нуля).
    box.innerHTML = `<svg viewBox="0 0 ${w} ${h}" width="${w}" height="${h}" preserveAspectRatio="none">`
      + (kind === 'cpu' ? `<polygon class="sp-a" points="${first},${h} ${line} ${w},${h}"/>` : '')
      + `<polyline class="sp-l" points="${line}"/>`
      + `<circle class="sp-d" cx="${w}" cy="${sc.y(sc.vals[sc.vals.length - 1]).toFixed(1)}" r="3"/>`
      + `<line class="sp-x" x1="0" x2="0" y1="0" y2="${h}" hidden/><circle class="sp-h" r="4" hidden/></svg>`;
  }
  // Наведение на график: что было столько-то секунд назад.
  function sparkHover(e) {
    const box = e.currentTarget;
    const kind = box.dataset.kind;
    const pts = sparkPts(kind);
    const svg = box.querySelector('svg');
    if (!svg || pts.length < 2) return;
    const r = box.getBoundingClientRect();
    const sc = scales(kind, pts, r.width, r.height);
    const now = sc.now;
    const at = now - ((r.width - (e.clientX - r.left)) / r.width) * sc.span;
    let best = pts[0];
    for (const p of pts) if (Math.abs(p.at - at) < Math.abs(best.at - at)) best = p;
    const px = sc.x(best.at);
    const val = kind === 'mem' ? best.mem : best.cpu;
    const py = sc.y(val);
    const xl = svg.querySelector('.sp-x');
    const dot = svg.querySelector('.sp-h');
    xl.removeAttribute('hidden'); dot.removeAttribute('hidden');
    xl.setAttribute('x1', px); xl.setAttribute('x2', px);
    dot.setAttribute('cx', px); dot.setAttribute('cy', py);
    const ago = Math.round((now - best.at) / 1000);
    tip.innerHTML = `<span>${ago ? `${ago} с назад` : 'Сейчас'}</span><b>${kind === 'mem' ? tkMem(val) : tkCpu(val)}</b>`;
    const pr = page.getBoundingClientRect();
    tip.style.left = `${r.left - pr.left + px}px`;
    tip.style.top = `${r.top - pr.top + page.scrollTop - 6}px`;
    tip.classList.add('on');
  }
  function sparkLeave(e) {
    tip.classList.remove('on');
    e.currentTarget.querySelectorAll('.sp-x,.sp-h').forEach((n) => n.setAttribute('hidden', ''));
  }
  for (const box of page.querySelectorAll('.tk-spark')) {
    box.addEventListener('pointermove', sparkHover);
    box.addEventListener('pointerleave', sparkLeave);
  }

  // ---------- строки вкладок ----------
  function makeRow(t) {
    const el = document.createElement('div');
    el.className = 'tk-row in';
    el.tabIndex = 0;
    el.setAttribute('role', 'listitem');
    el.dataset.id = t.id;
    el.innerHTML = '<span class="tk-fav"></span>'
      + '<span class="tk-name"><span class="tk-title"></span><span class="tk-host"></span><span class="tk-tags"></span></span>'
      + '<span class="tk-mem"><b></b><i class="tk-bar"><i></i></i></span>'
      + '<span class="tk-cpu"></span>'
      + `<span class="tk-act"><button class="ib sm" data-k="sleep" title="Усыпить — выгрузить из памяти, вкладка останется">${icon('moon')}</button>`
      + `<button class="ib sm" data-k="close" title="Закрыть вкладку">${icon('x')}</button></span>`;
    el.addEventListener('animationend', () => el.classList.remove('in'), { once: true });
    return el;
  }
  function tagsHtml(t, use) {
    const tags = [];
    if (t.audible) tags.push(`<span class="tk-tag" title="Играет звук">${icon('vol')}</span>`);
    if (t.no_sleep || t.site_no_sleep) tags.push(`<span class="tk-tag" title="Не засыпает сама">${icon('cup')}</span>`);
    if (t.ai) tags.push('<span class="tk-tag txt" title="Открыта из панели ИИ">ИИ</span>');
    if (use?.shared) tags.push('<span class="tk-tag txt" title="Делит процесс с другими вкладками того же сайта: здесь его доля">общий процесс</span>');
    return tags.join('');
  }
  function renderTabs(v) {
    const tabs = ax_tabs().filter((t) => !tkInternal(t));
    const use = new Map((v.tabs || []).map((u) => [u.id, u]));
    const now = Date.now();
    for (const [id, at] of sleeping) if (now - at > 6000) sleeping.delete(id);
    // Длина полосы — от самой тяжёлой строки на странице (вкладки и сам браузер вместе).
    const maxMem = Math.max(1, ...(v.tabs || []).map((u) => u.memory), ...(v.parts || []).map((p) => p.memory));
    const order = tabs.map((t, i) => ({ t, i, u: use.get(t.id) }));
    const memOf = (o) => (o.t.sleeping ? -1 : o.u ? o.u.memory : 0);
    const cpuOf = (o) => (o.t.sleeping ? -1 : o.u ? o.u.cpu : 0);
    if (sort === 'mem') order.sort((a, b) => memOf(b) - memOf(a) || a.i - b.i);
    else if (sort === 'cpu') order.sort((a, b) => cpuOf(b) - cpuOf(a) || memOf(b) - memOf(a) || a.i - b.i);
    // Ушедшие вкладки плавно схлопываются.
    const ids = new Set(tabs.map((t) => t.id));
    for (const [id, el] of rows) {
      if (ids.has(id)) continue;
      rows.delete(id);
      collapse(el);
    }
    const before = new Map();
    if (ax.speed()) for (const [id, el] of rows) before.set(id, el.getBoundingClientRect().top);
    let prev = null;
    for (const { t, u } of order) {
      let el = rows.get(t.id);
      if (!el) { el = makeRow(t); rows.set(t.id, el); }
      const title = t.title || t.url;
      const key = tkHost(t.url) || t.url;
      const fav = el.querySelector('.tk-fav');
      if (fav.dataset.key !== key) { fav.dataset.key = key; fav.innerHTML = ax.favHtml(t.url); }
      el.querySelector('.tk-title').textContent = title;
      el.querySelector('.tk-host').textContent = tkHost(t.url);
      const tags = tagsHtml(t, u);
      const tagsEl = el.querySelector('.tk-tags');
      if (tagsEl.dataset.v !== tags) { tagsEl.dataset.v = tags; tagsEl.innerHTML = tags; }
      el.title = `${title}\n${t.url}`;
      const zz = !!t.sleeping;
      const going = !zz && sleeping.has(t.id);
      if (zz) sleeping.delete(t.id);
      el.classList.toggle('zz', zz);
      el.classList.toggle('going', going);
      const memB = el.querySelector('.tk-mem b');
      const bar = el.querySelector('.tk-bar > i');
      if (zz) { memB.textContent = 'спит'; bar.style.width = '0%'; }
      else if (going) { memB.textContent = 'засыпает…'; }
      else if (!v.per_tab) { memB.textContent = '—'; bar.style.width = '0%'; }
      else if (u) { memB.textContent = tkMem(u.memory); bar.style.width = `${Math.max(1.5, (u.memory / maxMem) * 100)}%`; }
      else { memB.textContent = '…'; bar.style.width = '0%'; }
      el.querySelector('.tk-cpu').textContent = zz || !v.per_tab || !v.cpu_ready || !u ? '' : tkCpu(u.cpu);
      const sleepBtn = el.querySelector('[data-k=sleep]');
      sleepBtn.hidden = zz || going;
      // Строка на своём месте: двигаем, только если она стоит не там.
      const want = prev ? prev.nextElementSibling : list.firstElementChild;
      if (el !== want) list.insertBefore(el, want);
      prev = el;
    }
    // Строки сменили места — переезжают плавно, а не прыгают (FLIP).
    if (before.size) {
      for (const [id, top] of before) {
        const el = rows.get(id);
        if (!el) continue;
        const dy = top - el.getBoundingClientRect().top;
        if (Math.abs(dy) > 1) el.animate([{ transform: `translateY(${dy}px)` }, { transform: 'none' }], { duration: ax.ms(320), easing: 'cubic-bezier(.22,1,.36,1)' });
      }
    }
    list.classList.toggle('empty', !tabs.length);
    if (!tabs.length) list.dataset.empty = 'Открытых сайтов нет: только служебные вкладки Axiom';
    $('#tkLinux', page).hidden = !!v.per_tab;
  }
  function collapse(el) {
    if (!ax.speed()) { el.remove(); return; }
    const h = el.getBoundingClientRect().height;
    el.classList.add('out');
    el.animate(
      [{ height: `${h}px`, opacity: 1 }, { height: '0px', opacity: 0, paddingTop: '0px', paddingBottom: '0px' }],
      { duration: ax.ms(220), easing: 'cubic-bezier(.4,0,.2,1)', fill: 'forwards' },
    ).finished.then(() => el.remove(), () => el.remove());
  }

  // ---------- сам браузер ----------
  function renderParts(v) {
    const maxMem = Math.max(1, ...(v.tabs || []).map((u) => u.memory), ...(v.parts || []).map((p) => p.memory));
    const parts = (v.parts || []).filter((p) => PARTS[p.part]).sort((a, b) => b.memory - a.memory);
    const keep = new Set(parts.map((p) => p.part));
    for (const [k, el] of partRows) if (!keep.has(k)) { partRows.delete(k); el.remove(); }
    let prev = null;
    for (const p of parts) {
      const d = PARTS[p.part];
      const [name, note] = !v.per_tab && d.linux ? d.linux : [d.name, d.note];
      let el = partRows.get(p.part);
      if (!el) {
        el = document.createElement('div');
        el.className = 'tk-row part';
        el.setAttribute('role', 'listitem');
        el.innerHTML = `<span class="tk-ic">${icon(d.ic)}</span>`
          + '<span class="tk-name"><span class="tk-title"></span><span class="tk-host"></span></span>'
          + '<span class="tk-mem"><b></b><i class="tk-bar"><i></i></i></span><span class="tk-cpu"></span><span class="tk-act"></span>';
        partRows.set(p.part, el);
      }
      el.querySelector('.tk-title').textContent = name;
      el.querySelector('.tk-host').textContent = note;
      el.title = `${name}: ${p.procs} ${tkPlural(p.procs, 'процесс', 'процесса', 'процессов')}`;
      el.querySelector('.tk-mem b').textContent = tkMem(p.memory);
      el.querySelector('.tk-bar > i').style.width = `${Math.max(1.5, (p.memory / maxMem) * 100)}%`;
      el.querySelector('.tk-cpu').textContent = v.cpu_ready ? tkCpu(p.cpu) : '';
      const want = prev ? prev.nextElementSibling : partsEl.firstElementChild;
      if (el !== want) partsEl.insertBefore(el, want);
      prev = el;
    }
  }

  // ---------- ответ Rust ----------
  function onData(m) {
    if (!m || typeof m !== 'object' || !m.total) return;
    last = m;
    if (!shown) return;
    const now = Date.now();
    hist.push({ at: now, mem: m.total.memory, cpu: m.cpu_ready ? m.total.cpu : null });
    hist = hist.filter((h) => now - h.at <= SPAN * 1000 + 1500);
    page.classList.add('ready');
    renderSummary(m);
    renderTabs(m);
    renderParts(m);
  }
  // Вкладки поменялись (усыпили, закрыли, сменился заголовок) — строки сразу, не ждём замера.
  function sync() { if (shown && last) { renderTabs(last); renderSummary(last); } }

  // ---------- действия ----------
  function rowOf(e) { return e.target.closest('.tk-row:not(.part)'); }
  list.addEventListener('click', (e) => {
    const row = rowOf(e);
    if (!row || row.classList.contains('out')) return;
    const id = +row.dataset.id;
    const k = e.target.closest('[data-k]')?.dataset.k;
    if (k === 'sleep') { sleepTab(id); return; }
    if (k === 'close') { ax.send({ cmd: 'close_tab', id }); return; }
    ax.send({ cmd: 'activate_tab', id });
  });
  list.addEventListener('keydown', (e) => {
    const row = rowOf(e);
    if (!row || e.target !== row) return;
    const all = [...list.querySelectorAll('.tk-row:not(.out)')];
    const i = all.indexOf(row);
    if (e.key === 'Enter') { e.preventDefault(); ax.send({ cmd: 'activate_tab', id: +row.dataset.id }); }
    else if (e.key === 'ArrowDown') { e.preventDefault(); all[Math.min(i + 1, all.length - 1)]?.focus(); }
    else if (e.key === 'ArrowUp') { e.preventDefault(); all[Math.max(i - 1, 0)]?.focus(); }
  });
  function sleepTab(id) {
    sleeping.set(id, Date.now());
    ax.send({ cmd: 'tasks_sleep', id });
    if (last) renderTabs(last);
  }
  page.addEventListener('click', (e) => {
    const k = e.target.closest('[data-k]')?.dataset.k;
    if (k === 'sleep-all') {
      const going = ax_tabs().filter((t) => !tkInternal(t) && !t.sleeping && !t.audible && !t.no_sleep && !t.site_no_sleep);
      for (const t of going) sleeping.set(t.id, Date.now());
      ax.send({ cmd: 'tasks_sleep_all' });
      if (last) renderTabs(last);
    } else if (k === 'settings') ax.openSettings('memory');
    const s = e.target.closest('[data-sort]')?.dataset.sort;
    if (s && s !== sort) { sort = s; syncSort(); if (last) renderTabs(last); }
  });
  function syncSort() {
    page.querySelectorAll('[data-sort]').forEach((b) => {
      b.classList.toggle('on', b.dataset.sort === sort);
      b.setAttribute('aria-checked', String(b.dataset.sort === sort));
    });
  }
  syncSort();

  // ---------- снаружи ----------
  function ask() { if (document.visibilityState === 'visible') ax.send({ cmd: 'tasks' }); }
  // `on` — активна вкладка диспетчера. Пока она на экране — замер раз в секунду.
  function render(on) {
    if (on === shown) { if (on) sync(); return; }
    shown = on;
    clearInterval(poll);
    if (!on) { tip.classList.remove('on'); return; }
    hist = [];
    page.scrollTop = 0;
    ask();
    poll = setInterval(ask, 1000);
    if (last) { renderTabs(last); }
  }

  return { render, onData, sync, isShown: () => shown, last: () => last };
}

// Плашка памяти во вкладке под курсором: навели на вкладку — через секунду в ней
// появляется, сколько она занимает. Карточку поверх страницы не рисуем: страница —
// отдельное окно над интерфейсом, под карточкой её пришлось бы подменять снимком.
function createTabMem(ax) {
  const DELAY = 900;
  // Пробежали мышью по вкладкам — не спрашиваем: замер просим, только задержавшись.
  const PREFETCH = 250;
  const strip = document.getElementById('tablist');
  let el = null;
  let id = null;
  let showTimer = 0;
  let askTimer = 0;
  let poll = 0;
  let on = false;

  function tabOf(tid) { return (ax.tabs() || []).find((t) => t.id === tid); }
  function text() {
    const t = tabOf(id);
    const v = ax.last();
    if (!t || tkInternal(t)) return null;
    if (t.sleeping) return 'спит';
    if (!v) return '…';
    if (!v.per_tab) return null;
    const u = (v.tabs || []).find((x) => x.id === id);
    return u ? tkMem(u.memory) : '…';
  }
  function paint() {
    if (!el || !on) return;
    const s = text();
    const pill = el.querySelector('.tmem');
    if (!pill) return;
    if (s == null) { el.classList.remove('mem'); return; }
    if (pill.textContent !== s) pill.textContent = s;
    pill.classList.toggle('wait', s === '…');
    el.classList.add('mem');
  }
  function ask() { ax.send({ cmd: 'tasks' }); }
  function show() {
    on = true;
    paint();
    clearInterval(poll);
    poll = setInterval(ask, 1000);
  }
  function hide() {
    clearTimeout(showTimer);
    clearTimeout(askTimer);
    clearInterval(poll);
    on = false;
    if (el) el.classList.remove('mem');
    el = null;
    id = null;
  }
  function enter(tabEl) {
    if (tabEl === el) return;
    const was = on;
    if (el) el.classList.remove('mem');
    el = tabEl;
    id = +tabEl.dataset.id;
    clearTimeout(showTimer);
    clearTimeout(askTimer);
    // Плашка уже на соседней вкладке — переезжает сразу, без новой задержки.
    if (was) { ask(); show(); return; }
    on = false;
    askTimer = setTimeout(ask, PREFETCH);
    showTimer = setTimeout(show, DELAY);
  }
  strip.addEventListener('pointerover', (e) => {
    const t = e.target.closest('.tab');
    if (t && !t.classList.contains('out') && e.pointerType !== 'touch') enter(t);
  });
  strip.addEventListener('pointerout', (e) => {
    const to = e.relatedTarget?.closest?.('.tab');
    if (!to || !strip.contains(to)) hide();
  });
  // Клик, перетаскивание и колесо — не до плашки.
  strip.addEventListener('pointerdown', hide);
  addEventListener('blur', hide);

  return { onData: paint, sync: paint };
}
