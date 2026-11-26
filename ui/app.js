'use strict';
// Интерфейс Axiom. Состояние вкладок приходит из Rust (axiom.recv),
// команды уходят через window.ipc. Здесь только отрисовка и ввод.
(() => {
  const $ = (s, r = document) => r.querySelector(s);
  const $$ = (s, r = document) => Array.from(r.querySelectorAll(s));
  const root = document.documentElement;
  const app = $('#app');
  const NEWTAB = 'axiom://newtab';
  // Вкладка настроек: своего webview нет, пока она активна, интерфейс показывает лист настроек.
  const SETTINGS = 'axiom://settings';
  const isSettings = (t) => !!t && t.url === SETTINGS;
  // Вкладка истории: тоже без webview, страницу рисует ui/history.js.
  const HISTORY = 'axiom://history';
  const isHistory = (t) => !!t && t.url === HISTORY;
  // Во вкладке открыта страница сайта, а не служебный экран Axiom.
  const isPage = (t) => !!t && t.url !== NEWTAB && t.url !== SETTINGS && t.url !== HISTORY;
  const reduceMotion = matchMedia('(prefers-reduced-motion: reduce)');
  const darkScheme = matchMedia('(prefers-color-scheme: dark)');

  const send = (msg) => {
    if (window.ipc) window.ipc.postMessage(JSON.stringify(msg));
    else console.debug('ipc →', msg);
  };
  const esc = (s) => String(s ?? '').replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
  const clamp = (v, lo, hi, d) => (Number.isFinite(+v) ? Math.min(hi, Math.max(lo, +v)) : d);
  const parseUrl = (u) => { try { return new URL(u); } catch { return null; } };
  // Страница результатов любой поисковой системы (searchQueryOf — в settings.js).
  const searchQ = (u) => (u ? searchQueryOf(u.href, B.search_custom) : null);
  const isDdg = (u) => searchQ(u) != null;

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
    layout: 'one', barPos: 'top', radius: 10, gap: 6, font: 'system', customFont: '', fontSize: 12.5, speed: 1,
    showNav: true, showAiBtn: true, fullUrl: false, customCss: '', aiWidth: 400, island: true, islandVideo: true, islandPip: true, islandCollapsed: false, islandSide: 'panel',
  };
  const FAV_COLORS = ['#7F77DD', '#1D9E75', '#D85A30', '#378ADD', '#D4537E', '#BA7517', '#639922', '#534AB7'];

  // ---------- состояние ----------
  const S = { tabs: [], active: null, aiOpen: false, aiProxy: false, canRestore: false, topSites: null, maximized: false };
  let A = { ...DEFAULTS };
  // no_sleep_sites: null — настройки ещё не пришли, Rust список не трогает.
  let B = { double_search: true, ai_proxy: '', ai_new_chat: true, sleep_minutes: 5, live_tabs: 3, no_sleep_sites: null, onboarded: true, check_updates: true, strip_tracking: true, crash_reports: true, search_engine: 'duckduckgo', search_custom: '', user_agent: 'default', user_agent_custom: '', referer: 'default', referer_custom: '' };
  let SYS = { installed: false, default_browser: false, chrome_found: false, can_install: false, version: '' };
  let aiReady = false;
  let prevActive = null;

  const activeTab = () => S.tabs.find((t) => t.id === S.active);
  const speed = () => (reduceMotion.matches ? 0 : clamp(A.speed, 0, 3, 1));
  const ms = (n) => Math.round(n * speed());

  // Пока поверх контента что-то открыто (подсказки, загрузки, приветствие),
  // нативные webview страниц прячутся — иначе они перекрыли бы интерфейс.
  // На их месте остаётся снимок страницы (ui/snapshot.js).
  const snap = createSnapshot({
    send,
    // Что сейчас закрыто нативными webview: id вкладки со страницей и панель ИИ.
    native() {
      const t = activeTab();
      return { page: isPage(t) && !t.sleeping && !t.waking ? t.id : null, ai: S.aiOpen && aiReady && !isSettings(t) };
    },
  });
  const overlayReasons = new Set();
  function setOverlay(reason, on) {
    // Под другим окном страница спрятана — окошку масштаба там нечего показывать.
    if (on) zm.close();
    const before = overlayReasons.size > 0;
    if (on) overlayReasons.add(reason); else overlayReasons.delete(reason);
    const after = overlayReasons.size > 0;
    if (on) snap.opening(reason, !before);
    if (before !== after) send({ cmd: 'overlay', open: after });
    if (!on) snap.closing(reason, !after);
  }

  // ---------- масштаб страницы (ui/zoom.js) ----------
  const zm = createZoom({
    send, speed,
    // Открыта страница с живым webview: только её масштаб и меняется.
    isPage: () => { const t = activeTab(); return isPage(t) && !t.sleeping; },
    // Место страницы: правее неё — панель ИИ (тоже нативный webview) или край окна.
    page: () => $('#content').getBoundingClientRect(),
  });

  // ---------- загрузки (ui/downloads.js) ----------
  const dl = createDownloads({ esc, send, ms, speed, setOverlay });

  // ---------- островок (ui/island.js) ----------
  // favHtml объявлена ниже (function), к первому вызову она уже есть.
  const isl = createIsland({ esc, send, ms, setOverlay, favHtml: (u) => favHtml(u), tabs: () => S.tabs, active: () => S.active, layout: () => A.layout, collapse: (on) => setA('islandCollapsed', !!on) });

  // ---------- обновления (ui/update.js) ----------
  const upd = createUpdate({ esc, send, ms, speed, setOverlay, copyText: (t) => copyText(t), toast: (t) => toast(t), onChange: () => panels.sync() });

  // ---------- перевод страниц (ui/translate.js) ----------
  const tr = createTranslate({ esc, send, speed, setOverlay, toast: (t) => toast(t) });

  // ---------- меню вкладки (ui/tabmenu.js) ----------
  const tm = createTabMenu({
    send, ms, setOverlay, NEWTAB, tabs: () => S.tabs, canRestore: () => S.canRestore, volume: () => SYS.platform !== 'linux',
    // Меню панели: перенести её к другому краю окна или открыть раскладку.
    barDown: () => barDown(),
    bar(what) {
      if (what === 'move') setA('barPos', barDown() ? 'top' : 'bottom');
      else if (what === 'layout') openSettings('layout');
    },
  });
  // ---------- ссылки на приложения (ui/applink.js) ----------
  const xap = createAppLink({ esc, send, setOverlay });
  // ---------- разрешения сайтов (ui/permissions.js) ----------
  // Функции ниже объявлены через function: к первому вызову они уже есть.
  const pm = createPermissions({ esc, send, setOverlay, isPage, toast: (t) => toast(t), activeTab: () => activeTab(), favHtml: (u) => favHtml(u), openSettings: (s) => openSettings(s) });

  // ---------- иконки и фавиконки ----------
  const fillIcons = (scope = document) => $$('[data-i]', scope).forEach((el) => { el.innerHTML = icon(el.dataset.i); });

  function favHtml(url) {
    const u = parseUrl(url);
    if (url === SETTINGS) return `<span class="fav set">${icon('sliders')}</span>`;
    if (url === HISTORY) return `<span class="fav set">${icon('clock')}</span>`;
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
  // Панель (вкладки и адресная строка) у нижнего края окна — для одной и двух строк.
  function barDown() { return A.layout !== 'side' && A.barPos === 'bottom'; }
  let placed = '';
  function place() {
    const l = A.layout;
    app.dataset.layout = l;
    app.dataset.bar = A.barPos === 'bottom' ? 'bottom' : 'top';
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

  // ---------- свой CSS ----------
  // Свой CSS действует на сам интерфейс, где есть адреса плиток и заголовки вкладок.
  // Картинка по селектору атрибута (`.tab[title^="Банк"] { background: url(https://…) }`)
  // выдала бы их чужому серверу, поэтому всё, что может что-то загрузить, вырезаем:
  // url() (кроме data:), image-set, cross-fade, src(), @import, @font-face и т. п.
  const CSS_LOADS = /(?<![\w-])(?:-[a-z]+-)?(?:url|src|image|image-set|cross-fade|element|paint)\(/g;
  // Экранирование (`\75rl(`) для разбора CSS — те же буквы: раскрываем его до проверки.
  function cssPlain(s) {
    return String(s).replace(/\\(?:([0-9a-f]{1,6})[ \t\r\n\f]?|([\s\S]))/gi, (_, hex, ch) => {
      if (!hex) return /[\r\n\f]/.test(ch) ? '' : ch;
      const n = parseInt(hex, 16);
      return n > 0 && n <= 0x10ffff && (n < 0xd800 || n > 0xdfff) ? String.fromCodePoint(n) : '�';
    }).toLowerCase();
  }
  function cssLoads(text) {
    const s = cssPlain(text);
    if (/@import|@font-face|@namespace/.test(s)) return true;
    for (const m of s.matchAll(CSS_LOADS)) {
      // url(data:…) ничего не загружает.
      if (m[0] === 'url(' && /^\s*["']?\s*data:/.test(s.slice(m.index + m[0].length))) continue;
      return true;
    }
    return false;
  }
  // Убирает из правил всё, что может что-то загрузить. true — что-то убрано.
  function cleanRules(owner) {
    let cut = false;
    const rules = owner.cssRules;
    for (let i = rules.length - 1; i >= 0; i--) {
      const r = rules[i];
      if (r instanceof CSSImportRule || r instanceof CSSFontFaceRule || r.constructor.name === 'CSSNamespaceRule') {
        owner.deleteRule(i);
        cut = true;
        continue;
      }
      if (r.style) {
        for (const name of [...r.style]) {
          if (cssLoads(r.style.getPropertyValue(name))) { r.style.removeProperty(name); cut = true; }
        }
      }
      if (r.cssRules) cut = cleanRules(r) || cut;
      else if (!r.style && cssLoads(r.cssText)) { owner.deleteRule(i); cut = true; }
    }
    return cut;
  }
  let cssCut = false;
  // Разбираем CSS браузером и применяем уже очищенный разбор, без повторного чтения текста.
  // Разобрать не вышло (старый движок) — применяем как есть, только если он ничего не грузит.
  function applyUserCss(text) {
    const css = typeof text === 'string' ? text : '';
    const style = $('#userCss');
    const adopt = 'adoptedStyleSheets' in document;
    let sheet = null;
    cssCut = false;
    if (css.trim() && adopt && typeof CSSStyleSheet === 'function') {
      try {
        sheet = new CSSStyleSheet();
        sheet.replaceSync(css);
        cssCut = cleanRules(sheet);
      } catch { sheet = null; cssCut = false; }
    }
    if (sheet) {
      style.textContent = '';
      document.adoptedStyleSheets = [sheet];
      return;
    }
    if (adopt) document.adoptedStyleSheets = [];
    cssCut = cssLoads(css);
    style.textContent = cssCut ? '' : css;
  }

  // Видео из чёлки и «Видео в окне» — только Windows: на Linux чёлка с кнопками и загрузками.
  // Снимка страницы под окнами на Linux нет (src/tabs/snapshot.rs): там раскрытый островок
  // не заходит на страницу (`overPage`), иначе она спряталась бы до серого фона.
  function configureIsland() {
    const win = SYS.platform === 'windows';
    isl.configure({ enabled: A.island !== false, video: win && A.islandVideo !== false, pip: win && A.islandPip !== false, collapsed: !!A.islandCollapsed, side: A.islandSide, down: barDown(), overPage: SYS.platform !== 'linux' });
  }
  function applyAppearance() {
    const mode =A.mode === 'system' ? (darkScheme.matches ? 'dark' : 'light') : (A.mode === 'light' ? 'light' : 'dark');
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
    applyUserCss(A.customCss);
    renderDs();
    place();
    configureIsland();
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
        search_engine: String(B.search_engine || 'duckduckgo'),
        search_custom: String(B.search_custom || ''),
        double_search: !!B.double_search,
        ai_proxy: String(B.ai_proxy || ''),
        ai_new_chat: B.ai_new_chat !== false,
        sleep_minutes: Math.round(clamp(B.sleep_minutes, 0, 1440, 5)),
        live_tabs: Math.round(clamp(B.live_tabs, 0, 30, 3)),
        no_sleep_sites: Array.isArray(B.no_sleep_sites) ? B.no_sleep_sites.map(String) : null,
        onboarded: !!B.onboarded,
        force_dark: !!B.force_dark,
        check_updates: B.check_updates !== false,
        strip_tracking: B.strip_tracking !== false,
        crash_reports: B.crash_reports !== false,
        // null — бета-версии ещё не включали и не выключали: Rust не трогает поле.
        beta: typeof B.beta === 'boolean' ? B.beta : null,
        gpu: String(B.gpu || 'auto'),
        user_agent: String(B.user_agent || 'default'),
        user_agent_custom: String(B.user_agent_custom || ''),
        referer: String(B.referer || 'default'),
        referer_custom: String(B.referer_custom || ''),
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
        el.innerHTML = `<span class="favslot"></span><span class="tt"></span><button class="ta" hidden></button><button class="tx" title="Закрыть (Ctrl+W)">${icon('x')}</button>`;
        el.addEventListener('animationend', () => el.classList.remove('in'), { once: true });
        tabEls.set(t.id, el);
      }
      el.classList.toggle('on', t.id === S.active);
      el.classList.toggle('zz', !!t.sleeping);
      el.classList.toggle('ld', !!t.loading && !t.sleeping);
      const awake = !!t.no_sleep || !!t.site_no_sleep;
      el.classList.toggle('nosleep', awake);
      const title = t.title || (t.url === NEWTAB ? 'Новая вкладка' : t.url);
      el.querySelector('.tt').textContent = title;
      el.title = t.sleeping ? `${title} — спит` : awake ? `${title} — не засыпает` : title;
      renderTabSound(el, t);
      const key = parseUrl(t.url)?.origin || t.url;
      const slot = el.querySelector('.favslot');
      if (slot.dataset.key !== key) { slot.dataset.key = key; slot.innerHTML = favHtml(t.url); }
      let want = prev ? prev.nextElementSibling : list.firstElementChild;
      // Промежуток под островок (ui/island.js) — не вкладка, его место считает островок.
      if (want?.classList.contains('isl-gap')) want = want.nextElementSibling;
      if (el !== want) list.insertBefore(el, want);
      prev = el;
    }
  }

  // Значок звука: динамик, пока страница играет звук, перечёркнутый — пока звук выключен.
  // Громкость вкладки ниже 100 % видна всегда, даже в тишине: динамик с одной волной
  // (0 % — без волн). Клик по нему (и Ctrl+M) выключает и включает звук, колесо мыши
  // над ним — громкость. На узкой вкладке он вместо фавиконки.
  function renderTabSound(el, t) {
    const b = el.querySelector('.ta');
    const vol = t.volume ?? 100;
    const state = t.muted ? 'mute' : vol < 100 ? volIcon(vol) : t.audible ? 'vol' : '';
    el.classList.toggle('snd', !!state);
    el.classList.toggle('muted', !!t.muted);
    if (b.dataset.s !== state) {
      b.dataset.s = state;
      b.hidden = !state;
      b.innerHTML = state ? icon(state) : '';
    }
    const what = t.muted ? 'Включить звук (Ctrl+M)' : 'Выключить звук (Ctrl+M)';
    const title = `${what}\nГромкость ${vol}% — колесо мыши`;
    if (b.title !== title) {
      b.title = title;
      b.setAttribute('aria-label', `${what}, громкость ${vol}%`);
    }
  }

  // Колесо над значком динамика: громкость вкладки ±5 % и подсказка с процентами рядом.
  const volTip = document.createElement('div');
  volTip.className = 'voltip';
  volTip.setAttribute('role', 'status');
  volTip.innerHTML = '<span class="vt-ic"></span><i class="vt-bar"><i></i></i><b></b>';
  app.append(volTip);
  let volTipTimer = 0;
  let wheel = { id: 0, acc: 0, at: 0 };
  function showVolTip(b, vol, muted) {
    const ic = muted ? 'mute' : volIcon(vol);
    const slot = volTip.querySelector('.vt-ic');
    if (slot.dataset.i !== ic) { slot.dataset.i = ic; slot.innerHTML = icon(ic); }
    volTip.querySelector('.vt-bar').style.setProperty('--v', `${vol}%`);
    volTip.querySelector('b').textContent = `${vol}%`;
    // Слева от значка, по его середине; у левого края окна — справа. Всегда в полосе
    // вкладок: ниже неё лежит страница, а её webview выше интерфейса.
    const r = b.getBoundingClientRect();
    const w = volTip.offsetWidth;
    const h = volTip.offsetHeight;
    const left = r.left - 6 - w >= 4 ? r.left - 6 - w : r.right + 6;
    Object.assign(volTip.style, { left: `${Math.round(left)}px`, top: `${Math.round(r.top + (r.height - h) / 2)}px` });
    volTip.classList.add('on');
    clearTimeout(volTipTimer);
    volTipTimer = setTimeout(() => volTip.classList.remove('on'), 900);
  }
  $('#tablist').addEventListener('wheel', (e) => {
    // Linux: громкость вкладок пока выключена (см. src/tabs/volume.rs).
    if (SYS.platform === 'linux') return;
    const b = e.target.closest('.ta');
    const el = b && !b.hidden && e.target.closest('.tab');
    if (!el || el.classList.contains('out')) return;
    e.preventDefault();
    const t = S.tabs.find((x) => x.id === +el.dataset.id);
    if (!t || isInternal(t)) return;
    // Тачпад шлёт много мелких шагов: копим до одного щелчка колеса.
    const now = performance.now();
    if (wheel.id !== t.id || now - wheel.at > 300) wheel = { id: t.id, acc: 0, at: now };
    wheel.at = now;
    wheel.acc += e.deltaMode === 1 ? e.deltaY * 33 : e.deltaMode === 2 ? e.deltaY * 800 : e.deltaY;
    const vol = t.volume ?? 100;
    if (Math.abs(wheel.acc) < 40) { showVolTip(b, vol, t.muted); return; }
    const next = volStep(vol, wheel.acc < 0);
    wheel.acc = 0;
    if (next !== vol) {
      // Сразу у себя: следующий щелчок считает от нового значения, не дожидаясь Rust.
      t.volume = next;
      renderTabSound(el, t);
      tm.sync();
      send({ cmd: 'set_volume', id: t.id, volume: next });
    }
    showVolTip(b, next, t.muted);
  }, { passive: false });

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
    const onSound = !!e.target.closest('.ta');
    const vertical = app.dataset.layout === 'side';
    const start = vertical ? e.clientY : e.clientX;
    let dragging = false;
    el.setPointerCapture(e.pointerId);
    const move = (ev) => {
      const p = vertical ? ev.clientY : ev.clientX;
      if (!dragging && !onClose && !onSound && Math.abs(p - start) > 5) { dragging = true; el.classList.add('drag'); }
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
        isl.sync();
      } else if (onClose) {
        send({ cmd: 'close_tab', id });
      } else if (onSound) {
        send({ cmd: 'toggle_mute', id });
      } else {
        send({ cmd: 'activate_tab', id });
      }
    };
    el.addEventListener('pointermove', move);
    el.addEventListener('pointerup', up);
    el.addEventListener('pointercancel', up);
  });
  $('#tablist').addEventListener('auxclick', (e) => { if (e.button === 1) e.preventDefault(); });
  // Правый клик по вкладке — её меню (ui/tabmenu.js), по пустому месту панели — меню панели.
  $('#tablist').addEventListener('contextmenu', (e) => {
    const el = e.target.closest('.tab');
    // Пустое место ряда вкладок — меню панели (ниже); в боковой панели меню нет.
    if (!el && A.layout !== 'side') return;
    e.preventDefault();
    if (el && !el.classList.contains('out')) tm.open(+el.dataset.id, e.clientX, e.clientY);
  });
  document.addEventListener('contextmenu', (e) => {
    if (e.defaultPrevented || A.layout === 'side') return;
    if (!e.target.closest('#row1, #row2') || e.target.closest('button,input,textarea,select,.tab,.omni,.wc,[data-nodrag]')) return;
    e.preventDefault();
    tm.open(null, e.clientX, e.clientY);
  });
  $('#newTabBtn').addEventListener('click', () => send({ cmd: 'new_tab' }));

  // ---------- контент и новая вкладка (ui/newtab.js) ----------
  const newtab = createNewtab({
    esc, send, ms, speed,
    colors: FAV_COLORS,
    system: () => SYS,
    // Поверх открыто окно (настройки, загрузки…): ввод с клавиатуры не перехватываем.
    blocked: () => overlayReasons.size > 0,
    // Поиск новой вкладки работает через подсказки адресной строки.
    sug: { input: (el) => updateSug(el), key: (e) => onOmniKey(e), hide: () => hideSug() },
    openSettings(section) { dl.close(); upd.close(); panels.open(section); },
    openHistory: () => openHistory(),
  });
  // ---------- вкладка истории (ui/history.js) ----------
  const hist = createHistory({ esc, send, ms, speed, toast: (t) => toast(t), favHtml: (u) => favHtml(u) });
  function renderContent() {
    const t = activeTab();
    const nt = !t || t.url === NEWTAB;
    const c = $('#content');
    c.classList.toggle('nt', nt);
    const zz = !!t && !nt && (!!t.sleeping || !!t.waking);
    c.classList.toggle('zz', zz);
    if (zz) renderWaking(t);
    newtab.render(nt, S.topSites);
    const hs = isHistory(t);
    c.classList.toggle('hs', hs);
    hist.render(hs, t?.id);
    if (hs && wantHistory) { hist.focus(wantHistory); wantHistory = null; }
    if (S.active !== prevActive) {
      prevActive = S.active;
      if (nt && mode === 'url' && !overlayReasons.has('onb')) requestAnimationFrame(() => newtab.focus());
    }
  }

  // Заглушка спящей или просыпающейся страницы: иконка и название сайта.
  function renderWaking(t) {
    const fav = $('#wakeFav');
    const key = parseUrl(t.url)?.origin || t.url;
    if (fav.dataset.key !== key) { fav.dataset.key = key; fav.innerHTML = favHtml(t.url); }
    $('#wakeName').textContent = t.title || parseUrl(t.url)?.hostname.replace(/^www\./, '') || t.url;
  }

  // ---------- адресная строка ----------
  const omni = $('#omni');
  const inp = $('#omniInput');
  const view = $('#omniView');
  let mode = 'url';

  function setIc(name) { $('#omniIc').innerHTML = icon(name); }
  // Адрес, который строка показывает без правки: к нему возвращает Esc.
  const shownUrl = () => { const t = activeTab(); return isPage(t) ? t.url : ''; };

  // Набранное, но не отправленное остаётся в строке, когда из неё уходят (клик по
  // странице, перетаскивание окна, другое окно), как в Chrome: у каждой вкладки своё.
  // Сбрасывают его Esc, Enter и переход вкладки на другой адрес.
  const drafts = new Map(); // id вкладки → { text, url }: что набрали поверх какого адреса
  let fieldTab = null; // вкладка, чей текст сейчас в поле
  let typed = false; // в поле печатали с тех пор, как его заполнили
  function draftFor(t) {
    const d = t && drafts.get(t.id);
    if (!d) return null;
    if (d.url === (isPage(t) ? t.url : '')) return d.text;
    drafts.delete(t.id);
    return null;
  }
  function keepDraft() {
    if (!typed || mode !== 'url') return;
    typed = false;
    const t = S.tabs.find((x) => x.id === fieldTab);
    const url = isPage(t) ? t.url : '';
    if (t && inp.value.trim() && inp.value !== url) drafts.set(t.id, { text: inp.value, url });
    else drafts.delete(fieldTab);
  }
  function dropDraft() {
    typed = false;
    drafts.delete(fieldTab);
  }
  function fillField() {
    fieldTab = S.active;
    typed = false;
    inp.value = draftFor(activeTab()) ?? shownUrl();
  }

  function renderOmni() {
    const t = activeTab();
    const star = $('#starBtn');
    const pageOpen = isPage(t);
    star.hidden = !pageOpen;
    star.classList.toggle('on', !!t?.bookmarked);
    star.setAttribute('aria-pressed', String(!!t?.bookmarked));
    star.title = t?.bookmarked ? 'В закладках — убрать (Ctrl+D)' : 'Добавить в закладки (Ctrl+D)';
    tr.render(t, S.tabs);
    pm.renderOmni(t);
    if (mode === 'find') return;
    if (document.activeElement === inp) {
      // Вкладку сменили, не выходя из строки (Ctrl+Tab): набранное остаётся своей вкладке.
      if (fieldTab !== S.active) { keepDraft(); fillField(); inp.select(); }
      return;
    }
    for (const id of drafts.keys()) if (!S.tabs.some((x) => x.id === id)) drafts.delete(id);
    const draft = draftFor(t);
    omni.classList.toggle('draft', draft != null);
    if (!pageOpen) {
      omni.classList.add('empty');
      inp.value = draft ?? '';
      view.innerHTML = '';
      setIc('search');
      return;
    }
    omni.classList.remove('empty');
    inp.value = draft ?? t.url;
    const u = parseUrl(t.url);
    // Адрес без хоста (исходный код страницы, view-source:) показываем целиком.
    if (!u || !u.host) { view.textContent = t.url; setIc('globe'); return; }
    if (isDdg(u)) {
      view.innerHTML = `<span class="h">${esc(searchQ(u))}</span>`;
      setIc('search');
      return;
    }
    const path = (u.pathname + u.search + u.hash).replace(/^\/$/, '');
    view.innerHTML = A.fullUrl
      ? `<span class="h">${esc(u.host)}</span><span class="p">${esc(path)}</span>`
      : `<span class="h">${esc(u.host.replace(/^www\./, ''))}</span>`;
    setIc(u.protocol === 'https:' ? 'lock' : 'globe');
  }

  // Короткое сообщение поверх адреса: около полутора секунд вместе с появлением
  // и угасанием (CSS), длинное (ошибка) — чуть дольше, чтобы успеть прочитать.
  let msgTimer;
  function toast(text) {
    const m = $('#omniMsg');
    const s = String(text ?? '');
    clearTimeout(msgTimer);
    m.textContent = s;
    m.classList.add('on');
    msgTimer = setTimeout(hideMsg, Math.min(3600, 1100 + Math.max(0, s.length - 30) * 40));
  }
  function hideMsg() {
    clearTimeout(msgTimer);
    $('#omniMsg').classList.remove('on');
  }

  inp.addEventListener('focus', () => {
    omni.classList.add('focus');
    hideMsg();
    isl.onOmniFocus();
    if (mode === 'url') fillField();
    requestAnimationFrame(() => { if (document.activeElement === inp) inp.select(); });
  });
  inp.addEventListener('blur', () => {
    keepDraft();
    // Фокус ушёл из окна интерфейса — на страницу или в другое окно. Поле при этом
    // остаётся «активным» и при возврате фокуса снова выделило бы адрес и спрятало
    // кнопки под курсором (звезда не нажималась). Снимаем фокус по-настоящему.
    if (document.activeElement === inp) inp.blur();
    inp.setSelectionRange(0, 0);
    omni.classList.remove('focus');
    hideSug();
    if (mode === 'find') exitFind();
    renderOmni();
  });
  // Кнопки в строке срабатывают с первого клика: нажатие не уводит фокус в поле,
  // не выделяет адрес и не прячет сами кнопки. Клик по значку слева — как по полю,
  // а на странице сайта значок — кнопка карточки сайта (ui/permissions.js).
  omni.addEventListener('mousedown', (e) => {
    if (e.button !== 0 || e.target === inp) return;
    e.preventDefault();
    if (!e.target.closest('button') && !e.target.closest('.omni-ic.pm-click')) inp.focus();
  });
  inp.addEventListener('input', () => { hideMsg(); if (mode === 'find') find(false); else { typed = true; updateSug(); } });
  inp.addEventListener('keydown', onOmniKey);

  const looksLikeUrl = (q) => !/\s/.test(q) && (/^[a-z][a-z0-9+.-]*:\/\//i.test(q) || /^localhost(:\d+)?(\/|$)/i.test(q) || /^[^/?#]+\.[\p{L}]{2,}(:\d+)?([/?#]|$)/u.test(q)
    || /^\d{1,3}(\.\d{1,3}){3}(:\d+)?([/?#]|$)/.test(q) || /^\[[0-9a-f:.]+\](:\d+)?([/?#]|$)/i.test(q));

  let sugQ = '';
  let sugHist = [];
  let sugItems = [];
  let sugSel = 0;
  let sugTimer;
  // Чьи подсказки: адресной строки или поиска новой вкладки (ui/newtab.js).
  let sugSrc = inp;
  function updateSug(src = inp) {
    sugSrc = src;
    const q = src.value.trim();
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
    items.push({ kind: 'search', ic: 'search', title: q, sub: B.double_search ? `${searchEngineName(B)} + ИИ` : searchEngineName(B), input: q });
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
    const own = sugSrc === inp;
    const r = (own ? omni : newtab.anchor()).getBoundingClientRect();
    const w = Math.min(Math.max(r.width, own ? 460 : 0), innerWidth - 16);
    const left = Math.min(Math.max(8, r.left), innerWidth - 8 - w);
    const el = $('#sug');
    // Панель снизу — список над адресной строкой.
    const up = own && barDown();
    el.classList.toggle('up', up);
    el.style.width = Math.round(w) + 'px';
    const top = up ? r.top - 6 - el.offsetHeight : r.bottom + 6;
    Object.assign(el.style, { left: Math.round(left) + 'px', top: Math.round(top) + 'px' });
  }
  function showSug() {
    const el = $('#sug');
    positionSug();
    if (!el.classList.contains('on')) {
      el.classList.add('on', 'anim');
      setTimeout(() => el.classList.remove('anim'), ms(400) + 50);
      // Список под поиском новой вкладки не заходит на панель ИИ: её не прячем.
      if (sugSrc === inp) setOverlay('sug', true);
    }
  }
  function hideSug() {
    const el = $('#sug');
    if (el.classList.contains('on')) {
      el.classList.remove('on');
      if (overlayReasons.has('sug')) setOverlay('sug', false);
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
    sugSrc = e.currentTarget;
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
      const q = e.currentTarget.value.trim();
      if (!q) return;
      if (e.ctrlKey || e.metaKey) { askAi(q); return; }
      const fallback = looksLikeUrl(q) ? { kind: 'go', input: q } : { kind: 'search', input: q };
      runItem(open ? sugItems[sugSel] : fallback, e.altKey);
    } else if (e.key === 'Escape') {
      // Первый Esc возвращает исходный адрес (и закрывает подсказки), следующий — выходит из строки.
      e.preventDefault();
      const field = e.currentTarget;
      if (open) hideSug();
      if (field !== inp) { if (!open) field.blur(); return; }
      const url = shownUrl();
      dropDraft();
      if (inp.value !== url) { inp.value = url; inp.select(); } else inp.blur();
    }
  }
  function runItem(it, newTab) {
    if (!it) return;
    if (it.kind === 'ai') { askAi(it.input); return; }
    send(newTab ? { cmd: 'new_tab', input: it.input } : { cmd: 'navigate', input: it.input });
    if (it.kind === 'search' && B.double_search) send({ cmd: 'ai', open: true, query: it.input });
    if (sugSrc === inp) dropDraft();
    hideSug();
    sugSrc.blur();
  }
  function askAi(q) {
    send({ cmd: 'ai', open: true, query: q });
    if (sugSrc === inp) dropDraft();
    hideSug();
    sugSrc.blur();
  }

  function renderDs() {
    const b = $('#dsBtn');
    const on = !!B.double_search;
    b.classList.toggle('on', on);
    b.setAttribute('aria-pressed', String(on));
    const name = searchEngineName(B);
    b.title = on ? `Двойной поиск включён: ${name} + ИИ` : `Двойной поиск выключен: только ${name}`;
  }
  $('#dsBtn').addEventListener('click', () => {
    B.double_search = !B.double_search;
    renderDs();
    // Пока в строке набирают запрос, сообщение закрыло бы текст: видно и по кнопке, и по подсказкам.
    if (document.activeElement === inp) { if (sugQ) renderSug(); }
    else toast(B.double_search ? 'Двойной поиск включён' : 'Двойной поиск выключен');
    panels.sync();
    saveSettings(true);
  });
  $('#starBtn').addEventListener('click', () => send({ cmd: 'toggle_bookmark' }));

  // «Не усыплять»: вкладку не усыпят ни таймер, ни нехватка памяти, ни лимит живых вкладок.
  // Кружка горит и у вкладки, чей сайт в списке «не усыплять»: тогда она убирает сайт из списка.
  function renderAwake() {
    const t = activeTab();
    const b = $('#awakeBtn');
    const site = !!t?.site_no_sleep;
    const on = !!t?.no_sleep || site;
    b.hidden = !isPage(t);
    b.classList.toggle('on', on);
    b.setAttribute('aria-pressed', String(on));
    b.title = site ? 'Сайт не засыпает — снова усыплять на этом сайте' : on ? 'Вкладка не засыпает — разрешить сон' : 'Не усыплять вкладку';
  }
  $('#awakeBtn').addEventListener('click', () => {
    const t = activeTab();
    if (t) send({ cmd: t.site_no_sleep ? 'toggle_site_no_sleep' : 'toggle_no_sleep', id: t.id });
  });

  // ---------- поиск на странице ----------
  function startFind() {
    if (document.activeElement === inp) keepDraft();
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
    // Ctrl+L из поиска на странице: в поле снова адрес (или набранное), а не запрос поиска.
    if (document.activeElement === inp) fillField();
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
    $('#aiBtn').title = S.aiOpen ? 'Закрыть AI Mode (Ctrl+K)' : 'AI Mode: спросить ИИ (Ctrl+K)';
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
    const field = document.activeElement;
    let q = (field === inp && mode === 'url') || field === newtab.input ? field.value.trim() : '';
    if (!q) { const u = parseUrl(activeTab()?.url || ''); if (isDdg(u)) q = searchQ(u) || ''; }
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
  // Значки кнопок окна нарисованы в пикселях экрана (capIcon): при смене масштаба
  // (окно перетащили на другой монитор) их нужно нарисовать заново.
  function renderCaps() {
    $('[data-win=maximize]').dataset.cap = S.maximized ? 'restore' : 'max';
    for (const b of $$('[data-cap]', $('#wc'))) b.innerHTML = capIcon(b.dataset.cap);
    alignCaps();
  }
  // На 125% и 150% кнопка может стоять на дробном пикселе экрана (окно шириной 1281 px и т. п.):
  // сдвигаем начало viewBox на эту дробь, и линии снова попадают в центры пикселей.
  let capsQueued = false;
  function alignCaps() {
    if (capsQueued) return;
    capsQueued = true;
    requestAnimationFrame(() => {
      capsQueued = false;
      const d = devicePixelRatio || 1;
      for (const svg of $$('svg.cap', $('#wc'))) {
        const r = svg.getBoundingClientRect();
        if (!r.width) continue;
        const frac = (v) => +(((v * d) % 1 + 1) % 1).toFixed(3);
        svg.setAttribute('viewBox', `${frac(r.left)} ${frac(r.top)} ${16 * d} ${16 * d}`);
      }
    });
  }
  addEventListener('resize', alignCaps);
  new ResizeObserver(alignCaps).observe($('#wc'));
  (function watchScale() {
    matchMedia(`(resolution: ${devicePixelRatio}dppx)`).addEventListener('change', () => { renderCaps(); watchScale(); }, { once: true });
  })();
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
    if (mod && !e.shiftKey && c === 'KeyH') return 'history';
    if ((mod && e.shiftKey && c === 'KeyR') || (c === 'F5' && (mod || e.shiftKey))) return 'hard_reload';
    if ((mod && c === 'KeyR') || c === 'F5') return 'reload';
    if ((c === 'F12' && !mod && !e.altKey && !e.shiftKey) || (mod && e.shiftKey && c === 'KeyI')) return 'devtools';
    if (mod && !e.shiftKey && c === 'KeyU') return 'view_source';
    if (mod && !e.shiftKey && c === 'KeyP') return 'print';
    if (mod && !e.shiftKey && c === 'KeyM') return 'mute';
    if (mod && (c === 'Equal' || c === 'NumpadAdd')) return 'zoom_in';
    if (mod && (c === 'Minus' || c === 'NumpadSubtract')) return 'zoom_out';
    if (mod && !e.shiftKey && (c === 'Digit0' || c === 'Numpad0')) return 'zoom_reset';
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
    tm.close();
    if (a !== 'downloads' && dl.isOpen()) dl.close();
    if (upd.isOpen()) upd.close();
    switch (a) {
      case 'focus_omnibox': if (mode === 'find') exitFind(); inp.focus(); inp.select(); break;
      case 'toggle_ai': toggleAi(); break;
      // На вкладке истории Ctrl+F ищет в самой истории.
      case 'find': if (isHistory(activeTab())) hist.focus(); else startFind(); break;
      case 'settings': openSettings(); break;
      case 'history': if (!overlayReasons.has('onb')) openHistory(); break;
      case 'downloads': if (!overlayReasons.has('onb')) dl.toggle(); break;
      default: send({ cmd: 'hotkey', action: a });
    }
  }
  $('#setBtn').addEventListener('click', () => { dl.close(); upd.close(); openSettings(); });

  // ---------- настройки и приветствие (ui/settings.js) ----------
  const panels = createPanels({
    get A() { return A; },
    get B() { return B; },
    PRESETS, ACCENTS,
    esc, send, toast, swap, ms, speed, setOverlay, copyText, setA, mergeAppearance,
    cssLoads, cssCut: () => cssCut,
    system: () => SYS,
    update: () => upd.state(),
    translate: tr,
    perms: pm,
    openUpdate() { dl.close(); upd.open(); },
    openNotes() { dl.close(); upd.openNotes(); },
    closeSettings,
    openHistory: () => openHistory(),
    applyAppearance,
    saveSettings,
    refresh() { renderDs(); panels.sync(); },
    resetAppearance() { A = { ...DEFAULTS, aiWidth: A.aiWidth }; applyAppearance(); },
  });

  // ---------- вкладка настроек ----------
  // Кнопка, Ctrl+, и #settings:раздел: вкладка настроек одна. Rust переходит на неё
  // или открывает рядом с активной, а раздел покажем, когда она станет активной.
  let wantSection = null;
  function openSettings(section) {
    if (isSettings(activeTab())) { panels.open(section); return; }
    wantSection = section || null;
    send({ cmd: 'open_settings' });
  }
  function closeSettings() {
    const t = S.tabs.find(isSettings);
    if (t) send({ cmd: 'close_tab', id: t.id });
  }
  function renderSettings() {
    if (isSettings(activeTab())) {
      panels.open(wantSection);
      wantSection = null;
    } else {
      panels.hide();
    }
  }

  // ---------- вкладка истории ----------
  // Ctrl+H, ссылка на новой вкладке, кнопка в настройках и #history[:запрос]: вкладка
  // истории одна, Rust переходит на неё или открывает рядом с активной.
  let wantHistory = null;
  function openHistory(q) {
    if (isHistory(activeTab())) { hist.focus(q); return; }
    wantHistory = q || null;
    send({ cmd: 'open_history' });
  }

  // ---------- приём сообщений из Rust ----------
  function render() {
    renderTabs();
    renderContent();
    renderOmni();
    zm.render(activeTab());
    renderAwake();
    renderAi();
    renderSettings();
    snap.sync();
    tm.sync();
    isl.sync();
  }
  const isObj = (v) => v && typeof v === 'object' && !Array.isArray(v);
  const sitesIn = (v) => (Array.isArray(v) ? v.filter((x) => typeof x === 'string') : []);
  let firstSettings = true;
  const handlers = {
    settings(m) {
      const s = m.settings || {};
      A = { ...DEFAULTS, ...(isObj(s.appearance) ? s.appearance : {}) };
      B = { double_search: s.double_search !== false, ai_proxy: s.ai_proxy || '', ai_new_chat: s.ai_new_chat !== false, sleep_minutes: s.sleep_minutes ?? 5, live_tabs: s.live_tabs ?? 3, no_sleep_sites: sitesIn(s.no_sleep_sites), onboarded: !!s.onboarded, force_dark: !!s.force_dark, check_updates: s.check_updates !== false, strip_tracking: s.strip_tracking !== false, crash_reports: s.crash_reports !== false, beta: typeof s.beta === 'boolean' ? s.beta : null, gpu: s.gpu || 'auto', search_engine: s.search_engine || 'duckduckgo', search_custom: s.search_custom || '', user_agent: s.user_agent || 'default', user_agent_custom: s.user_agent_custom || '', referer: s.referer || 'default', referer_custom: s.referer_custom || '' };
      tr.onSettings(s.translate);
      applyAppearance();
      if (firstSettings) {
        // #settings:раздел, #onb:шаг, #downloads, #update[:install|:notes|:current], #translate[:menu|:revert], #history[:запрос]
        // или #import:браузер (сразу начать перенос) — только для разработки (debug-сборка).
        const [screen, arg] = location.hash.slice(1).split(':');
        if (screen === 'settings') openSettings(arg);
        else if (screen === 'onb') panels.openOnboarding(+arg || 0);
        else if (screen === 'import') { openSettings('data'); panels.importSoon(arg); }
        else if (screen === 'downloads') dl.open();
        else if (screen === 'update') upd.openSoon(arg);
        else if (screen === 'translate') tr.debug(arg);
        else if (screen === 'history') openHistory(arg);
        else if (!B.onboarded) panels.openOnboarding();
      }
      firstSettings = false;
    },
    // Список сайтов «не усыплять» поменял Rust: меню вкладки или чистка вписанного.
    no_sleep_sites(m) {
      B.no_sleep_sites = sitesIn(m.sites);
      panels.sync();
    },
    state(m) {
      Object.assign(S, {
        tabs: Array.isArray(m.tabs) ? m.tabs : [],
        active: m.active,
        aiOpen: !!m.ai_open,
        aiProxy: !!m.ai_proxy,
        canRestore: !!m.can_restore,
        // Плитки приходят только для новой вкладки (null — открыта страница).
        topSites: Array.isArray(m.top_sites) ? m.top_sites : S.topSites,
      });
      render();
    },
    suggestions(m) { if (m.q === sugQ && mode === 'url' && document.activeElement === sugSrc) { sugHist = m.items || []; renderSug(); } },
    hotkey(m) { hotkey(m.action); },
    find(m) { omni.classList.toggle('nf', !m.found); },
    toast(m) { toast(m.text); },
    window(m) {
      S.maximized = !!m.maximized;
      app.classList.toggle('max', S.maximized);
      $('[data-win=maximize]').title = S.maximized ? 'Восстановить' : 'Развернуть';
      renderCaps();
    },
    system(m) { SYS = { ...SYS, ...(m.status || {}) }; panels.onSystem(); newtab.onSystem(); configureIsland(); },
    imported(m) { panels.onImported(m); newtab.onImported(m); },
    installed(m) { panels.onInstalled(m); },
    downloads(m) { dl.onList(m); isl.onDownloads(m); },
    download_progress(m) { dl.onProgress(m); isl.onProgress(m); },
    media(m) { isl.onMedia(m); },
    peek(m) { isl.onPeek(m); },
    update(m) { upd.onState(m); },
    update_notes(m) { upd.onNotes(m); },
    translate(m) { tr.onState(m); },
    snapshot(m) { snap.onMessage(m); },
    // Закрыли активную вкладку настроек: лист уходит плавно, потом Rust покажет страницу.
    settings_leave() { panels.leave(() => send({ cmd: 'settings_left' })); },
    app_link(m) { xap.onRequest(m); },
    permission_ask(m) { pm.onAsk(m); },
    permissions(m) { pm.onList(m); },
    zoom(m) { zm.onZoom(m); },
    report_info(m) { panels.onReportInfo(m); },
    reported(m) { panels.onReported(m); },
    history(m) { hist.onPage(m); },
    history_count(m) { hist.onCount(m); },
  };
  window.axiom = Object.freeze({
    recv(msg) {
      try { handlers[msg?.type]?.(msg); } catch (err) { console.error(err); }
    },
  });

  // ---------- старт ----------
  fillIcons();
  renderCaps();
  applyAppearance();
  render();
  send({ cmd: 'ready' });
})();
