'use strict';
// Интерфейс Axiom. Состояние вкладок приходит из Rust (axiom.recv),
// команды уходят через window.ipc. Здесь только отрисовка и ввод.
(() => {
  const $ = (s, r = document) => r.querySelector(s);
  const $$ = (s, r = document) => Array.from(r.querySelectorAll(s));
  const root = document.documentElement;
  const app = $('#app');
  const NEWTAB = 'axiom://newtab';
  const reduceMotion = matchMedia('(prefers-reduced-motion: reduce)');
  const darkScheme = matchMedia('(prefers-color-scheme: dark)');

  const send = (msg) => {
    if (window.ipc) window.ipc.postMessage(JSON.stringify(msg));
    else console.debug('ipc →', msg);
  };
  const esc = (s) => String(s ?? '').replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
  const clamp = (v, lo, hi, d) => (Number.isFinite(+v) ? Math.min(hi, Math.max(lo, +v)) : d);
  const parseUrl = (u) => { try { return new URL(u); } catch { return null; } };
  const isDdg = (u) => u && /(^|\.)duckduckgo\.com$/.test(u.hostname) && u.searchParams.has('q');

  // ---------- темы ----------
  const PRESETS = {
    graphite: { name: 'Графит', mode: 'dark', hue: 265, tint: 0.012, accent: '#8b9cff', oled: false },
    paper: { name: 'Бумага', mode: 'light', hue: 85, tint: 0.006, accent: '#2f6bff', oled: false },
    moss: { name: 'Мох', mode: 'dark', hue: 155, tint: 0.03, accent: '#7fd1a0', oled: false },
    sand: { name: 'Песок', mode: 'light', hue: 70, tint: 0.035, accent: '#c8553d', oled: false },
    void: { name: 'Пустота', mode: 'dark', hue: 0, tint: 0, accent: '#ffffff', oled: true },
    dusk: { name: 'Сумерки', mode: 'dark', hue: 305, tint: 0.04, accent: '#d7a6ff', oled: false },
  };
  const ACCENTS = ['#8b9cff', '#2f6bff', '#7fd1a0', '#c8553d', '#f0b429', '#ff7aa2', '#ffffff'];
  const FONTS = {
    system: '"Segoe UI Variable Text","Segoe UI",system-ui,-apple-system,BlinkMacSystemFont,"SF Pro Text",Roboto,"Helvetica Neue",sans-serif',
    rounded: 'ui-rounded,"Nunito","Varela Round","Segoe UI Variable Text","Segoe UI",system-ui,sans-serif',
    mono: '"JetBrains Mono","Cascadia Code","Cascadia Mono",ui-monospace,"SF Mono",Consolas,monospace',
    serif: '"Iowan Old Style","Charter","Georgia",ui-serif,serif',
  };
  const DEFAULTS = {
    preset: 'graphite', mode: 'dark', hue: 265, tint: 0.012, oled: false, accent: '#8b9cff',
    layout: 'one', radius: 10, gap: 6, font: 'system', customFont: '', fontSize: 12.5, speed: 1,
    showNav: true, showAiBtn: true, fullUrl: false, customCss: '', aiWidth: 400,
  };
  const FAV_COLORS = ['#7F77DD', '#1D9E75', '#D85A30', '#378ADD', '#D4537E', '#BA7517', '#639922', '#534AB7'];

  // ---------- состояние ----------
  const S = { tabs: [], active: null, aiOpen: false, aiProxy: false, canRestore: false, topSites: [], maximized: false };
  let A = { ...DEFAULTS };
  let B = { double_search: true, ai_proxy: '', sleep_minutes: 5, live_tabs: 3, onboarded: true, check_updates: true };
  let SYS = { installed: false, default_browser: false, chrome_found: false, can_install: false, version: '' };
  let aiReady = false;
  let prevActive = null;

  const activeTab = () => S.tabs.find((t) => t.id === S.active);
  const speed = () => (reduceMotion.matches ? 0 : clamp(A.speed, 0, 3, 1));
  const ms = (n) => Math.round(n * speed());

  // Пока поверх контента что-то открыто (подсказки, настройки, приветствие),
  // нативные webview страниц прячутся — иначе они перекрыли бы интерфейс.
  // На их месте остаётся снимок страницы (ui/snapshot.js).
  const snap = createSnapshot({
    send,
    // Что сейчас закрыто нативными webview: id вкладки со страницей и панель ИИ.
    native() {
      const t = activeTab();
      return { page: t && t.url !== NEWTAB && !t.sleeping ? t.id : null, ai: S.aiOpen && aiReady };
    },
  });
  const overlayReasons = new Set();
  function setOverlay(reason, on) {
    const before = overlayReasons.size > 0;
    if (on) overlayReasons.add(reason); else overlayReasons.delete(reason);
    const after = overlayReasons.size > 0;
    if (on) snap.opening(reason, !before);
    if (before !== after) send({ cmd: 'overlay', open: after });
    if (!on) snap.closing(reason, !after);
  }

  // ---------- загрузки (ui/downloads.js) ----------
  const dl = createDownloads({ esc, send, ms, speed, setOverlay });

  // ---------- обновления (ui/update.js) ----------
  const upd = createUpdate({ esc, send, ms, speed, setOverlay, copyText: (t) => copyText(t), toast: (t) => toast(t), onChange: () => panels.sync() });

  // ---------- перевод страниц (ui/translate.js) ----------
  const tr = createTranslate({ esc, send, speed, setOverlay, toast: (t) => toast(t) });

  // ---------- ссылки на приложения (ui/applink.js) ----------
  const xap = createAppLink({ esc, send, setOverlay });

  // ---------- иконки и фавиконки ----------
  const fillIcons = (scope = document) => $$('[data-i]', scope).forEach((el) => { el.innerHTML = icon(el.dataset.i); });

  function favHtml(url) {
    const u = parseUrl(url);
    if (!u || url === NEWTAB) return `<span class="fav" style="--f:var(--s3)">${icon('spark')}</span>`;
    const host = u.hostname.replace(/^www\./, '');
    let h = 0;
    for (const ch of host) h = (h * 31 + ch.charCodeAt(0)) >>> 0;
    const letter = esc((host[0] || '?').toUpperCase());
    const img = /^https?:$/.test(u.protocol) ? `<img class="fi" alt="" src="${esc(u.origin)}/favicon.ico">` : '';
    return `<span class="fav" style="--f:${FAV_COLORS[h % FAV_COLORS.length]}"><b>${letter}</b>${img}</span>`;
  }
  document.addEventListener('load', (e) => { if (e.target.matches?.('img.fi')) e.target.parentElement.classList.add('ok'); }, true);
  document.addEventListener('error', (e) => { if (e.target.matches?.('img.fi')) e.target.remove(); }, true);

  // ---------- раскладка ----------
  let placed = '';
  function place() {
    const l = A.layout;
    app.dataset.layout = l;
    if (l === placed) { queueLayout(); return; }
    placed = l;
    const [nav, tabs, omni, acts, wc, grow] = ['#nav', '#tabs', '#omni', '#acts', '#wc', '#growA'].map((s) => $(s));
    if (l === 'two') {
      $('#row1').append(tabs, wc);
      $('#row2').append(nav, omni, acts);
      $('#parts').append(grow);
    } else if (l === 'side') {
      $('#sideTop').append(nav, grow, wc);
      $('#side').append(omni, tabs, acts);
    } else {
      $('#row1').append(nav, tabs, omni, acts, wc);
      $('#parts').append(grow);
    }
    queueLayout();
  }

  let layoutQueued = false;
  let lastLayout = '';
  function queueLayout() {
    if (layoutQueued) return;
    layoutQueued = true;
    requestAnimationFrame(() => {
      layoutQueued = false;
      const rect = (el) => {
        const b = el.getBoundingClientRect();
        return { x: Math.round(b.left), y: Math.round(b.top), w: Math.round(b.width), h: Math.round(b.height) };
      };
      const msg = { cmd: 'layout', content: rect($('#content')), ai: S.aiOpen && aiReady ? rect($('#aiBody')) : null };
      const key = JSON.stringify(msg);
      if (key !== lastLayout) { lastLayout = key; send(msg); }
    });
  }
  const ro = new ResizeObserver(queueLayout);
  ro.observe($('#content'));
  ro.observe($('#aiBody'));
  addEventListener('resize', () => { queueLayout(); if ($('#sug').classList.contains('on')) positionSug(); });

  // ---------- внешний вид ----------
  const canvas = document.createElement('canvas');
  canvas.width = canvas.height = 1;
  const cx = canvas.getContext('2d', { willReadFrequently: true });
  const probe = document.createElement('i');
  probe.style.cssText = 'position:absolute;width:0;height:0;visibility:hidden';
  document.body.append(probe);
  function toHex(css) {
    cx.clearRect(0, 0, 1, 1);
    cx.fillStyle = '#000';
    cx.fillStyle = css;
    cx.fillRect(0, 0, 1, 1);
    const [r, g, b] = cx.getImageData(0, 0, 1, 1).data;
    return '#' + [r, g, b].map((v) => v.toString(16).padStart(2, '0')).join('');
  }
  let lastColors = '';
  function sendThemeColors() {
    requestAnimationFrame(() => {
      probe.style.color = 'var(--bg)';
      const win = toHex(getComputedStyle(probe).color);
      probe.style.color = 'var(--pg)';
      const page = toHex(getComputedStyle(probe).color);
      // Сайты получают тему Axiom (prefers-color-scheme); «Авто» — как в системе.
      const scheme = A.mode === 'system' ? 'auto' : root.dataset.mode;
      const key = win + page + scheme;
      if (key !== lastColors) { lastColors = key; send({ cmd: 'theme_colors', window: win, page, scheme }); }
    });
  }
  const hex = (v, d) => (/^#[0-9a-f]{6}$/i.test(v) ? v : d);
  function fgFor(c) {
    const n = parseInt(c.slice(1), 16);
    return 0.299 * (n >> 16 & 255) + 0.587 * (n >> 8 & 255) + 0.114 * (n & 255) > 150 ? '#0b0b10' : '#ffffff';
  }

  function applyAppearance() {
    const mode = A.mode === 'system' ? (darkScheme.matches ? 'dark' : 'light') : (A.mode === 'light' ? 'light' : 'dark');
    root.dataset.mode = mode;
    root.dataset.oled = String(!!A.oled && mode === 'dark');
    const st = root.style;
    const accent = hex(A.accent, DEFAULTS.accent);
    st.setProperty('--h', clamp(A.hue, 0, 360, 265));
    st.setProperty('--c', clamp(A.tint, 0, 0.1, 0.012));
    st.setProperty('--acc', accent);
    st.setProperty('--acc-fg', fgFor(accent));
    st.setProperty('--r', clamp(A.radius, 0, 24, 10) + 'px');
    st.setProperty('--gap', clamp(A.gap, 0, 24, 6) + 'px');
    st.setProperty('--fs', clamp(A.fontSize, 10, 18, 12.5) + 'px');
    st.setProperty('--k', speed());
    st.setProperty('--ai-w', clamp(A.aiWidth, 280, 2000, 400) + 'px');
    const custom = String(A.customFont || '').replace(/["\\;{}<>]/g, '').trim();
    st.setProperty('--font', A.font === 'custom' && custom ? `"${custom}",${FONTS.system}` : (FONTS[A.font] || FONTS.system));
    app.classList.toggle('no-nav', !A.showNav);
    app.classList.toggle('no-ai-btn', !A.showAiBtn);
    $('#userCss').textContent = typeof A.customCss === 'string' ? A.customCss : '';
    $('#dsBtn').classList.toggle('on', !!B.double_search);
    place();
    dl.reposition();
    upd.reposition();
    renderOmni();
    panels.sync();
    sendThemeColors();
  }
  darkScheme.addEventListener('change', () => { if (A.mode === 'system') applyAppearance(); });

  // Плавная смена темы: новый вид «разливается» кругом от точки клика.
  function swap(fn, ev) {
    if (!document.startViewTransition || !speed()) { fn(); return; }
    const x = ev?.clientX ?? innerWidth / 2;
    const y = ev?.clientY ?? innerHeight / 2;
    const vt = document.startViewTransition(fn);
    vt.ready.then(() => {
      const r = Math.hypot(Math.max(x, innerWidth - x), Math.max(y, innerHeight - y));
      document.documentElement.animate(
        { clipPath: [`circle(0px at ${x}px ${y}px)`, `circle(${r}px at ${x}px ${y}px)`] },
        { duration: ms(650), easing: 'cubic-bezier(.22,1,.36,1)', pseudoElement: '::view-transition-new(root)' },
      );
    }).catch(() => {});
  }

  let saveTimer;
  function saveSettings(now) {
    clearTimeout(saveTimer);
    const run = () => send({
      cmd: 'save_settings',
      settings: {
        appearance: A,
        double_search: !!B.double_search,
        ai_proxy: String(B.ai_proxy || ''),
        sleep_minutes: Math.round(clamp(B.sleep_minutes, 0, 1440, 5)),
        live_tabs: Math.round(clamp(B.live_tabs, 0, 20, 3)),
        onboarded: !!B.onboarded,
        force_dark: !!B.force_dark,
        check_updates: B.check_updates !== false,
      },
    });
    if (now) run(); else saveTimer = setTimeout(run, 350);
  }

  function setA(key, value) {
    A[key] = value;
    if (['hue', 'tint', 'accent', 'mode', 'oled'].includes(key)) A.preset = 'custom';
    applyAppearance();
    saveSettings();
  }
  function mergeAppearance(obj) {
    const next = { ...A };
    for (const k of Object.keys(DEFAULTS)) if (k in obj && typeof obj[k] === typeof DEFAULTS[k]) next[k] = obj[k];
    A = next;
    A.preset = 'custom';
    applyAppearance();
  }
  function copyText(text) {
    const fallback = () => {
      const t = document.createElement('textarea');
      t.value = text;
      document.body.append(t);
      t.select();
      document.execCommand('copy');
      t.remove();
    };
    if (navigator.clipboard?.writeText) navigator.clipboard.writeText(text).catch(fallback); else fallback();
  }

  // ---------- вкладки ----------
  const tabEls = new Map();
  function renderTabs() {
    const list = $('#tablist');
    const ids = new Set(S.tabs.map((t) => t.id));
    for (const [id, el] of tabEls) {
      if (ids.has(id)) continue;
      tabEls.delete(id);
      el.classList.remove('in');
      el.classList.add('out');
      setTimeout(() => el.remove(), ms(420) + 30);
    }
    let prev = null;
    for (const t of S.tabs) {
      let el = tabEls.get(t.id);
      if (!el) {
        el = document.createElement('div');
        el.className = 'tab in';
        el.dataset.id = t.id;
        el.setAttribute('role', 'tab');
        el.innerHTML = `<span class="favslot"></span><span class="tt"></span><button class="tx" title="Закрыть (Ctrl+W)">${icon('x')}</button>`;
        el.addEventListener('animationend', () => el.classList.remove('in'), { once: true });
        tabEls.set(t.id, el);
      }
      el.classList.toggle('on', t.id === S.active);
      el.classList.toggle('zz', !!t.sleeping);
      el.classList.toggle('ld', !!t.loading && !t.sleeping);
      el.classList.toggle('nosleep', !!t.no_sleep);
      const title = t.title || (t.url === NEWTAB ? 'Новая вкладка' : t.url);
      el.querySelector('.tt').textContent = title;
      el.title = t.sleeping ? `${title} — спит` : t.no_sleep ? `${title} — не засыпает` : title;
      const key = parseUrl(t.url)?.origin || t.url;
      const slot = el.querySelector('.favslot');
      if (slot.dataset.key !== key) { slot.dataset.key = key; slot.innerHTML = favHtml(t.url); }
      const want = prev ? prev.nextElementSibling : list.firstElementChild;
      if (el !== want) list.insertBefore(el, want);
      prev = el;
    }
  }

  function flip(mutate) {
    const els = $$('.tab', $('#tablist'));
    const before = new Map(els.map((x) => [x, x.getBoundingClientRect()]));
    mutate();
    for (const x of els) {
      if (x.classList.contains('drag')) continue;
      const a = before.get(x);
      const b = x.getBoundingClientRect();
      const dx = a.left - b.left;
      const dy = a.top - b.top;
      if (dx || dy) x.animate([{ transform: `translate(${dx}px,${dy}px)` }, { transform: 'none' }], { duration: ms(220), easing: 'cubic-bezier(.22,1,.36,1)' });
    }
  }

  $('#tablist').addEventListener('pointerdown', (e) => {
    const el = e.target.closest('.tab');
    if (!el || el.classList.contains('out')) return;
    const id = +el.dataset.id;
    if (e.button === 1) { e.preventDefault(); send({ cmd: 'close_tab', id }); return; }
    if (e.button !== 0) return;
    const onClose = !!e.target.closest('.tx');
    const vertical = app.dataset.layout === 'side';
    const start = vertical ? e.clientY : e.clientX;
    let dragging = false;
    el.setPointerCapture(e.pointerId);
    const move = (ev) => {
      const p = vertical ? ev.clientY : ev.clientX;
      if (!dragging && !onClose && Math.abs(p - start) > 5) { dragging = true; el.classList.add('drag'); }
      if (!dragging) return;
      let before = null;
      for (const s of $$('.tab:not(.out)', $('#tablist'))) {
        if (s === el) continue;
        const r = s.getBoundingClientRect();
        if (p < (vertical ? r.top + r.height / 2 : r.left + r.width / 2)) { before = s; break; }
      }
      if (before !== el.nextElementSibling && before !== el) flip(() => $('#tablist').insertBefore(el, before));
    };
    const up = () => {
      el.removeEventListener('pointermove', move);
      el.removeEventListener('pointerup', up);
      el.removeEventListener('pointercancel', up);
      if (dragging) {
        el.classList.remove('drag');
        const order = $$('.tab:not(.out)', $('#tablist')).map((x) => +x.dataset.id);
        send({ cmd: 'move_tab', id, to: order.indexOf(id) });
      } else if (onClose) {
        send({ cmd: 'close_tab', id });
      } else {
        send({ cmd: 'activate_tab', id });
      }
    };
    el.addEventListener('pointermove', move);
    el.addEventListener('pointerup', up);
    el.addEventListener('pointercancel', up);
  });
  $('#tablist').addEventListener('auxclick', (e) => { if (e.button === 1) e.preventDefault(); });
  $('#newTabBtn').addEventListener('click', () => send({ cmd: 'new_tab' }));

  // ---------- контент и новая вкладка ----------
  function renderContent() {
    const t = activeTab();
    const nt = !t || t.url === NEWTAB;
    const c = $('#content');
    c.classList.toggle('nt', nt);
    c.classList.toggle('zz', !!t && !nt && !!t.sleeping);
    if (nt) renderNewtab();
    if (S.active !== prevActive) {
      prevActive = S.active;
      if (nt && mode === 'url' && !overlayReasons.has('onb') && !overlayReasons.has('sheet')) requestAnimationFrame(() => inp.focus());
    }
  }

  function renderNewtab() {
    tickClock();
    $('#tiles').innerHTML = S.topSites.slice(0, 8).map((s) =>
      `<button class="tile" data-url="${esc(s.url)}" title="${esc(s.url)}">${favHtml(s.url)}<span>${esc(s.title)}</span></button>`).join('');
  }
  function tickClock() {
    const now = new Date();
    $('#clock').textContent = now.toLocaleTimeString('ru-RU', { hour: '2-digit', minute: '2-digit' });
    $('#date').textContent = now.toLocaleDateString('ru-RU', { weekday: 'long', day: 'numeric', month: 'long' });
  }
  setInterval(() => { if ($('#content').classList.contains('nt')) tickClock(); }, 10000);
  $('#tiles').addEventListener('click', (e) => {
    const b = e.target.closest('.tile');
    if (b) send({ cmd: 'navigate', input: b.dataset.url });
  });

  // ---------- адресная строка ----------
  const omni = $('#omni');
  const inp = $('#omniInput');
  const view = $('#omniView');
  let mode = 'url';

  function setIc(name) { $('#omniIc').innerHTML = icon(name); }

  function renderOmni() {
    const t = activeTab();
    const star = $('#starBtn');
    const pageOpen = !!t && t.url !== NEWTAB;
    star.hidden = !pageOpen;
    star.classList.toggle('on', !!t?.bookmarked);
    star.title = t?.bookmarked ? 'Убрать из закладок (Ctrl+D)' : 'В закладки (Ctrl+D)';
    tr.render(t, S.tabs);
    if (document.activeElement === inp || mode === 'find') return;
    if (!pageOpen) {
      omni.classList.add('empty');
      inp.value = '';
      view.innerHTML = '';
      setIc('search');
      return;
    }
    omni.classList.remove('empty');
    inp.value = t.url;
    const u = parseUrl(t.url);
    if (!u) { view.textContent = t.url; setIc('globe'); return; }
    if (isDdg(u)) {
      view.innerHTML = `<span class="h">${esc(u.searchParams.get('q'))}</span>`;
      setIc('search');
      return;
    }
    const path = (u.pathname + u.search + u.hash).replace(/^\/$/, '');
    view.innerHTML = A.fullUrl
      ? `<span class="h">${esc(u.host)}</span><span class="p">${esc(path)}</span>`
      : `<span class="h">${esc(u.host.replace(/^www\./, ''))}</span>`;
    setIc(u.protocol === 'https:' ? 'lock' : 'globe');
  }

  function toast(text) {
    const m = $('#omniMsg');
    m.textContent = text;
    m.classList.remove('on');
    void m.offsetWidth;
    m.classList.add('on');
  }
  $('#omniMsg').addEventListener('animationend', (e) => e.target.classList.remove('on'));

  inp.addEventListener('focus', () => {
    omni.classList.add('focus');
    if (mode === 'url') {
      const t = activeTab();
      inp.value = t && t.url !== NEWTAB ? t.url : '';
    }
    requestAnimationFrame(() => inp.select());
  });
  inp.addEventListener('blur', () => {
    omni.classList.remove('focus');
    hideSug();
    if (mode === 'find') exitFind();
    renderOmni();
  });
  inp.addEventListener('input', () => { if (mode === 'find') find(false); else updateSug(); });
  inp.addEventListener('keydown', onOmniKey);

  const looksLikeUrl = (q) => !/\s/.test(q) && (/^[a-z][a-z0-9+.-]*:\/\//i.test(q) || /^localhost(:\d+)?(\/|$)/i.test(q) || /^[^/?#]+\.[\p{L}]{2,}(:\d+)?([/?#]|$)/u.test(q)
    || /^\d{1,3}(\.\d{1,3}){3}(:\d+)?([/?#]|$)/.test(q) || /^\[[0-9a-f:.]+\](:\d+)?([/?#]|$)/i.test(q));

  let sugQ = '';
  let sugHist = [];
  let sugItems = [];
  let sugSel = 0;
  let sugTimer;
  function updateSug() {
    const q = inp.value.trim();
    sugQ = q;
    if (!q) { hideSug(); return; }
    clearTimeout(sugTimer);
    sugTimer = setTimeout(() => send({ cmd: 'suggest', q }), 40);
    renderSug();
  }
  function renderSug() {
    const q = sugQ;
    if (!q) { hideSug(); return; }
    const items = [];
    if (looksLikeUrl(q)) items.push({ kind: 'go', ic: 'globe', title: q, sub: 'Перейти', input: q });
    items.push({ kind: 'search', ic: 'search', title: q, sub: B.double_search ? 'DuckDuckGo + ИИ' : 'DuckDuckGo', input: q });
    for (const h of sugHist.slice(0, 7)) {
      items.push({ kind: h.kind, ic: h.kind === 'bookmark' ? 'star' : 'clock', title: h.title || h.url, sub: h.url.replace(/^https?:\/\/(www\.)?/, ''), input: h.url });
    }
    items.push({ kind: 'ai', ic: 'sparkles', title: 'Спросить ИИ', sub: q, input: q, kbd: 'Ctrl+Enter' });
    sugItems = items;
    sugSel = Math.min(Math.max(sugSel, 0), items.length - 1);
    $('#sug').innerHTML = items.map((it, i) =>
      `<div class="si${i === sugSel ? ' on' : ''}" data-i="${i}" role="option">${icon(it.ic)}<span class="st">${esc(it.title)}</span><span class="ss">${esc(it.sub)}</span>${it.kbd ? `<kbd>${it.kbd}</kbd>` : ''}</div>`).join('');
    showSug();
  }
  function positionSug() {
    const r = omni.getBoundingClientRect();
    const w = Math.min(Math.max(r.width, 460), innerWidth - 16);
    const left = Math.min(Math.max(8, r.left), innerWidth - 8 - w);
    Object.assign($('#sug').style, { left: left + 'px', top: r.bottom + 6 + 'px', width: w + 'px' });
  }
  function showSug() {
    const el = $('#sug');
    positionSug();
    if (!el.classList.contains('on')) {
      el.classList.add('on', 'anim');
      setTimeout(() => el.classList.remove('anim'), ms(400) + 50);
      setOverlay('sug', true);
    }
  }
  function hideSug() {
    const el = $('#sug');
    if (el.classList.contains('on')) {
      el.classList.remove('on');
      setOverlay('sug', false);
    }
    // Запрос подсказок мог уйти, а ответ прийти уже после Enter: он не должен
    // открыть список снова (под ним спрятана страница).
    clearTimeout(sugTimer);
    sugQ = '';
    sugHist = [];
    sugSel = 0;
  }
  const markSel = () => $$('.si', $('#sug')).forEach((e, i) => e.classList.toggle('on', i === sugSel));
  $('#sug').addEventListener('mousedown', (e) => {
    e.preventDefault();
    const row = e.target.closest('.si');
    if (row) runItem(sugItems[+row.dataset.i], e.altKey);
  });
  $('#sug').addEventListener('mousemove', (e) => {
    const row = e.target.closest('.si');
    if (row && +row.dataset.i !== sugSel) { sugSel = +row.dataset.i; markSel(); }
  });

  function onOmniKey(e) {
    if (mode === 'find') {
      if (e.key === 'Enter') { e.preventDefault(); find(e.shiftKey); }
      else if (e.key === 'Escape') { e.preventDefault(); inp.blur(); }
      return;
    }
    const open = $('#sug').classList.contains('on');
    if (e.key === 'ArrowDown' && open) { e.preventDefault(); sugSel = (sugSel + 1) % sugItems.length; markSel(); }
    else if (e.key === 'ArrowUp' && open) { e.preventDefault(); sugSel = (sugSel - 1 + sugItems.length) % sugItems.length; markSel(); }
    else if (e.key === 'Enter') {
      e.preventDefault();
      const q = inp.value.trim();
      if (!q) return;
      if (e.ctrlKey || e.metaKey) { askAi(q); return; }
      const fallback = looksLikeUrl(q) ? { kind: 'go', input: q } : { kind: 'search', input: q };
      runItem(open ? sugItems[sugSel] : fallback, e.altKey);
    } else if (e.key === 'Escape') {
      e.preventDefault();
      if (open) hideSug(); else inp.blur();
    }
  }
  function runItem(it, newTab) {
    if (!it) return;
    if (it.kind === 'ai') { askAi(it.input); return; }
    send(newTab ? { cmd: 'new_tab', input: it.input } : { cmd: 'navigate', input: it.input });
    if (it.kind === 'search' && B.double_search) send({ cmd: 'ai', open: true, query: it.input });
    hideSug();
    inp.blur();
  }
  function askAi(q) {
    send({ cmd: 'ai', open: true, query: q });
    hideSug();
    inp.blur();
  }

  $('#dsBtn').addEventListener('click', () => {
    B.double_search = !B.double_search;
    $('#dsBtn').classList.toggle('on', B.double_search);
    toast(B.double_search ? 'Двойной поиск включён' : 'Двойной поиск выключен');
    panels.sync();
    saveSettings(true);
  });
  $('#starBtn').addEventListener('click', () => send({ cmd: 'toggle_bookmark' }));

  // «Не усыплять»: вкладку не усыпят ни таймер, ни нехватка памяти, ни лимит живых вкладок.
  function renderAwake() {
    const t = activeTab();
    const b = $('#awakeBtn');
    const on = !!t?.no_sleep;
    b.hidden = !t || t.url === NEWTAB;
    b.classList.toggle('on', on);
    b.setAttribute('aria-pressed', String(on));
    b.title = on ? 'Разрешить вкладке засыпать' : 'Не усыплять вкладку';
  }
  $('#awakeBtn').addEventListener('click', () => { if (S.active != null) send({ cmd: 'toggle_no_sleep', id: S.active }); });

  // ---------- поиск на странице ----------
  function startFind() {
    mode = 'find';
    hideSug();
    omni.classList.add('find');
    omni.classList.remove('empty');
    inp.placeholder = 'Найти на странице';
    inp.value = '';
    setIc('search');
    inp.focus();
  }
  function exitFind() {
    mode = 'url';
    omni.classList.remove('find', 'nf');
    inp.placeholder = 'Поиск или адрес';
    send({ cmd: 'find', q: '', backwards: false });
    renderOmni();
  }
  let findTimer;
  function find(backwards) {
    clearTimeout(findTimer);
    const q = inp.value;
    if (!q) { omni.classList.remove('nf'); return; }
    findTimer = setTimeout(() => send({ cmd: 'find', q, backwards: !!backwards }), 60);
  }

  // ---------- панель ИИ ----------
  let aiTimer;
  function renderAi() {
    $('#aiBtn').classList.toggle('on', S.aiOpen);
    $('#pdot').classList.toggle('on', S.aiProxy);
    $('#pdot').title = S.aiProxy ? 'Через прокси' : 'Без прокси';
    if (S.aiOpen === app.classList.contains('ai-open')) return;
    clearTimeout(aiTimer);
    aiReady = false;
    $('#aiBody').classList.remove('ready');
    app.classList.toggle('ai-open', S.aiOpen);
    queueLayout();
    // Нативная панель появляется, когда закончится анимация выезда.
    // Скелетон под готовой панелью не виден: прячем, чтобы его анимация не перерисовывала окно каждый кадр.
    if (S.aiOpen) aiTimer = setTimeout(() => { aiReady = true; $('#aiBody').classList.add('ready'); queueLayout(); }, ms(420));
  }
  function toggleAi() {
    if (S.aiOpen) { send({ cmd: 'ai', open: false }); return; }
    let q = document.activeElement === inp && mode === 'url' ? inp.value.trim() : '';
    if (!q) { const u = parseUrl(activeTab()?.url || ''); if (isDdg(u)) q = u.searchParams.get('q') || ''; }
    send({ cmd: 'ai', open: true, query: q || null });
  }
  $('#aiBtn').addEventListener('click', toggleAi);
  $('#aiClose').addEventListener('click', () => send({ cmd: 'ai', open: false }));
  $('#aiPop').addEventListener('click', () => send({ cmd: 'ai_pop_out' }));

  $('#aiResizer').addEventListener('pointerdown', (e) => {
    const bar = e.currentTarget;
    bar.setPointerCapture(e.pointerId);
    bar.classList.add('drag');
    const move = (ev) => {
      A.aiWidth = Math.round(clamp(innerWidth - ev.clientX - 10, 280, innerWidth * 0.65, 400));
      root.style.setProperty('--ai-w', A.aiWidth + 'px');
    };
    const up = () => {
      bar.classList.remove('drag');
      bar.removeEventListener('pointermove', move);
      bar.removeEventListener('pointerup', up);
      bar.removeEventListener('pointercancel', up);
      panels.sync();
      saveSettings();
    };
    bar.addEventListener('pointermove', move);
    bar.addEventListener('pointerup', up);
    bar.addEventListener('pointercancel', up);
  });

  // ---------- окно ----------
  document.addEventListener('mousedown', (e) => {
    if (e.button !== 0) return;
    const edge = e.target.closest('[data-edge]');
    if (edge) { send({ cmd: 'resize_window', dir: edge.dataset.edge }); return; }
    if (!e.target.closest('[data-drag]') || e.target.closest('button,input,textarea,select,.tab,.omni,[data-nodrag]')) return;
    send(e.detail === 2 ? { cmd: 'window', action: 'maximize' } : { cmd: 'window', action: 'drag' });
  });
  $('#wc').addEventListener('click', (e) => {
    const b = e.target.closest('[data-win]');
    if (b) send({ cmd: 'window', action: b.dataset.win });
  });
  $('#nav').addEventListener('click', (e) => {
    const b = e.target.closest('[data-act]');
    if (b) send({ cmd: b.dataset.act });
  });

  // ---------- горячие клавиши (когда фокус в интерфейсе) ----------
  function keyAction(e) {
    // AltGr на Windows приходит как Ctrl+Alt: это ввод символа (# { [ @ ł), а не сочетание.
    const mod = (e.ctrlKey || e.metaKey) && !e.altKey && !e.getModifierState('AltGraph');
    const c = e.code;
    if (mod && e.shiftKey && c === 'KeyT') return 'restore_tab';
    if (mod && !e.shiftKey && c === 'KeyT') return 'new_tab';
    if (mod && c === 'KeyW') return 'close_tab';
    if (mod && c === 'Tab') return e.shiftKey ? 'prev_tab' : 'next_tab';
    if (mod && c === 'KeyL') return 'focus_omnibox';
    if (mod && c === 'KeyK') return 'toggle_ai';
    if (mod && c === 'KeyF') return 'find';
    if (mod && c === 'KeyD') return 'bookmark';
    if (mod && c === 'KeyJ') return 'downloads';
    if ((mod && c === 'KeyR') || c === 'F5') return 'reload';
    if (c === 'F11' && !mod && !e.altKey && !e.shiftKey) return 'fullscreen';
    if (mod && c === 'Comma') return 'settings';
    if (e.altKey && c === 'ArrowLeft') return 'back';
    if (e.altKey && c === 'ArrowRight') return 'forward';
    if (mod && /^Digit[1-9]$/.test(c)) return 'tab_' + c.slice(5);
    return null;
  }
  document.addEventListener('keydown', (e) => {
    const a = keyAction(e);
    if (a) {
      if (overlayReasons.has('onb') && a !== 'reload') { e.preventDefault(); return; }
      e.preventDefault();
      e.stopPropagation();
      hotkey(a);
      return;
    }
    if (e.key === 'Escape' && dl.isOpen()) { e.preventDefault(); dl.close(); return; }
    if (e.key === 'Escape' && upd.isOpen()) { e.preventDefault(); upd.close(); return; }
    if (e.key === 'Escape' && panels.isOpen() && document.activeElement !== inp) { e.preventDefault(); panels.close(); }
  }, true);
  function hotkey(a) {
    if (a !== 'settings' && panels.isOpen()) panels.close();
    if (a !== 'downloads' && dl.isOpen()) dl.close();
    if (upd.isOpen()) upd.close();
    switch (a) {
      case 'focus_omnibox': if (mode === 'find') exitFind(); inp.focus(); inp.select(); break;
      case 'toggle_ai': toggleAi(); break;
      case 'find': startFind(); break;
      case 'settings': panels.toggle(); break;
      case 'downloads': if (!overlayReasons.has('onb')) dl.toggle(); break;
      default: send({ cmd: 'hotkey', action: a });
    }
  }
  $('#setBtn').addEventListener('click', () => { dl.close(); upd.close(); panels.toggle(); });

  // ---------- настройки и приветствие (ui/settings.js) ----------
  const panels = createPanels({
    get A() { return A; },
    get B() { return B; },
    PRESETS, ACCENTS,
    esc, send, toast, swap, ms, speed, setOverlay, copyText, setA, mergeAppearance,
    system: () => SYS,
    update: () => upd.state(),
    translate: tr,
    openUpdate() { panels.close(); dl.close(); upd.open(); },
    applyAppearance,
    saveSettings,
    refresh() { $('#dsBtn').classList.toggle('on', !!B.double_search); panels.sync(); },
    resetAppearance() { A = { ...DEFAULTS, aiWidth: A.aiWidth }; applyAppearance(); },
  });

  // ---------- приём сообщений из Rust ----------
  function render() {
    renderTabs();
    renderContent();
    renderOmni();
    renderAwake();
    renderAi();
    snap.sync();
  }
  const isObj = (v) => v && typeof v === 'object' && !Array.isArray(v);
  let firstSettings = true;
  const handlers = {
    settings(m) {
      const s = m.settings || {};
      A = { ...DEFAULTS, ...(isObj(s.appearance) ? s.appearance : {}) };
      B = { double_search: s.double_search !== false, ai_proxy: s.ai_proxy || '', sleep_minutes: s.sleep_minutes ?? 5, live_tabs: s.live_tabs ?? 3, onboarded: !!s.onboarded, force_dark: !!s.force_dark, check_updates: s.check_updates !== false };
      tr.onSettings(s.translate);
      applyAppearance();
      if (firstSettings) {
        // #settings:раздел, #onb:шаг, #downloads, #update[:install], #translate[:menu|:revert]
        // или #import:браузер (сразу начать перенос) — только для разработки (debug-сборка).
        const [screen, arg] = location.hash.slice(1).split(':');
        if (screen === 'settings') panels.open(arg);
        else if (screen === 'onb') panels.openOnboarding(+arg || 0);
        else if (screen === 'import') { panels.open('data'); panels.importSoon(arg); }
        else if (screen === 'downloads') dl.open();
        else if (screen === 'update') upd.openSoon(arg === 'install');
        else if (screen === 'translate') tr.debug(arg);
        else if (!B.onboarded) panels.openOnboarding();
      }
      firstSettings = false;
    },
    state(m) {
      Object.assign(S, {
        tabs: Array.isArray(m.tabs) ? m.tabs : [],
        active: m.active,
        aiOpen: !!m.ai_open,
        aiProxy: !!m.ai_proxy,
        canRestore: !!m.can_restore,
        topSites: Array.isArray(m.top_sites) && m.top_sites.length ? m.top_sites : S.topSites,
      });
      render();
    },
    suggestions(m) { if (m.q === sugQ && mode === 'url' && document.activeElement === inp) { sugHist = m.items || []; renderSug(); } },
    hotkey(m) { hotkey(m.action); },
    find(m) { omni.classList.toggle('nf', !m.found); },
    toast(m) { toast(m.text); },
    window(m) {
      S.maximized = !!m.maximized;
      app.classList.toggle('max', S.maximized);
      const b = $('[data-win=maximize]');
      b.innerHTML = icon(S.maximized ? 'restore' : 'max');
      b.title = S.maximized ? 'Восстановить' : 'Развернуть';
    },
    system(m) { SYS = { ...SYS, ...(m.status || {}) }; panels.onSystem(); },
    imported(m) { panels.onImported(m); },
    installed(m) { panels.onInstalled(m); },
    downloads(m) { dl.onList(m); },
    download_progress(m) { dl.onProgress(m); },
    update(m) { upd.onState(m); },
    translate(m) { tr.onState(m); },
    snapshot(m) { snap.onMessage(m); },
    app_link(m) { xap.onRequest(m); },
  };
  window.axiom = Object.freeze({
    recv(msg) {
      try { handlers[msg?.type]?.(msg); } catch (err) { console.error(err); }
    },
  });

  // ---------- старт ----------
  fillIcons();
  applyAppearance();
  render();
  send({ cmd: 'ready' });
})();
